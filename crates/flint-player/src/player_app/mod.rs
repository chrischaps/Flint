//! Player application implementing winit ApplicationHandler
//!
//! Runs the game loop with physics, input, and first-person camera.

#[cfg(test)]
mod asset_search_tests;
mod camera_cut;
#[cfg(test)]
mod capture_gate_tests;
#[cfg(test)]
mod debug_key_tests;
mod debug_panels;
mod egui_input;
#[cfg(test)]
mod event_bus_tests;
mod events;
pub mod expression_feed;
#[cfg(test)]
mod extension_tests;
pub mod fonts;
mod frame;
pub mod frame_rate;
pub mod game_core;
pub mod headless_animation;
pub mod headless_game;
#[cfg(test)]
mod hold_tests;
mod hud_render;
mod init;
mod input_config;
pub mod input_script;
mod living_ab;
pub mod living_props;
mod living_subsystem;
pub(crate) mod living_tuning;
#[cfg(feature = "debug-hud")]
mod music_guide_panel;
mod music_session;
mod pacing;
mod perf;
mod persist;
pub mod procgen_assets;
mod quality;
pub mod render_scale;
mod scene_entry;
#[cfg(test)]
mod scene_entry_tests;
pub(crate) mod scene_loading;
mod schemas;
pub mod script_commands;
mod shallow_wooze;
mod state;
pub mod style_commands;
pub(crate) mod subsystem;
#[cfg(test)]
mod subsystem_tests;
mod text_halo;
#[cfg(feature = "debug-hud")]
mod timeline_panel;
#[cfg(test)]
mod touch_controls_tests;
mod touch_overlay;
mod transition;
#[cfg(test)]
mod transition_tests;
#[cfg(test)]
mod vfs_tests;
#[cfg(target_arch = "wasm32")]
pub mod web_pacing;
#[cfg(target_arch = "wasm32")]
pub mod web_page;
#[cfg(target_arch = "wasm32")]
mod web_perf;
#[cfg(target_arch = "wasm32")]
pub mod web_startup;
mod world_text;

#[cfg(target_os = "android")]
use events::AndroidGamepadTracker;
use flint_audio::AudioSystem;
use flint_render::{Camera, RenderContext, SceneRenderer};
use flint_runtime::pad::Gilrs;
use flint_runtime::InputState;
pub use game_core::GameCore;
pub use state::{LaunchOptions, SceneSettings};
use std::collections::HashMap;
use std::sync::Arc;
use transition::TransitionPhase;
use winit::window::Window;

// Component-name conventions shared by the debug panels (debug_panels.rs)
// and the frame loop's panel-drain blocks. Kept here, the defining module,
// so both siblings reach them via `super::`.

/// Game-side day/night component driven by a script; the player only knows
/// it to offer the F3 time scrubber (see flint-debug-ui tod_panel).
#[cfg(feature = "debug-hud")]
const TIME_OF_DAY_COMPONENT: &str = "time_of_day";
#[cfg(feature = "debug-hud")]
const WEATHER_COMPONENT: &str = "weather";
/// Reality-tear controller (rare render-mode world events) driven by a
/// game-side script; the F3 Reality panel forces/ends tears for tuning.
#[cfg(feature = "debug-hud")]
const REALITY_COMPONENT: &str = "reality";
/// Second-raft visitor event controller driven by a game-side script; the
/// F3 Visitor panel shows its phase/day and can force a visit for tuning.
#[cfg(feature = "debug-hud")]
const RAFT_VISITOR_COMPONENT: &str = "raft_visitor";
/// Dead-calm ocean-stillness event controller driven by a game-side script;
/// the F3 Dead Calm panel shows its phase/envelope and can force/end one.
#[cfg(feature = "debug-hud")]
const DEAD_CALM_COMPONENT: &str = "dead_calm";
/// Live-tunable camera settings applied to the render camera at scene load
/// and edited through the F3 Camera panel (see flint-debug-ui camera_panel).
const CAMERA_TUNING_COMPONENT: &str = "camera_tuning";

/// Panel name for the terrain grass debug overlay (F3) — the engine-side
/// sibling of `music_guide_panel::MUSIC_GUIDE_PANEL` /
/// `timeline_panel::MANIFEST_MAP_PANEL`.
#[cfg(feature = "debug-hud")]
const GRASS_DEBUG_PANEL: &str = "Grass Debug";

/// Frame-capture settings for the player (`--screenshot` family of flags).
///
/// Times are compared against `GameClock::total_time`, so they are game
/// seconds since the first tick, not wall-clock seconds since launch.
#[derive(Debug, Clone, Default)]
pub struct CaptureConfig {
    /// Output path. Base name for numbered files when `every` is set
    /// (`hud.png` -> `hud_0001.png`, `hud_0002.png`, ...).
    pub path: Option<std::path::PathBuf>,
    /// Game time (s) at which to take the single screenshot. Default 0 =
    /// first frame.
    pub at: f64,
    /// Interval (s) between repeated screenshots; None = single shot.
    pub every: Option<f64>,
    /// Game time (s) at which the player exits on its own.
    pub exit_after: Option<f64>,
    /// Timed input played into `InputState` (`--input-script`).
    pub input_script: Option<input_script::InputScript>,
    /// Open the F2 stats overlay on the first frame (`--stats`). F2 still
    /// closes it.
    pub show_stats: bool,
    /// Internal: the first capture has been written.
    pub(crate) taken: bool,
    /// Internal: number of numbered frames written so far.
    pub(crate) sequence: u32,
    /// Internal: files actually on disk, and attempts that failed, with the
    /// last failure's reason (#639: a run that asks for a shot writes it or
    /// says why it didn't).
    pub(crate) written: u32,
    pub(crate) failed: u32,
    pub(crate) last_failure: Option<String>,
    /// Internal: the last game time a frame reached the capture point,
    /// so a missing shot can say how far the run got.
    pub(crate) last_frame_time: Option<f64>,
    /// Internal: game time of the next scheduled interval capture.
    pub(crate) next_due: f64,
    /// Internal: frames presented and game time accumulated after the first
    /// `PERF_SKIP_SECS`, reported on `--exit-after` as an average frame time.
    pub(crate) perf_frames: u32,
    pub(crate) perf_time: f64,
    /// Internal: render stats summed over the same frames, reported beside
    /// the frame time.
    pub(crate) perf_draws: u64,
    pub(crate) perf_entity_draws: u64,
    pub(crate) perf_culled: u64,
    /// Of those, set aside by level of detail / draw distance (#56)
    pub(crate) perf_lod_culled: u64,
    pub(crate) perf_triangles: u64,
    pub(crate) perf_shadow_triangles: u64,
    /// GPU frame times summed over frames that had one (ADR 0108)
    pub(crate) perf_gpu_ms: f64,
    pub(crate) perf_gpu_frames: u64,
    /// The same frames' dt and present-to-present times, reported as a
    /// spread so uneven pacing shows at an even mean.
    pub(crate) perf_pacing: pacing::Pacing,
}

/// Frames a single screenshot is retried on after a failed read-back or
/// save before the run gives up on it (#639).
pub(crate) const CAPTURE_RETRIES: u32 = 30;

/// Game seconds ignored by the `--exit-after` frame-time report (loading,
/// shader warm-up, first-frame texture uploads).
pub(crate) const PERF_SKIP_SECS: f64 = 2.0;

impl CaptureConfig {
    /// Whether this run is a capture: it shoots, stops on its own or plays
    /// an input script. A capture hears its script alone: real keys, the
    /// mouse, the wheel and real pads are dropped before egui or the game
    /// sees them, so a stray scroll over its window can't move its shot
    /// (ADR 0110's amendment).
    pub fn is_capture(&self) -> bool {
        self.path.is_some() || self.exit_after.is_some() || self.input_script.is_some()
    }

    /// A single shot is due (its time reached) but not yet on disk, and
    /// still has retries left: `--exit-after` waits for it (#639).
    pub(crate) fn shot_pending(&self, now: f64) -> bool {
        self.path.is_some()
            && self.every.is_none()
            && self.written == 0
            && self.failed <= CAPTURE_RETRIES
            && now + 1e-6 >= self.at
    }

    /// Record a capture that didn't reach the disk. A single shot is tried
    /// again on the next frame until `CAPTURE_RETRIES` run out; a sequence's
    /// frame is lost, and the run will fail for it.
    pub(crate) fn note_failure(&mut self, reason: String) {
        self.failed += 1;
        self.last_failure = Some(reason);
        if self.every.is_none() && self.failed <= CAPTURE_RETRIES {
            self.taken = false;
        }
    }

    /// Why a run that asked for screenshots didn't get them all, if it
    /// didn't: none written, or (in a sequence) any frame lost.
    pub fn missing(&self) -> Option<String> {
        let path = self.path.as_ref()?;
        let why = || match (&self.last_failure, self.last_frame_time) {
            (Some(reason), _) => reason.clone(),
            (None, None) => "no frame was drawn".to_string(),
            (None, Some(t)) => format!(
                "the run ended at t={t:.3}s, before --screenshot-at {:.3}",
                self.at
            ),
        };
        if self.written == 0 {
            return Some(format!(
                "--screenshot {} was never written: {}",
                path.display(),
                why()
            ));
        }
        if self.every.is_some() && self.failed > 0 {
            return Some(format!(
                "--screenshot-every lost {} of {} frames of {}: {}",
                self.failed,
                self.failed + self.written,
                path.display(),
                why()
            ));
        }
        None
    }

    /// Build a capture schedule. `every <= 0` means single-shot.
    pub fn new(
        path: Option<std::path::PathBuf>,
        at: f64,
        every: Option<f64>,
        exit_after: Option<f64>,
    ) -> Self {
        Self {
            path,
            at,
            every: every.filter(|s| *s > 0.0),
            exit_after,
            ..Default::default()
        }
    }
}

pub struct PlayerApp {
    /// The game with no window (ADR 0117 decision 4): the world, its clock,
    /// scripts, physics, animation, particles, states and the subsystems,
    /// and the frame order (`game_core::step`). Declared before `audio`, so
    /// whatever a subsystem holds of the audio engine goes before the
    /// engine.
    pub core: GameCore,
    pub input: InputState,
    // Scene-declared music session (F3, ADR 0019): declared before `audio`
    // so the suite's kira handles drop before the shared AudioManager
    // (ADR 0017 drop-order rule).
    music: state::MusicState,
    pub audio: AudioSystem,

    /// What the host asked for before the event loop started (the command
    /// line, the page's query, a dev profile).
    pub launch: LaunchOptions,
    /// What the current scene's file says, replaced whole at a transition.
    pub scene: SceneSettings,

    // Rendering
    window: Option<Arc<Window>>,
    render_context: Option<RenderContext>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,
    /// Camera cuts, style tweens, the style reader and F4's look
    style: state::StyleState,
    /// The quality settings in force and the render scale's steps
    quality: state::QualityState,
    /// Which refreshes the web player draws (#140, `?fps=`); `None` draws
    /// every one, as the desktop always does. See `set_frame_rate`.
    frame_pacer: Option<frame_rate::FramePacer>,

    // Skeletal animation: entity_id → asset name for bone matrix updates
    skeletal_entity_assets: HashMap<flint_core::EntityId, String>,

    /// egui, the scripts' 2D draw commands and world text, and their fonts
    hud: state::Hud,

    // Asset catalog (optional, for content-addressed asset resolution)
    assets: state::AssetSources,

    /// The window's focus, the cursor and the keys held
    focus: state::Focus,

    /// The post-processing scripts asked for this frame, stamped over the
    /// renderer's config before it draws
    post: state::PostOverrides,

    /// `vfs::generation()` when the scene's audio was last loaded. A late
    /// bundle (#98) installs a grown file source mid-game; when the count
    /// moves, the audio that couldn't be found before is loaded.
    pub(super) audio_vfs_generation: u64,

    /// Screenshot capture driven by `--screenshot` / `--screenshot-at` /
    /// `--screenshot-every` / `--exit-after`. Captures the presented frame
    /// (scene + egui HUD) right before `present()`.
    pub capture: CaptureConfig,
    /// How the run ends, and why it failed if it did
    outcome: state::Outcome,
    /// A windowless device for the tests that take a scene through a
    /// transition with a real renderer (#723): `RenderContext` needs a
    /// surface, so tests hand the renderer's device and queue over here.
    #[cfg(test)]
    test_gpu: Option<&'static flint_render::HeadlessContext>,

    // Input config layering + remap persistence
    input_layers: state::InputLayers,

    // Optional gamepad backend
    gilrs: Option<Gilrs>,
    /// A script's `rumble()`s playing now, through `gilrs` (#748).
    pad_rumble: flint_runtime::PadRumble,

    // Where the persistent store saves, and when
    saves: state::Saves,

    // Scene transition lifecycle
    transition_phase: TransitionPhase,

    // Schema directories and where the list came from (ADR 0114), kept for
    // transitions and chunks
    schema_dirs: flint_core::project::SchemaDirs,
    // The registry itself, read once and kept (ADR 0115, #683): a chunk
    // uses it as it is, a transition re-reads only for other directories,
    // and a failed read keeps it
    schemas: schemas::HeldSchemas,

    // Terrain data for height queries
    terrain: Option<(flint_terrain::Terrain, flint_terrain::TerrainConfig)>,

    // Debug overlay panels (F3 toggle)
    #[cfg(feature = "debug-hud")]
    debug_panels: Vec<Box<dyn flint_debug_ui::DebugPanel>>,
    /// F3 Particles panel toggle: when false the sim still runs but nothing
    /// is uploaded to the renderer (ADR 0068).
    particles_render_enabled: bool,

    /// The F2 stats overlay and the perf readout
    stats: state::Stats,

    // Chunk streaming: chunk_id → spawned entity IDs
    loaded_chunks: HashMap<String, Vec<flint_core::EntityId>>,

    // Android gamepad tracking (maps winit DeviceId to InputState gamepad slots)
    #[cfg(target_os = "android")]
    android_gamepad: AndroidGamepadTracker,

    // Android JNI axis reader for trigger/right-stick values from GameActivity
    #[cfg(target_os = "android")]
    android_axis_reader: Option<fn() -> [f32; 4]>,

    /// Game-installed hooks (`add_extension`), in the order they were added.
    extensions: Vec<Box<dyn crate::extension::PlayerExtension>>,

    /// The device request in flight on the web (#99): `RenderContext::new`
    /// resolves into it, and startup finishes on the loop.
    #[cfg(target_arch = "wasm32")]
    pending_render: web_startup::RenderSlot,
    /// Wakes the event loop when the device arrives (`set_event_proxy`).
    #[cfg(target_arch = "wasm32")]
    event_proxy: Option<winit::event_loop::EventLoopProxy<()>>,
    /// The browser's pointer lock, followed each frame (#102).
    #[cfg(target_arch = "wasm32")]
    pointer_lock: web_page::PointerLock,
}
