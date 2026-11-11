//! Flint CLI - Command-line interface for the Flint engine

mod commands;
mod file_dialog;

use anyhow::Result;
use clap::{Parser, Subcommand};
use commands::{
    asset, bundle, calibrate, edit_router, entity, export, gen, gen_preview, init, play,
    play_chart, play_suite, prefab, preview, query, render, render_suite, replay_chart, scene,
    schema, spike_rumble, spline_edit, style, terrain_edit, tex_edit, validate, validate_suite,
};

#[derive(Parser)]
#[command(name = "flint")]
#[command(about = "CLI-first game engine for AI-assisted development", long_about = None)]
#[command(version)]
struct Cli {
    /// Project root: the folder holding flint.toml, the content store
    /// (.flint/) and the asset catalog (assets/). Found by walking up from
    /// the working directory if omitted
    #[arg(long, global = true, value_name = "DIR")]
    project: Option<std::path::PathBuf>,

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
    Edit(edit_router::EditArgs),

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
    SplineEdit(spline_edit::EditArgs),

    /// Validate a scene against constraints
    Validate(validate::ValidateArgs),

    /// Pack a game folder into .flintpack bundles for the web player
    Bundle(bundle::BundleArgs),

    /// Export a game for elsewhere: `flint export web <game>` writes a
    /// folder any static server plays it from
    #[command(subcommand)]
    Export(export::ExportCommands),

    /// Validate a musical suite manifest (and optional beatmap chart)
    ValidateSuite(validate_suite::ValidateSuiteArgs),

    /// Play a validated suite manifest's stems sample-locked (Milestone 0)
    PlaySuite(play_suite::PlaySuiteArgs),

    /// Tap-to-beat calibration: write the player's median offset to logs/latency/
    Calibrate(calibrate::CalibrateArgs),

    /// Play a suite against its beatmap chart with live gamepad capture (Phase 2 dev harness)
    PlayChart(play_chart::PlayChartArgs),

    /// Rumble spike (ADR 0025): fire the ff motors, time the command paths,
    /// log to logs/latency/
    SpikeRumble(spike_rumble::SpikeRumbleArgs),

    /// Replay a recorded or synthetic session through judgment, fully headless
    ReplayChart(replay_chart::ReplayChartArgs),

    /// Render a suite offline and deterministically: a music `.suite.toml`
    /// to WAV, or a `.goldens.toml` of scenes gated against golden PNGs
    RenderSuite(render_suite::RenderSuiteArgs),

    /// Prefab operations
    #[command(subcommand)]
    Prefab(prefab::PrefabCommands),

    /// Asset management operations
    #[command(subcommand)]
    Asset(asset::AssetCommands),

    /// Play a scene with first-person controls and physics
    Play(play::PlayArgs),

    /// Preview a 3D model file (GLB/glTF) with orbit camera
    #[command(hide = true)]
    Preview(preview::PreviewArgs),

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
    Render(render::RenderArgs),

    /// User-authored styles: new (from a template), check (no GPU),
    /// prelude (the generated code), list
    #[command(subcommand)]
    Style(style::StyleCommands),
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,rfd=off")),
        )
        // Stdout belongs to what a command answers with, such as --report json.
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    if let Some(dir) = &cli.project {
        if !dir.is_dir() {
            anyhow::bail!("--project {}: not a directory", dir.display());
        }
        let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.clone());
        // Reads the directory's flint.toml: a bad manifest stops here.
        let project =
            flint_core::project::Project::at(dir).map_err(|e| anyhow::anyhow!("--project {e}"))?;
        flint_core::project::set_override(project);
    }

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
        Commands::Validate(args) => validate::run(args),
        Commands::Bundle(args) => bundle::run(args),
        Commands::Export(cmd) => export::run(cmd),
        Commands::ValidateSuite(args) => validate_suite::run(args),
        Commands::PlaySuite(args) => play_suite::run(args),
        Commands::Calibrate(args) => calibrate::run(args),
        Commands::PlayChart(args) => play_chart::run(args),
        Commands::SpikeRumble(args) => spike_rumble::run(args),
        Commands::ReplayChart(args) => replay_chart::run(args),
        Commands::RenderSuite(args) => render_suite::run(args),
        Commands::Play(args) => play::run(args),
        Commands::Prefab(cmd) => prefab::run(cmd),
        Commands::Asset(cmd) => asset::run(cmd),
        Commands::Edit(args) => edit_router::run(args),
        Commands::SplineEdit(args) => spline_edit::run(args),
        Commands::Preview(args) => preview::run(args),
        Commands::Serve {
            scene,
            watch,
            schemas,
            no_inspector,
        } => {
            // Serve uses first schemas path (viewer doesn't need multi-dir yet)
            let schemas_path = schemas.first().map(|s| s.as_str()).unwrap_or("schemas");
            flint_viewer::app::run(
                &scene,
                watch,
                schemas_path,
                !no_inspector,
                false,
                Default::default(),
            )
        }
        Commands::Gen(args) => gen::run(args),
        Commands::GenPreview(args) => gen_preview::run(args),
        Commands::TerrainEdit(args) => terrain_edit::run(args),
        Commands::TexEdit(args) => tex_edit::run(args),
        Commands::Render(mut args) => {
            // --variant builds each variant's command line on this one (#291)
            args.command_line = commands::variants::subcommand_words(std::env::args_os());
            render::run(args)
        }
        Commands::Style(cmd) => style::run(cmd),
    }
}
