//! A hidden page sleeps (#162): its sound stops and its game holds.
//!
//! A browser keeps running a page the player has left, another app on
//! Android or another tab on a desktop, but throttles its main thread. The
//! main-thread stream (`crate::web`) then misses buffers and stutters, and
//! the worklet (`crate::worklet`) mixes on as if the player were listening.
//! So when the page is hidden, every `AudioContext` Flint plays through
//! that is running is suspended, and when it is shown again those, and
//! only those, are resumed: a context still waiting for its first gesture
//! stays waiting, as it would have.
//!
//! Suspended, a context's clock stops, so kira's clocks and the rhythm
//! sessions timed against them stop with it. The game stops too: hidden
//! pages get no `requestAnimationFrame`, so no frame runs, and the player
//! skips the gap on its first frame back ([`sleeps`] tells it there was
//! one). Sound and game come back together, where they were.
//!
//! [`PageSleep`] decides; it is plain Rust so the tests run off the
//! browser. The wasm half below feeds it the page's `visibilitychange`,
//! `pagehide` and `pageshow`.

/// What [`PageSleep`] needs of an audio context.
pub trait SleepingContext {
    /// Playing now (not suspended, not closed, not interrupted).
    fn running(&self) -> bool;
}

/// The page's sleep and the contexts it put to sleep.
#[derive(Debug)]
pub struct PageSleep<C> {
    hidden: bool,
    slept: Vec<C>,
    sleeps: u64,
}

impl<C> Default for PageSleep<C> {
    fn default() -> Self {
        Self {
            hidden: false,
            slept: Vec::new(),
            sleeps: 0,
        }
    }
}

impl<C: SleepingContext> PageSleep<C> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The page was hidden (`visibilitychange` to hidden, or `pagehide`).
    /// Returns the contexts to suspend: those of `contexts` running now.
    /// A second hide before a show (`pagehide` follows `visibilitychange`)
    /// returns none.
    pub fn hide(&mut self, contexts: impl IntoIterator<Item = C>) -> &[C] {
        if self.hidden {
            return &[];
        }
        self.hidden = true;
        self.sleeps += 1;
        self.slept = contexts.into_iter().filter(|c| c.running()).collect();
        &self.slept
    }

    /// The page was shown again. Returns the contexts [`hide`](Self::hide)
    /// suspended, to resume; a show without a hide returns none.
    pub fn show(&mut self) -> Vec<C> {
        if !self.hidden {
            return Vec::new();
        }
        self.hidden = false;
        std::mem::take(&mut self.slept)
    }

    /// Whether the page is hidden.
    pub fn hidden(&self) -> bool {
        self.hidden
    }

    /// How many times the page has been hidden. A player that saw a
    /// different count last frame slept in between.
    pub fn sleeps(&self) -> u64 {
        self.sleeps
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use super::{PageSleep, SleepingContext};
    use std::cell::{Cell, RefCell};
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(module = "/src/worklet.js")]
    extern "C" {
        fn flint_audio_contexts() -> js_sys::Array;
        fn flint_audio_sleep();
    }

    impl SleepingContext for web_sys::AudioContext {
        fn running(&self) -> bool {
            self.state() == web_sys::AudioContextState::Running
        }
    }

    thread_local! {
        static SLEEP: RefCell<PageSleep<web_sys::AudioContext>> = RefCell::new(PageSleep::new());
        static LISTENING: Cell<bool> = const { Cell::new(false) };
    }

    fn document() -> Option<web_sys::Document> {
        web_sys::window().and_then(|w| w.document())
    }

    fn contexts() -> Vec<web_sys::AudioContext> {
        flint_audio_contexts()
            .iter()
            .filter_map(|c| c.dyn_into().ok())
            .collect()
    }

    fn hide(why: &str) {
        SLEEP.with(|s| {
            let mut s = s.borrow_mut();
            let was_hidden = s.hidden();
            if !was_hidden {
                flint_audio_sleep();
            }
            let slept = s.hide(contexts());
            for ctx in slept {
                let _ = ctx.suspend();
            }
            if !was_hidden {
                web_sys::console::log_1(
                    &format!(
                        "[flint-audio] page hidden ({why}): {} AudioContext(s) suspended, game held",
                        slept.len()
                    )
                    .into(),
                );
            }
        });
    }

    fn show(why: &str) {
        SLEEP.with(|s| {
            let mut s = s.borrow_mut();
            if !s.hidden() {
                return;
            }
            let woken = s.show();
            for ctx in &woken {
                let _ = ctx.resume();
            }
            web_sys::console::log_1(
                &format!(
                    "[flint-audio] page shown ({why}): {} AudioContext(s) resumed",
                    woken.len()
                )
                .into(),
            );
        });
    }

    /// Listen for the page being hidden and shown, once.
    pub fn listen() {
        if LISTENING.with(|l| l.replace(true)) {
            return;
        }
        let Some(doc) = document() else { return };
        let on_visibility = Closure::<dyn FnMut()>::new(|| {
            if document().is_some_and(|d| d.hidden()) {
                hide("visibilitychange");
            } else {
                show("visibilitychange");
            }
        });
        let _ = doc.add_event_listener_with_callback(
            "visibilitychange",
            on_visibility.as_ref().unchecked_ref(),
        );
        on_visibility.forget();
        if let Some(window) = web_sys::window() {
            // pagehide: the page is put in the back/forward cache, or left.
            // pageshow: it came back from that cache, perhaps still hidden.
            let on_pagehide = Closure::<dyn FnMut()>::new(|| hide("pagehide"));
            let _ = window
                .add_event_listener_with_callback("pagehide", on_pagehide.as_ref().unchecked_ref());
            on_pagehide.forget();
            let on_pageshow = Closure::<dyn FnMut()>::new(|| {
                if !document().is_some_and(|d| d.hidden()) {
                    show("pageshow");
                }
            });
            let _ = window
                .add_event_listener_with_callback("pageshow", on_pageshow.as_ref().unchecked_ref());
            on_pageshow.forget();
        }
    }

    /// The page is hidden now.
    pub fn hidden() -> bool {
        SLEEP.with(|s| s.borrow().hidden()) || document().is_some_and(|d| d.hidden())
    }

    /// How many times the page has been hidden since [`listen`].
    pub fn sleeps() -> u64 {
        SLEEP.with(|s| s.borrow().sleeps())
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::{hidden, listen, sleeps};

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    /// A context that is running or not.
    #[derive(Clone, Debug)]
    struct Ctx {
        name: &'static str,
        running: Rc<Cell<bool>>,
    }

    impl Ctx {
        fn new(name: &'static str, running: bool) -> Self {
            Self {
                name,
                running: Rc::new(Cell::new(running)),
            }
        }
    }

    impl SleepingContext for Ctx {
        fn running(&self) -> bool {
            self.running.get()
        }
    }

    fn names(cs: &[Ctx]) -> Vec<&'static str> {
        cs.iter().map(|c| c.name).collect()
    }

    #[test]
    fn hiding_suspends_the_running_and_showing_resumes_them() {
        let worklet = Ctx::new("worklet", true);
        let mut sleep = PageSleep::new();
        assert_eq!(names(sleep.hide([worklet.clone()])), ["worklet"]);
        assert!(sleep.hidden());
        assert_eq!(names(&sleep.show()), ["worklet"]);
        assert!(!sleep.hidden());
    }

    #[test]
    fn a_context_not_yet_running_is_left_to_its_gesture() {
        // Before Begin, or an iPad's context waiting for a tap: hiding it
        // doesn't touch it, and showing doesn't resume it outside a gesture.
        let waiting = Ctx::new("waiting", false);
        let playing = Ctx::new("playing", true);
        let mut sleep = PageSleep::new();
        assert_eq!(names(sleep.hide([waiting, playing])), ["playing"]);
        assert_eq!(names(&sleep.show()), ["playing"]);
    }

    #[test]
    fn pagehide_after_visibilitychange_is_one_sleep() {
        let ctx = Ctx::new("ctx", true);
        let mut sleep = PageSleep::new();
        assert_eq!(sleep.hide([ctx.clone()]).len(), 1);
        // Suspended now; the second hide must not forget it.
        ctx.running.set(false);
        assert!(sleep.hide([ctx.clone()]).is_empty());
        assert_eq!(sleep.sleeps(), 1);
        assert_eq!(names(&sleep.show()), ["ctx"]);
        // pageshow after visibilitychange: nothing more to resume.
        assert!(sleep.show().is_empty());
    }

    #[test]
    fn a_show_without_a_hide_resumes_nothing() {
        let mut sleep: PageSleep<Ctx> = PageSleep::new();
        assert!(sleep.show().is_empty());
        assert_eq!(sleep.sleeps(), 0);
    }

    #[test]
    fn each_hide_counts_so_the_player_skips_each_gap() {
        let ctx = Ctx::new("ctx", true);
        let mut sleep = PageSleep::new();
        for n in 1..=3 {
            sleep.hide([ctx.clone()]);
            sleep.show();
            assert_eq!(sleep.sleeps(), n);
        }
    }
}
