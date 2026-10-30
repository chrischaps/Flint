//! The perf readout on the page (#120, docs/design/web.md): `?perf=1`
//! shows the numbers `perf.rs` keeps in the page's `#perf` panel, twice a
//! second, with a button that copies them as one line.
//!
//! The frame times are the browser's own: a `requestAnimationFrame` loop
//! of the page's, timed by the timestamps rAF hands it, which fall on the
//! display's frames. The player's callbacks start a little earlier or
//! later from frame to frame, so timing those would show jitter that no
//! one sees.
//!
//! wgpu can't say which adapter it got on the web (its `AdapterInfo` is
//! empty there), so the page asks the browser itself, once, with the same
//! high-performance preference wgpu asks with.

use super::pacing::Spread;
use super::perf::{browser_name, readout, summary_line, WINDOW_SECS};
use super::PlayerApp;
use std::cell::RefCell;
use std::collections::VecDeque;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

/// How often the panel is redrawn, seconds.
const EVERY: f64 = 0.5;

thread_local! {
    /// The adapter as the browser describes it, once its answer is in.
    static ADAPTER: RefCell<Option<String>> = const { RefCell::new(None) };
    static ASKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// rAF timestamps of the last ten seconds, ms.
    static RAF: RefCell<VecDeque<f64>> = const { RefCell::new(VecDeque::new()) };
    static RAF_STARTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Start the page's rAF loop, once: each frame's timestamp goes in `RAF`.
fn start_raf() {
    use std::rc::Rc;
    use wasm_bindgen::closure::Closure;
    if RAF_STARTED.with(|s| s.replace(true)) {
        return;
    }
    type Tick = Closure<dyn FnMut(f64)>;
    let tick: Rc<RefCell<Option<Tick>>> = Rc::new(RefCell::new(None));
    let again = tick.clone();
    *tick.borrow_mut() = Some(Closure::new(move |t: f64| {
        RAF.with(|r| {
            let mut r = r.borrow_mut();
            r.push_back(t);
            while r.front().is_some_and(|&f| f < t - WINDOW_SECS * 1000.0) {
                r.pop_front();
            }
        });
        if let (Some(window), Some(f)) = (web_sys::window(), again.borrow().as_ref()) {
            let _ = window.request_animation_frame(f.as_ref().unchecked_ref());
        }
    }));
    // The loop holds itself (`again`) for as long as the page lives.
    let first = tick.borrow();
    if let (Some(window), Some(f)) = (web_sys::window(), first.as_ref()) {
        let _ = window.request_animation_frame(f.as_ref().unchecked_ref());
    }
}

/// The rAF intervals in the window, and the seconds they cover.
fn raf_frames() -> Option<(Spread, f64)> {
    RAF.with(|r| {
        let r = r.borrow();
        let ms: Vec<f32> = r
            .iter()
            .zip(r.iter().skip(1))
            .map(|(a, b)| (b - a) as f32)
            .collect();
        let secs = match (r.front(), r.back()) {
            (Some(a), Some(b)) => (b - a) / 1000.0,
            _ => 0.0,
        };
        Spread::of(&ms).map(|s| (s, secs))
    })
}

fn element(id: &str) -> Option<web_sys::Element> {
    web_sys::window()?.document()?.get_element_by_id(id)
}

fn get(object: &wasm_bindgen::JsValue, key: &str) -> Option<wasm_bindgen::JsValue> {
    js_sys::Reflect::get(object, &key.into())
        .ok()
        .filter(|v| !v.is_undefined() && !v.is_null())
}

/// `navigator.gpu.requestAdapter()`'s `info`: "nvidia ampere" or, where the
/// browser says more, its description. Browsers keep this vague on purpose.
async fn ask_adapter() -> String {
    let unknown = || "adapter not named".to_string();
    let Some(gpu) = web_sys::window().and_then(|w| get(&w.navigator(), "gpu")) else {
        return unknown();
    };
    let Some(request) =
        get(&gpu, "requestAdapter").and_then(|f| f.dyn_into::<js_sys::Function>().ok())
    else {
        return unknown();
    };
    let options = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &options,
        &"powerPreference".into(),
        &"high-performance".into(),
    );
    let Ok(promise) = request.call1(&gpu, &options) else {
        return unknown();
    };
    let Ok(adapter) = JsFuture::from(js_sys::Promise::from(promise)).await else {
        return unknown();
    };
    let Some(info) = get(&adapter, "info") else {
        return unknown();
    };
    let field = |k: &str| {
        get(&info, k)
            .and_then(|v| v.as_string())
            .filter(|s| !s.is_empty())
    };
    if let Some(description) = field("description") {
        return description;
    }
    let named: Vec<String> = ["vendor", "architecture", "device"]
        .iter()
        .filter_map(|k| field(k))
        .collect();
    if named.is_empty() {
        unknown()
    } else {
        named.join(" ")
    }
}

fn adapter() -> String {
    if !ASKED.with(|a| a.replace(true)) {
        wasm_bindgen_futures::spawn_local(async {
            let name = ask_adapter().await;
            ADAPTER.with(|a| *a.borrow_mut() = Some(name));
        });
    }
    ADAPTER
        .with(|a| a.borrow().clone())
        .unwrap_or_else(|| "asking the browser".to_string())
}

impl PlayerApp {
    /// Draw the readout into the page's `#perf` panel when it's due.
    pub(super) fn show_perf(&mut self) {
        start_raf();
        if !self.perf_due(EVERY) {
            return;
        }
        let Some(panel) = element("perf") else {
            return;
        };
        let user_agent = web_sys::window()
            .and_then(|w| w.navigator().user_agent().ok())
            .unwrap_or_default();
        let Some((mut summary, context)) = self.perf_snapshot(browser_name(&user_agent), adapter())
        else {
            return;
        };
        if let Some((frames, secs)) = raf_frames() {
            summary.frames = frames;
            summary.secs = secs;
        }
        let _ = panel.remove_attribute("hidden");
        if let Some(text) = element("perf-text") {
            text.set_text_content(Some(&readout(&summary, &context)));
        }
        // The copy button reads it (index.html).
        let _ = panel.set_attribute("data-line", &summary_line(&summary, &context));
    }
}
