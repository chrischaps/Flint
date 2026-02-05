//! Flint CLI - Command-line interface for the Flint engine

mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use commands::{asset, entity, init, play, query, render, scene, schema, serve, validate};

#[derive(Parser)]
#[command(name = "flint")]
#[command(about = "CLI-first game engine for AI-assisted development", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize a new Flint project
    Init {
        /// Project name/directory
        name: String,
    },

    /// Entity operations
    #[command(subcommand)]
    Entity(entity::EntityCommands),

    /// Scene operations
    #[command(subcommand)]
    Scene(scene::SceneCommands),

    /// Query entities
    Query {
        /// Query string (e.g., "entities where archetype == 'door'")
        query: String,

        /// Path to scene file
        #[arg(long)]
        scene: Option<String>,

        /// Output format (json or toml)
        #[arg(long, default_value = "json")]
        format: String,
    },

    /// Show schema information
    Schema {
        /// Component or archetype name
        name: String,

        /// Path to schemas directory
        #[arg(long, default_value = "schemas")]
        schemas: String,
    },

    /// Start the scene viewer with hot-reload
    Serve {
        /// Path to scene file
        scene: String,

        /// Watch for file changes
        #[arg(long)]
        watch: bool,

        /// Path to schemas directory
        #[arg(long, default_value = "schemas")]
        schemas: String,

        /// Show egui inspector panels (entity tree, inspector, stats)
        #[arg(long)]
        inspector: bool,
    },

    /// Validate a scene against constraints
    Validate {
        /// Path to scene file
        scene: String,

        /// Apply auto-fixes
        #[arg(long)]
        fix: bool,

        /// Preview fixes without applying
        #[arg(long)]
        dry_run: bool,

        /// Show diff of changes
        #[arg(long)]
        output_diff: bool,

        /// Path to schemas directory
        #[arg(long, default_value = "schemas")]
        schemas: String,

        /// Output format (json or toml)
        #[arg(long, default_value = "text")]
        format: String,
    },

    /// Asset management operations
    #[command(subcommand)]
    Asset(asset::AssetCommands),

    /// Play a scene with first-person controls and physics
    Play {
        /// Path to scene file
        scene: String,

        /// Path to schemas directory
        #[arg(long, default_value = "schemas")]
        schemas: String,

        /// Launch in fullscreen mode
        #[arg(long)]
        fullscreen: bool,
    },

    /// Render a scene to a PNG image (headless)
    Render {
        /// Path to scene file
        scene: String,

        /// Output image path
        #[arg(short, long, default_value = "render.png")]
        output: String,

        /// Image width in pixels
        #[arg(long, default_value = "1920")]
        width: u32,

        /// Image height in pixels
        #[arg(long, default_value = "1080")]
        height: u32,

        /// Path to schemas directory
        #[arg(long, default_value = "schemas")]
        schemas: String,

        /// Camera orbit distance
        #[arg(long)]
        distance: Option<f32>,

        /// Camera horizontal angle in degrees
        #[arg(long)]
        yaw: Option<f32>,

        /// Camera vertical angle in degrees
        #[arg(long)]
        pitch: Option<f32>,

        /// Camera look-at point (comma-separated x,y,z)
        #[arg(long, value_parser = parse_vec3)]
        target: Option<[f32; 3]>,

        /// Field of view in degrees
        #[arg(long)]
        fov: Option<f32>,

        /// Disable ground grid
        #[arg(long)]
        no_grid: bool,

        /// Debug visualization mode
        #[arg(long, value_parser = parse_debug_mode)]
        debug_mode: Option<String>,

        /// Enable wireframe overlay on solid geometry
        #[arg(long)]
        wireframe_overlay: bool,

        /// Show face-normal direction arrows
        #[arg(long)]
        show_normals: bool,

        /// Disable tone mapping for raw linear output
        #[arg(long)]
        no_tonemapping: bool,

        /// Enable shadow mapping
        #[arg(long)]
        shadows: bool,

        /// Shadow map resolution per cascade (default: 1024)
        #[arg(long, default_value = "1024")]
        shadow_resolution: u32,
    },
}

fn parse_debug_mode(s: &str) -> Result<String, String> {
    match s {
        "wireframe" | "normals" | "depth" | "uv" | "unlit" | "metalrough" => Ok(s.to_string()),
        _ => Err(format!(
            "unknown debug mode '{}'; valid values: wireframe, normals, depth, uv, unlit, metalrough",
            s
        )),
    }
}

fn parse_vec3(s: &str) -> Result<[f32; 3], String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 3 {
        return Err(format!("expected 3 comma-separated values, got {}", parts.len()));
    }
    let x: f32 = parts[0].trim().parse().map_err(|e| format!("invalid x: {}", e))?;
    let y: f32 = parts[1].trim().parse().map_err(|e| format!("invalid y: {}", e))?;
    let z: f32 = parts[2].trim().parse().map_err(|e| format!("invalid z: {}", e))?;
    Ok([x, y, z])
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { name } => init::run(&name),
        Commands::Entity(cmd) => entity::run(cmd),
        Commands::Scene(cmd) => scene::run(cmd),
        Commands::Query { query, scene, format } => {
            query::run(&query, scene.as_deref(), &format)
        }
        Commands::Schema { name, schemas } => schema::run(&name, &schemas),
        Commands::Validate {
            scene,
            fix,
            dry_run,
            output_diff,
            schemas,
            format,
        } => validate::run(validate::ValidateArgs {
            scene,
            fix,
            dry_run,
            output_diff,
            schemas,
            format,
        }),
        Commands::Play {
            scene,
            schemas,
            fullscreen,
        } => play::run(play::PlayArgs {
            scene,
            schemas,
            fullscreen,
        }),
        Commands::Asset(cmd) => asset::run(cmd),
        Commands::Serve { scene, watch, schemas, inspector } => {
            if inspector {
                flint_viewer::app::run(&scene, watch, &schemas, true)
            } else {
                serve::run(&scene, watch, &schemas)
            }
        }
        Commands::Render {
            scene,
            output,
            width,
            height,
            schemas,
            distance,
            yaw,
            pitch,
            target,
            fov,
            no_grid,
            debug_mode,
            wireframe_overlay,
            show_normals,
            no_tonemapping,
            shadows,
            shadow_resolution,
        } => render::run(render::RenderArgs {
            scene,
            output,
            width,
            height,
            schemas,
            distance,
            yaw,
            pitch,
            target,
            fov,
            no_grid,
            debug_mode,
            wireframe_overlay,
            show_normals,
            no_tonemapping,
            shadows,
            shadow_resolution,
        }),
    }
}
