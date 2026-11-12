//! `PlayerApp`'s fields, grouped by what they belong to (#724, ADR 0117
//! step 1): what the host asked for at launch, what the current scene's file
//! says, what scripts asked of this frame's post-processing, living props and
//! wooze, the HUD, and so on. Each group is one field of `PlayerApp`, so a
//! feature's state is in one place and the struct reads as a list of parts.
//!
//! Moving a field into a group changes no behaviour: every value starts as it
//! did, and every read and write is the same one through the group.

use super::input_config::{InputConfigPaths, PendingRebind};
use super::{camera_cut, expression_feed, fonts, music_session, perf, procgen_assets};
use super::{egui_input, render_scale, text_halo, world_text};
use flint_asset::{AssetCatalog, ContentStore};
use flint_runtime::{InputConfig, SaveDebounce};
use flint_script::context::DrawItem;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

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
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            fullscreen: false,
            window_size: (1280, 720),
            window_options: crate::WindowArgs::default(),
            msaa_sample_count: 1,
            procgen_mode: procgen_assets::ProcGenMode::Immediate,
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
            save_dir: None,
            expression_feed: None,
            compare: None,
            compare_at_start: None,
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
    pub living: Option<flint_procgen::LivingDef>,
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
            living: None,
            wooze: None,
        }
    }
}

/// What scripts asked of this frame's post-processing, drained from their
/// requests each frame and stamped over the renderer's config before it
/// draws (`set_bloom`, `set_render_mode`, ...).
#[derive(Default)]
pub(super) struct PostOverrides {
    /// The plain values: vignette, bloom, exposure, chromatic aberration,
    /// radial blur, SSAO, fog density, desaturation and depth of field.
    pub(super) script: flint_script::PostProcessOverrides,
    pub(super) fog_color: Option<[f32; 3]>,
    pub(super) render_mode: Option<(u32, f32)>,
    pub(super) mode_params: Option<[f32; 4]>,
    /// True while a script-driven reality tear is on screen. Unlike the
    /// other (sticky) overrides, the mode is zeroed the frame the script
    /// stops calling set_render_mode — a dead or hot-reloaded script must
    /// never freeze a tear over the world.
    pub(super) mode_was_active: bool,
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
        self.fog_color = requests.fog_color.take();
        self.render_mode = requests.render_mode.take();
        self.mode_params = requests.mode_params.take();
    }

    /// Whether anything is to be stamped this frame: a value asked for, or
    /// a tear to end.
    pub(super) fn any(&self) -> bool {
        let s = &self.script;
        s.vignette.is_some()
            || s.bloom.is_some()
            || s.exposure.is_some()
            || s.chromatic_aberration.is_some()
            || s.radial_blur.is_some()
            || s.ssao_intensity.is_some()
            || s.fog_density.is_some()
            || s.desaturation.is_some()
            || s.dof_strength.is_some()
            || s.dof_focus_distance.is_some()
            || s.dof_focus_range.is_some()
            || self.fog_color.is_some()
            || self.render_mode.is_some()
            || self.mode_params.is_some()
            || self.mode_was_active
    }

    /// Write what was asked over `config`. A render mode is transient,
    /// unlike the sticky values: when no script asks for one this frame, a
    /// tear that was on is switched off.
    pub(super) fn stamp(&mut self, config: &mut flint_render::PostProcessConfig) {
        let s = &self.script;
        if let Some(v) = s.vignette {
            config.vignette_enabled = v > 0.001;
            config.vignette_intensity = v;
        }
        if let Some(b) = s.bloom {
            config.bloom_intensity = b;
        }
        if let Some(e) = s.exposure {
            config.exposure = e;
        }
        if let Some(ca) = s.chromatic_aberration {
            config.chromatic_aberration = ca;
        }
        if let Some(rb) = s.radial_blur {
            config.radial_blur = rb;
        }
        if let Some(si) = s.ssao_intensity {
            config.ssao_intensity = si;
        }
        if let Some(fd) = s.fog_density {
            config.fog_density = fd;
        }
        if let Some(d) = s.desaturation {
            config.desaturate = d;
        }
        if let Some(strength) = s.dof_strength {
            config.dof_strength = strength;
        }
        if let Some(fd) = s.dof_focus_distance {
            config.dof_focus_distance = fd;
        }
        if let Some(fr) = s.dof_focus_range {
            config.dof_focus_range = fr;
        }
        if let Some(fc) = self.fog_color {
            config.fog_color = fc;
        }
        // Reality tear: transient, unlike the sticky overrides above.
        match self.render_mode {
            Some((mode, mix)) => {
                config.render_mode = mode;
                config.mode_mix = mix;
                self.mode_was_active = mode != 0 && mix > 0.0;
            }
            None if self.mode_was_active => {
                config.render_mode = 0;
                config.mode_mix = 0.0;
                self.mode_was_active = false;
            }
            None => {}
        }
        if let Some(mp) = self.mode_params {
            config.mode_params = mp;
        }
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
            world_text_pipeline: None,
            textures: HashMap::new(),
            fonts: HashSet::new(),
            fonts_warned: HashSet::new(),
            sprites_warned: HashSet::new(),
            text_halos: Default::default(),
            fonts_root: Default::default(),
            fonts_installed: false,
        }
    }
}

/// The scene-declared music session (F3, ADR 0019) and what it changed.
/// `PlayerApp` declares it before `audio`, so the suite's kira handles drop
/// before the shared AudioManager (ADR 0017 drop-order rule).
#[derive(Default)]
pub(super) struct MusicState {
    pub(super) session: Option<music_session::MusicSession>,
    /// Set when a session with `quit_on_finish` ends on its own; the event
    /// loop exits at the end of that frame (checked in RedrawRequested,
    /// where the ActiveEventLoop is in scope).
    pub(super) exit_requested: bool,
    /// Ladder-driven post params: the scene's authored base, captured at
    /// session start and written back after teardown (ADR 0021).
    pub(super) pp_base: Option<music_session::LadderPostBase>,
    pub(super) pp_restore: Option<music_session::LadderPostBase>,
}

/// The F2 stats overlay and the perf readout.
#[derive(Default)]
pub(super) struct Stats {
    /// The F2 overlay is open
    pub(super) show: bool,
    pub(super) frame_times: std::collections::VecDeque<f64>,
    /// The player turned the renderer's GPU timing on for the overlay
    /// (ADR 0108), so it turns it off again when the overlay closes
    pub(super) gpu_timing: bool,
    /// The perf readout's recorder (`?perf=1` on the web, #120): frame and
    /// phase times for the last ten seconds. `None` costs nothing.
    pub(super) perf: Option<perf::PerfRecorder>,
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
    /// A look F4's picker chose this frame (#356; `Some(None)`: none),
    /// laid under the scene's keys at the start of the next render.
    pub(super) pending_look: Option<Option<String>>,
}

/// The input config's layers (built-in, game, user overrides, CLI) and a
/// rebind in progress.
pub(super) struct InputLayers {
    /// `--input-config`: the CLI's layer, over everything else
    pub(super) config_override: Option<String>,
    pub(super) config_paths: Option<InputConfigPaths>,
    pub(super) user_override: InputConfig,
    pub(super) pending_rebind: Option<PendingRebind>,
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
            },
            pending_rebind: None,
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
}

impl Default for Focus {
    fn default() -> Self {
        Self {
            cursor_captured: false,
            window_focused: true,
            real_pad_slots: Default::default(),
            key_modifiers: Default::default(),
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

/// The asset catalog and content store, for content-addressed asset
/// resolution (both optional).
pub(super) struct AssetSources {
    pub(super) catalog: Option<AssetCatalog>,
    pub(super) content_store: Option<ContentStore>,
}

impl AssetSources {
    /// The working directory's `assets/` catalog and `.flint/assets` store.
    pub(super) fn from_working_dir() -> Self {
        Self {
            catalog: AssetCatalog::load_from_directory("assets").ok(),
            content_store: Some(ContentStore::new(".flint/assets")),
        }
    }
}
