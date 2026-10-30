//! The page around a game in a browser (#102, docs/design/web.md): the
//! Begin click and the pointer lock.
//!
//! A browser gives a page sound and a locked pointer only from a user
//! gesture, and takes the lock back on its own when the player presses
//! Escape, without telling winit. So on the web a script's
//! `set_cursor_captured(true)` is a *wish*: the lock is asked for only while
//! the page holds a gesture's activation, and the player reads whether it
//! is held every frame. Mouse motion moves the view only while it is, so
//! a pointer sweeping over the page before the first click turns nothing.
//! When the browser lets go, the player releases the cursor just as
//! Escape does on a desktop, and the game's own click-to-recapture asks
//! again.
//!
//! The page's Begin button (`flint_web.rs`) calls [`begin`] from its click:
//! it locks the pointer then and there, inside the gesture, and leaves word
//! for the player to start its sound on the next frame.

use super::PlayerApp;
use std::cell::Cell;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

/// How long to wait before asking for the lock again after a request the
/// browser didn't grant (Chrome refuses one for about a second after the
/// lock is left with Escape).
const RETRY_MS: f64 = 1000.0;

thread_local! {
    static BEGIN_REQUESTED: Cell<bool> = const { Cell::new(false) };
}

/// What the player knows about the browser's pointer lock.
#[derive(Default)]
pub(super) struct PointerLock {
    /// The lock is held now (read each frame).
    pub held: bool,
    /// When the last request went out, in page milliseconds.
    requested_at: Option<f64>,
}

/// The page's Begin gesture: lock the pointer on the player's canvas now,
/// while the click's activation is live, and have the player start its
/// sound next frame (`PlayerApp::start_audio`). Call it from the click.
pub fn begin() {
    request_lock();
    BEGIN_REQUESTED.with(|b| b.set(true));
}

fn document() -> Option<web_sys::Document> {
    web_sys::window()?.document()
}

fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now())
}

/// Whether the pointer is locked to the page's canvas.
fn lock_held() -> bool {
    document().is_some_and(|d| d.pointer_lock_element().is_some())
}

/// Whether the page holds a gesture's transient activation, so a lock
/// request would be granted. A browser without `navigator.userActivation`
/// is asked anyway.
fn has_activation() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    let activation = js_sys::Reflect::get(&window.navigator(), &"userActivation".into())
        .ok()
        .filter(|v| v.is_object());
    match activation {
        Some(a) => js_sys::Reflect::get(&a, &"isActive".into()).is_ok_and(|v| v.is_truthy()),
        None => true,
    }
}

/// Ask for the lock on the player's canvas. Newer browsers answer with a
/// promise that rejects when they refuse; the refusal is caught here so it
/// doesn't reach the console as an uncaught error.
fn request_lock() {
    let Some(canvas) =
        document().and_then(|d| d.get_element_by_id(super::web_startup::WEB_CANVAS_ID))
    else {
        return;
    };
    let Ok(request) = js_sys::Reflect::get(&canvas, &"requestPointerLock".into())
        .and_then(|f| f.dyn_into::<js_sys::Function>())
    else {
        return;
    };
    if let Ok(answer) = request.call0(&canvas) {
        if let Ok(promise) = answer.dyn_into::<js_sys::Promise>() {
            let refused: Closure<dyn FnMut(JsValue)> = Closure::new(|e: JsValue| {
                tracing::debug!("pointer lock refused: {e:?}");
            });
            let _ = promise.catch(&refused);
            refused.forget();
        }
    }
}

impl PlayerApp {
    /// Once a frame: follow the browser's pointer lock. Ask for it while a
    /// script wants the cursor and the page has a gesture to ask with;
    /// release the cursor when the browser has let go of a lock it held.
    pub(super) fn sync_pointer_lock(&mut self) {
        if BEGIN_REQUESTED.with(|b| b.replace(false)) {
            let sound = self.start_audio();
            tracing::info!(
                "[flint-web] begin: sound {}",
                if sound { "on" } else { "unavailable" }
            );
        }

        let held = lock_held();
        let was_held = std::mem::replace(&mut self.pointer_lock.held, held);
        if held {
            self.pointer_lock.requested_at = None;
            return;
        }
        if !self.cursor_captured {
            return;
        }
        if was_held {
            // Escape, a tab switch or the browser's own reasons: what Escape
            // does on a desktop. The game's recapture asks again.
            self.release_cursor();
            return;
        }
        let due = self
            .pointer_lock
            .requested_at
            .is_none_or(|t| now_ms() - t > RETRY_MS);
        if due && has_activation() {
            self.pointer_lock.requested_at = Some(now_ms());
            request_lock();
        }
    }
}
