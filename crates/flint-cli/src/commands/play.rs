//! Play command — launches the game player for a scene

use anyhow::{Context, Result};
use flint_scene::load_scene;
use winit::event_loop::{ControlFlow, EventLoop};

#[derive(clap::Args)]
pub struct PlayArgs {
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
    /// Motion on twos for the whole frame (#405): every entity's drawn position
    /// and pose change only this many times a second, as does the post time
    /// styles read (12 = on twos at 24 fps; 0 = off). Over every scene's
    /// [post_process] motion_hold, kept through transitions
    #[arg(long, value_name = "RATE")]
    pub motion_hold: Option<f32>,
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
    pub save_dir: Option<std::path::PathBuf>,
}

pub fn run(mut args: PlayArgs) -> Result<()> {
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
    // No scene named: the manifest's default (ADR 0114), or an error that
    // says what to pass
    if args.scene.is_empty() {
        let project = flint_core::project::Project::current()?;
        let scene = project.default_scene()?;
        args.scene = flint_core::project::shown(&scene);
        println!(
            "Opening the project's default scene: {} (`[project] scene` in {})",
            args.scene,
            flint_core::project::shown(&project.manifest_path())
        );
    }
    // The one resolver (ADR 0114): --schemas, else the manifest, else discovery
    let (registry, schema_dirs) =
        super::common_args::load_schemas(Some(&args.scene), &args.schemas)?;
    super::common_args::warn_dropped_schemas(&schema_dirs);
    if schema_dirs.dirs.is_empty() {
        println!("Warning: No schemas directories found");
    }

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
    let (world, mut scene_file) =
        load_scene(&args.scene, &registry).context("Failed to load scene")?;
    // Living props' tables: checked, and the preset [wooze] names filled in (#686)
    flint_procgen::settle_scene(&mut scene_file, std::path::Path::new(&args.scene))
        .context("Failed to load scene")?;

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

    app.launch.save_dir = args.save_dir.clone();
    app.launch.window_size = args.window.size();
    app.launch.window_options = args.window.clone();
    app.launch.expression_feed = expression_feed;

    // Apply initial mixer bus volumes from CLI
    app.audio
        .set_bus_volume(flint_player::Bus::Music, args.music_volume);
    app.audio
        .set_bus_volume(flint_player::Bus::Sfx, args.sfx_volume);

    app.launch.style_specs = args.style.styles.clone();
    app.launch.style_time = args.style.style_time;
    app.launch.motion_hold = args.motion_hold;

    // Everything the scene file says (skybox, ambient and shading levers,
    // camera, post-processing with its look, living props, wooze and the
    // scene tables extensions read), taken as flint-player and every
    // transition take it (#723, #686)
    app.apply_scene_settings(&scene_file);

    // The registry and its directories, kept for transitions and chunks
    app.set_schemas(registry, schema_dirs);

    event_loop.run_app(&mut app)?;
    // A player that failed to start exits non-zero (#639).
    if let Some(failure) = app.run_failure() {
        anyhow::bail!(failure);
    }

    Ok(())
}
