//! Flint Player - Standalone game player binary
//!
//! Launches a first-person game session from a scene file with physics.
//!
//! Usage:
//!   flint-player <scene.toml> [--schemas <path>] [--fullscreen]

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
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Load schemas from all directories
    let existing: Vec<&str> = args.schemas.iter().map(|s| s.as_str()).filter(|p| Path::new(p).exists()).collect();
    let registry = if !existing.is_empty() {
        SchemaRegistry::load_from_directories(&existing).context("Failed to load schemas")?
    } else {
        println!("Warning: No schemas directories found");
        SchemaRegistry::new()
    };

    // Load scene
    let (world, scene_file) =
        load_scene(&args.scene, &registry).context("Failed to load scene")?;

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
    println!("  F5       - Toggle bloom");
    println!("  F6       - Toggle post-processing");
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

    // Pass skybox path from scene environment settings
    if let Some(env) = &scene_file.environment {
        app.skybox_path = env.skybox.clone();
    }

    // Pass post-processing settings from scene
    app.scene_post_process = scene_file.post_process.clone();

    // Preserve schema paths for scene transitions
    app.set_schema_paths(args.schemas);

    event_loop.run_app(&mut app)?;

    Ok(())
}
