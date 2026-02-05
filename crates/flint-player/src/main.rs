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

    /// Path to schemas directory
    #[arg(long, default_value = "schemas")]
    schemas: String,

    /// Launch in fullscreen mode
    #[arg(long)]
    fullscreen: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Load schemas
    let registry = if Path::new(&args.schemas).exists() {
        SchemaRegistry::load_from_directory(&args.schemas).context("Failed to load schemas")?
    } else {
        println!("Warning: Schemas directory not found: {}", args.schemas);
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
    println!("  F11      - Toggle fullscreen");

    // Create and run the event loop
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = PlayerApp::new(world, args.scene, args.fullscreen);
    event_loop.run_app(&mut app)?;

    Ok(())
}
