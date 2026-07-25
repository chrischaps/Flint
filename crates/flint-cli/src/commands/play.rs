//! Play command — launches the game player for a scene

use anyhow::{Context, Result};
use flint_scene::load_scene;
use flint_schema::SchemaRegistry;
use std::path::Path;
use winit::event_loop::{ControlFlow, EventLoop};

pub struct PlayArgs {
    pub scene: String,
    pub schemas: Vec<String>,
    pub fullscreen: bool,
    pub input_config: Option<String>,
    pub music_volume: f64,
    pub sfx_volume: f64,
}

pub fn run(args: PlayArgs) -> Result<()> {
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
    println!("  F1       - Cycle debug mode");
    println!("  F4       - Toggle shadows");
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

    // Apply initial mixer bus volumes from CLI
    app.audio.set_bus_volume(flint_player::Bus::Music, args.music_volume);
    app.audio.set_bus_volume(flint_player::Bus::Sfx, args.sfx_volume);

    // Pass skybox path from scene environment settings
    if let Some(env) = &scene_file.environment {
        app.skybox_path = env.skybox.clone();
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
