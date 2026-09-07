//! Player application implementing winit ApplicationHandler
//!
//! Runs the game loop with physics, input, and first-person camera.

mod debug_panels;
mod events;
pub mod fonts;
mod frame;
mod hud_render;
mod init;
mod input_config;
#[cfg(feature = "debug-hud")]
mod music_guide_panel;
mod music_session;
pub(crate) mod scene_loading;
mod script_commands;
#[cfg(feature = "debug-hud")]
mod timeline_panel;
mod transition;

#[cfg(target_os = "android")]
use events::AndroidGamepadTracker;
use flint_animation::AnimationSystem;
use flint_asset::{AssetCatalog, ContentStore};
use flint_audio::AudioSystem;
use flint_ecs::FlintWorld;
use flint_particles::ParticleSystem;
use flint_physics::PhysicsSystem;
use flint_render::{Camera, RenderContext, SceneRenderer};
use flint_runtime::{GameClock, GameStateMachine, InputConfig, InputState, PersistentStore};
use flint_script::context::DrawItem;
use flint_script::ScriptSystem;
use gilrs::Gilrs;
use std::collections::{HashMap, HashSet};
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
    /// Internal: the first capture has been written.
    pub(crate) taken: bool,
    /// Internal: number of numbered frames written so far.
    pub(crate) sequence: u32,
    /// Internal: game time of the next scheduled interval capture.
    pub(crate) next_due: f64,
}

impl CaptureConfig {
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
    // Core state
    pub world: FlintWorld,
    pub scene_path: String,

    // Systems
    pub clock: GameClock,
    pub input: InputState,
    pub physics: PhysicsSystem,
    // Scene-declared music session (F3, ADR 0019): declared before `audio`
    // so the suite's kira handles drop before the shared AudioManager
    // (ADR 0017 drop-order rule).
    music_session: Option<music_session::MusicSession>,
    /// Set when a session with `quit_on_finish` ends on its own; the event
    /// loop exits at the end of that frame (checked in RedrawRequested,
    /// where the ActiveEventLoop is in scope).
    music_exit_requested: bool,
    pub audio: AudioSystem,
    pub animation: AnimationSystem,
    pub particles: ParticleSystem,
    pub script: ScriptSystem,

    // Rendering
    window: Option<Arc<Window>>,
    render_context: Option<RenderContext>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,

    // Skeletal animation: entity_id → asset name for bone matrix updates
    skeletal_entity_assets: HashMap<flint_core::EntityId, String>,

    // HUD + egui overlay
    egui_ctx: egui::Context,
    egui_winit: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,

    // Script-driven 2D draw commands
    draw_commands: Vec<DrawItem>,
    ui_textures: HashMap<String, egui::TextureHandle>,
    /// Family names registered from `<project>/fonts/` (file stems + manifest
    /// aliases). The HUD renderer falls back to the default font for names
    /// outside this set.
    ui_fonts: HashSet<String>,
    /// Unknown font names already warned about (one warn per name).
    ui_fonts_warned: HashSet<String>,
    /// The `fonts/` root the current font set was scanned from; rescan on
    /// scene change only when this differs.
    ui_fonts_root: Option<std::path::PathBuf>,
    /// Set after the first egui pass: `Context::fonts()` panics before then,
    /// so the script `measure_text` closure falls back to an estimate until
    /// this flips.
    egui_fonts_ready: Arc<std::sync::atomic::AtomicBool>,

    // Asset catalog (optional, for content-addressed asset resolution)
    catalog: Option<AssetCatalog>,
    content_store: Option<ContentStore>,

    // Window options
    pub fullscreen: bool,
    cursor_captured: bool,

    // Environment
    pub skybox_path: Option<String>,
    /// Scene-authored hemisphere ambient (sky, ground); None = renderer default
    pub scene_ambient: Option<([f32; 3], [f32; 3])>,
    /// MSAA sample count for the scene renderer (1 = off, 4 = on; ADR 0058).
    /// Set from the --msaa CLI flag before the window/renderer exist.
    pub msaa_sample_count: u32,
    /// Scene-authored diffuse terminator wrap; None = 0 = legacy shading
    pub scene_diffuse_wrap: Option<f32>,
    pub scene_oren_nayar: Option<f32>,
    pub scene_sheen: Option<([f32; 3], f32)>,

    // Scene-level camera configuration
    pub scene_camera: Option<flint_scene::CameraDef>,

    // Scene-level post-processing overrides
    pub scene_post_process: Option<flint_scene::PostProcessDef>,

    // Script-driven post-processing overrides (applied per-frame before render)
    pp_vignette_override: Option<f32>,
    pp_bloom_override: Option<f32>,
    pp_exposure_override: Option<f32>,
    pp_chromatic_aberration_override: Option<f32>,
    pp_radial_blur_override: Option<f32>,
    pp_ssao_intensity_override: Option<f32>,
    pp_fog_density_override: Option<f32>,
    pp_desaturation_override: Option<f32>,
    pp_dof_strength_override: Option<f32>,
    pp_dof_focus_distance_override: Option<f32>,
    pp_dof_focus_range_override: Option<f32>,
    pp_fog_color_override: Option<[f32; 3]>,
    pp_render_mode_override: Option<(u32, f32)>,
    pp_mode_params_override: Option<[f32; 4]>,
    /// True while a script-driven reality tear is on screen. Unlike the
    /// other (sticky) overrides, the mode is zeroed the frame the script
    /// stops calling set_render_mode — a dead or hot-reloaded script must
    /// never freeze a tear over the world.
    pp_mode_was_active: bool,

    /// Rendering & Effects panel's "freeze script post overrides" switch
    /// (ADR 0053): while true the per-frame script/ladder override stamp is
    /// skipped so panel edits to contended post fields stick. Cached from
    /// the panel each frame (one-frame toggle lag, imperceptible).
    #[cfg(feature = "debug-hud")]
    pp_debug_freeze: bool,

    // Ladder-driven post params: the scene's authored base, captured at
    // session start and written back after teardown (ADR 0021).
    music_pp_base: Option<music_session::LadderPostBase>,
    music_pp_restore: Option<music_session::LadderPostBase>,

    /// `[scene] preload_audio` — false skips the blanket audio/ preload
    /// (silent scenes start instantly); audio_source + session stems
    /// unaffected. Set from the scene file before startup, like msaa.
    pub scene_preload_audio: bool,

    /// Initial window size in physical pixels (`--width`/`--height`).
    /// Ignored on Android, where the surface is the screen.
    pub window_size: (u32, u32),
    /// Screenshot capture driven by `--screenshot` / `--screenshot-at` /
    /// `--screenshot-every` / `--exit-after`. Captures the presented frame
    /// (scene + egui HUD) right before `present()`.
    pub capture: CaptureConfig,
    /// Set once `--exit-after` elapses; the redraw handler exits the loop.
    exit_requested: bool,

    // Input config layering + remap persistence
    input_config_override: Option<String>,
    scene_input_config: Option<String>,
    input_config_paths: Option<InputConfigPaths>,
    user_override_config: InputConfig,
    pending_rebind: Option<PendingRebind>,

    // Optional gamepad backend
    gilrs: Option<Gilrs>,

    // State machine + persistence (survive scene transitions)
    state_machine: GameStateMachine,
    persistent_store: PersistentStore,

    // Scene transition lifecycle
    transition_phase: TransitionPhase,

    // Schema paths preserved across transitions
    schema_paths: Vec<String>,

    // Terrain data for height queries
    terrain: Option<(flint_terrain::Terrain, flint_terrain::TerrainConfig)>,

    // Debug overlay panels (F3 toggle)
    #[cfg(feature = "debug-hud")]
    debug_panels: Vec<Box<dyn flint_debug_ui::DebugPanel>>,
    /// F3 Particles panel toggle: when false the sim still runs but nothing
    /// is uploaded to the renderer (ADR 0068).
    particles_render_enabled: bool,

    // Rendering stats overlay (F2 toggle)
    show_stats: bool,
    stats_frame_times: std::collections::VecDeque<f64>,

    // Procedural generation resolver for runtime asset resolution
    procgen_resolver: flint_procgen::ProcGenResolver,

    // Chunk streaming: chunk_id → spawned entity IDs
    loaded_chunks: HashMap<String, Vec<flint_core::EntityId>>,

    // Android gamepad tracking (maps winit DeviceId to InputState gamepad slots)
    #[cfg(target_os = "android")]
    android_gamepad: AndroidGamepadTracker,

    // Android JNI axis reader for trigger/right-stick values from GameActivity
    #[cfg(target_os = "android")]
    android_axis_reader: Option<fn() -> [f32; 4]>,
}

use input_config::{InputConfigPaths, PendingRebind};
