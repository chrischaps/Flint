//! Flint Player - Standalone game player binary
//!
//! Launches a first-person game session from a scene file with physics.
//!
//! Usage:
//!   flint-player <scene.toml> [--schemas <path>] [--fullscreen]
//!                [--width <px>] [--height <px>]
//!                [--screenshot <out.png>] [--screenshot-at <secs>]
//!                [--screenshot-every <secs>] [--exit-after <secs>]
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
use std::path::Path;
use winit::event_loop::{ControlFlow, EventLoop};

#[derive(Parser)]
#[command(name = "flint-player")]
#[command(about = "Flint game player - run scenes with physics and first-person controls")]
struct Args {
    /// Path to scene file
    #[arg(required_unless_present = "list_gpus", default_value = "")]
    scene: String,

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
        .filter(|p| Path::new(p).exists())
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

    let mut app = PlayerApp::new(
        world,
        args.scene,
        args.fullscreen,
        args.input_config,
        scene_file.scene.input_config.clone(),
    );

    app.msaa_sample_count = args.msaa;
    app.scene_preload_audio = scene_file.scene.preload_audio;
    app.window_size = (args.width, args.height);
    app.capture = flint_player::CaptureConfig::new(
        args.screenshot.map(std::path::PathBuf::from),
        args.screenshot_at,
        args.screenshot_every,
        args.exit_after,
    );

    // Apply initial mixer bus volumes from CLI (e.g. --music-volume 0)
    app.audio
        .set_bus_volume(flint_audio::Bus::Music, args.music_volume);
    app.audio
        .set_bus_volume(flint_audio::Bus::Sfx, args.sfx_volume);

    // Pass skybox path + ambient from scene environment settings
    if let Some(env) = &scene_file.environment {
        app.skybox_path = env.skybox.clone();
        if env.ambient_sky.is_some() || env.ambient_ground.is_some() {
            app.scene_ambient = Some((
                env.ambient_sky
                    .unwrap_or(flint_render::LightUniforms::DEFAULT_AMBIENT_SKY),
                env.ambient_ground
                    .unwrap_or(flint_render::LightUniforms::DEFAULT_AMBIENT_GROUND),
            ));
        }
        app.scene_diffuse_wrap = env.diffuse_wrap;
        app.scene_oren_nayar = env.oren_nayar;
        app.scene_sheen = env
            .sheen_strength
            .map(|s| (env.sheen_color.unwrap_or([1.0; 3]), s));
    }

    // Pass camera settings from scene
    app.scene_camera = scene_file.camera.clone();

    // Pass post-processing settings from scene
    app.scene_post_process = scene_file.post_process.clone();

    // Preserve schema paths for scene transitions (includes auto-discovered)
    app.set_schema_paths(all_schemas);

    event_loop.run_app(&mut app)?;

    Ok(())
}
