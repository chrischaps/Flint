//! Player application implementing winit ApplicationHandler
//!
//! Runs the game loop with physics, input, and first-person camera.

mod camera_cut;
#[cfg(test)]
mod capture_gate_tests;
#[cfg(test)]
mod debug_key_tests;
mod debug_panels;
mod egui_input;
mod events;
pub mod expression_feed;
#[cfg(test)]
mod extension_tests;
pub mod fonts;
mod frame;
pub mod frame_rate;
pub mod headless_scripts;
#[cfg(test)]
mod hold_tests;
mod hud_render;
mod init;
mod input_config;
pub mod input_script;
mod living_ab;
pub mod living_props;
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
pub(crate) mod scene_loading;
mod script_commands;
mod shallow_wooze;
pub mod style_commands;
mod text_halo;
#[cfg(feature = "debug-hud")]
mod timeline_panel;
#[cfg(test)]
mod touch_controls_tests;
mod touch_overlay;
mod transition;
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
use flint_animation::AnimationSystem;
use flint_asset::{AssetCatalog, ContentStore};
use flint_audio::AudioSystem;
use flint_ecs::FlintWorld;
use flint_input_capture::pad::Gilrs;
use flint_particles::ParticleSystem;
use flint_physics::PhysicsSystem;
use flint_render::{Camera, RenderContext, SceneRenderer};
use flint_runtime::{
    GameClock, GameStateMachine, InputConfig, InputState, PersistentStore, SaveDebounce,
};
use flint_script::context::DrawItem;
use flint_script::ScriptSystem;
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
    /// Resets style history when the camera cuts (#343)
    camera_cut: camera_cut::CameraCutDetector,
    /// Style settings the engine is easing for scripts (#370)
    style_tweens: style_commands::StyleTweens,
    /// The renderer's `post_config_generation` scripts' style reads were
    /// last handed (#372); `None` until the first.
    style_reader_generation: Option<u64>,

    // Skeletal animation: entity_id → asset name for bone matrix updates
    skeletal_entity_assets: HashMap<flint_core::EntityId, String>,

    // HUD + egui overlay
    egui_ctx: egui::Context,
    egui_winit: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,

    // Script-driven 2D draw commands
    draw_commands: Vec<DrawItem>,
    /// Depth-tested world text queued this frame (ADR 0104), laid out in
    /// the egui pass and drawn before the HUD.
    world_texts: Vec<flint_core::world_text::WorldText>,
    world_text_pipeline: Option<world_text::WorldTextPipeline>,
    ui_textures: HashMap<String, egui::TextureHandle>,
    /// Family names registered from `<project>/fonts/` (file stems + manifest
    /// aliases). The HUD renderer falls back to the default font for names
    /// outside this set.
    ui_fonts: HashSet<String>,
    /// Unknown font names already warned about (one warn per name).
    ui_fonts_warned: HashSet<String>,
    /// Wide-stroke and glow pictures for HUD and world text (#626).
    text_halos: text_halo::TextHalos,
    /// The `fonts/` root the current font set was scanned from; rescan on
    /// scene change only when this differs.
    ui_fonts_root: Option<std::path::PathBuf>,
    /// Whether egui has been given the fonts at least once. Until then
    /// `refresh_project_fonts` always installs them.
    ui_fonts_installed: bool,
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
    /// Whether the window has the keyboard focus; real pads are dropped
    /// and released while it doesn't (ADR 0110).
    window_focused: bool,
    /// The `InputState` slots real pads (gilrs) have written, released when
    /// the window loses focus.
    real_pad_slots: std::collections::HashSet<u32>,
    /// The modifiers held now, real or scripted, for `PlayerExtension::on_key`.
    key_modifiers: winit::keyboard::ModifiersState,

    // Environment
    pub skybox_path: Option<String>,
    /// Scene-authored hemisphere ambient (sky, ground); None = renderer default
    pub scene_ambient: Option<([f32; 3], [f32; 3])>,
    /// MSAA sample count for the scene renderer (1 = off, 4 = on; ADR 0058).
    /// Set from the --msaa CLI flag before the window/renderer exist.
    pub msaa_sample_count: u32,
    /// When scene procgen assets are made (#32): during load, or behind
    /// placeholders on the frame-budgeted queue (`--procgen-defer`).
    pub procgen_mode: procgen_assets::ProcGenMode,
    /// The scene's render scale: a ceiling (fixed, and capped by pixel
    /// ratio) and whether to step under it to hold the frame rate (#122).
    /// The desktop's default draws at the surface; a page's caps DPR 1.5.
    pub render_scale: render_scale::RenderScaleSetting,
    /// Keep procgen outputs on disk between runs (#181), in the project's
    /// `.flint/procgen-cache/` (or `FLINT_PROCGEN_CACHE=<dir>`; `=0` turns
    /// it off). On by default natively; off for a bundle, whose files are
    /// not on the disk, and always off on the web.
    pub procgen_disk_cache: bool,
    /// Whether `scene_path` names a file on the disk the player may write
    /// (#364: the style editors' Commit to scene). On natively; off for a
    /// bundle and on the web.
    pub scene_file_writable: bool,
    dynamic_scale: render_scale::DynamicScale,
    /// `key=value,...` quality settings applied over `msaa_sample_count`,
    /// `render_scale` and the `FLINT_SHADOW*` switches at startup
    /// (FlintKart#55): `--quality`, the page's `?quality=`, an Android dev
    /// profile's `[quality]`. Scripts change them later with `set_quality`.
    pub quality_overrides: String,
    /// `--pixels-per-point` (#435): the scale point-sized things are drawn
    /// at (ASCII cells, egui, the HUD) instead of the window's scale factor,
    /// so a capture looks the same on any display. `None` follows the window.
    pub pixels_per_point: Option<f32>,
    /// `--safe-area L,T,R,B` (#660): stand-in safe-area insets in points,
    /// for trying a notch-aware HUD on a desktop. `None` reads the page's
    /// (zeros natively).
    pub safe_area: Option<[f32; 4]>,
    /// `--touch-device` (#660): report a touch-first device to scripts
    /// (`is_touch_device()`) on a desktop. A phone page and Android are one
    /// anyway.
    pub touch_device: bool,
    /// The quality settings in force (see `quality.rs`).
    quality: flint_render::QualitySettings,
    /// Which refreshes the web player draws (#140, `?fps=`); `None` draws
    /// every one, as the desktop always does. See `set_frame_rate`.
    frame_pacer: Option<frame_rate::FramePacer>,
    /// Game time, in seconds, at which the clock stops (it steps by zero
    /// from then on, and scripts keep running on a still world). With
    /// `clock.frame_step` it holds a page on an exact moment for a
    /// screenshot (#102, the web's `?hold_at=`); `--screenshot-at` is the
    /// desktop's way.
    pub hold_at: Option<f64>,
    /// Scene-authored diffuse terminator wrap; None = 0 = legacy shading
    pub scene_diffuse_wrap: Option<f32>,
    /// The scene's `[environment] shadows` and `default_lights` (#627),
    /// true when it doesn't say.
    pub scene_shadows: bool,
    pub scene_default_lights: bool,
    pub scene_oren_nayar: Option<f32>,
    pub scene_sheen: Option<([f32; 3], f32)>,

    // Scene-level camera configuration
    pub scene_camera: Option<flint_scene::CameraDef>,

    // Scene-level post-processing overrides
    pub scene_post_process: Option<flint_scene::PostProcessDef>,
    /// `--look` (#435): a look (a name or a `.look.toml` path) merged over
    /// every scene's `[post_process]` as it loads, as `flint render --look`
    /// does. Found again at each load, after that scene's user styles.
    pub look: Option<String>,
    /// `--style name[:key=value,...]` words (#328), written over every
    /// scene's `[post_process]` at load, after each transition and after a
    /// user style's hot reload, so they keep winning. See
    /// [`PlayerApp::scene_post_config`].
    pub style_specs: Vec<String>,
    /// `--style-time`: hold the post time styles (and the ocean, grain and
    /// render modes, which share it) see at this many seconds.
    pub style_time: Option<f32>,

    // The scene's `[living]` table, and room to blend its props each frame
    pub scene_living: Option<flint_scene::LivingDef>,
    living_blend: flint_procgen::BlendFrame,
    // The scene's `[wooze]` table, and the field made from it, sampled on a clock
    // of its own that stops when animation does. No table: no field, no samples.
    pub scene_wooze: Option<flint_scene::WoozeDef>,
    wooze: Option<flint_core::wooze::WoozeField>,
    wooze_time: f64,
    wooze_missing_warned: bool,
    // Shallow wooze (#215): the clocks of the meshes on the field with no poses.
    shallow_wooze: shallow_wooze::ShallowWooze,
    // The shading boil's beats (#214), on the same clock as the field.
    boil_clock: flint_core::wooze::BoilClock,
    // Woozy grass (#252): the grass's sway and comb on the same clock.
    grass_wooze: shallow_wooze::GrassWooze,
    // What's been done to a running grove by hand (#218).
    pub(crate) living_tune: living_tuning::LivingTune,

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
    /// A look F4's picker chose this frame (#356; `Some(None)`: none),
    /// laid under the scene's keys at the start of the next render.
    pending_look: Option<Option<String>>,

    // Ladder-driven post params: the scene's authored base, captured at
    // session start and written back after teardown (ADR 0021).
    music_pp_base: Option<music_session::LadderPostBase>,
    music_pp_restore: Option<music_session::LadderPostBase>,

    /// `[scene] preload_audio` — false skips the blanket audio/ preload
    /// (silent scenes start instantly); audio_source + session stems
    /// unaffected. Set from the scene file before startup, like msaa.
    pub scene_preload_audio: bool,
    /// `vfs::generation()` when the scene's audio was last loaded. A late
    /// bundle (#98) installs a grown file source mid-game; when the count
    /// moves, the audio that couldn't be found before is loaded.
    pub(super) audio_vfs_generation: u64,

    /// Initial window size in physical pixels (`--width`/`--height`).
    /// Ignored on Android, where the surface is the screen.
    pub window_size: (u32, u32),
    /// Where the window opens, its frame, whether it stays on top and whether
    /// it takes the keyboard (`--position`, `--borderless`, `--always-on-top`,
    /// `--no-activate`; #611). Its width and height are ignored here: the
    /// size is `window_size`.
    pub window_options: crate::WindowArgs,
    /// Screenshot capture driven by `--screenshot` / `--screenshot-at` /
    /// `--screenshot-every` / `--exit-after`. Captures the presented frame
    /// (scene + egui HUD) right before `present()`.
    pub capture: CaptureConfig,
    /// State from outside the process for scripts' `expression()` and
    /// `on_expression` (`--expression-timeline`, `--expression-listen`;
    /// Flint#518). `None` when neither was given.
    pub expression_feed: Option<expression_feed::ExpressionFeed>,
    /// Set once `--exit-after` elapses; the redraw handler exits the loop.
    exit_requested: bool,
    /// Why `initialize` failed, kept so the binary can exit non-zero (#639).
    init_error: Option<String>,

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
    /// When to write `save/persist.toml` after a change (see `persist.rs`).
    persist_debounce: SaveDebounce,

    // Scene transition lifecycle
    transition_phase: TransitionPhase,

    // Schema directories and where the list came from (ADR 0114), kept for
    // transitions and chunks
    schema_dirs: flint_core::project::SchemaDirs,

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
    /// The player turned the renderer's GPU timing on for the overlay
    /// (ADR 0108), so it turns it off again when the overlay closes
    stats_gpu_timing: bool,
    /// The perf readout's recorder (`?perf=1` on the web, #120): frame and
    /// phase times for the last ten seconds. `None` costs nothing.
    perf: Option<perf::PerfRecorder>,

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

    /// Game-installed hooks (`add_extension`), in the order they were added.
    extensions: Vec<Box<dyn crate::extension::PlayerExtension>>,
    /// Which held keys and buttons belong to egui (game egui layer, ADR 0073).
    egui_input: egui_input::EguiInputGate,

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

use input_config::{InputConfigPaths, PendingRebind};
