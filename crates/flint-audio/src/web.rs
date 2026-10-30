//! Sound in the browser (#101): a kira backend on cpal's WebAudio host that
//! measures its own steadiness.
//!
//! cpal renders WebAudio on the main thread, between frames: each buffer's
//! `onended` asks for the next one, which is scheduled to start where the
//! one before it ends. With two buffers in flight, a frame that holds the
//! thread longer than one buffer makes the next start late. kira's own
//! `CpalBackend` drops cpal's timestamps, so this one keeps them and feeds a
//! [`BufferMeter`]: the console gets a line for each late buffer and a
//! summary every [`crate::meter::REPORT_SECONDS`].

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleRate, Stream, StreamConfig};
use kira::backend::{Backend, Renderer};
use send_wrapper::SendWrapper;

use crate::meter::BufferMeter;

/// How the WebAudio stream is opened.
#[derive(Debug, Clone)]
pub struct WebAudioSettings {
    /// Frames per buffer. Two are in flight, so this is also how long a
    /// frame can hold the main thread before sound drops: 2048 at 44.1 kHz
    /// is 46 ms. Bigger buys headroom with latency.
    pub buffer_frames: u32,
    /// The `AudioContext`'s rate. The browser resamples to the hardware.
    pub sample_rate: u32,
    /// Where the stream records its steadiness. Keep a clone to read it:
    /// kira owns the backend once the manager is built.
    pub meter: Arc<Mutex<BufferMeter>>,
}

impl Default for WebAudioSettings {
    fn default() -> Self {
        Self {
            buffer_frames: 2048,
            sample_rate: 44_100,
            meter: Arc::default(),
        }
    }
}

fn log(msg: &str) {
    web_sys::console::log_1(&msg.into());
}

fn warn(msg: &str) {
    web_sys::console::warn_1(&msg.into());
}

fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now())
}

/// Signed `a - b` in ms between two cpal instants.
fn signed_ms(a: cpal::StreamInstant, b: cpal::StreamInstant) -> f64 {
    match a.duration_since(&b) {
        Some(d) => d.as_secs_f64() * 1000.0,
        None => -(b.duration_since(&a).unwrap_or_default().as_secs_f64() * 1000.0),
    }
}

/// Log the stream's `AudioContext` when it stops running or fails. A
/// failure (Chrome's `error` event: the device went away or the renderer
/// died) stops every buffer, so it is the one thing the late-buffer count
/// can't show. One resume is tried; whether sound came back is the next
/// state line.
fn watch(ctx: &web_sys::AudioContext) {
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    let c = ctx.clone();
    let on_state = Closure::<dyn FnMut()>::new(move || {
        let state = c.state();
        let line = format!(
            "[flint-audio] AudioContext {state:?} at {:.1} s",
            c.current_time()
        );
        if state == web_sys::AudioContextState::Running {
            log(&line);
        } else {
            warn(&line);
        }
    });
    ctx.set_onstatechange(Some(on_state.as_ref().unchecked_ref()));
    on_state.forget();

    let c = ctx.clone();
    let on_error = Closure::<dyn FnMut(web_sys::Event)>::new(move |_: web_sys::Event| {
        warn(&format!(
            "[flint-audio] the audio device or WebAudio renderer failed at {:.1} s ({:?}); \
             trying to resume",
            c.current_time(),
            c.state()
        ));
        let _ = c.resume();
    });
    let _ = ctx.add_event_listener_with_callback("error", on_error.as_ref().unchecked_ref());
    on_error.forget();
}

enum State {
    Empty,
    Ready {
        device: cpal::Device,
        config: StreamConfig,
    },
    Playing {
        _stream: Stream,
    },
}

/// kira's backend for the web. Open it from a user gesture: a browser keeps
/// an `AudioContext` made before one suspended.
pub struct WebAudioBackend {
    state: SendWrapper<State>,
    meter: Arc<Mutex<BufferMeter>>,
}

/// Why the browser gave no sound.
#[derive(Debug)]
pub enum WebAudioError {
    NoWebAudio,
    Build(cpal::BuildStreamError),
    Play(cpal::PlayStreamError),
}

impl std::fmt::Display for WebAudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoWebAudio => f.write_str("this browser has no WebAudio"),
            Self::Build(e) => write!(f, "WebAudio stream: {e}"),
            Self::Play(e) => write!(f, "WebAudio play: {e}"),
        }
    }
}

impl std::error::Error for WebAudioError {}

impl Backend for WebAudioBackend {
    type Settings = WebAudioSettings;
    type Error = WebAudioError;

    fn setup(
        settings: Self::Settings,
        _internal_buffer_size: usize,
    ) -> Result<(Self, u32), Self::Error> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or(WebAudioError::NoWebAudio)?;
        let frames = settings.buffer_frames.max(128);
        let config = StreamConfig {
            channels: 2,
            sample_rate: SampleRate(settings.sample_rate),
            buffer_size: BufferSize::Fixed(frames),
        };
        if let Ok(mut m) = settings.meter.lock() {
            *m = BufferMeter::new(frames as f64 * 1000.0 / settings.sample_rate as f64);
        }
        Ok((
            Self {
                state: SendWrapper::new(State::Ready { device, config }),
                meter: settings.meter,
            },
            settings.sample_rate,
        ))
    }

    fn start(&mut self, mut renderer: Renderer) -> Result<(), Self::Error> {
        let State::Ready { device, config } = std::mem::replace(&mut *self.state, State::Empty)
        else {
            panic!("WebAudioBackend started twice");
        };
        let channels = config.channels;
        let meter = self.meter.clone();
        let mut last_request: Option<f64> = None;
        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                    let t0 = now_ms();
                    renderer.on_start_processing();
                    renderer.process(data, channels);
                    let mix = now_ms() - t0;
                    let ts = info.timestamp();
                    let slack = signed_ms(ts.playback, ts.callback);
                    let wait = last_request.map_or(0.0, |l| t0 - l);
                    last_request = Some(t0);
                    let Ok(mut m) = meter.lock() else { return };
                    let lines = m.record(slack, mix, t0, wait);
                    if let Some(l) = lines.late {
                        warn(&l);
                    }
                    if let Some(s) = lines.summary {
                        if lines.summary_warns {
                            warn(&s);
                        } else {
                            log(&s);
                        }
                    }
                },
                |e| warn(&format!("[flint-audio] stream error: {e}")),
                None,
            )
            .map_err(WebAudioError::Build)?;
        stream.play().map_err(WebAudioError::Play)?;
        let cpal::platform::StreamInner::WebAudio(web_stream) = stream.as_inner();
        watch(web_stream.audio_context());
        let buffer_ms = self.meter.lock().map(|m| m.buffer_ms()).unwrap_or_default();
        log(&format!(
            "[flint-audio] WebAudio open: {} Hz, {buffer_ms:.0} ms buffers, two in flight",
            config.sample_rate.0
        ));
        self.state = SendWrapper::new(State::Playing { _stream: stream });
        Ok(())
    }
}
