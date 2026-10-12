//! Player extensions: Rust code an embedding game installs into
//! `PlayerApp` (the first step toward the roadmap's plugin system).
//!
//! A game that wraps `PlayerApp` in its own binary can add native Rhai
//! functions, run per-frame Rust before the scripts, and see window events
//! before the player does. Every method has a no-op default, and a player
//! with no extensions behaves exactly as it did before they existed.
//!
//! ```ignore
//! struct MyGame { /* shared state */ }
//!
//! impl PlayerExtension for MyGame {
//!     fn register_script_api(&mut self, engine: &mut rhai::Engine) {
//!         engine.register_fn("my_score", || 42_i64);
//!     }
//!     fn pre_update(&mut self, ctx: &mut ExtensionCtx) {
//!         // apply network state to ctx.world before on_update runs
//!     }
//! }
//!
//! player.add_extension(Box::new(MyGame { .. }));
//! ```

use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_import::ImportedMaterial;
use flint_render::{Camera, SceneRenderer, Vertex};
use flint_runtime::{GameClock, GameEvent, InputState};
use winit::event::WindowEvent;

/// Hooks an embedding game installs with `PlayerApp::add_extension`.
///
/// Extensions run in the order they were added. All methods default to
/// doing nothing, so an extension implements only what it needs.
pub trait PlayerExtension {
    /// Register native functions, types or modules on the script engine.
    ///
    /// Called once, from `add_extension`. The engine outlives scene
    /// transitions, so registrations persist for the whole run. Flint's own
    /// API is registered first; a function registered here with the same
    /// name and parameter types replaces the engine's.
    ///
    /// The engine uses rhai's `sync` feature, so registered closures and
    /// the state they capture must be `Send + Sync` (`Arc<Mutex<_>>`,
    /// atomics).
    fn register_script_api(&mut self, _engine: &mut rhai::Engine) {}

    /// Per-frame Rust, run after the clock ticks and before the scripts'
    /// `on_update`, so anything written to the world here is visible to
    /// scripts in the same frame.
    fn pre_update(&mut self, _ctx: &mut ExtensionCtx) {}

    /// See a window event before the player does. Return `true` to consume
    /// it. Only input events (keyboard, IME, mouse buttons, wheel, cursor
    /// movement, touch) can be consumed; lifecycle events such as
    /// `Resized`, `CloseRequested` and `RedrawRequested` always reach the
    /// player whatever this returns. The first extension to consume an
    /// event stops it reaching later extensions.
    ///
    /// egui sees events first (see `ui`): input it wants, such as typing
    /// in a focused text field or a click on an egui window, never arrives
    /// here.
    fn on_window_event(&mut self, _event: &WindowEvent) -> bool {
        false
    }

    /// Draw the game's own egui UI. Called every frame from inside the
    /// player's egui pass, after the scripts' `on_draw_ui` and before the
    /// debug panels, so it draws over the script HUD and under the panels.
    ///
    /// Installing any extension routes window events through egui first.
    /// An event egui wants is consumed, and extensions, the player and
    /// scripts never see it: keys and IME while a widget has keyboard
    /// focus (a `TextEdit` being typed in, so Escape there unfocuses the
    /// field rather than quitting), and clicks and the wheel over an egui
    /// area. Clicks on empty space still reach the game.
    ///
    /// Use windows and areas, or side and top panels. A `CentralPanel`
    /// covers the whole window, so it takes every click from the game.
    fn ui(&mut self, _ctx: &egui::Context) {}
}

/// What an extension can reach during `pre_update`.
pub struct ExtensionCtx<'a> {
    pub world: &'a mut FlintWorld,
    /// Already ticked for this frame.
    pub clock: &'a GameClock,
    /// This frame's input, before scripts read it.
    pub input: &'a InputState,
    /// Last frame's camera; scripts and physics may move it later this frame.
    pub camera: &'a Camera,
    /// The renderer and GPU handles, for uploading meshes and textures.
    /// `None` until the window and renderer exist.
    pub gpu: Option<ExtensionGpu<'a>>,
    /// Events fired with `fire_event`, queued for this frame's scripts.
    pub(crate) fired_events: &'a mut Vec<GameEvent>,
}

impl ExtensionCtx<'_> {
    /// Fire a custom game event, as a script's `fire_event_data` does.
    ///
    /// It joins this frame's events, so scripts' `on_event(name, data)`
    /// receives it before their `on_update` runs this frame. Audio triggers
    /// see it too. A table `data` arrives in scripts as a map; anything else
    /// arrives as `#{ value: data }`.
    ///
    /// ```ignore
    /// let mut data = toml::Table::new();
    /// data.insert("count".into(), 3.into());
    /// ctx.fire_event("inventory_changed", data);
    /// ```
    pub fn fire_event(&mut self, name: impl Into<String>, data: impl Into<toml::Value>) {
        self.fired_events.push(GameEvent::Custom {
            name: name.into(),
            data: data.into(),
        });
    }

    /// Draw the runtime mesh `mesh` at `entity`'s transform, by setting the
    /// entity's `model` to `{ asset = mesh, runtime = true }`. The mesh may be
    /// uploaded before or after (see [`ExtensionGpu::upload_mesh`]); until it
    /// exists the entity draws nothing. Any other `model` fields are kept.
    pub fn attach_mesh(&mut self, entity: EntityId, mesh: &str) -> flint_core::Result<()> {
        let mut model = toml::Table::new();
        model.insert("asset".into(), mesh.into());
        model.insert("runtime".into(), true.into());
        self.world
            .merge_component(entity, flint_core::components::MODEL, model.into())
    }
}

/// The renderer and the device and queue it draws with.
pub struct ExtensionGpu<'a> {
    pub renderer: &'a mut SceneRenderer,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
}

impl ExtensionGpu<'_> {
    /// Create or replace the runtime mesh `name`. Entities whose
    /// `model.asset` is `name` draw it this frame (see
    /// [`ExtensionCtx::attach_mesh`]). Replacing frees the old GPU buffers
    /// once this frame's draw lists let go of them, so a mesh can be rebuilt
    /// as often as its data changes.
    ///
    /// The mesh is culled against the camera by the bounds of `vertices`.
    /// Use [`ImportedMaterial::vertex_colored`] to colour per vertex; vertex
    /// colours are linear, so convert sRGB with `flint_render::srgb_to_linear`.
    pub fn upload_mesh(
        &mut self,
        name: &str,
        vertices: &[Vertex],
        indices: &[u32],
        material: ImportedMaterial,
    ) {
        self.renderer
            .load_procedural_mesh(self.device, name, vertices, indices, material);
    }

    /// Remove the runtime mesh `name`; entities naming it draw nothing
    /// until it is uploaded again. Returns whether it existed.
    pub fn remove_mesh(&mut self, name: &str) -> bool {
        self.renderer.remove_mesh(name)
    }
}

/// Whether an extension may consume `event`. Lifecycle events must always
/// reach the player, or a consuming extension could stall the frame loop
/// or leave the surface the wrong size.
pub(crate) fn is_consumable(event: &WindowEvent) -> bool {
    matches!(
        event,
        WindowEvent::KeyboardInput { .. }
            | WindowEvent::Ime(_)
            | WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::CursorMoved { .. }
            | WindowEvent::Touch(_)
    )
}

/// Offer `event` to each extension in order. Returns whether one consumed
/// it; the rest are skipped once one has. Non-consumable events go to
/// every extension and are never reported as consumed.
pub(crate) fn dispatch_window_event(
    extensions: &mut [Box<dyn PlayerExtension>],
    event: &WindowEvent,
) -> bool {
    let consumable = is_consumable(event);
    for ext in extensions.iter_mut() {
        if ext.on_window_event(event) && consumable {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    type Log = Arc<Mutex<Vec<String>>>;

    /// Records each call; consumes window events when `consume` is set.
    struct Recorder {
        name: &'static str,
        log: Log,
        consume: bool,
    }

    impl PlayerExtension for Recorder {
        fn pre_update(&mut self, _ctx: &mut ExtensionCtx) {
            self.log
                .lock()
                .unwrap()
                .push(format!("{} pre_update", self.name));
        }
        fn on_window_event(&mut self, _event: &WindowEvent) -> bool {
            self.log
                .lock()
                .unwrap()
                .push(format!("{} event", self.name));
            self.consume
        }
    }

    fn recorders(consume: [bool; 2]) -> (Vec<Box<dyn PlayerExtension>>, Log) {
        let log = Log::default();
        let exts: Vec<Box<dyn PlayerExtension>> = vec![
            Box::new(Recorder {
                name: "a",
                log: log.clone(),
                consume: consume[0],
            }),
            Box::new(Recorder {
                name: "b",
                log: log.clone(),
                consume: consume[1],
            }),
        ];
        (exts, log)
    }

    fn cursor_moved() -> WindowEvent {
        WindowEvent::CursorMoved {
            device_id: winit::event::DeviceId::dummy(),
            position: winit::dpi::PhysicalPosition::new(10.0, 20.0),
        }
    }

    #[test]
    fn unconsumed_input_reaches_every_extension_in_order() {
        let (mut exts, log) = recorders([false, false]);
        assert!(!dispatch_window_event(&mut exts, &cursor_moved()));
        assert_eq!(*log.lock().unwrap(), ["a event", "b event"]);
    }

    #[test]
    fn first_consumer_stops_input_events() {
        let (mut exts, log) = recorders([true, false]);
        assert!(dispatch_window_event(&mut exts, &cursor_moved()));
        assert_eq!(*log.lock().unwrap(), ["a event"]);
    }

    #[test]
    fn lifecycle_events_cannot_be_consumed() {
        let (mut exts, log) = recorders([true, true]);
        let resized = WindowEvent::Resized(winit::dpi::PhysicalSize::new(800, 600));
        assert!(!dispatch_window_event(&mut exts, &resized));
        assert!(!dispatch_window_event(
            &mut exts,
            &WindowEvent::RedrawRequested
        ));
        assert!(!dispatch_window_event(
            &mut exts,
            &WindowEvent::CloseRequested
        ));
        // Every extension still saw every event.
        assert_eq!(log.lock().unwrap().len(), 6);
    }

    #[test]
    fn default_methods_do_nothing() {
        struct Empty;
        impl PlayerExtension for Empty {}
        let mut exts: Vec<Box<dyn PlayerExtension>> = vec![Box::new(Empty)];
        assert!(!dispatch_window_event(&mut exts, &cursor_moved()));
    }
}
