//! The browser's mix, off the main thread (#136).
//!
//! On a phone, mixing in cpal's main-thread stream (`crate::web`) took about
//! a third of the frame, and every long frame made a buffer late. Here the
//! mix runs in an `AudioWorklet` instead, on the browser's audio thread, in
//! a second small wasm (`flint-audio-worklet`: kira and nothing else). The
//! page and the worklet share no memory, so the engine talks to it through
//! a [`Link`]: a frame's calls are written as commands, and a microtask
//! after the first one posts them as one message once the frame's task is
//! done. Each sound crosses once, as a transferred `Float32Array`.
//!
//! `worklet.js` holds the page's half and the processor. The processor
//! meters itself and posts each buffer's numbers back, and they land in the
//! same [`BufferMeter`] the main-thread stream fed, so the console's
//! late-buffer lines, its summaries and `AudioEngine::buffer_stats` read as
//! before. If the worklet can't start, the engine falls back to the
//! main-thread stream (`AudioEngine::poll`).

use std::sync::{Arc, Mutex};

use send_wrapper::SendWrapper;
use wasm_bindgen::prelude::*;

use crate::engine::WebMixer;
use crate::meter::BufferMeter;
use crate::remote::Link;

/// Where the page serves the worklet's wasm, next to its own (trunk copies
/// it there; crates/flint-player/index.html).
pub const WASM_URL: &str = "flint_audio_worklet.wasm";

/// How the worklet's `AudioContext` trades latency for steadiness. A
/// phone's "interactive" buffers are a few ms; "balanced" gives the audio
/// thread more room while one-shots still land within a frame or two.
const LATENCY_HINT: &str = "balanced";

#[wasm_bindgen(module = "/src/worklet.js")]
extern "C" {
    fn flint_worklet_preload(url: &str);
    fn flint_worklet_wanted() -> bool;
    fn flint_audio_unlock(latency: &str);
    fn flint_audio_keep_awake(ctx: &web_sys::AudioContext);
    fn flint_audio_release_unlocked();

    #[derive(Clone)]
    type FlintWorklet;

    #[wasm_bindgen(constructor, catch)]
    fn new(
        url: &str,
        meter_frames: u32,
        latency: &str,
        on_stats: &js_sys::Function,
    ) -> Result<FlintWorklet, JsValue>;

    #[wasm_bindgen(method, getter)]
    fn state(this: &FlintWorklet) -> u32;

    #[wasm_bindgen(method)]
    fn load(this: &FlintWorklet, id: u32, rate: u32, channels: u32, samples: js_sys::Float32Array);

    #[wasm_bindgen(method)]
    fn batch(this: &FlintWorklet, bytes: js_sys::Uint8Array);

    #[wasm_bindgen(method)]
    fn kick(this: &FlintWorklet, f: &js_sys::Function);
}

/// Start fetching the worklet's wasm. Called when an engine is made, long
/// before Begin.
pub fn preload() {
    flint_worklet_preload(WASM_URL);
}

/// Open the page's `AudioContext` from inside the gesture that begins the
/// game, before its handler returns (Float#2). iOS and iPadOS WebKit start
/// a context only there; the engine's [`start`](crate::engine::AudioEngine::start)
/// runs a frame later and takes this one. Also asks for the "playback"
/// audio session, so the ringer switch doesn't silence the game.
pub fn unlock() {
    flint_audio_unlock(LATENCY_HINT);
}

/// Resume `ctx` at the page's next gesture whenever it isn't running: the
/// main-thread stream's context, which cpal opens outside the gesture, and
/// any context iOS interrupted. Closes the context [`unlock`] opened if the
/// worklet never took it.
pub(crate) fn keep_awake(ctx: &web_sys::AudioContext) {
    flint_audio_release_unlocked();
    flint_audio_keep_awake(ctx);
}

/// Where this page asks to mix: the worklet, unless it can't run here or
/// the address says `?mixer=main`.
pub fn requested() -> WebMixer {
    if flint_worklet_wanted() {
        WebMixer::Worklet
    } else {
        WebMixer::MainThread
    }
}

fn console(warn: bool, line: &str) {
    if warn {
        web_sys::console::warn_1(&line.into());
    } else {
        web_sys::console::log_1(&line.into());
    }
}

/// The worklet's `AudioContext` and processor, as the engine holds them.
pub struct WorkletStream {
    worklet: SendWrapper<FlintWorklet>,
}

impl WorkletStream {
    /// Open the worklet's `AudioContext` (call it inside the page's
    /// gesture) and start its processor. The returned [`Link`] posts what
    /// is written to it once per frame; what is posted before the processor
    /// is ready waits for it. `None` when the browser refused the context.
    pub fn open(meter: Arc<Mutex<BufferMeter>>, meter_frames: u32) -> Option<(Self, Link)> {
        let reported_errors = std::cell::Cell::new(0.0);
        let on_stats =
            Closure::<dyn FnMut(js_sys::Float64Array)>::new(move |v: js_sys::Float64Array| {
                let v = v.to_vec();
                let [slack, mix, now, wait, errors, buffer_ms, peak] = v[..] else {
                    return;
                };
                if errors > reported_errors.get() {
                    reported_errors.set(errors);
                    console(
                        true,
                        &format!(
                            "[flint-audio] the worklet's mixer refused {errors} commands so far"
                        ),
                    );
                }
                let Ok(mut m) = meter.lock() else { return };
                if !m.is_worklet() || m.buffer_ms() != buffer_ms {
                    *m = BufferMeter::worklet(buffer_ms);
                }
                m.peak(peak);
                let lines = m.record(slack, mix, now, wait);
                if let Some(l) = lines.late {
                    console(true, &l);
                }
                if let Some(s) = lines.summary {
                    console(lines.summary_warns, &s);
                }
            });
        let worklet = FlintWorklet::new(
            WASM_URL,
            meter_frames.max(128),
            LATENCY_HINT,
            on_stats.as_ref().unchecked_ref(),
        )
        .map_err(|e| console(true, &format!("[flint-audio] no AudioContext: {e:?}")))
        .ok()?;
        on_stats.forget();

        // The link and the flush that empties it hold each other; both live
        // as long as the page.
        let slot: Arc<Mutex<Option<Link>>> = Arc::default();
        let flush = {
            let worklet = worklet.clone();
            let slot = slot.clone();
            Closure::<dyn FnMut()>::new(move || {
                let Some(link) = slot.lock().ok().and_then(|s| s.clone()) else {
                    return;
                };
                let out = link.take();
                for l in out.loads {
                    let samples = js_sys::Float32Array::from(&l.samples[..]);
                    worklet.load(l.id, l.sample_rate, l.channels, samples);
                }
                if !out.batch.is_empty() {
                    worklet.batch(js_sys::Uint8Array::from(&out.batch[..]));
                }
            })
        };
        let kick = SendWrapper::new((worklet.clone(), flush));
        let link = Link::new(Some(Box::new(move || {
            let (worklet, flush) = &*kick;
            worklet.kick(flush.as_ref().unchecked_ref());
        })));
        if let Ok(mut s) = slot.lock() {
            *s = Some(link.clone());
        }
        Some((
            Self {
                worklet: SendWrapper::new(worklet),
            },
            link,
        ))
    }

    /// The processor has started and is mixing.
    pub fn running(&self) -> bool {
        self.worklet.state() == 1
    }

    /// The worklet couldn't start; the engine mixes on the main thread.
    pub fn failed(&self) -> bool {
        self.worklet.state() == 2
    }
}
