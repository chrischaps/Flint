//! Flint CLI - Command-line interface for the Flint engine

mod check;
mod commands;
mod file_dialog;

use anyhow::Result;
use clap::{Parser, Subcommand};
use commands::{
    asset, bundle, check as check_cmd, dxc, edit_router, entity, examples, export, gen,
    gen_preview, init, legacy_names, music, play, prefab, preview, query, render, render_goldens,
    scene, schema, script, spike_rumble, spline_edit, style, terrain_edit, tex_edit, validate,
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
    /// Start a game project: a manifest, a lit scene, a HUD script, an
    /// input config and an agent guide, ready to render and play
    Init(init::InitArgs),

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

        /// Schema directories, later ones overriding earlier (repeatable).
        /// Without it: the project's flint.toml `schemas`, else schemas/ in
        /// the working directory, beside the scene and one level up
        #[arg(long, action = clap::ArgAction::Append)]
        schemas: Vec<String>,
    },

    /// Show schema information: one component or archetype, --list for
    /// all of them, --format json / json-schema for the schemas as data
    Schema(schema::SchemaArgs),

    /// The script API: `flint script api [--json] [--filter x]` lists every
    /// function a script can call, and the callbacks
    #[command(subcommand)]
    Script(script::ScriptCommands),

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

        /// Schema directories, later ones overriding earlier (repeatable).
        /// Without it: the project's flint.toml `schemas`, else schemas/ in the
        /// working directory, beside the scene and one level up
        #[arg(long, action = clap::ArgAction::Append)]
        schemas: Vec<String>,

        /// Hide the egui inspector panels (entity tree, inspector, stats)
        #[arg(long)]
        no_inspector: bool,
    },

    /// Open the interactive track editor for a scene with spline data
    #[command(hide = true)]
    SplineEdit(spline_edit::EditArgs),

    /// Validate a scene against its schemas and constraints (and fix what
    /// a constraint can fix)
    Validate(Box<validate::ValidateArgs>),

    /// The pre-flight: everything wrong with a project or a scene in one
    /// run (schemas, entity references, assets, files, styles, constraints)
    Check(Box<check_cmd::CheckArgs>),

    /// The curated example scenes, from examples/index.toml: list them by
    /// topic, or --check that every one is indexed and validates
    Examples(examples::ExamplesArgs),

    /// Pack a game folder into .flintpack bundles for the web player
    Bundle(bundle::BundleArgs),

    /// Export a game for elsewhere: `flint export web <game>` writes a
    /// folder any static server plays it from
    #[command(subcommand)]
    Export(export::ExportCommands),

    /// Music suites and rhythm charts: validate, play, render to WAV,
    /// calibrate, chart play / chart replay
    #[command(subcommand)]
    Music(music::MusicCommands),

    /// The rumble spike (ADR 0025), an example since #709:
    /// `cargo run -p flint-input-capture --example spike_rumble`
    #[command(hide = true)]
    SpikeRumble(spike_rumble::SpikeRumbleArgs),

    /// Render a `.goldens.toml` of scenes and gate each against its golden
    /// PNG (`--bless` rewrites them)
    Goldens(render_goldens::GoldensArgs),

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

    /// DX12's shader compiler, linked in: what DX12 compiles with here, and
    /// DXC's notices for a Windows build that ships
    #[command(subcommand)]
    Dxc(dxc::DxcCommands),

    /// User-authored styles: new (from a template), check (no GPU),
    /// prelude (the generated code), list
    #[command(subcommand)]
    Style(style::StyleCommands),
}

fn main() -> Result<()> {
    // The old names (`play-chart` is `music chart play`, #709;
    // `render-suite <x>.goldens.toml` is `goldens`, #327) still run
    let (args, moved) = legacy_names::route(std::env::args_os().collect());
    // Before parsing: `<old name> --help` prints its help and exits there
    if let Some(note) = moved {
        eprintln!("{note}");
    }
    let cli = Cli::parse_from(args);
    match &cli.command {
        // The player says what `flint-player` says, as its `--log-format`
        // and `--quiet` ask (#716)
        Commands::Play(play) => flint_player::logging::init(&play.player.log),
        _ => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                        // `script=info`: what scripts print, and the script
                        // system's reloads (#703). `load=debug`: each file a
                        // render or an editor loads, which the player counts
                        // into one summary instead (#716)
                        tracing_subscriber::EnvFilter::new("warn,rfd=off,script=info,load=debug")
                    }),
                )
                // Stdout belongs to what a command answers with, such as --report json.
                .with_writer(std::io::stderr)
                .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
                .init();
        }
    }
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
        Commands::Init(args) => init::run(args),
        Commands::Entity(cmd) => entity::run(cmd),
        Commands::Scene(cmd) => scene::run(cmd),
        Commands::Query {
            query,
            scene,
            format,
            schemas,
        } => query::run(&query, scene.as_deref(), &format, &schemas),
        Commands::Schema(args) => schema::run(args),
        Commands::Script(cmd) => script::run(cmd),
        Commands::Validate(args) => validate::run(*args),
        Commands::Check(args) => check_cmd::run(*args),
        Commands::Examples(args) => examples::run(args),
        Commands::Bundle(args) => bundle::run(args),
        Commands::Export(cmd) => export::run(cmd),
        Commands::Dxc(cmd) => dxc::run(cmd),
        Commands::Music(cmd) => music::run(cmd),
        Commands::SpikeRumble(args) => spike_rumble::run(args),
        Commands::Goldens(args) => render_goldens::run_command(args),
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
        } => flint_viewer::app::run(
            &scene,
            watch,
            &schemas,
            !no_inspector,
            false,
            Default::default(),
            Default::default(),
        ),
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
