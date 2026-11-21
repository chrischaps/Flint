//! `PlayerApp`'s fields, grouped by what they belong to (#724, ADR 0117
//! step 1): what the host asked for at launch, what the current scene's file
//! says, what scripts asked of this frame's post-processing, living props and
//! wooze, the HUD, and so on. Each group is one field of `PlayerApp`, so a
//! feature's state is in one place and the struct reads as a list of parts.
//!
//! Moving a field into a group changes no behaviour: every value starts as it
//! did, and every read and write is the same one through the group.

use super::input_config::{InputConfigPaths, PendingRebind};
use super::{camera_cut, expression_feed, fonts, hitch, hitch_watch, perf};
use super::{egui_input, render_scale, text_halo, world_text};
use flint_asset::HeldAssets;
use flint_core::project::Project;
use flint_runtime::{InputConfig, SaveDebounce};
use flint_script::context::DrawItem;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// When the player generates a scene's procgen assets (`LaunchOptions::procgen_mode`).
/// A build without the `procgen` feature (#754) generates none either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProcGenMode {
    /// Generate during scene load; the first frame shows the real assets.
    #[default]
    Immediate,
    /// Show placeholders at once and generate on the frame-budgeted queue.
    Deferred,
}

/// What the host asked for before the event loop started: the command line's
/// flags (`flint-player`, `flint play`), the web page's query, an Android dev
/// profile. Set once, before `run_app`; nothing in a scene changes them.
pub struct LaunchOptions {
    /// Open fullscreen.
    pub fullscreen: bool,
    /// Initial window size in physical pixels (`--width`/`--height`).
    /// Ignored on Android, where the surface is the screen.
    pub window_size: (u32, u32),
    /// Where the window opens, its frame, whether it stays on top and whether
    /// it takes the keyboard (`--position`, `--borderless`, `--always-on-top`,
    /// `--no-activate`; #611). Its width and height are ignored here: the
    /// size is `window_size`.
    pub window_options: crate::WindowArgs,
    /// MSAA sample count for the scene renderer (1 = off, 4 = on; ADR 0058).
    /// Set from the --msaa CLI flag before the window/renderer exist.
    pub msaa_sample_count: u32,
    /// Parts of the frame skipped or drawn N times, by label (#1317:
    /// `--skip-pass`, `--repeat-pass`, the page's `?skip_pass=` and
    /// `?repeat_pass=`), handed to the renderer when it is made. `None`
    /// draws every part once.
    pub pass_knobs: Option<flint_render::PassKnobs>,
    /// When scene procgen assets are made (#32): during load, or behind
    /// placeholders on the frame-budgeted queue (`--procgen-defer`).
    pub procgen_mode: ProcGenMode,
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
    /// Game time, in seconds, at which the clock stops (it steps by zero
    /// from then on, and scripts keep running on a still world). With
    /// `clock.frame_step` it holds a page on an exact moment for a
    /// screenshot (#102, the web's `?hold_at=`); `--screenshot-at` is the
    /// desktop's way.
    pub hold_at: Option<f64>,
    /// `--look` (#435): a look (a name or a `.look.toml` path) merged over
    /// every scene's `[post_process]` as it loads, as `flint render --look`
    /// does. Found again at each load, after that scene's user styles.
    pub look: Option<String>,
    /// `--style name[:key=value,...]` words (#328), written over every
    /// scene's `[post_process]` at load, after each transition and after a
    /// user style's hot reload, so they keep winning. See
    /// [`PlayerApp::scene_post_config`](super::PlayerApp::scene_post_config).
    pub style_specs: Vec<String>,
    /// `--style-time`: hold the post time styles (and the ocean, grain and
    /// render modes, which share it) see at this many seconds.
    pub style_time: Option<f32>,
    /// `--motion-hold`: the whole-frame motion on twos rate (#405), written
    /// over every scene's `[post_process] motion_hold` at load.
    pub motion_hold: Option<f32>,
    /// `--save-dir`: keep this run's saves in this folder
    /// (`<dir>/<slot>.toml`) and read no other (#726).
    pub save_dir: Option<std::path::PathBuf>,
    /// State from outside the process for scripts' `expression()` and
    /// `on_expression` (`--expression-timeline`, `--expression-listen`;
    /// Flint#518). `None` when neither was given.
    pub expression_feed: Option<expression_feed::ExpressionFeed>,
    /// `--compare A,B` (#238): the Living Props presets to compare, kept
    /// across scenes and changed by the panel's picks, and whether to start
    /// comparing (split or not) once a scene has props to compare.
    pub compare: Option<(String, String)>,
    pub compare_at_start: Option<bool>,
    /// `--strict-scripts` (#693): a run with any script error, at load or
    /// run time, fails (`PlayerApp::run_failure`), so a headless run of a
    /// broken game can't exit 0.
    pub strict_scripts: bool,
    /// Keep the `persist_*` store in memory only: read no save and write
    /// none (#714: a playtest starts from no save, and leaves none).
    pub saves_in_memory: bool,
    /// `--load-report` or `--perf` (#1092): record each transition's and
    /// chunk's load and say its table (the start-up's is begun before the
    /// app exists, by whoever builds it).
    pub load_report: bool,
    /// The screen scripts see, in points, when there is no window (#714:
    /// a playtest's `screen_width()` is its `--width`, not egui's
    /// placeholder). `None` keeps egui's.
    pub headless_screen: Option<(f32, f32)>,
    /// `--inspect-listen` (#713): the loopback socket that answers queries
    /// about the live world, handed to its subsystem at the start.
    #[cfg(not(target_arch = "wasm32"))]
    pub inspect: Option<super::inspect::InspectListener>,
    /// `--dump-world` (#713): the world written to a file at game times,
    /// handed to its subsystem at the start; `run_failure` asks it whether
    /// every time came.
    #[cfg(not(target_arch = "wasm32"))]
    pub world_dump: Option<super::world_dump::WorldDump>,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            fullscreen: false,
            window_size: (1280, 720),
            window_options: crate::WindowArgs::default(),
            msaa_sample_count: 1,
            pass_knobs: None,
            procgen_mode: ProcGenMode::Immediate,
            render_scale: Default::default(),
            procgen_disk_cache: cfg!(not(target_arch = "wasm32")),
            scene_file_writable: cfg!(not(target_arch = "wasm32")),
            quality_overrides: String::new(),
            pixels_per_point: None,
            safe_area: None,
            touch_device: false,
            hold_at: None,
            look: None,
            style_specs: Vec::new(),
            style_time: None,
            motion_hold: None,
            save_dir: None,
            expression_feed: None,
            compare: None,
            compare_at_start: None,
            strict_scripts: false,
            saves_in_memory: false,
            load_report: false,
            headless_screen: None,
            #[cfg(not(target_arch = "wasm32"))]
            inspect: None,
            #[cfg(not(target_arch = "wasm32"))]
            world_dump: None,
        }
    }
}

/// What the current scene's file says about how it is drawn and run, taken
/// by `apply_scene_settings` and replaced whole at each transition: a block
/// the scene doesn't author is the default here, never the last scene's.
pub struct SceneSettings {
    /// `[environment] skybox`, a path found beside the scene and above it.
    pub skybox_path: Option<String>,
    /// Scene-authored hemisphere ambient (sky, ground); None = renderer default
    pub ambient: Option<([f32; 3], [f32; 3])>,
    /// Scene-authored diffuse terminator wrap; None = 0 = legacy shading
    pub diffuse_wrap: Option<f32>,
    /// The scene's `[environment] shadows` and `default_lights` (#627),
    /// true when it doesn't say.
    pub shadows: bool,
    pub default_lights: bool,
    pub oren_nayar: Option<f32>,
    pub sheen: Option<([f32; 3], f32)>,
    /// Scene-level camera configuration (`[camera]`)
    pub camera: Option<flint_scene::CameraDef>,
    /// Scene-level post-processing (`[post_process]`), with the look it names
    /// and `--look` laid in
    pub post_process: Option<flint_scene::PostProcessDef>,
    /// The current scene's top-level tables as its file wrote them (#686):
    /// the engine's (`[camera]`, `[post_process]`, ...) and any a game added
    /// (`[weather]`). What `ExtensionCtx::scene_table` reads. A host that
    /// builds the app itself sets it from `SceneFile::scene_tables`, as
    /// `apply_scene_settings` does; a scene transition replaces it.
    pub tables: toml::Table,
    /// `[scene] preload_audio` — false skips the blanket audio/ preload
    /// (silent scenes start instantly); audio_source + session stems
    /// unaffected. Set from the scene file before startup, like msaa.
    pub preload_audio: bool,
    /// `[scene] input_config`: the scene's own input config, over the
    /// project's.
    pub input_config: Option<String>,
    /// The scene's `[living]` and `[wooze]` tables (living props, #211-#252),
    /// with a `[wooze] preset` filled in.
    #[cfg(feature = "living")]
    pub living: Option<flint_procgen::LivingDef>,
    #[cfg(feature = "living")]
    pub wooze: Option<flint_procgen::WoozeDef>,
}

impl Default for SceneSettings {
    fn default() -> Self {
        Self {
            skybox_path: None,
            ambient: None,
            diffuse_wrap: None,
            shadows: true,
            default_lights: true,
            oren_nayar: None,
            sheen: None,
            camera: None,
            post_process: None,
            tables: toml::Table::new(),
            preload_audio: true,
            input_config: None,
            #[cfg(feature = "living")]
            living: None,
            #[cfg(feature = "living")]
            wooze: None,
        }
    }
}

/// What scripts asked of this frame's post-processing, drained from their
/// requests each frame and stamped over the renderer's config before it
/// draws (`set_post`, `set_render_mode`).
#[derive(Default)]
pub(super) struct PostOverrides {
    /// The `[post_process]` keys scripts set (`set_post`, #853).
    pub(super) script: flint_script::PostProcessOverrides,
    /// The deprecated `set_render_mode` (#746): this frame's mode and mix,
    /// and the params, held from the last `set_render_mode_params`.
    pub(super) render_mode: Option<(u32, f32)>,
    pub(super) mode_params: Option<[f32; 4]>,
    pub(super) held_mode_params: [f32; 4],
    /// What the mode overwrote, put back the frame the script stops asking
    /// (a dead or hot-reloaded script must never freeze a tear over the
    /// world), and the ocean tint it draws (`flint_render::render_mode`).
    pub(super) mode_shim: flint_render::render_mode::RenderModeShim,
    /// Rendering & Effects panel's "freeze script post overrides" switch
    /// (ADR 0053): while true the per-frame script/ladder override stamp is
    /// skipped so panel edits to contended post fields stick. Cached from
    /// the panel each frame (one-frame toggle lag, imperceptible).
    #[cfg(feature = "debug-hud")]
    pub(super) debug_freeze: bool,
}

impl PostOverrides {
    /// Take this frame's requests from what scripts asked (the commands are
    /// in by now), replacing last frame's whole.
    pub(super) fn take_from(&mut self, requests: &mut flint_script::FrameRequests) {
        self.script = std::mem::take(&mut requests.post);
        self.render_mode = requests.render_mode.take();
        self.mode_params = requests.mode_params.take();
        if let Some(params) = self.mode_params {
            self.held_mode_params = params;
        }
    }

    /// Whether anything is to be stamped this frame: a value asked for, or
    /// a tear to end.
    pub(super) fn any(&self) -> bool {
        !self.script.is_empty()
            || self.render_mode.is_some()
            || self.mode_params.is_some()
            || self.mode_shim.active()
    }

    /// Write what was asked over `config`. A render mode is transient,
    /// unlike the sticky values: when no script asks for one this frame, a
    /// tear that was on is switched off.
    pub(super) fn stamp(&mut self, config: &mut flint_render::PostProcessConfig) {
        super::headless_game::stamp_post(config, &self.script);
        // The deprecated render mode (#746): transient, unlike the sticky
        // overrides above
        let mode = self
            .render_mode
            .map(|(mode, mix)| flint_render::render_mode::RenderMode {
                mode,
                mix,
                params: self.held_mode_params,
            });
        self.mode_shim.stamp(config, mode);
    }

    /// The ocean tint the render mode draws (blood, the underside), for the
    /// frame's inputs.
    pub(super) fn ocean_tint(&self) -> Option<flint_render::OceanTint> {
        self.mode_shim.ocean_tint()
    }
}

/// The HUD: egui, the script-driven 2D draw commands and world text, and
/// the fonts and pictures they draw with.
pub(super) struct Hud {
    pub(super) egui_ctx: egui::Context,
    pub(super) egui_winit: Option<egui_winit::State>,
    pub(super) egui_renderer: Option<egui_wgpu::Renderer>,
    /// Which held keys and buttons belong to egui (game egui layer, ADR 0073).
    pub(super) egui_input: egui_input::EguiInputGate,
    /// Set after the first egui pass: `Context::fonts()` panics before then,
    /// so the script `measure_text` closure falls back to an estimate until
    /// this flips.
    pub(super) egui_fonts_ready: Arc<std::sync::atomic::AtomicBool>,
    /// Script-driven 2D draw commands
    pub(super) draw_commands: Vec<DrawItem>,
    /// Depth-tested world text queued this frame (ADR 0104), laid out in
    /// the egui pass and drawn before the HUD.
    pub(super) world_texts: Vec<flint_core::world_text::WorldText>,
    /// Depth-tested world lines the scripts drew this frame (ADR 0083),
    /// handed to the renderer with the frame's inputs (#739).
    pub(super) world_lines: Vec<flint_core::world_line::WorldLine>,
    /// The screen field the scripts filled this frame (#859), handed to
    /// the renderer with the frame's inputs: what a style with
    /// `blend_source = "field"` follows.
    pub(super) style_field: flint_core::style_field::StyleField,
    pub(super) world_text_pipeline: Option<world_text::WorldTextPipeline>,
    pub(super) textures: HashMap<String, egui::TextureHandle>,
    /// Family names registered from `<project>/fonts/` (file stems + manifest
    /// aliases). The HUD renderer falls back to the default font for names
    /// outside this set.
    pub(super) fonts: HashSet<String>,
    /// Unknown font names already warned about (one warn per name).
    pub(super) fonts_warned: HashSet<String>,
    /// UI sprites already warned about as missing or unreadable, by scene
    /// folder and name: a draw command asks for its sprite every frame.
    pub(super) sprites_warned: HashSet<String>,
    /// Wide-stroke and glow pictures for HUD and world text (#626).
    pub(super) text_halos: text_halo::TextHalos,
    /// The font files and manifest the current font set was read from;
    /// read again on a scene change only when the new scene sees others.
    pub(super) fonts_root: fonts::ProjectFonts,
    /// Whether egui has been given the fonts at least once. Until then
    /// `refresh_project_fonts` always installs them.
    pub(super) fonts_installed: bool,
    /// What a debug key with nothing to act on said (#367), shown for a
    /// moment at the top of the window.
    pub(super) key_note: super::key_note::KeyNote,
    /// The F1 card of the run's debug keys is showing (#717).
    pub(super) key_table_open: bool,
}

impl Default for Hud {
    fn default() -> Self {
        Self {
            egui_ctx: egui::Context::default(),
            egui_winit: None,
            egui_renderer: None,
            egui_input: Default::default(),
            egui_fonts_ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            draw_commands: Vec::new(),
            world_texts: Vec::new(),
            world_lines: Vec::new(),
            style_field: Default::default(),
            world_text_pipeline: None,
            textures: HashMap::new(),
            fonts: HashSet::new(),
            fonts_warned: HashSet::new(),
            sprites_warned: HashSet::new(),
            text_halos: Default::default(),
            fonts_root: Default::default(),
            fonts_installed: false,
            key_note: Default::default(),
            key_table_open: false,
        }
    }
}

/// The F2 stats overlay, the frame history and the perf readout.
pub(super) struct Stats {
    /// What the F2 overlay shows (#1078): nothing, all of it, or the
    /// frame-time graph alone
    pub(super) view: super::StatsView,
    /// What the last HUD pass drew: a view drawn for the first frame since
    /// it changed leaves a note of what it cost (#1129)
    pub(super) drawn_view: super::StatsView,
    /// The frame-time graph's buffers and ceiling, kept across frames
    #[cfg(feature = "debug-hud")]
    pub(super) graph: super::stats_view::StatsGraph,
    pub(super) frame_times: std::collections::VecDeque<f64>,
    /// The player turned the renderer's GPU timing on for the overlay
    /// (ADR 0108), so it turns it off again when the overlay closes
    pub(super) gpu_timing: bool,
    /// The frame recorder: the frame history (#1076, the last
    /// `HISTORY_FRAMES` frames, recorded always where the build has
    /// `frame-history`), which the perf readout (`--perf`, `?perf=1`,
    /// #120) sums up when it is on (`perf.readout`).
    pub(super) perf: perf::PerfRecorder,
    /// What counts as a hitch, and the hitches so far (#1076), judged as
    /// each frame's clock comes in.
    pub(super) hitches: hitch::HitchDetector,
    /// What the run keeps of them (#1079): the hitch log, the exit
    /// report's counts and the `--max-hitches` gate.
    pub(super) watch: hitch_watch::HitchWatch,
    /// The renderer's GPU-call reading at the last perf readout (#1091):
    /// the next readout's window is the difference.
    pub(super) gpu_calls_then: Option<perf::GpuCallsReading>,
    /// The last frame F2 saw create GPU objects: its game time and what it
    /// made, so a one-frame creation stays readable on the overlay.
    pub(super) last_gpu_objects: Option<(f64, flint_render::gpu_counts::GpuCalls)>,
    /// Every load's report so far, the start-up first (#1092)
    pub(super) loads: Vec<flint_core::load_report::LoadReport>,
    /// The start-up's wait from `run_app` to the first `resumed`, timed
    /// as the window's (#1092)
    pub(super) awaiting_window: Option<flint_core::load_report::Span>,
    /// `--perf-report FILE` (#1093): where the exit report's JSON goes and
    /// the run's phase and script totals it is built from.
    pub(super) report: Option<super::perf_report::ReportTo>,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            view: super::StatsView::Off,
            drawn_view: super::StatsView::Off,
            #[cfg(feature = "debug-hud")]
            graph: Default::default(),
            frame_times: Default::default(),
            gpu_timing: false,
            // `frame-history`, and `FLINT_FRAME_HISTORY` over it
            perf: perf::PerfRecorder::recording_always(super::frame_history::always_on_from_env()),
            // `FLINT_HITCH_FACTOR` / `FLINT_HITCH_FLOOR_MS`, read once.
            hitches: hitch::HitchDetector::new(hitch::HitchThresholds::from_env()),
            watch: Default::default(),
            gpu_calls_then: None,
            last_gpu_objects: None,
            loads: Vec::new(),
            awaiting_window: None,
            report: None,
        }
    }
}

/// When the `persist_*` store (the game core's) is written (see
/// `persist.rs`).
pub(super) struct Saves {
    /// When to write the save file after a change.
    pub(super) debounce: SaveDebounce,
    /// The last save failure logged, so one that lasts is said once.
    pub(super) error: Option<String>,
    /// The OS user directory saves and input rebinds go under
    /// (`user_files::config_dir`), a field so a test can name another.
    pub(crate) user_config_dir: Option<std::path::PathBuf>,
}

impl Default for Saves {
    fn default() -> Self {
        Self {
            debounce: SaveDebounce::default(),
            error: None,
            user_config_dir: crate::user_files::config_dir(),
        }
    }
}

/// How the run is to end, and why it fails if it does.
#[derive(Default)]
pub(super) struct Outcome {
    /// Set once `--exit-after` elapses; the redraw handler exits the loop.
    pub(super) exit_requested: bool,
    /// Why `initialize` failed, kept so the binary can exit non-zero (#639).
    pub(super) init_error: Option<String>,
    /// The first scene a transition could not enter, with the path it was
    /// read from and why (#723). The scene playing then is kept, and the run
    /// exits non-zero when it ends.
    pub(super) scene_failure: Option<String>,
}

/// What the styles keep between frames.
#[derive(Default)]
pub(super) struct StyleState {
    /// Resets style history when the camera cuts (#343)
    pub(super) camera_cut: camera_cut::CameraCutDetector,
    /// A look F4's picker chose this frame (#356; `Some(None)`: none) and
    /// the seconds to fade to it over (#496; 0 cuts), laid under the
    /// scene's keys at the start of the next render.
    pub(super) pending_look: Option<(Option<String>, f64)>,
    /// A cross-fade to a look in progress (#496) and the look it ends on,
    /// stepped on game time each render; dropped at its end, a cut, or a
    /// scene's teardown.
    pub(super) look_fade: Option<(flint_render::look_fade::LookFade, Option<String>)>,
}

/// The input config's layers (built-in, game, user overrides, CLI) and a
/// rebind in progress.
pub(super) struct InputLayers {
    /// `--input-config`: the CLI's layer, over everything else
    pub(super) config_override: Option<String>,
    pub(super) config_paths: Option<InputConfigPaths>,
    pub(super) user_override: InputConfig,
    pub(super) pending_rebind: Option<PendingRebind>,
    /// The actions the game's layer and `--input-config` name: what the
    /// controls line lists (#716), with the player's rebinds on them. Empty
    /// when neither names any, and the built-ins are the game's controls.
    pub(super) named_actions: std::collections::BTreeSet<String>,
    /// The controls line last said, so a transition says it again only
    /// when the bindings changed
    pub(super) said_controls: Option<String>,
}

impl InputLayers {
    pub(super) fn new(config_override: Option<String>) -> Self {
        Self {
            config_override,
            config_paths: None,
            user_override: InputConfig {
                version: 1,
                game_id: String::new(),
                actions: Default::default(),
                touch_controls: None,
                debug_keys: None,
            },
            pending_rebind: None,
            named_actions: Default::default(),
            said_controls: None,
        }
    }
}

/// The window's keyboard focus, the cursor, and the keys held.
pub(super) struct Focus {
    pub(super) cursor_captured: bool,
    /// Whether the window has the keyboard focus; real pads are dropped
    /// and released while it doesn't (ADR 0110).
    pub(super) window_focused: bool,
    /// The `InputState` slots real pads (gilrs) have written, released when
    /// the window loses focus.
    pub(super) real_pad_slots: HashSet<u32>,
    /// The modifiers held now, real or scripted, for `PlayerExtension::on_key`.
    pub(super) key_modifiers: winit::keyboard::ModifiersState,
    /// Keys held whose press was a debug key (#717): their repeats and
    /// release don't reach the game either.
    pub(super) debug_keys_held: HashSet<winit::keyboard::KeyCode>,
}

impl Default for Focus {
    fn default() -> Self {
        Self {
            cursor_captured: false,
            window_focused: true,
            real_pad_slots: Default::default(),
            key_modifiers: Default::default(),
            debug_keys_held: Default::default(),
        }
    }
}

/// The quality settings in force (see `quality.rs`), the render scale's
/// steps under its ceiling, and a script's `set_quality` waiting for the
/// frame's end.
#[derive(Default)]
pub(super) struct QualityState {
    pub(super) settings: flint_render::QualitySettings,
    pub(super) dynamic_scale: render_scale::DynamicScale,
    /// A script's `set_quality`, applied between the frame's scripts and
    /// its draw (FlintKart#55).
    pub(super) script_request: Option<flint_core::quality::QualitySettings>,
}

/// The project's asset catalog and content store, for content-addressed
/// asset resolution (#1065): `<root>/assets` and `<root>/.flint/assets`,
/// the ones `flint validate` and `flint check` ask
/// ([`HeldAssets::of`]), wherever the player was started from. Read on
/// first use and kept for that project root and file source, so a
/// transition into another project, or the web's bundle installed after
/// the app was made, reads again.
#[derive(Default)]
pub(super) struct AssetSources {
    /// The root and `vfs::generation` read for, and what was found there
    /// (`None`: that project has no catalog).
    read: Option<(PathBuf, u64, Option<HeldAssets>)>,
}

impl AssetSources {
    /// `project`'s catalog and store, read now unless they are held.
    pub(super) fn of(&mut self, project: &Project) -> Option<&HeldAssets> {
        let generation = flint_core::vfs::generation();
        let held = matches!(&self.read,
            Some((root, read_at, _)) if *root == project.root && *read_at == generation);
        if !held {
            self.read = Some((project.root.clone(), generation, HeldAssets::of(project)));
        }
        self.read
            .as_ref()
            .and_then(|(_, _, assets)| assets.as_ref())
    }
}
