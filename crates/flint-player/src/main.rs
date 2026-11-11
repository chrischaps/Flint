//! Flint Player - Standalone game player binary
//!
//! Launches a first-person game session from a scene file with physics.
//!
//! Usage:
//!   flint-player <scene.toml> [--schemas <path>] [--fullscreen]
//!                [--width <px>] [--height <px>]
//!                [--screenshot <out.png>] [--screenshot-at <secs>]
//!                [--screenshot-every <secs>] [--exit-after <secs>]
//!                [--frame-step <secs>] [--input-script <file>]
//!                [--expression-timeline <file>] [--expression-listen <ip:port>]
//!   flint-player --bundle <game.flintpack> [scene.toml]
//!
//! Screenshots capture the presented frame — the 3D scene *and* the egui
//! HUD — right before `present()`, so they show exactly what a player sees.
//! `flint render` remains the headless, script-free snapshot; this path is
//! for verifying script-driven UI and live gameplay state.

use anyhow::{Context, Result};
use clap::Parser;
use flint_core::bundle::Bundle;
use flint_core::vfs;
use flint_player::PlayerApp;
use flint_scene::load_scene;
use flint_schema::SchemaRegistry;
use std::sync::Arc;
use winit::event_loop::{ControlFlow, EventLoop};

#[derive(Parser)]
#[command(name = "flint-player")]
#[command(about = "Flint game player - run scenes with physics and first-person controls")]
struct Args {
    /// Path to scene file (with --bundle: optional, a path inside the bundle)
    #[arg(required_unless_present_any = ["list_gpus", "bundle"], default_value = "")]
    scene: String,

    /// Play a game bundle made by `flint bundle` (#98) instead of the disk.
    /// Every read goes to its files; its late bundles (audio) beside it are
    /// added a second after start, the way the web player adds them
    #[arg(long, value_name = "FILE")]
    bundle: Option<std::path::PathBuf>,

    /// Schema directories, later ones overriding earlier (repeatable).
    /// Without it: the project's flint.toml `schemas`, else schemas/ in the
    /// working directory, beside the scene and one level up
    #[arg(long, action = clap::ArgAction::Append)]
    schemas: Vec<String>,

    /// Launch in fullscreen mode
    #[arg(long)]
    fullscreen: bool,

    /// Optional input config overlay path
    #[arg(long)]
    input_config: Option<String>,

    /// Initial music bus volume (linear, 0.0 = muted, 1.0 = full)
    #[arg(long, default_value_t = 1.0)]
    music_volume: f64,

    /// Initial SFX bus volume (linear, 0.0 = muted, 1.0 = full)
    #[arg(long, default_value_t = 1.0)]
    sfx_volume: f64,

    /// MSAA sample count for the scene passes: 1 (off) or 4 (ADR 0058)
    #[arg(long, default_value_t = 1)]
    msaa: u32,

    /// Don't generate procgen assets during load: show placeholders (a
    /// magenta box or texel) and swap each real asset in as the
    /// frame-budgeted queue makes it (#32)
    #[arg(long)]
    procgen_defer: bool,

    /// Draw the 3D scene at this fraction of the window's pixels on each
    /// side (0.25-1), upscaled under a full-resolution HUD; `auto` steps
    /// down to hold 60 fps and back up when it can (#122)
    #[arg(long, value_name = "SCALE|auto")]
    render_scale: Option<String>,

    /// Quality settings as `key=value,...`, applied over --msaa and
    /// --render-scale (FlintKart#55). Keys: render_scale, render_scale_auto,
    /// msaa, shadows, shadow_resolution, shadow_cascades, shadow_distance,
    /// post_process, bloom, ssao, ssao_samples, fog, volumetric. Scripts
    /// change them while running with set_quality()
    #[arg(long, value_name = "KEY=VALUE,...")]
    quality: Option<String>,

    /// On-screen touch controls from the input config's `[touch_controls]`
    /// block (#133): `auto` shows them once a finger touches, `always`
    /// shows them and lets the mouse's left button drive them, `never`
    #[arg(long, value_name = "auto|always|never", default_value = "auto")]
    touch_controls: String,

    /// Stand-in safe-area insets in points, `left,top,right,bottom` (#660):
    /// what a notch or home bar would cover, for trying a HUD that reads
    /// `safe_area()` on a desktop. The touch controls keep inside them too
    #[arg(long, value_name = "L,T,R,B")]
    safe_area: Option<String>,

    /// Report a touch-first device to scripts (`is_touch_device()`, #660),
    /// as a phone's page or Android does from the first frame
    #[arg(long)]
    touch_device: bool,

    /// Turn on a registered style over the scene's post-processing, as
    /// `flint render --style` does (#328): `name[:key=value,...]`, e.g.
    /// `ascii`, `ascii:cell=12,ramp=blocks`, `pencil:spacing=5`,
    /// `ascii:off`. Repeatable; a later word wins. Written over every
    /// scene's `[post_process]` again after each transition, so it survives
    /// them. `--style help` lists every style and its keys
    #[arg(long = "style", value_name = "NAME[:KEY=VALUE,...]")]
    styles: Vec<String>,

    /// Hold the post time animated styles see (and the ocean, grain and
    /// render modes, which share it) at this many seconds
    #[arg(long, value_name = "SECONDS")]
    style_time: Option<f32>,

    /// GPU adapter to use (case-insensitive substring of the adapter name,
    /// e.g. "NVIDIA"). Overrides WGPU_ADAPTER_NAME; default prefers the
    /// high-performance adapter. Use --list-gpus to see names.
    #[arg(long)]
    gpu: Option<String>,

    /// List available GPU adapters and exit
    #[arg(long)]
    list_gpus: bool,

    /// `--width`/`--height`, `--position`, `--borderless`, `--always-on-top`
    /// and `--no-activate` (#611)
    #[command(flatten)]
    window: flint_player::WindowArgs,

    /// Write a PNG of the presented frame (scene + HUD) to this path.
    /// With --screenshot-every the path becomes a numbered base name
    /// (hud.png -> hud_0001.png, hud_0002.png, ...).
    #[arg(long, value_name = "PATH")]
    screenshot: Option<String>,

    /// Game time in seconds at which to take the screenshot (first frame
    /// whose elapsed time reaches this value). Default 0 = first frame.
    #[arg(long, default_value_t = 0.0, value_name = "SECS")]
    screenshot_at: f64,

    /// Keep capturing every N seconds after --screenshot-at until exit,
    /// writing numbered files next to --screenshot
    #[arg(long, value_name = "SECS")]
    screenshot_every: Option<f64>,

    /// Compare two Living Props presets on the same grove from the start
    /// (#238): A,B, each a preset's name, or nothing for the knobs as the scene
    /// loaded them (",linklater"). F6 flips between them, F7 splits the view
    #[arg(long, value_name = "A,B")]
    compare: Option<String>,

    /// With --compare: start split, A on the left and B on the right
    #[arg(long, requires = "compare")]
    split: bool,

    /// Exit the player once game time reaches this many seconds
    #[arg(long, value_name = "SECS")]
    exit_after: Option<f64>,

    /// Advance the game exactly this many seconds per frame, whatever the
    /// wall time. With `--screenshot-every` equal to it, every frame is
    /// written: smooth offline video however slow the capture runs.
    #[arg(long, value_name = "SECS")]
    frame_step: Option<f64>,

    /// Seed scripts' `random()` and `random_range()`: one sequence, the same
    /// every run (#635). `--frame-step` seeds them too, with the seed
    /// `flint render --scripts` uses, unless this names another; without
    /// either they draw from the wall clock
    #[arg(long, value_name = "N")]
    seed: Option<u64>,

    /// Play every scene under a look (#279, #435): a built-in
    /// (ascii_noir, matrix, ink_wash, graphite, watercolor), a
    /// `styles/<name>.look.toml` beside the scene, or a path. Merged over the
    /// scene's `[post_process]` as `flint render --look` does
    #[arg(long, value_name = "NAME|PATH")]
    look: Option<String>,

    /// Draw point-sized things (ASCII cells, egui panels, the HUD) at this
    /// many pixels a point instead of the window's scale factor, so a
    /// capture is the same on any display (#435). 2 at 3840x2160 lays a
    /// 4K frame out as 1920x1080 points
    #[arg(long, value_name = "SCALE")]
    pixels_per_point: Option<f32>,

    /// Play timed key, gamepad and mouse input from a file (see
    /// `player_app/input_script.rs` for the format)
    #[arg(long, value_name = "FILE")]
    input_script: Option<String>,

    /// Play expression states from a JSON timeline on the game clock:
    /// `{"events": [{..., "t": secs}, ...]}`. Scripts read the latest with
    /// `expression()` and hear each in `on_expression(state)` (Flint#518)
    #[arg(long, value_name = "FILE")]
    expression_timeline: Option<String>,

    /// Take expression states as newline-delimited JSON objects over TCP at
    /// this loopback address (e.g. 127.0.0.1:7777). Off unless given; only
    /// 127.0.0.1 or ::1 can listen (Flint#518)
    #[arg(long, value_name = "IP:PORT")]
    expression_listen: Option<String>,

    /// Open the rendering stats overlay (F2) from the first frame, so a
    /// capture shows it
    #[arg(long)]
    stats: bool,

    /// Print a perf line every 10 s: frame p50/p99, each phase's CPU time
    /// and each pass's GPU time, as the web page's `?perf=1` copies it
    #[arg(long)]
    perf: bool,

    /// Don't watch the game's files for changes (#633): scripts, UI
    /// documents, user styles, glyph sets and palettes load once and never
    /// reload, and nothing polls the disk. For captures and shipped builds
    #[arg(long)]
    no_hot_reload: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let args = Args::parse();
    if args.list_gpus {
        let adapters = flint_render::list_adapters();
        if adapters.is_empty() {
            eprintln!("No GPU adapters found: {}", flint_render::no_adapter_hint());
        }
        for a in adapters {
            println!("{a}");
        }
        return Ok(());
    }
    flint_render::set_gpu_override(args.gpu.clone());
    if args.no_hot_reload {
        flint_core::file_watch::disable();
    }

    let mut args = args;
    let late_bundles = match args.bundle.clone() {
        Some(path) => Some(open_bundle(&path, &mut args)?),
        None => None,
    };

    // The one resolver (ADR 0114): --schemas, else the manifest, else
    // discovery. A bundle settled its list when it was packed
    // (`args.schemas` holds it by now), plus what lies beside the scene.
    let scene = std::path::Path::new(&args.scene);
    let (registry, schema_dirs) = if args.bundle.is_some() {
        let dirs = flint_core::project::host_schema_dirs(&args.schemas, Some(scene));
        let registry =
            SchemaRegistry::load_from_directories(&dirs.dirs).context("Failed to load schemas")?;
        (registry, dirs)
    } else {
        match SchemaRegistry::resolve(Some(scene), &args.schemas) {
            Ok(loaded) => loaded,
            // Says which directory and who named it: nothing to add
            Err(flint_core::FlintError::SchemaDirs(e)) => return Err(e.into()),
            Err(e) => return Err(e).context("Failed to load schemas"),
        }
    };
    // TEMPORARY, for one release (ADR 0114 decision 3, #765)
    for warning in schema_dirs.dropped_warnings() {
        eprintln!("{warning}");
    }
    if schema_dirs.dirs.is_empty() {
        println!("Warning: No schemas directories found");
    }

    // --style words, checked once the scene's own styles are registered
    // (a user style is known only then, #274) so a typo stops here
    if !args.styles.is_empty() {
        flint_player::load_user_styles(&args.scene);
        flint_render::stylize::check_style_specs(&args.styles).map_err(anyhow::Error::msg)?;
    }

    // Load scene
    let (world, scene_file) = load_scene(&args.scene, &registry).context("Failed to load scene")?;

    println!("Loaded scene: {}", scene_file.scene.name);
    println!("Entities: {}", world.entity_count());
    println!();
    println!("Controls:");
    println!("  WASD     - Move");
    println!("  Mouse    - Look");
    println!("  Space    - Jump");
    println!("  Shift    - Sprint");
    println!("  Escape   - Release cursor / Exit (the game's, if it binds Escape)");
    println!("  F2       - Rendering stats overlay");
    println!("  F3       - Scene debug panels");
    println!("  F4       - Rendering & Effects menu");
    println!("  F11      - Toggle fullscreen");

    // Create and run the event loop
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    if let Some((source, late)) = late_bundles {
        if !late.is_empty() {
            add_late_bundles_soon(source, late);
        }
    }

    let mut app = PlayerApp::new(
        world,
        args.scene,
        args.fullscreen,
        args.input_config,
        scene_file.scene.input_config.clone(),
    );

    app.msaa_sample_count = args.msaa;
    // A bundle's files aren't on the disk: nothing to key a disk cache by.
    app.procgen_disk_cache = args.bundle.is_none();
    app.scene_file_writable = args.bundle.is_none();
    if args.procgen_defer {
        app.procgen_mode = flint_player::ProcGenMode::Deferred;
    }
    if let Some(value) = &args.render_scale {
        app.render_scale = flint_player::RenderScaleSetting::default()
            .parse(value)
            .ok_or_else(|| anyhow::anyhow!("--render-scale {value}: a number or auto"))?;
    }
    if let Some(quality) = &args.quality {
        // Refuse a typo here rather than run with a setting silently dropped.
        let errors = flint_render::QualitySettings::default().apply_str(quality);
        if let Some(e) = errors.first() {
            anyhow::bail!("--quality: {e}");
        }
        app.quality_overrides = quality.clone();
    }
    let touch_mode = flint_runtime::TouchControlsMode::parse(&args.touch_controls)
        .ok_or_else(|| anyhow::anyhow!("--touch-controls: auto, always or never"))?;
    app.input.touch_controls_mut().set_mode(touch_mode);
    if let Some(spec) = &args.safe_area {
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
        app.safe_area = Some([l, t, r, b]);
    }
    app.touch_device = args.touch_device;
    app.pixels_per_point = args.pixels_per_point.filter(|p| *p > 0.0);
    if let Some(name) = &args.look {
        // Refuse a bad name now. The scene's user styles first: a look may
        // name one (they are loaded again, harmlessly, with the scene)
        flint_player::load_user_styles(&app.scene_path);
        let look = flint_render::look::find(name, &app.scene_path)
            .map_err(|e| anyhow::anyhow!("--look: {e}"))?;
        println!("Look: {} ({})", look.name, look.source);
        app.look = Some(name.clone());
    }
    app.style_specs = args.styles.clone();
    app.style_time = args.style_time;
    // Skybox, ambient, shading levers, camera and post from the scene file
    app.apply_scene_settings(&scene_file);
    app.window_size = args.window.size();
    app.window_options = args.window.clone();
    app.capture = flint_player::CaptureConfig::new(
        args.screenshot.map(std::path::PathBuf::from),
        args.screenshot_at,
        args.screenshot_every,
        args.exit_after,
    );
    if let Some(path) = &args.input_script {
        app.capture.input_script = Some(flint_player::InputScript::load(path)?);
    }
    app.expression_feed = flint_player::ExpressionFeed::open(
        args.expression_timeline.as_deref(),
        args.expression_listen.as_deref(),
    )?;
    app.capture.show_stats = args.stats;
    if let Some(compare) = &args.compare {
        let (a, b) = compare
            .split_once(',')
            .ok_or_else(|| anyhow::anyhow!("--compare {compare}: two presets, as A,B"))?;
        if b.trim().is_empty() {
            anyhow::bail!("--compare {compare}: B needs a preset's name");
        }
        app.compare_living_at_start(a.trim(), b.trim(), args.split);
    }
    if args.perf {
        app.enable_perf(true);
    }
    app.clock.frame_step = args.frame_step.filter(|s| *s > 0.0);
    // A stepped run is meant to repeat, so its scripts' randomness does too
    if let Some(seed) = args
        .seed
        .or(app.clock.frame_step.map(|_| flint_player::SCRIPT_SEED))
    {
        app.script.seed_random(seed);
    }

    // Apply initial mixer bus volumes from CLI (e.g. --music-volume 0)
    app.audio
        .set_bus_volume(flint_audio::Bus::Music, args.music_volume);
    app.audio
        .set_bus_volume(flint_audio::Bus::Sfx, args.sfx_volume);

    // Preserve schema paths for scene transitions (includes auto-discovered)
    app.set_schema_dirs(schema_dirs);

    event_loop.run_app(&mut app)?;
    // A failed start or a screenshot that never reached the disk exits
    // non-zero, never as a clean run (#639).
    if let Some(failure) = app.run_failure() {
        anyhow::bail!(failure);
    }

    Ok(())
}

/// Serve every read from a bundle, its start scene and schemas filling in
/// what the command line left out. Returns its files and its late bundles'
/// paths.
fn open_bundle(
    path: &std::path::Path,
    args: &mut Args,
) -> Result<(vfs::MemorySource, Vec<std::path::PathBuf>)> {
    #[allow(clippy::disallowed_methods)] // the bundle itself is on the disk
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let bundle = Bundle::parse(bytes).with_context(|| path.display().to_string())?;
    let manifest = &bundle.manifest;
    println!(
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

    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    let late = manifest.late.iter().map(|l| dir.join(&l.url)).collect();
    let source = bundle.to_memory_source();
    vfs::install(Arc::new(source.clone()));
    Ok((source, late))
}

/// Add the late bundles a few seconds in, the way the web player adds them
/// once it is running, so the desktop takes the same path: the vfs grows and
/// the player loads the audio that just arrived.
fn add_late_bundles_soon(mut source: vfs::MemorySource, paths: Vec<std::path::PathBuf>) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        for path in paths {
            #[allow(clippy::disallowed_methods)] // the bundle itself is on the disk
            let bytes = std::fs::read(&path).map_err(|e| e.to_string());
            match bytes.and_then(|b| Bundle::parse(b).map_err(|e| e.to_string())) {
                Ok(bundle) => {
                    println!(
                        "Late bundle {}: {} files, {:.2} MB",
                        bundle.manifest.part,
                        bundle.manifest.files.len(),
                        bundle.len() as f64 / 1e6
                    );
                    bundle.insert_into(&mut source);
                }
                Err(e) => eprintln!("Late bundle {}: {e}", path.display()),
            }
        }
        vfs::install(Arc::new(source));
    });
}
