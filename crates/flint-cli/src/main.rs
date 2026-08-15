//! Flint CLI - Command-line interface for the Flint engine

mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use commands::{
    asset, edit_router, entity, gen, gen_preview, init, play, play_chart, prefab, preview, query,
    render, play_suite, render_suite, scene, schema, spline_edit, terrain_edit, tex_edit, validate,
    validate_suite,
};

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

    /// Open a file in the appropriate interactive editor
    Edit {
        /// Path to file (.scene.toml, .procgen.toml, .terrain.toml, .glb, .gltf)
        file: String,

        /// Paths to schemas directories (can specify multiple)
        #[arg(long, default_value = "schemas", action = clap::ArgAction::Append)]
        schemas: Vec<String>,

        /// Window width in pixels
        #[arg(long)]
        width: Option<u32>,

        /// Window height in pixels
        #[arg(long)]
        height: Option<u32>,

        /// Disable the ground grid
        #[arg(long)]
        no_grid: bool,

        /// Watch for file changes
        #[arg(long)]
        watch: bool,

        /// Override seed
        #[arg(long)]
        seed: Option<u64>,

        /// Hide the egui inspector panels (scene only)
        #[arg(long)]
        no_inspector: bool,

        /// Open the spline/track editor (scene only)
        #[arg(long)]
        spline: bool,

        /// Start with auto-orbit enabled (model/procgen)
        #[arg(long)]
        auto_orbit: bool,

        /// Camera orbit distance (model)
        #[arg(long)]
        distance: Option<f32>,

        /// Camera horizontal angle in degrees (model)
        #[arg(long)]
        yaw: Option<f32>,

        /// Camera vertical angle in degrees (model)
        #[arg(long)]
        pitch: Option<f32>,

        /// Camera look-at point as comma-separated x,y,z (model)
        #[arg(long, value_parser = parse_vec3)]
        target: Option<[f32; 3]>,

        /// Field of view in degrees (model)
        #[arg(long)]
        fov: Option<f32>,

        /// Disable animation playback (model)
        #[arg(long)]
        no_animate: bool,

        /// Start with a specific animation clip by name (model)
        #[arg(long)]
        clip: Option<String>,

        /// Animation playback speed multiplier (model)
        #[arg(long)]
        anim_speed: Option<f32>,

        /// Render to a PNG file instead of opening a window (model)
        #[arg(long)]
        render: Option<String>,
    },

    /// Start the scene viewer with hot-reload
    #[command(hide = true)]
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

    /// Open the interactive track editor for a scene with spline data
    #[command(hide = true)]
    SplineEdit {
        /// Path to scene file
        scene: String,

        /// Paths to schemas directories (can specify multiple)
        #[arg(long, default_value = "schemas", action = clap::ArgAction::Append)]
        schemas: Vec<String>,
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

    /// Validate a musical suite manifest (and optional beatmap chart)
    ValidateSuite {
        /// Path to the suite manifest (.suite.toml)
        manifest: String,

        /// Beatmap chart (.chart.toml) to cross-check against the manifest
        #[arg(long)]
        chart: Option<String>,

        /// Skip the asset pass (stem file existence/sample-rate/duration)
        #[arg(long)]
        no_assets: bool,

        /// Directory the manifest's file paths are relative to (default: cwd)
        #[arg(long)]
        base_dir: Option<String>,
    },

    /// Play a validated suite manifest's stems sample-locked (Milestone 0)
    PlaySuite {
        /// Path to the suite manifest (.suite.toml)
        manifest: String,

        /// Directory the manifest's file paths are relative to (default: cwd)
        #[arg(long)]
        base_dir: Option<String>,

        /// Stop after this many bars (default: play to the end)
        #[arg(long)]
        bars: Option<u64>,
    },

    /// Play a suite against its beatmap chart with live gamepad capture (Phase 2 dev harness)
    PlayChart {
        /// Path to the suite manifest (.suite.toml)
        manifest: String,

        /// Beatmap chart (.chart.toml) for the suite
        #[arg(long)]
        chart: String,

        /// Directory the manifest's file paths are relative to (default: cwd)
        #[arg(long)]
        base_dir: Option<String>,

        /// Stop after this many bars (default: play to the end)
        #[arg(long)]
        bars: Option<u64>,

        /// Run the input-granularity spike for N seconds and exit (no audio)
        #[arg(long)]
        spike_input_secs: Option<u64>,
    },

    /// Render a scripted suite session to WAV, offline and deterministic
    RenderSuite {
        /// Path to the suite manifest (.suite.toml)
        manifest: String,

        /// Event script (.events.toml) of scheduled bus changes and markers
        #[arg(long)]
        script: Option<String>,

        /// Output WAV path (32-bit float stereo)
        #[arg(long, short)]
        output: String,

        /// Directory the manifest's file paths are relative to (default: cwd)
        #[arg(long)]
        base_dir: Option<String>,

        /// Render this many bars (default: length of the longest stem)
        #[arg(long, conflicts_with = "duration_seconds")]
        duration_bars: Option<i64>,

        /// Render this many seconds
        #[arg(long)]
        duration_seconds: Option<f64>,

        /// Status line cadence: bar (default) or beat
        #[arg(long)]
        status_every: Option<String>,

        /// Processing chunk size in frames (= scheduling granularity)
        #[arg(long, default_value_t = 128)]
        chunk_frames: usize,
    },

    /// Prefab operations
    #[command(subcommand)]
    Prefab(prefab::PrefabCommands),

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

    /// Preview a 3D model file (GLB/glTF) with orbit camera
    #[command(hide = true)]
    Preview {
        /// Path to model file (.glb or .gltf). If omitted, opens an empty window for drag-and-drop.
        model: Option<String>,

        /// Render to a PNG file instead of opening a window
        #[arg(long)]
        render: Option<String>,

        /// Image width in pixels
        #[arg(long, default_value = "1280")]
        width: u32,

        /// Image height in pixels
        #[arg(long, default_value = "720")]
        height: u32,

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

        /// Watch model file for changes and auto-reload
        #[arg(long)]
        watch: bool,

        /// Disable animation playback (animations play by default when present)
        #[arg(long)]
        no_animate: bool,

        /// Start with a specific animation clip by name
        #[arg(long)]
        clip: Option<String>,

        /// Animation playback speed multiplier (default: 1.0)
        #[arg(long, default_value = "1.0")]
        anim_speed: f32,

        /// Sample animation at a specific time in seconds (headless --render mode only)
        #[arg(long)]
        anim_time: Option<f32>,

        /// Start with auto-orbit enabled
        #[arg(long)]
        auto_orbit: bool,
    },

    /// Run a procedural generation spec
    Gen(gen::GenArgs),

    /// Interactive previewer for procedural generation specs
    #[command(hide = true)]
    GenPreview(gen_preview::GenPreviewArgs),

    /// Interactive terrain editor with procgen, sculpting, and painting
    #[command(hide = true)]
    TerrainEdit(terrain_edit::TerrainEditArgs),

    /// Visual node editor for texture pipeline specs
    #[command(hide = true)]
    TexEdit(tex_edit::TexEditArgs),

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

        /// Dither intensity (enables ordered dither; default: 0.03, 0 = disabled)
        #[arg(long)]
        dither_intensity: Option<f32>,

        /// Volumetric light density (enables god rays; default: 1.0)
        #[arg(long)]
        volumetric_density: Option<f32>,

        /// Volumetric ray-march sample count (default: 32)
        #[arg(long)]
        volumetric_samples: Option<u32>,

        /// Kuwahara filter radius (enables Kuwahara; default: 4)
        #[arg(long)]
        kuwahara_radius: Option<u32>,
        /// Kuwahara sector sharpness (default: 8.0)
        #[arg(long)]
        kuwahara_sharpness: Option<f32>,
        /// Kuwahara sector hardness (default: 8.0)
        #[arg(long)]
        kuwahara_hardness: Option<f32>,
        /// Kuwahara anisotropy strength (0=isotropic, 1=full; default: 1.0)
        #[arg(long)]
        kuwahara_anisotropy: Option<f32>,
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
        return Err(format!(
            "expected 3 comma-separated values, got {}",
            parts.len()
        ));
    }
    let x: f32 = parts[0]
        .trim()
        .parse()
        .map_err(|e| format!("invalid x: {}", e))?;
    let y: f32 = parts[1]
        .trim()
        .parse()
        .map_err(|e| format!("invalid y: {}", e))?;
    let z: f32 = parts[2]
        .trim()
        .parse()
        .map_err(|e| format!("invalid z: {}", e))?;
    Ok([x, y, z])
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,rfd=off")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Init { name } => init::run(&name),
        Commands::Entity(cmd) => entity::run(cmd),
        Commands::Scene(cmd) => scene::run(cmd),
        Commands::Query {
            query,
            scene,
            format,
        } => query::run(&query, scene.as_deref(), &format),
        Commands::Schema { name, schemas } => schema::run(
            &name,
            &schemas.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        ),
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
        Commands::ValidateSuite {
            manifest,
            chart,
            no_assets,
            base_dir,
        } => validate_suite::run(validate_suite::ValidateSuiteArgs {
            manifest,
            chart,
            no_assets,
            base_dir,
        }),
        Commands::PlaySuite {
            manifest,
            base_dir,
            bars,
        } => play_suite::run(play_suite::PlaySuiteArgs {
            manifest,
            base_dir,
            bars,
        }),
        Commands::PlayChart {
            manifest,
            chart,
            base_dir,
            bars,
            spike_input_secs,
        } => play_chart::run(play_chart::PlayChartArgs {
            manifest,
            chart,
            base_dir,
            bars,
            spike_input_secs,
        }),
        Commands::RenderSuite {
            manifest,
            script,
            output,
            base_dir,
            duration_bars,
            duration_seconds,
            status_every,
            chunk_frames,
        } => render_suite::run(render_suite::RenderSuiteArgs {
            manifest,
            script,
            output,
            base_dir,
            duration_bars,
            duration_seconds,
            status_every,
            chunk_frames,
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
        Commands::Prefab(cmd) => prefab::run(cmd),
        Commands::Asset(cmd) => asset::run(cmd),
        Commands::Edit {
            file,
            schemas,
            width,
            height,
            no_grid,
            watch,
            seed,
            no_inspector,
            spline,
            auto_orbit,
            distance,
            yaw,
            pitch,
            target,
            fov,
            no_animate,
            clip,
            anim_speed,
            render: render_to,
        } => edit_router::run(edit_router::EditArgs {
            file,
            schemas,
            width,
            height,
            no_grid,
            watch,
            seed,
            no_inspector,
            spline,
            auto_orbit,
            distance,
            yaw,
            pitch,
            target,
            fov,
            no_animate,
            clip,
            anim_speed,
            render: render_to,
        }),
        Commands::SplineEdit { scene, schemas } => {
            spline_edit::run(spline_edit::EditArgs { scene, schemas })
        }
        Commands::Preview {
            model,
            render: render_output,
            width,
            height,
            distance,
            yaw,
            pitch,
            target,
            fov,
            no_grid,
            watch,
            no_animate,
            clip,
            anim_speed,
            anim_time,
            auto_orbit,
        } => preview::run(preview::PreviewArgs {
            model,
            render: render_output,
            width,
            height,
            distance,
            yaw,
            pitch,
            target,
            fov,
            no_grid,
            watch,
            no_animate,
            clip,
            anim_speed,
            anim_time,
            auto_orbit,
        }),
        Commands::Serve {
            scene,
            watch,
            schemas,
            no_inspector,
        } => {
            // Serve uses first schemas path (viewer doesn't need multi-dir yet)
            let schemas_path = schemas.first().map(|s| s.as_str()).unwrap_or("schemas");
            flint_viewer::app::run(&scene, watch, schemas_path, !no_inspector)
        }
        Commands::Gen(args) => gen::run(args),
        Commands::GenPreview(args) => gen_preview::run(args),
        Commands::TerrainEdit(args) => terrain_edit::run(args),
        Commands::TexEdit(args) => tex_edit::run(args),
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
            dither_intensity,
            volumetric_density,
            volumetric_samples,
            kuwahara_radius,
            kuwahara_sharpness,
            kuwahara_hardness,
            kuwahara_anisotropy,
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
            dither_intensity,
            volumetric_density,
            volumetric_samples,
            kuwahara_radius,
            kuwahara_sharpness,
            kuwahara_hardness,
            kuwahara_anisotropy,
        }),
    }
}
