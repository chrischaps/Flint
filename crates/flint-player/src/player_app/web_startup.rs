//! Startup in a browser (#99, docs/design/web.md).
//!
//! A desktop blocks on the GPU device in `initialize`. A browser's main
//! thread can't block: the adapter and device arrive as promises, and the
//! event loop has to keep turning while they resolve. So on the web
//! `initialize` makes the canvas and starts `RenderContext::new` with
//! `spawn_local`. The future parks the finished context in a shared slot and
//! wakes the loop with a user event (or a redraw, if the page gave no
//! proxy). `take_ready_render_context` then runs the rest of startup on the
//! loop, exactly as a desktop does after `block_on`.
//!
//! Until then the player is *not ready*: `render_context` is `None`, and
//! `window_event` drops every event, so nothing ticks, draws or reads input
//! against a device that isn't there.

use super::PlayerApp;
use anyhow::Context;
use flint_render::{RenderContext, RenderError};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::platform::web::WindowAttributesExtWebSys;
use winit::window::{Window, WindowAttributes};

/// The id of the page canvas the player draws into. A page without one
/// gets a canvas appended to `<body>`.
pub const WEB_CANVAS_ID: &str = "flint";

/// Where the device request leaves its result for the event loop.
pub(super) type RenderSlot = Rc<RefCell<Option<Result<RenderContext, RenderError>>>>;

/// Attach the window to the page's `#flint` canvas, or append one. The
/// page's CSS sizes it, so no inner size is set here.
pub(super) fn canvas_attributes(attrs: WindowAttributes) -> WindowAttributes {
    use wasm_bindgen::JsCast;
    let canvas = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(WEB_CANVAS_ID))
        .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok());
    match canvas {
        Some(canvas) => attrs.with_canvas(Some(canvas)),
        None => attrs.with_append(true),
    }
}

impl PlayerApp {
    /// Give the player a proxy on its event loop, so the device's arrival
    /// wakes the loop with a user event. Call it before `spawn_app`.
    pub fn set_event_proxy(&mut self, proxy: EventLoopProxy<()>) {
        self.event_proxy = Some(proxy);
    }

    /// Start the device request; `take_ready_render_context` finishes
    /// startup when it lands.
    pub(super) fn request_render_context(&mut self, window: Arc<Window>) {
        let slot = self.pending_render.clone();
        let proxy = self.event_proxy.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = RenderContext::new(window.clone()).await;
            *slot.borrow_mut() = Some(result);
            let woke = proxy.is_some_and(|p| p.send_event(()).is_ok());
            if !woke {
                window.request_redraw();
            }
        });
    }

    /// If the device has arrived, run the rest of startup. Called from
    /// `user_event` and from any window event while not ready.
    pub(super) fn take_ready_render_context(&mut self, event_loop: &ActiveEventLoop) {
        let Some(result) = self.pending_render.borrow_mut().take() else {
            return;
        };
        let Some(window) = self.window.clone() else {
            return;
        };
        let started = result
            .context("Failed to initialize render context")
            .and_then(|ctx| self.finish_initialize(window.clone(), ctx));
        match started {
            Ok(()) => {
                // The canvas may have been laid out again while the device
                // was on its way; size the surface to what it is now.
                self.handle_resize(window.inner_size());
                window.request_redraw();
            }
            Err(e) => {
                tracing::error!("Failed to initialize player: {e:#}");
                event_loop.exit();
            }
        }
    }
}
