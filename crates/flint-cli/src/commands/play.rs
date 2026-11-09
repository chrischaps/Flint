//! Play command — launches the game player for a scene

use anyhow::{Context, Result};
use flint_scene::load_scene;
use flint_schema::SchemaRegistry;
use std::path::Path;
use winit::event_loop::{ControlFlow, EventLoop};

#[derive(clap::Args)]
pub struct PlayArgs {
    /// Path to scene file
    #[arg(required_unless_present = "list_gpus", default_value = "")]
    pub scene: String,

    /// Paths to schemas directories (can specify multiple)
    #[arg(long, default_value = "schemas", action = clap::ArgAction::Append)]
    pub schemas: Vec<String>,

    /// Launch in fullscreen mode
    #[arg(long)]
    pub fullscreen: bool,

    /// `--width`/`--height`, `--position`, `--borderless`, `--always-on-top`
    /// and `--no-activate`: the window's size and place, its frame, whether
    /// it stays on top and whether it takes the keyboard (#611)
    #[command(flatten)]
    pub window: flint_player::WindowArgs,

    /// Optional input config overlay path
    #[arg(long)]
    pub input_config: Option<String>,

    /// Initial music bus volume (linear, 0.0 = muted, 1.0 = full)
    #[arg(long, default_value_t = 1.0)]
    pub music_volume: f64,

    /// Initial SFX bus volume (linear, 0.0 = muted, 1.0 = full)
    #[arg(long, default_value_t = 1.0)]
    pub sfx_volume: f64,

    /// GPU adapter to use (case-insensitive substring of the adapter name,
    /// e.g. "NVIDIA"). Overrides WGPU_ADAPTER_NAME; default prefers the
    /// high-performance adapter. Use --list-gpus to see names.
    #[arg(long)]
    pub gpu: Option<String>,

    /// List available GPU adapters and exit
    #[arg(long)]
    pub list_gpus: bool,

    /// `--style name[:key=value,...]` over every scene's post-processing,
    /// kept through transitions, and `--style-time` (#328)
    #[command(flatten)]
    pub style: crate::commands::common_args::StyleArgs,
    /// Play expression states from a JSON timeline on the game clock:
    /// `{"events": [{..., "t": secs}, ...]}`. Scripts read the latest with
    /// `expression()` and hear each in `on_expression(state)` (Flint#518)
    #[arg(long, value_name = "FILE")]
    pub expression_timeline: Option<String>,

    /// Take expression states as newline-delimited JSON objects over TCP at
    /// this loopback address (e.g. 127.0.0.1:7777). Off unless given; only
    /// 127.0.0.1 or ::1 can listen (Flint#518)
    #[arg(long, value_name = "IP:PORT")]
    pub expression_listen: Option<String>,
}

pub fn run(args: PlayArgs) -> Result<()> {
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

    // A --style word may name one of the scene's own styles (#274): check
    // the words once those are registered
    if !args.style.styles.is_empty() {
        flint_player::load_user_styles(&args.scene);
        args.style.check().map_err(anyhow::Error::msg)?;
    }
    // Expression sources first, so a bad timeline or a taken port fails
    // before a window opens
    let expression_feed = flint_player::ExpressionFeed::open(
        args.expression_timeline.as_deref(),
        args.expression_listen.as_deref(),
    )?;

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

    let mut app = flint_player::PlayerApp::new(
        world,
        args.scene,
        args.fullscreen,
        args.input_config,
        scene_file.scene.input_config.clone(),
    );

    app.scene_preload_audio = scene_file.scene.preload_audio;
    app.window_size = args.window.size();
    app.window_options = args.window.clone();
    app.expression_feed = expression_feed;

    // Apply initial mixer bus volumes from CLI
    app.audio
        .set_bus_volume(flint_player::Bus::Music, args.music_volume);
    app.audio
        .set_bus_volume(flint_player::Bus::Sfx, args.sfx_volume);

    // Pass skybox path from scene environment settings
    if let Some(env) = &scene_file.environment {
        app.skybox_path = env.skybox.clone();
    }

    // Pass camera settings from scene
    app.scene_camera = scene_file.camera.clone();

    // Pass post-processing settings from scene
    // with the look it names laid under its keys (#356)
    app.set_scene_post_process(scene_file.post_process.clone());
    app.style_specs = args.style.styles.clone();
    app.style_time = args.style.style_time;

    // The scene's living props: what their shadows follow, and how woozy it is
    app.scene_living = scene_file.living.clone();
    app.scene_wooze = scene_file.wooze.clone();

    // Preserve schema paths for scene transitions (includes auto-discovered)
    app.set_schema_paths(all_schemas);

    event_loop.run_app(&mut app)?;

    Ok(())
}
