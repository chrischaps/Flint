//! The browser half of frame pacing (#140, `frame_rate.rs`): each frame's
//! rAF timestamp, and which frames the player draws.
//!
//! winit draws in its own `requestAnimationFrame` callback and doesn't
//! pass on the timestamp, which falls on the display's refresh. So a rAF
//! loop of the page's keeps it. Callbacks in one frame run in the order
//! they were requested; this loop requests its next during its callback
//! and winit during `about_to_wait`, after the player's, so from the
//! second frame on this one runs first and the player reads this frame's
//! time. Should it ever read a stale one, it takes `performance.now()`.
//!
//! The perf readout's frames (`web_perf.rs`) are the drawn ones, at these
//! timestamps, so `?perf=1` shows 33.3 ms when every other refresh is.
//!
//! The page can hold the game (#27): while `<html>` has `data-hold` (a
//! game that plays sideways, held upright where the phone can't be locked
//! to landscape, with the turn-your-phone prompt over it), no frame runs.
//! Game time stops, the mix fades out, and when the attribute goes the
//! clock skips the gap, so the race carries on where it was.

use super::PlayerApp;
use std::cell::{Cell, RefCell};
use wasm_bindgen::JsCast;

thread_local! {
    /// This frame's rAF timestamp, ms.
    static FRAME_TIME: Cell<f64> = const { Cell::new(f64::NAN) };
    static STARTED: Cell<bool> = const { Cell::new(false) };
    /// The timestamp the player last drew or skipped at.
    static LAST_USED: Cell<f64> = const { Cell::new(f64::NAN) };
    /// Whether the page held the game last refresh.
    static HELD: Cell<bool> = const { Cell::new(false) };
}

/// The attribute on `<html>` by which the page holds the game.
const HOLD_ATTRIBUTE: &str = "data-hold";

fn page_holds() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .is_some_and(|root| root.has_attribute(HOLD_ATTRIBUTE))
}

pub(super) fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now())
}

/// Start the page's frame clock, once.
fn start() {
    use std::rc::Rc;
    use wasm_bindgen::closure::Closure;
    if STARTED.with(|s| s.replace(true)) {
        return;
    }
    type Tick = Closure<dyn FnMut(f64)>;
    let tick: Rc<RefCell<Option<Tick>>> = Rc::new(RefCell::new(None));
    let again = tick.clone();
    *tick.borrow_mut() = Some(Closure::new(move |t: f64| {
        FRAME_TIME.with(|f| f.set(t));
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

/// This frame's time: its rAF timestamp, or now if that is stale.
fn frame_time(now: f64) -> f64 {
    start();
    let t = FRAME_TIME.with(Cell::get);
    let last = LAST_USED.with(Cell::get);
    // Fresh: newer than the last frame's, and from this frame.
    let t = if t.is_finite() && (last.is_nan() || t > last) && now - t < 100.0 {
        t
    } else {
        now
    };
    LAST_USED.with(|l| l.set(t));
    t
}

/// Touch-first devices pace by default: a coarse primary pointer is a
/// finger. Desktops, touchscreen laptops included, draw every refresh.
pub fn coarse_pointer() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(pointer: coarse)").ok().flatten())
        .is_some_and(|m| m.matches())
}

impl PlayerApp {
    /// A rAF call: true when this refresh is drawn. Skipped ones cost a
    /// timestamp and a comparison.
    pub(super) fn web_frame_begin(&mut self) -> bool {
        if self.follow_page_hold() {
            return false;
        }
        if self.frame_pacer.is_none() && self.perf.is_none() {
            return true;
        }
        let now = now_ms();
        let t = frame_time(now);
        let due = self.frame_pacer.as_mut().is_none_or(|p| p.due(t, now));
        if due {
            super::web_perf::record_frame(t);
        }
        due
    }

    /// True while the page holds the game. Going in fades the mix out;
    /// coming out brings it back and resyncs the clock.
    fn follow_page_hold(&mut self) -> bool {
        let held = page_holds();
        if HELD.with(|h| h.replace(held)) != held {
            self.audio.engine.set_held(held);
            if held {
                tracing::info!("[hold] the page holds the game");
            } else {
                self.clock.resync();
                tracing::info!("[hold] the page let go of the game");
            }
        }
        held
    }

    /// A drawn frame's work is done: let the pacer judge it, and pace the
    /// clock to the refresh. Each frame then steps whole refreshes, as on
    /// the desktop: 33.3 ms when four of 120 Hz are held, 41.7 when a
    /// frame ran to five. (Paced to 33.3 itself, frames that miss it would
    /// step 33, 67, 33 of game time.)
    pub(super) fn web_frame_end(&mut self) {
        let Some(pacer) = &mut self.frame_pacer else {
            return;
        };
        if pacer.drew(now_ms()) {
            tracing::info!("[frame-rate] {}", pacer.describe());
        }
        let period = pacer.refresh_ms().map(|ms| ms / 1000.0);
        let current = self.clock.vsync_period();
        let changed = match (period, current) {
            (Some(a), Some(b)) => (a - b).abs() > 1e-5,
            (a, b) => a.is_some() != b.is_some(),
        };
        if changed {
            self.clock.set_vsync_period(period);
        }
    }
}
