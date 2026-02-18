//! Flint CLI - Command-line interface for the Flint engine

mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use commands::{asset, entity, init, play, query, render, scene, schema, validate};

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

        /// Paths to schemas directories (can specify multiple)
        #[arg(long, default_value = "schemas", action = clap::ArgAction::Append)]
        schemas: Vec<String>,
    },

    /// Start the scene viewer with hot-reload
    Serve {
        /// Path to scene file
        scene: String,

        /// Watch for file changes
        #[arg(long)]
        watch: bool,

        /// Paths to schemas directories (can specify multiple)
        #[arg(long, default_value = "schemas", action = clap::ArgAction::Append)]
        schemas: Vec<String>,

        /// Hide the egui inspector panels (entity tree, inspector, stats)
        #[arg(long)]
        no_inspector: bool,
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

        /// Paths to schemas directories (can specify multiple)
        #[arg(long, default_value = "schemas", action = clap::ArgAction::Append)]
        schemas: Vec<String>,

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

        /// Paths to schemas directories (can specify multiple)
        #[arg(long, default_value = "schemas", action = clap::ArgAction::Append)]
        schemas: Vec<String>,

        /// Launch in fullscreen mode
        #[arg(long)]
        fullscreen: bool,

        /// Optional input config overlay path
        #[arg(long)]
        input_config: Option<String>,
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

        /// Paths to schemas directories (can specify multiple)
        #[arg(long, default_value = "schemas", action = clap::ArgAction::Append)]
        schemas: Vec<String>,

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

        /// Disable shadow mapping
        #[arg(long)]
        no_shadows: bool,

        /// Shadow map resolution per cascade (default: 1024)
        #[arg(long, default_value = "1024")]
        shadow_resolution: u32,

        /// Disable post-processing (bloom, vignette, tonemapping in composite pass)
        #[arg(long)]
        no_postprocess: bool,

        /// Bloom intensity (enables bloom; default: 0.04)
        #[arg(long)]
        bloom_intensity: Option<f32>,

        /// Bloom brightness threshold (default: 1.0)
        #[arg(long)]
        bloom_threshold: Option<f32>,

        /// Exposure multiplier (default: 1.0)
        #[arg(long)]
        exposure: Option<f32>,

        /// SSAO sample radius (default: 0.5)
        #[arg(long)]
        ssao_radius: Option<f32>,

        /// SSAO intensity multiplier (default: 1.0, 0 = disabled)
        #[arg(long)]
        ssao_intensity: Option<f32>,

        /// Fog density (enables fog; default: 0.02, 0 = disabled)
        #[arg(long)]
        fog_density: Option<f32>,

        /// Fog color as comma-separated R,G,B (default: 0.7,0.75,0.82)
        #[arg(long, value_parser = parse_vec3)]
        fog_color: Option<[f32; 3]>,

        /// Fog height falloff (enables height fog; default: 0.1)
        #[arg(long)]
        fog_height_falloff: Option<f32>,
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
        Commands::Schema { name, schemas } => schema::run(&name, &schemas.iter().map(|s| s.as_str()).collect::<Vec<_>>()),
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
            input_config,
        } => play::run(play::PlayArgs {
            scene,
            schemas,
            fullscreen,
            input_config,
        }),
        Commands::Asset(cmd) => asset::run(cmd),
        Commands::Serve { scene, watch, schemas, no_inspector } => {
            // Serve uses first schemas path (viewer doesn't need multi-dir yet)
            let schemas_path = schemas.first().map(|s| s.as_str()).unwrap_or("schemas");
            flint_viewer::app::run(&scene, watch, schemas_path, !no_inspector)
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
            no_shadows,
            shadow_resolution,
            no_postprocess,
            bloom_intensity,
            bloom_threshold,
            exposure,
            ssao_radius,
            ssao_intensity,
            fog_density,
            fog_color,
            fog_height_falloff,
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
            no_shadows,
            shadow_resolution,
            no_postprocess,
            bloom_intensity,
            bloom_threshold,
            exposure,
            ssao_radius,
            ssao_intensity,
            fog_density,
            fog_color,
            fog_height_falloff,
        }),
    }
}
