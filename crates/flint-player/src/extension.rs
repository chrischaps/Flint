//! Player extensions: Rust code an embedding game installs into
//! `PlayerApp` (the first step toward the roadmap's plugin system; ADR 0071).
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

use flint_audio::{AudioSystem, Bus};
use flint_core::{EntityId, Vec3};
use flint_ecs::FlintWorld;
use flint_import::ImportedMaterial;
use flint_render::{Camera, GrassGround, SceneRenderer, Vertex};
use flint_runtime::{GameClock, GameEvent, InputState, PersistentStore};
use winit::event::{ElementState, WindowEvent};
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};

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

    /// See a key go down or up before the player does: a real one, or one
    /// an `--input-script` plays (ADR 0103). Return `true` to consume it,
    /// as for `on_window_event`.
    ///
    /// A scripted key has no winit `KeyEvent` (nothing outside winit can
    /// build one), so it never reaches `on_window_event`. Read keys here and
    /// a capture can press them. A real key comes here straight after this
    /// extension's `on_window_event` passes on its `KeyboardInput`, so
    /// handle a key in one hook or the other, not both. Keys winit can't
    /// name (`PhysicalKey::Unidentified`), and the synthetic presses winit
    /// sends for keys held as the window gains focus, come only to
    /// `on_window_event`.
    ///
    /// egui sees keys first, as it does window events: a key typed into a
    /// focused field never arrives here.
    fn on_key(&mut self, _key: &KeyInput) -> bool {
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

    /// Add to or change egui's fonts before the player installs them
    /// (ADR 0081). `defs` already holds the project's `fonts/` (each file
    /// under its stem, the `fonts.toml` aliases, and its `[egui]` defaults).
    ///
    /// Called whenever the player sets egui's fonts: at start, on a scene
    /// change into another project root, and when an extension is added
    /// after the fonts are in. Fonts set here survive those reloads, which
    /// fonts set with `Context::set_fonts` from `ui` do not. A named family
    /// added here is one scripts can draw in too.
    ///
    /// Families exist from the first egui pass after the call, so laying
    /// text out in one from `ui` is safe on every frame.
    fn fonts(&mut self, _defs: &mut egui::FontDefinitions) {}

    /// Name the game's own debug panels (ADR 0107). Each joins the
    /// engine's (grass, ocean, camera, ...): F3 opens and closes it with
    /// them, and it docks in their columns under its name. The player asks
    /// when it builds its panels, on each scene load and when the extension
    /// is added, so a name given later appears at the next scene load.
    /// Empty, the default, adds none.
    ///
    /// Only a player built with its `debug-hud` feature (the default) has
    /// debug panels. Without it this is never called.
    fn debug_panels(&self) -> Vec<String> {
        Vec::new()
    }

    /// Draw the contents of the debug panel `name`, one that
    /// [`debug_panels`](Self::debug_panels) named. Called every frame while
    /// the panel is open, after [`ui`](Self::ui), so it reads the game's
    /// state as `ui` left it. The heading and the scrolling are the
    /// player's.
    fn debug_panel_ui(&mut self, _name: &str, _ui: &mut egui::Ui) {}
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
    /// The store scripts read and write with `persist_get` / `persist_set`
    /// (`save/persist.toml`; ADR 0080). Writes here are visible to scripts
    /// this frame, and join the player's save: one second after a burst of
    /// changes, on scene transitions and on exit. Call
    /// [`PersistentStore::request_flush`] to save at the end of this frame
    /// instead. Scripts can read every key, so keep secrets out of it.
    pub persist: &'a mut PersistentStore,
    /// The player's audio: play sounds, load sounds made in memory, and set
    /// the mixer buses (ADR 0084). See [`ExtensionAudio`].
    pub audio: ExtensionAudio<'a>,
    /// The meshes uploaded as ray targets ([`MeshOptions::ray_target`]),
    /// where the last frame drew them (ADR 0105): cast a ray or a segment
    /// through it to find the first thing drawn along it. Empty until the
    /// first frame is drawn. Scripts cast through the same scene.
    pub rays: std::sync::Arc<flint_core::ray_mesh::RayScene>,
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

/// The player's audio, as an extension reaches it (ADR 0084).
///
/// Sounds are cached by name, the same cache scripts' `play_sound` and
/// `audio_source.file` read: a sound loaded here can be played from a
/// script, and a file the scene preloaded (`audio/<file>`) can be played
/// from here. Loading needs no audio device; playing without one does
/// nothing, so a game needn't check.
///
/// ```ignore
/// // Once: a sound made in code, mono at 44.1 kHz.
/// ctx.audio.load_samples("blip", 44_100, &[&samples])?;
/// // Any frame:
/// ctx.audio.play("blip", &SoundOptions::new().volume(0.5).pitch(1.2))?;
/// ctx.audio.play("audio/door.ogg", &SoundOptions::new().at(pos).distances(2.0, 60.0))?;
/// // Heard from far off, fading like a sound in the open:
/// ctx.audio.play("bell", &SoundOptions::new().at(pos).distances(10.0, 400.0).rolloff(0.7))?;
/// ctx.audio.set_bus_volume(Bus::Music, 0.4);
/// ```
pub struct ExtensionAudio<'a> {
    pub(crate) system: &'a mut AudioSystem,
}

impl<'a> ExtensionAudio<'a> {
    pub(crate) fn new(system: &'a mut AudioSystem) -> Self {
        Self { system }
    }

    /// Whether there's a device to play on. Without one, every sound is
    /// silent, though loading still works.
    pub fn is_available(&self) -> bool {
        self.system.engine.is_available()
    }

    /// Whether a sound is cached under `name`.
    pub fn has_sound(&self, name: &str) -> bool {
        self.system.engine.has_sound(name)
    }

    /// Cache a sound made in memory: one channel (heard on both sides) or
    /// two (left, right) of samples in -1..1 at `sample_rate` Hz. Replaces
    /// any sound cached under `name`; one already playing plays on.
    ///
    /// A looping `audio_source` naming a sound that isn't cached yet stays
    /// silent, so load before spawning the entity that plays it.
    pub fn load_samples(
        &mut self,
        name: &str,
        sample_rate: u32,
        channels: &[&[f32]],
    ) -> flint_core::Result<()> {
        self.system
            .engine
            .load_sound_from_samples(name, sample_rate, channels)
    }

    /// Cache an encoded sound held in memory (WAV, OGG, FLAC or MP3), such
    /// as one from `include_bytes!`. Replaces any sound cached under `name`.
    pub fn load_bytes(&mut self, name: &str, bytes: Vec<u8>) -> flint_core::Result<()> {
        self.system.engine.load_sound_from_bytes(name, bytes)
    }

    /// Play the cached sound `name` once. An unknown name is an error;
    /// with no device it does nothing.
    pub fn play(&mut self, name: &str, options: &SoundOptions) -> flint_core::Result<()> {
        let engine = &mut self.system.engine;
        if !engine.has_sound(name) {
            return Err(flint_core::FlintError::AudioError(format!(
                "Sound not cached: {name}"
            )));
        }
        if !engine.is_available() {
            return Ok(());
        }
        match options.at {
            Some(at) => engine.play_oneshot_at(
                name,
                at,
                options.volume,
                options.pitch,
                options.distances,
                options.rolloff,
                options.bus,
            ),
            None => engine
                .play_non_spatial(name, options.volume, options.pitch, false, options.bus)
                .map(drop),
        }
    }

    /// Set a mixer bus's volume (linear: 0 mutes, 1 is unity). It scales
    /// everything on the bus, scripts' sounds and `audio_source`s included,
    /// on top of their own volumes. It stays set across scene transitions.
    pub fn set_bus_volume(&mut self, bus: Bus, volume: f64) {
        self.system.set_bus_volume(bus, volume);
    }
}

/// How [`ExtensionAudio::play`] plays a sound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundOptions {
    /// Linear: 1 is the sound as made.
    pub volume: f64,
    /// Playback rate: 2 is an octave up and twice as fast.
    pub pitch: f64,
    pub bus: Bus,
    /// Where the sound is, world units, so it pans and fades with distance
    /// from the listener. `None` plays it flat, from nowhere.
    pub at: Option<Vec3>,
    /// For a placed sound, `(min, max)` from the listener: full volume
    /// within `min`, silent past `max`.
    pub distances: (f32, f32),
    /// The curve between them (see [`flint_audio::engine::DEFAULT_ROLLOFF`]):
    /// 2 holds the sound up and drops it near `max`; below 1 it falls away
    /// soon after `min` and tails off, like a real sound in the open.
    pub rolloff: f32,
}

impl Default for SoundOptions {
    fn default() -> Self {
        Self {
            volume: 1.0,
            pitch: 1.0,
            bus: Bus::Sfx,
            at: None,
            distances: flint_audio::engine::DEFAULT_DISTANCES,
            rolloff: flint_audio::engine::DEFAULT_ROLLOFF,
        }
    }
}

impl SoundOptions {
    /// Full volume, normal pitch, on the sfx bus, from nowhere.
    pub fn new() -> Self {
        Self::default()
    }
    pub fn volume(self, volume: f64) -> Self {
        Self { volume, ..self }
    }
    pub fn pitch(self, pitch: f64) -> Self {
        Self { pitch, ..self }
    }
    pub fn bus(self, bus: Bus) -> Self {
        Self { bus, ..self }
    }
    /// Place the sound at `at`.
    pub fn at(self, at: Vec3) -> Self {
        Self {
            at: Some(at),
            ..self
        }
    }
    /// The falloff of a placed sound (default 1 to 25).
    pub fn distances(self, min: f32, max: f32) -> Self {
        Self {
            distances: (min, max),
            ..self
        }
    }
    /// The falloff curve of a placed sound (default 2).
    pub fn rolloff(self, rolloff: f32) -> Self {
        Self { rolloff, ..self }
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

    /// [`upload_mesh`](Self::upload_mesh), with [`MeshOptions`]: a mesh
    /// uploaded with `ray_target` is met by rays cast through
    /// [`ExtensionCtx::rays`] and by scripts' `raycast_meshes` and
    /// `screen_to_mesh`, wherever an entity draws it (ADR 0105). An upload
    /// without it, or [`remove_mesh`](Self::remove_mesh), stops it being one.
    pub fn upload_mesh_with(
        &mut self,
        name: &str,
        vertices: &[Vertex],
        indices: &[u32],
        material: ImportedMaterial,
        options: MeshOptions,
    ) {
        self.upload_mesh(name, vertices, indices, material);
        if options.ray_target {
            let positions: Vec<[f32; 3]> = vertices.iter().map(|v| v.position).collect();
            let mesh = flint_core::ray_mesh::RayMesh::new(&positions, indices);
            self.renderer
                .set_ray_mesh(name, Some(std::sync::Arc::new(mesh)));
        }
    }

    /// Remove the runtime mesh `name`; entities naming it draw nothing
    /// until it is uploaded again. Returns whether it existed.
    pub fn remove_mesh(&mut self, name: &str) -> bool {
        self.renderer.remove_mesh(name)
    }

    /// Grow compute grass on the game's own ground: a density mask and a
    /// height over a world rectangle (ADR 0089). Replaces any grass loaded,
    /// a terrain's included. Call again whenever the ground changes, and
    /// [`set_grass_config`](Self::set_grass_config) to retune it without
    /// re-uploading the mask. Errs only when the mask or heights don't
    /// match their stated sizes; on a device with no compute grass it does
    /// nothing.
    pub fn set_grass_ground(
        &mut self,
        config: &flint_terrain::GrassConfig,
        ground: &GrassGround,
    ) -> Result<(), String> {
        self.renderer
            .set_grass_ground(self.device, self.queue, config, ground)
    }

    /// Change the grass's look (colours, wind, density, reach) and keep its
    /// ground. The instance buffer grows on the next frame if it must.
    pub fn set_grass_config(&mut self, config: flint_terrain::GrassConfig) {
        self.renderer.set_grass_config(config);
    }

    /// Repaint part of the grass ground's mask in place (ADR 0093): the
    /// `width × height` texels at (`x`, `y`), from `bytes`, one a texel,
    /// row by row toward +Z. Much cheaper than a whole
    /// [`set_grass_ground`](Self::set_grass_ground) for a small change,
    /// such as a clearing round something placed. Errs when no ground is
    /// set, or the rectangle runs off the mask or `bytes` is the wrong
    /// length; on a device with no compute grass it does nothing.
    pub fn update_grass_mask(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        bytes: &[u8],
    ) -> Result<(), String> {
        self.renderer
            .update_grass_mask(self.queue, x, y, width, height, bytes)
    }

    /// Remove the grass, whatever ground it grows on.
    pub fn clear_grass(&mut self) {
        self.renderer.unload_grass();
    }

    /// Lay a ground fog over a world rectangle (ADR 0106): each texel of
    /// the map has a floor and an amount, and the fog lies on that floor,
    /// thinning upward, so a mist can fill valleys at many heights where
    /// the height fog is one level layer. Replaces any set. How it looks
    /// is the post-process config's `ground_fog_*` (it shows only while
    /// `ground_fog_density` is over 0), so set the map once, and again
    /// when the ground changes, and change the look as often as you like.
    /// Errs when the map's texels don't fill its size, or a value isn't
    /// finite; a map with no fog on it clears it.
    pub fn set_ground_fog(&mut self, fog: &crate::GroundFog) -> Result<(), String> {
        self.renderer.set_ground_fog(self.device, self.queue, fog)
    }

    /// Remove the ground fog. Returns whether one was set.
    pub fn clear_ground_fog(&mut self) -> bool {
        self.renderer.clear_ground_fog()
    }

    /// Draw water on the game's own mesh (ADR 0094): a lake at its own
    /// level, a river stepping down a valley. It shades as the `ocean` does
    /// (waves, Fresnel, refraction, the cel ramp, shoreline foam), with the
    /// body's tint, wave scale and flow; fast flow foams white. Build the
    /// mesh with [`WaterBodyMesh::lake`](crate::WaterBodyMesh::lake) or
    /// [`river`](crate::WaterBodyMesh::river), or supply your own. Replaces
    /// any body set with this `id`. Errs when the mesh's indices are bad.
    pub fn set_water_body(
        &mut self,
        id: &str,
        mesh: &crate::WaterBodyMesh,
        look: crate::WaterBodyLook,
    ) -> Result<(), String> {
        self.renderer.set_water_body(self.device, id, mesh, look)
    }

    /// Change a water body's look and keep its mesh. Returns whether it existed.
    pub fn set_water_body_look(&mut self, id: &str, look: crate::WaterBodyLook) -> bool {
        self.renderer.set_water_body_look(id, look)
    }

    /// Remove the water body set with `id`. Returns whether it existed.
    pub fn remove_water_body(&mut self, id: &str) -> bool {
        self.renderer.remove_water_body(id)
    }

    /// Remove every water body the game set (a scene's `water_body`
    /// components stay).
    pub fn clear_water_bodies(&mut self) {
        self.renderer.clear_water_bodies();
    }
}

/// How a runtime mesh takes part beyond being drawn, for
/// [`ExtensionGpu::upload_mesh_with`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MeshOptions {
    /// Rays meet it (ADR 0105): the camera can keep out of it and a click
    /// can pick it. Its triangles are kept on the CPU in a bounding volume
    /// hierarchy, built at upload, so mark only what rays need to meet.
    pub ray_target: bool,
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

/// A key going down or up, for [`PlayerExtension::on_key`]: a real one or
/// one an `--input-script` plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyInput<'a> {
    /// The key where it sits on the keyboard, whatever the layout.
    pub key: KeyCode,
    pub state: ElementState,
    /// The OS repeating a held key. Never set on a scripted key.
    pub repeat: bool,
    /// The character the key stands for (winit's `logical_key`, when it's
    /// a character), on the release as on the press, and under Ctrl too:
    /// `?` for Shift and Slash on a US layout. A scripted key is read on a
    /// US layout.
    pub character: Option<&'a str>,
    /// The modifiers held as the key moved.
    pub modifiers: ModifiersState,
    /// Whether an `--input-script` pressed it.
    pub scripted: bool,
}

impl KeyInput<'_> {
    /// A first press: down, and not the OS repeating it.
    pub fn pressed(&self) -> bool {
        self.state == ElementState::Pressed && !self.repeat
    }
}

/// The `KeyInput` a real `KeyboardInput` is, if `on_key` should see it.
fn key_input<'a>(event: &'a WindowEvent, modifiers: ModifiersState) -> Option<KeyInput<'a>> {
    let WindowEvent::KeyboardInput {
        event,
        is_synthetic: false,
        ..
    } = event
    else {
        return None;
    };
    let PhysicalKey::Code(key) = event.physical_key else {
        return None;
    };
    let character = match &event.logical_key {
        Key::Character(c) => Some(c.as_str()),
        _ => None,
    };
    Some(KeyInput {
        key,
        state: event.state,
        repeat: event.repeat,
        character,
        modifiers,
        scripted: false,
    })
}

/// Offer `event` to each extension in order. Returns whether one consumed
/// it; the rest are skipped once one has. Non-consumable events go to
/// every extension and are never reported as consumed. A key goes to each
/// extension's `on_window_event` and then its `on_key`, with `modifiers`
/// the ones held now.
pub(crate) fn dispatch_window_event(
    extensions: &mut [Box<dyn PlayerExtension>],
    event: &WindowEvent,
    modifiers: ModifiersState,
) -> bool {
    let consumable = is_consumable(event);
    let key = key_input(event, modifiers);
    for ext in extensions.iter_mut() {
        if ext.on_window_event(event) && consumable {
            return true;
        }
        if key.as_ref().is_some_and(|key| ext.on_key(key)) {
            return true;
        }
    }
    false
}

/// Offer a scripted key to each extension's `on_key` in order. Returns
/// whether one consumed it.
pub(crate) fn dispatch_key(extensions: &mut [Box<dyn PlayerExtension>], key: &KeyInput) -> bool {
    extensions.iter_mut().any(|ext| ext.on_key(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn an_extension_loads_and_plays_sounds_made_in_memory() {
        use flint_runtime::RuntimeSystem;
        // As the player does at scene load: the listener placed sounds need.
        let mut system = AudioSystem::new();
        system.initialize(&mut FlintWorld::new()).unwrap();
        let mut audio = ExtensionAudio::new(&mut system);
        let click: Vec<f32> = (0..200).map(|i| 1.0 - i as f32 / 200.0).collect();
        audio.load_samples("click", 44_100, &[&click]).unwrap();
        assert!(audio.has_sound("click"));
        // Flat and placed, with or without a device.
        audio
            .play("click", &SoundOptions::new().volume(0.5).pitch(1.3))
            .unwrap();
        let placed = SoundOptions::new()
            .at(Vec3::new(4.0, 0.0, 2.0))
            .distances(2.0, 80.0)
            .rolloff(0.7)
            .bus(Bus::Music);
        assert_eq!(placed.distances, (2.0, 80.0));
        assert_eq!(placed.rolloff, 0.7);
        assert_eq!(SoundOptions::new().rolloff, 2.0);
        audio.play("click", &placed).unwrap();
        assert!(audio.play("missing", &SoundOptions::new()).is_err());
        audio.set_bus_volume(Bus::Sfx, 0.25);
        // Scripts and audio_sources read the same cache.
        assert!(system.engine.has_sound("click"));
    }

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
        fn on_key(&mut self, key: &KeyInput) -> bool {
            self.log.lock().unwrap().push(format!(
                "{} key {:?} {:?}",
                self.name, key.key, key.character
            ));
            self.consume
        }
    }

    fn dispatch(exts: &mut [Box<dyn PlayerExtension>], event: &WindowEvent) -> bool {
        dispatch_window_event(exts, event, ModifiersState::empty())
    }

    fn question_mark() -> KeyInput<'static> {
        KeyInput {
            key: KeyCode::Slash,
            state: ElementState::Pressed,
            repeat: false,
            character: Some("?"),
            modifiers: ModifiersState::SHIFT,
            scripted: true,
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
        assert!(!dispatch(&mut exts, &cursor_moved()));
        assert_eq!(*log.lock().unwrap(), ["a event", "b event"]);
    }

    #[test]
    fn first_consumer_stops_input_events() {
        let (mut exts, log) = recorders([true, false]);
        assert!(dispatch(&mut exts, &cursor_moved()));
        assert_eq!(*log.lock().unwrap(), ["a event"]);
    }

    #[test]
    fn lifecycle_events_cannot_be_consumed() {
        let (mut exts, log) = recorders([true, true]);
        let resized = WindowEvent::Resized(winit::dpi::PhysicalSize::new(800, 600));
        assert!(!dispatch(&mut exts, &resized));
        assert!(!dispatch(&mut exts, &WindowEvent::RedrawRequested));
        assert!(!dispatch(&mut exts, &WindowEvent::CloseRequested));
        // Every extension still saw every event.
        assert_eq!(log.lock().unwrap().len(), 6);
    }

    #[test]
    fn default_methods_do_nothing() {
        struct Empty;
        impl PlayerExtension for Empty {}
        let mut exts: Vec<Box<dyn PlayerExtension>> = vec![Box::new(Empty)];
        assert!(!dispatch(&mut exts, &cursor_moved()));
        assert!(!dispatch_key(&mut exts, &question_mark()));
    }

    #[test]
    fn a_key_reaches_every_extension_until_one_takes_it() {
        let (mut exts, log) = recorders([false, false]);
        assert!(!dispatch_key(&mut exts, &question_mark()));
        assert_eq!(
            *log.lock().unwrap(),
            ["a key Slash Some(\"?\")", "b key Slash Some(\"?\")"]
        );

        let (mut exts, log) = recorders([true, false]);
        assert!(dispatch_key(&mut exts, &question_mark()));
        assert_eq!(*log.lock().unwrap(), ["a key Slash Some(\"?\")"]);
    }

    #[test]
    fn only_a_first_press_is_pressed() {
        let press = question_mark();
        assert!(press.pressed());
        assert!(!KeyInput {
            repeat: true,
            ..press
        }
        .pressed());
        assert!(!KeyInput {
            state: ElementState::Released,
            ..press
        }
        .pressed());
    }
}
