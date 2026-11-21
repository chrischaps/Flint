//! One launch for the desktop player (#705): the flags `flint play` and
//! `flint-player` share, and the steps that turn them into a running
//! `PlayerApp`. Each binary flattens [`PlayerArgs`] and calls [`launch`]
//! (`flint-player` through [`PlayerCli`], which adds `--bundle`), so a flag
//! added here reaches both and a scene plays the same under either.
//!
//! The web player (`bin/flint_web.rs`, from its page's query) and Android
//! (`flint-android`, from its profile) build their `PlayerApp` from their
//! own sources; what they share with this is `PlayerApp::apply_scene_settings`
//! and [`PlayerApp::step_frames`].

use crate::PlayerApp;
use anyhow::{Context, Result};
use flint_core::bundle::Bundle;
use flint_core::frame_notes::NoteKind;
use flint_core::load_report::{self, LoadKind, Stage};
use flint_core::vfs;
use flint_scene::load_scene;
use flint_schema::SchemaRegistry;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use winit::event_loop::{ControlFlow, EventLoop};

/// Every flag of the desktop player but `--bundle`: `flint play` flattens
/// it whole, and `flint-player` flattens it beside `--bundle`
/// ([`PlayerCli`]), so the two take the same flags with the same help.
#[derive(clap::Args, Clone, Debug)]
pub struct PlayerArgs {
    /// Path to scene file. Without it: the project's default scene,
    /// flint.toml's `[project] scene`
    #[arg(default_value = "")]
    pub scene: String,

    /// Schema directories, later ones overriding earlier (repeatable).
    /// Without it: the project's flint.toml `schemas`, else schemas/ in the
    /// working directory, beside the scene and one level up
    #[arg(long, action = clap::ArgAction::Append)]
    pub schemas: Vec<String>,

    /// Launch in fullscreen mode
    #[arg(long)]
    pub fullscreen: bool,

    /// Optional input config overlay path
    #[arg(long)]
    pub input_config: Option<String>,

    /// Scales the music bus's starting volume (linear, 0.0 = muted, 1.0 = the
    /// volume the game's flint.toml gives it, or full)
    #[arg(long, default_value_t = 1.0)]
    pub music_volume: f64,

    /// Scales the SFX bus's starting volume (linear, 0.0 = muted, 1.0 = the
    /// volume the game's flint.toml gives it, or full)
    #[arg(long, default_value_t = 1.0)]
    pub sfx_volume: f64,

    /// MSAA sample count for the scene passes: 1 (off) or 4 (ADR 0058)
    #[arg(long, default_value_t = 1)]
    pub msaa: u32,

    /// Don't generate procgen assets during load: show placeholders (a
    /// magenta box or texel) and swap each real asset in as the
    /// frame-budgeted queue makes it (#32)
    #[arg(long)]
    pub procgen_defer: bool,

    /// Draw the 3D scene at this fraction of the window's pixels on each
    /// side (0.25-1), upscaled under a full-resolution HUD; `auto` steps
    /// down to hold 60 fps and back up when it can (#122)
    #[arg(long, value_name = "SCALE|auto")]
    pub render_scale: Option<String>,

    /// Quality settings as `key=value,...`, applied over --msaa and
    /// --render-scale (FlintKart#55). Keys: render_scale, render_scale_auto,
    /// msaa, shadows, shadow_resolution, shadow_cascades, shadow_distance,
    /// post_process, bloom, ssao, ssao_samples, fog, volumetric. Scripts
    /// change them while running with set_quality()
    #[arg(long, value_name = "KEY=VALUE,...")]
    pub quality: Option<String>,

    /// On-screen touch controls from the input config's `[touch_controls]`
    /// block (#133): `auto` shows them once a finger touches, `always`
    /// shows them and lets the mouse's left button drive them, `never`
    #[arg(long, value_name = "auto|always|never", default_value = "auto")]
    pub touch_controls: String,

    /// Stand-in safe-area insets in points, `left,top,right,bottom` (#660):
    /// what a notch or home bar would cover, for trying a HUD that reads
    /// `safe_area()` on a desktop. The touch controls keep inside them too
    #[arg(long, value_name = "L,T,R,B")]
    pub safe_area: Option<String>,

    /// Report a touch-first device to scripts (`is_touch_device()`, #660),
    /// as a phone's page or Android does from the first frame
    #[arg(long)]
    pub touch_device: bool,

    /// Turn on a registered style over the scene's post-processing, as
    /// `flint render --style` does (#328): `name[:key=value,...]`, e.g.
    /// `ascii`, `ascii:cell=12,ramp=blocks`, `pencil:spacing=5`,
    /// `ascii:off`. Repeatable; a later word wins. Written over every
    /// scene's `[post_process]` again after each transition, so it survives
    /// them. `--style help` lists every style and its keys
    #[arg(long = "style", value_name = "NAME[:KEY=VALUE,...]", value_parser = parse_style_word)]
    pub styles: Vec<String>,

    /// Hold the post time animated styles see (and the ocean, grain and
    /// render modes, which share it) at this many seconds
    #[arg(long, value_name = "SECONDS")]
    pub style_time: Option<f32>,

    /// Motion on twos for the whole frame (#405): every entity's drawn
    /// position and pose change only this many times a second, as does the
    /// post time styles read (12 = on twos at 24 fps; 0 = off). Over every
    /// scene's `[post_process] motion_hold`, kept through transitions
    #[arg(long, value_name = "RATE")]
    pub motion_hold: Option<f32>,

    /// GPU adapter to use (case-insensitive substring of the adapter name,
    /// e.g. "NVIDIA"). Overrides WGPU_ADAPTER_NAME; default prefers the
    /// high-performance adapter. Use --list-gpus to see names.
    #[arg(long)]
    pub gpu: Option<String>,

    /// List available GPU adapters and exit
    #[arg(long)]
    pub list_gpus: bool,

    /// Print the debug keys (#717) and exit: each key, its action and
    /// owner, what it does and when it is live, after the scene's input
    /// layers (their `[debug_keys]` rebinds, and keys the game binds,
    /// which go to the game). F1 shows the same in a running game
    #[arg(long)]
    pub list_keys: bool,

    /// With --list-keys: the table as JSON
    #[arg(long, requires = "list_keys")]
    pub json: bool,

    /// `--width`/`--height`, `--position`, `--borderless`, `--always-on-top`
    /// and `--no-activate`: the window's size and place, its frame, whether
    /// it stays on top and whether it takes the keyboard (#611)
    #[command(flatten)]
    pub window: crate::WindowArgs,

    /// Write a PNG of the presented frame (scene + HUD) to this path.
    /// With --screenshot-every the path becomes a numbered base name
    /// (hud.png -> hud_0001.png, hud_0002.png, ...).
    #[arg(long, value_name = "PATH")]
    pub screenshot: Option<String>,

    /// Game time in seconds at which to take the screenshot (first frame
    /// whose elapsed time reaches this value). Default 0 = first frame.
    #[arg(long, default_value_t = 0.0, value_name = "SECS")]
    pub screenshot_at: f64,

    /// Keep capturing every N seconds after --screenshot-at until exit,
    /// writing numbered files next to --screenshot
    #[arg(long, value_name = "SECS")]
    pub screenshot_every: Option<f64>,

    /// Compare two Living Props presets on the same grove from the start
    /// (#238): A,B, each a preset's name, or nothing for the knobs as the scene
    /// loaded them (",linklater"). F6 flips between them, F7 splits the view
    #[arg(long, value_name = "A,B")]
    pub compare: Option<String>,

    /// With --compare: start split, A on the left and B on the right
    #[arg(long, requires = "compare")]
    pub split: bool,

    /// Exit the player once game time reaches this many seconds
    #[arg(long, value_name = "SECS")]
    pub exit_after: Option<f64>,

    /// Advance the game exactly this many seconds per frame, whatever the
    /// wall time. With `--screenshot-every` equal to it, every frame is
    /// written: smooth offline video however slow the capture runs.
    #[arg(long, value_name = "SECS")]
    pub frame_step: Option<f64>,

    /// Seed scripts' `random()` and `random_range()`: one sequence, the same
    /// every run (#635). `--frame-step` seeds them too, with the seed
    /// `flint render --scripts` uses, unless this names another; without
    /// either they draw from the wall clock
    #[arg(long, value_name = "N")]
    pub seed: Option<u64>,

    /// Play every scene under a look (#279, #435): a built-in
    /// (ascii_noir, matrix, ink_wash, graphite, watercolor), a
    /// `styles/<name>.look.toml` beside the scene, or a path. Merged over the
    /// scene's `[post_process]` as `flint render --look` does
    #[arg(long, value_name = "NAME|PATH")]
    pub look: Option<String>,

    /// Draw point-sized things (ASCII cells, egui panels, the HUD) at this
    /// many pixels a point instead of the window's scale factor, so a
    /// capture is the same on any display (#435). 2 at 3840x2160 lays a
    /// 4K frame out as 1920x1080 points
    #[arg(long, value_name = "SCALE")]
    pub pixels_per_point: Option<f32>,

    /// Play timed key, gamepad and mouse input from a file (see
    /// `player_app/input_script.rs` for the format)
    #[arg(long, value_name = "FILE")]
    pub input_script: Option<String>,

    /// Play expression states from a JSON timeline on the game clock:
    /// `{"events": [{..., "t": secs}, ...]}`. Scripts read the latest with
    /// `expression()` and hear each in `on_expression(state)` (Flint#518)
    #[arg(long, value_name = "FILE")]
    pub expression_timeline: Option<String>,

    /// Take expression states as newline-delimited JSON objects over TCP at
    /// this loopback address (e.g. 127.0.0.1:7777). Off unless given; only
    /// 127.0.0.1 or ::1 can listen (Flint#518). It opens once the first frame
    /// is presented, and says so in one `listen` log line: a client that
    /// connects is heard from the next frame on (Flint#584)
    #[arg(long, value_name = "IP:PORT")]
    pub expression_listen: Option<String>,

    /// Answer questions about the live world at this loopback address
    /// (e.g. 127.0.0.1:7878), a line of JSON for each line asked: `flint
    /// query --attach ADDR "entities where ..."` and `flint entity show NAME
    /// --attach ADDR` ask it (#713). Off unless given; only 127.0.0.1 or ::1
    #[arg(long, value_name = "IP:PORT")]
    pub inspect_listen: Option<String>,

    /// Write the world as JSON to this file: each entity `--dump-query`
    /// picks, with its components' values, at each `--dump-world-at` time
    /// (#713). The run fails if a time never comes
    #[arg(long, value_name = "FILE")]
    pub dump_world: Option<PathBuf>,

    /// Game times in seconds for `--dump-world`, repeatable or comma
    /// separated (`--dump-world-at 1,2.5`); each is taken on the first frame
    /// at or after it. Default: `--exit-after`'s time, else the first frame
    #[arg(
        long,
        value_name = "SECS",
        value_delimiter = ',',
        requires = "dump_world"
    )]
    pub dump_world_at: Vec<f64>,

    /// Which entities `--dump-world` writes, in `flint query`'s grammar
    /// (`entities where archetype == 'player'`). Default: all of them
    #[arg(
        long,
        value_name = "QUERY",
        default_value = "entities",
        requires = "dump_world"
    )]
    pub dump_query: String,

    /// Open the rendering stats overlay (F2) from the first frame, so a
    /// capture shows it: `--stats` the whole of it (the frame-time graph,
    /// its hitch counters and the rendering stats), `--stats=graph` the
    /// graph and counters alone, in a corner
    #[arg(
        long,
        value_enum,
        value_name = "VIEW",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "full"
    )]
    pub stats: Option<crate::StatsView>,

    /// Print a perf line every 10 s: frame p50/p99, each phase's CPU time
    /// and each pass's GPU time, as the web page's `?perf=1` copies it
    #[arg(long)]
    pub perf: bool,

    /// Don't watch the game's files for changes (#633): scripts, UI
    /// documents, user styles, glyph sets and palettes load once and never
    /// reload, and nothing polls the disk. For captures and shipped builds
    #[arg(long)]
    pub no_hot_reload: bool,

    /// Keep this run's save data in DIR, as DIR/<slot>.toml, and read no
    /// other save (#726). Without it: the OS user directory,
    /// <config>/<game id>/saves/, or the project's save/ for a game with
    /// no id
    #[arg(long, value_name = "DIR")]
    pub save_dir: Option<PathBuf>,

    /// Exit non-zero when any script failed, to load, to compile or at run
    /// time (#693), so a headless --exit-after run of a broken game fails.
    /// Every error is counted either way, and --exit-after prints them
    #[arg(long)]
    pub strict_scripts: bool,

    /// `--hitch-log FILE` (a JSON line for every hitch), `--max-hitches N`,
    /// `--max-hitches-startup N` and `--max-hitches-of CLASS` (exit
    /// non-zero past them) (#1079)
    #[command(flatten)]
    pub hitches: crate::HitchArgs,

    /// `--perf-report FILE`: the `--exit-after` report as JSON, for
    /// `scripts/perf-compare.py` (#1093)
    #[command(flatten)]
    pub perf_report: crate::PerfReportArgs,

    /// `--capture-frame-at SECS` (repeatable), `--capture-on-hitch[=N]`
    /// (past the start; `--capture-on-hitch-startup` for the first 2 s too,
    /// #1207) and `--capture-dir DIR`: frames for the frame debugger, as
    /// `.flintframe` folders (#1086). Shift+F2 captures one too
    #[command(flatten)]
    pub frame_captures: crate::FrameCaptureArgs,

    /// `--record DIR` and `--record-fps N`: the window's frames as shown,
    /// written while the game plays without slowing it (#583)
    #[command(flatten)]
    pub record: crate::RecordArgs,

    /// `--trace FILE` (the engine's scopes, written at exit),
    /// `--trace-seconds SECS` (how far back; 0 the whole session) and
    /// `--trace-on-hitch[=N]`: Chrome trace-event JSON for Perfetto
    /// (#1097). Shift+F3 saves one mid-play
    #[command(flatten)]
    pub trace: crate::TraceArgs,

    /// `--load-report`: where each load's time went, a table a load
    /// (#1092). --perf prints it too
    #[command(flatten)]
    pub loads: crate::LoadReportArgs,

    /// `--skip-pass LABEL` and `--repeat-pass LABEL:N`: cost a part of the
    /// frame on any GPU by taking it away or drawing it N times (#1317)
    #[command(flatten)]
    pub pass_knobs: crate::PassKnobArgs,

    /// `--log-format text|json` and `--quiet`: how and how much the player
    /// says, which its binary's `main` hands `logging::init` (#716)
    #[command(flatten)]
    pub log: crate::logging::LogArgs,
}

/// `flint-player`'s command line: [`PlayerArgs`] and `--bundle`, the one
/// flag `flint play` hasn't (a bundle is what a shipped game plays).
#[derive(clap::Parser, Clone, Debug)]
#[command(name = "flint-player")]
#[command(about = "Flint game player - run scenes with physics and first-person controls")]
#[command(after_help = flint_core::env::help_section(&flint_core::env::PLAYER_GROUPS))]
pub struct PlayerCli {
    /// Play a game bundle made by `flint bundle` (#98) instead of the disk.
    /// Every read goes to its files; the scene is the bundle's start scene,
    /// or a path inside the bundle when one is named. Its late bundles
    /// (audio) beside it are added a second after start, the way the web
    /// player adds them
    #[arg(long, value_name = "FILE")]
    pub bundle: Option<PathBuf>,

    #[command(flatten)]
    pub args: PlayerArgs,
}

impl PlayerCli {
    /// Play what the command line names, from a bundle or the disk.
    pub fn run(self) -> Result<()> {
        match self.bundle {
            Some(bundle) => launch_bundle(self.args, &bundle),
            None => launch(self.args),
        }
    }
}

/// A `--style` word whose syntax is checked at once. An unknown name gets
/// through: it may be a user style the scene's `styles/` registers (#274),
/// checked once those are loaded.
fn parse_style_word(s: &str) -> Result<String, String> {
    match flint_render::stylize::StyleSpec::parse(s, &flint_render::stylize::StyleRegistry::new()) {
        Ok(_) => Ok(s.to_string()),
        Err(e) if e.starts_with("unknown style") => Ok(s.to_string()),
        Err(e) => Err(e),
    }
}

/// Play a scene from the disk: the flags' early switches (`--list-gpus`,
/// `--gpu`, `--no-hot-reload`), then [`PlayerArgs::prepare`], then the
/// event loop. A failed start, a screenshot that never reached the disk, a
/// script error under `--strict-scripts` or a hitch gate exits non-zero.
pub fn launch(args: PlayerArgs) -> Result<()> {
    start(args, None)
}

/// [`launch`] with every read served from a game bundle (`flint bundle`):
/// its start scene and schema list fill in what the flags left out.
pub fn launch_bundle(args: PlayerArgs, bundle: &Path) -> Result<()> {
    start(args, Some(bundle))
}

/// A `PlayerApp` built from the flags and its scene, before its window:
/// what [`launch`] runs. A test reads it to see what either entry point
/// would play.
pub struct Prepared {
    pub app: PlayerApp,
    /// The bundle's files and its late bundles' paths, added a few
    /// seconds after start
    late_bundles: Option<(vfs::MemorySource, Vec<PathBuf>)>,
}

fn start(args: PlayerArgs, bundle: Option<&Path>) -> Result<()> {
    let began = web_time::Instant::now();
    if args.list_gpus {
        let adapters = flint_render::list_adapters();
        if adapters.is_empty() {
            tracing::warn!(target: "player", "No GPU adapters found: {}", flint_render::no_adapter_hint());
        }
        // The answer to --list-gpus: stdout, where a caller reads it
        for a in adapters {
            println!("{a}");
        }
        return Ok(());
    }
    flint_render::set_gpu_override(args.gpu.clone());
    if args.no_hot_reload {
        flint_core::file_watch::disable();
    }

    if args.list_keys {
        let json = args.json;
        let Prepared { mut app, .. } = args.prepare(bundle)?;
        let table = app.list_debug_keys();
        if json {
            println!("{}", serde_json::to_string_pretty(&table)?);
        } else {
            print!("{}", table.text());
        }
        return Ok(());
    }

    // The start-up's load report (#1092), from here to the first frame on
    // screen; its name is the scene's, once it is up
    if args.loads.wanted(args.perf) {
        flint_core::load_report::begin_at(LoadKind::Start, "", began);
    }
    let Prepared {
        mut app,
        late_bundles,
    } = args.prepare(bundle)?;

    // The controls are said from the scene's own bindings once they are
    // loaded (`PlayerApp::say_controls`, #716), not as a fixed banner here
    let event_loop = load_report::timed(Stage::Window, String::new, EventLoop::new)?;
    event_loop.set_control_flow(ControlFlow::Poll);
    if let Some((source, late)) = late_bundles {
        if !late.is_empty() {
            add_late_bundles_soon(source, late);
        }
    }

    app.awaiting_window();
    event_loop.run_app(&mut app)?;
    // A failed start or a screenshot that never reached the disk exits
    // non-zero, never as a clean run (#639), as does a script error under
    // --strict-scripts (#693) or a hitch gate (#1079).
    if let Some(failure) = app.run_failure() {
        anyhow::bail!(failure);
    }
    Ok(())
}

impl PlayerArgs {
    /// Everything before the window: the bundle (if any), the scene (the
    /// project's default when none is named), its schemas through the one
    /// resolver (ADR 0114), its user styles and the `--style` and `--look`
    /// words checked against them, the scene loaded and settled, and a
    /// `PlayerApp` with every flag and the scene's settings applied
    /// (`apply_scene_settings`: lighting, shadows, user styles, post).
    /// Refuses a bad flag value here, before any window opens.
    pub fn prepare(mut self, bundle: Option<&Path>) -> Result<Prepared> {
        let late_bundles = match bundle {
            Some(path) => Some(open_bundle(path, &mut self)?),
            None => None,
        };
        let args = self;
        let from_bundle = bundle.is_some();

        let scene = resolve_scene(args.scene.clone())?;
        // Schemas, styles, the scene parsed and settled: the load's first
        // stage (#1092), a frame note no frame hears
        let read = flint_core::frame_notes::begin(NoteKind::SceneRead, || scene.clone());
        // The one resolver (ADR 0114): --schemas, else the manifest, else
        // discovery. A bundle settled its list when it was packed
        // (`args.schemas` holds it by now), plus what lies beside the scene.
        let scene_path = Path::new(&scene);
        let (registry, schema_dirs) = if from_bundle {
            let dirs = flint_core::project::host_schema_dirs(&args.schemas, Some(scene_path));
            let registry = SchemaRegistry::load_from_directories(&dirs.dirs)
                .context("Failed to load schemas")?;
            (registry, dirs)
        } else {
            match SchemaRegistry::resolve(Some(scene_path), &args.schemas) {
                Ok(loaded) => loaded,
                // Says which directory and who named it: nothing to add
                Err(flint_core::FlintError::SchemaDirs(e)) => return Err(e.into()),
                Err(e) => return Err(e).context("Failed to load schemas"),
            }
        };
        // TEMPORARY, for one release (ADR 0114 decision 3, #765)
        for warning in schema_dirs.dropped_warnings() {
            tracing::warn!(target: "load", "{warning}");
        }
        if schema_dirs.dirs.is_empty() {
            // The log is stderr: stdout is what a command answers with (#710)
            tracing::warn!(target: "load", "No schemas directories found");
        }

        // --style words and --look, checked once the scene's own styles
        // are registered (a user style is known only then, #274) so a typo
        // stops here. They are loaded again, harmlessly, with the scene.
        let styles = if !args.styles.is_empty() || args.look.is_some() {
            crate::load_user_styles(&scene)
        } else {
            flint_render::stylize::StyleRegistry::new()
        };
        flint_render::stylize::check_style_specs(&args.styles, &styles)
            .map_err(anyhow::Error::msg)?;
        if let Some(name) = &args.look {
            let look = flint_render::look::find(name, &scene)
                .map_err(|e| anyhow::anyhow!("--look: {e}"))?;
            tracing::info!(target: "player", "Look: {} ({})", look.name, look.source);
        }
        let render_scale = match &args.render_scale {
            Some(value) => Some(
                crate::RenderScaleSetting::default()
                    .parse(value)
                    .ok_or_else(|| anyhow::anyhow!("--render-scale {value}: a number or auto"))?,
            ),
            None => None,
        };
        if let Some(quality) = &args.quality {
            // Refuse a typo here rather than run with a setting silently dropped.
            let errors = flint_render::QualitySettings::default().apply_str(quality);
            if let Some(e) = errors.first() {
                anyhow::bail!("--quality: {e}");
            }
        }
        let touch_mode = flint_runtime::TouchControlsMode::parse(&args.touch_controls)
            .ok_or_else(|| anyhow::anyhow!("--touch-controls: auto, always or never"))?;
        let safe_area = args.safe_area.as_deref().map(parse_safe_area).transpose()?;
        let compare = args.compare.as_deref().map(parse_compare).transpose()?;
        let input_script = args
            .input_script
            .as_deref()
            .map(crate::InputScript::load)
            .transpose()?;
        // Expression sources before the scene, so a bad timeline or a taken
        // port fails before anything loads
        let expression_feed = crate::ExpressionFeed::open(
            args.expression_timeline.as_deref(),
            args.expression_listen.as_deref(),
        )?;
        // The live world's socket and file (#713), as early, for the same reason
        #[cfg(not(target_arch = "wasm32"))]
        let inspect = args
            .inspect_listen
            .as_deref()
            .map(crate::InspectListener::bind)
            .transpose()?;
        #[cfg(not(target_arch = "wasm32"))]
        let world_dump = match &args.dump_world {
            Some(path) => {
                let at = if args.dump_world_at.is_empty() {
                    vec![args.exit_after.unwrap_or(0.0)]
                } else {
                    args.dump_world_at.clone()
                };
                Some(crate::WorldDump::new(path.clone(), at, &args.dump_query)?)
            }
            None => None,
        };

        #[cfg_attr(not(feature = "living"), allow(unused_mut))]
        let (world, mut scene_file) =
            load_scene(&scene, &registry).with_context(|| flint_scene::load_failed(&scene))?;
        // Living props' tables: checked, and the preset [wooze] names filled in (#686)
        #[cfg(feature = "living")]
        flint_procgen::settle_scene(&mut scene_file, scene_path)
            .with_context(|| flint_scene::load_failed(scene_path))?;
        drop(read);
        // The scene's name and size are said with what it loaded, once it
        // is up (`enter_scene`'s summary). On stderr, so `--list-keys
        // --json`'s stdout is the table alone
        tracing::debug!(
            target: "load",
            "Parsed scene '{}': {} entities",
            scene_file.scene.name,
            world.entity_count()
        );

        let mut app = PlayerApp::new(
            world,
            scene,
            args.fullscreen,
            args.input_config.clone(),
            scene_file.scene.input_config.clone(),
        );
        let launch = &mut app.launch;
        launch.msaa_sample_count = args.msaa;
        launch.save_dir = args.save_dir.clone();
        // A bundle's files aren't on the disk: nothing to key a disk cache by.
        launch.procgen_disk_cache = !from_bundle;
        launch.scene_file_writable = !from_bundle;
        if args.procgen_defer {
            launch.procgen_mode = crate::ProcGenMode::Deferred;
        }
        if let Some(scale) = render_scale {
            launch.render_scale = scale;
        }
        if let Some(quality) = &args.quality {
            launch.quality_overrides = quality.clone();
        }
        launch.safe_area = safe_area;
        launch.touch_device = args.touch_device;
        launch.strict_scripts = args.strict_scripts;
        launch.pixels_per_point = args.pixels_per_point.filter(|p| *p > 0.0);
        launch.look = args.look.clone();
        launch.style_specs = args.styles.clone();
        launch.style_time = args.style_time;
        launch.motion_hold = args.motion_hold;
        launch.window_size = args.window.size();
        launch.window_options = args.window.clone();
        launch.pass_knobs = args.pass_knobs.knobs()?;
        launch.expression_feed = expression_feed;
        #[cfg(not(target_arch = "wasm32"))]
        {
            launch.inspect = inspect;
            launch.world_dump = world_dump;
        }
        app.input.touch_controls_mut().set_mode(touch_mode);
        args.hitches.apply(&mut app)?;
        args.perf_report.apply(&mut app, args.exit_after)?;
        args.frame_captures.apply(&mut app)?;
        args.record.apply(&mut app)?;
        args.trace.apply(&mut app)?;
        args.loads.apply(&mut app, args.perf);
        // Skybox, ambient, shading levers, shadows, default lights, camera,
        // user styles and post from the scene file, as every host and
        // transition take them (#723)
        app.apply_scene_settings(&scene_file);

        app.capture = crate::CaptureConfig::new(
            args.screenshot.map(PathBuf::from),
            args.screenshot_at,
            args.screenshot_every,
            args.exit_after,
        );
        app.capture.input_script = input_script;
        if let Some(view) = args.stats {
            app.capture.show_stats = true;
            app.capture.stats_view = view;
        }
        if let Some((a, b)) = compare {
            #[cfg(feature = "living")]
            app.compare_living_at_start(&a, &b, args.split);
            #[cfg(not(feature = "living"))]
            tracing::warn!(
                target: "player",
                "[living] --compare {a},{b}: this player is built without the `living` feature"
            );
        }
        if args.perf {
            app.enable_perf(true);
        }
        app.step_frames(args.frame_step, args.seed);

        // The game's buses (#730), then the flags over them: each scales
        // the volume its bus starts at (`--music-volume 0` mutes it)
        app.refresh_project();
        for (bus, scale) in [
            (flint_audio::Bus::MUSIC, args.music_volume),
            (flint_audio::Bus::SFX, args.sfx_volume),
        ] {
            let declared = app.audio.engine.bus_volume(bus).unwrap_or(1.0);
            app.audio.set_bus_volume(bus, declared * scale);
        }

        // The registry and its directories, kept for transitions and chunks
        app.set_schemas(registry, schema_dirs);
        Ok(Prepared { app, late_bundles })
    }
}

/// The scene named, or the project's default (ADR 0114) with a line
/// saying so, or an error that says what to pass.
fn resolve_scene(scene: String) -> Result<String> {
    if !scene.is_empty() {
        return Ok(scene);
    }
    let project = flint_core::project::Project::current()?;
    let default = flint_core::project::shown(&project.default_scene()?);
    tracing::info!(
        target: "player",
        "Opening the project's default scene: {} (`[project] scene` in {})",
        default,
        flint_core::project::shown(&project.manifest_path())
    );
    Ok(default)
}

/// `--safe-area L,T,R,B`: four insets in points, none negative
fn parse_safe_area(spec: &str) -> Result<[f32; 4]> {
    let sides: Vec<f32> = spec
        .split(',')
        .map(|v| v.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .map_err(|_| anyhow::anyhow!("--safe-area: four numbers, left,top,right,bottom"))?;
    let [l, t, r, b] = sides[..] else {
        anyhow::bail!("--safe-area: four numbers, left,top,right,bottom");
    };
    if sides.iter().any(|v| !v.is_finite() || *v < 0.0) {
        anyhow::bail!("--safe-area: insets are points, 0 or more");
    }
    Ok([l, t, r, b])
}

/// `--compare A,B`: two presets, A possibly empty (the scene's own knobs)
fn parse_compare(spec: &str) -> Result<(String, String)> {
    let (a, b) = spec
        .split_once(',')
        .ok_or_else(|| anyhow::anyhow!("--compare {spec}: two presets, as A,B"))?;
    if b.trim().is_empty() {
        anyhow::bail!("--compare {spec}: B needs a preset's name");
    }
    Ok((a.trim().to_owned(), b.trim().to_owned()))
}

/// Serve every read from a bundle, its start scene and schemas filling in
/// what the command line left out. Returns its files and its late bundles'
/// paths.
fn open_bundle(path: &Path, args: &mut PlayerArgs) -> Result<(vfs::MemorySource, Vec<PathBuf>)> {
    #[allow(clippy::disallowed_methods)] // the bundle itself is on the disk
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let bundle = Bundle::parse(bytes).with_context(|| path.display().to_string())?;
    let manifest = &bundle.manifest;
    tracing::info!(
        target: "load",
        "Bundle {}: {} files, {:.2} MB",
        manifest.name,
        manifest.files.len(),
        bundle.len() as f64 / 1e6
    );
    if args.scene.is_empty() {
        args.scene = manifest
            .scene
            .clone()
            .context("the bundle names no scene; pass one")?;
    }
    let mut schemas = manifest.schemas.clone();
    schemas.extend(
        args.schemas
            .drain(..)
            .filter(|s| !manifest.schemas.contains(s)),
    );
    args.schemas = schemas;

    let dir = path.parent().unwrap_or(Path::new("."));
    let late = manifest.late.iter().map(|l| dir.join(&l.url)).collect();
    let source = bundle.to_memory_source();
    vfs::install(Arc::new(source.clone()));
    Ok((source, late))
}

/// Add the late bundles a few seconds in, the way the web player adds them
/// once it is running, so the desktop takes the same path: the vfs grows and
/// the player loads the audio that just arrived.
fn add_late_bundles_soon(mut source: vfs::MemorySource, paths: Vec<PathBuf>) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        for path in paths {
            #[allow(clippy::disallowed_methods)] // the bundle itself is on the disk
            let bytes = std::fs::read(&path).map_err(|e| e.to_string());
            match bytes.and_then(|b| Bundle::parse(b).map_err(|e| e.to_string())) {
                Ok(bundle) => {
                    tracing::info!(
                        target: "load",
                        "Late bundle {}: {} files, {:.2} MB",
                        bundle.manifest.part,
                        bundle.manifest.files.len(),
                        bundle.len() as f64 / 1e6
                    );
                    bundle.insert_into(&mut source);
                }
                Err(e) => tracing::warn!(target: "load", "Late bundle {}: {e}", path.display()),
            }
        }
        vfs::install(Arc::new(source));
    });
}
