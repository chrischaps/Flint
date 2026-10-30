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
//!   flint-player --bundle <game.flintpack> [scene.toml]
//!
//! Screenshots capture the presented frame — the 3D scene *and* the egui
//! HUD — right before `present()`, so they show exactly what a player sees.
//! `flint render` remains the headless, script-free snapshot; this path is
//! for verifying script-driven UI and live gameplay state.

use anyhow::{Context, Result};
use clap::Parser;
use flint_player::PlayerApp;
use flint_scene::load_scene;
use flint_schema::SchemaRegistry;
use flint_core::bundle::Bundle;
use flint_core::vfs;
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

    /// Paths to schemas directories (can specify multiple)
    #[arg(long, default_value = "schemas", action = clap::ArgAction::Append)]
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

    /// GPU adapter to use (case-insensitive substring of the adapter name,
    /// e.g. "NVIDIA"). Overrides WGPU_ADAPTER_NAME; default prefers the
    /// high-performance adapter. Use --list-gpus to see names.
    #[arg(long)]
    gpu: Option<String>,

    /// List available GPU adapters and exit
    #[arg(long)]
    list_gpus: bool,

    /// Initial window width in pixels (ignored with --fullscreen)
    #[arg(long, default_value_t = 1280)]
    width: u32,

    /// Initial window height in pixels (ignored with --fullscreen)
    #[arg(long, default_value_t = 720)]
    height: u32,

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

    /// Exit the player once game time reaches this many seconds
    #[arg(long, value_name = "SECS")]
    exit_after: Option<f64>,

    /// Advance the game exactly this many seconds per frame, whatever the
    /// wall time. With `--screenshot-every` equal to it, every frame is
    /// written: smooth offline video however slow the capture runs.
    #[arg(long, value_name = "SECS")]
    frame_step: Option<f64>,

    /// Play timed key, gamepad and mouse input from a file (see
    /// `player_app/input_script.rs` for the format)
    #[arg(long, value_name = "FILE")]
    input_script: Option<String>,

    /// Open the rendering stats overlay (F2) from the first frame, so a
    /// capture shows it
    #[arg(long)]
    stats: bool,
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
        for a in flint_render::list_adapters() {
            println!("{a}");
        }
        return Ok(());
    }
    flint_render::set_gpu_override(args.gpu.clone());

    let mut args = args;
    let late_bundles = match args.bundle.clone() {
        Some(path) => Some(open_bundle(&path, &mut args)?),
        None => None,
    };

    // Merge explicit schemas with auto-discovered dirs from scene path
    let mut all_schemas = args.schemas.clone();
    for dir in flint_schema::discover_schema_dirs(&args.scene) {
        let s = dir.to_string_lossy().into_owned();
        if !all_schemas.contains(&s) {
            all_schemas.push(s);
        }
    }

    // Load schemas from all directories
    let existing: Vec<&str> = all_schemas
        .iter()
        .map(|s| s.as_str())
        .filter(|p| vfs::exists(p))
        .collect();
    let registry = if !existing.is_empty() {
        SchemaRegistry::load_from_directories(&existing).context("Failed to load schemas")?
    } else {
        println!("Warning: No schemas directories found");
        SchemaRegistry::new()
    };

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
    println!("  Escape   - Release cursor / Exit");
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
    // Skybox, ambient, shading levers, camera and post from the scene file
    app.apply_scene_settings(&scene_file);
    app.window_size = (args.width, args.height);
    app.capture = flint_player::CaptureConfig::new(
        args.screenshot.map(std::path::PathBuf::from),
        args.screenshot_at,
        args.screenshot_every,
        args.exit_after,
    );
    if let Some(path) = &args.input_script {
        app.capture.input_script = Some(flint_player::InputScript::load(path)?);
    }
    app.capture.show_stats = args.stats;
    app.clock.frame_step = args.frame_step.filter(|s| *s > 0.0);

    // Apply initial mixer bus volumes from CLI (e.g. --music-volume 0)
    app.audio
        .set_bus_volume(flint_audio::Bus::Music, args.music_volume);
    app.audio
        .set_bus_volume(flint_audio::Bus::Sfx, args.sfx_volume);

    // Preserve schema paths for scene transitions (includes auto-discovered)
    app.set_schema_paths(all_schemas);

    event_loop.run_app(&mut app)?;

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
