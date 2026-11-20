//! Flint CLI - Command-line interface for the Flint engine

mod check;
mod commands;
mod file_dialog;
mod output;

use anyhow::Result;
use clap::{Parser, Subcommand};
use commands::{
    asset, bundle, capture, check as check_cmd, dxc, edit_router, entity, env, examples, export,
    frame, gen, gen_preview, init, legacy_names, music, play, playtest, prefab, preview, profile,
    query, render, render_goldens, scene, schema, script, spike_rumble, spline_edit, style,
    terrain_edit, tex_edit, validate,
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

    /// Answer with exactly one JSON document on stdout (every other line
    /// goes to stderr); a failure is exit 1, with an `error` document when
    /// the command printed none. A command that opens a window refuses it
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start a game project: a manifest, a lit scene, a HUD script, an
    /// input config and an agent guide, ready to render and play
    Init(Box<init::InitArgs>),

    /// Entity operations
    #[command(subcommand)]
    Entity(Box<entity::EntityCommands>),

    /// Scene operations
    #[command(subcommand)]
    Scene(Box<scene::SceneCommands>),

    /// Query entities: of a scene file, or of a running game (--attach)
    Query(Box<query::QueryArgs>),

    /// Show schema information: one component or archetype, --list for
    /// all of them, --format json / json-schema for the schemas as data
    Schema(Box<schema::SchemaArgs>),

    /// The script API: `flint script api [--json] [--filter x]` lists every
    /// function a script can call, and the callbacks
    #[command(subcommand)]
    Script(Box<script::ScriptCommands>),

    /// Open a file in the appropriate interactive editor
    Edit(Box<edit_router::EditArgs>),

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
    SplineEdit(Box<spline_edit::EditArgs>),

    /// Validate a scene against its schemas and constraints (and fix what
    /// a constraint can fix)
    Validate(Box<validate::ValidateArgs>),

    /// The pre-flight: everything wrong with a project or a scene in one
    /// run (schemas, entity references, assets, files, styles, constraints)
    Check(Box<check_cmd::CheckArgs>),

    /// Read a frame capture (`flint render --frame-capture`): show its
    /// passes, draws, targets and GPU time, diff two, copy a pass's image out
    Frame(Box<frame::FrameArgs>),

    /// Read a scope trace (`flint-player --trace`): the top scopes, a
    /// frame's tree, why a frame was slow against the normal ones, two
    /// traces' deltas
    Profile(Box<profile::ProfileArgs>),

    /// The curated example scenes, from the project root's index.toml (a
    /// flint-examples checkout's): list them by topic, or --check that every
    /// one is indexed and validates
    Examples(Box<examples::ExamplesArgs>),

    /// Every FLINT_* environment switch: what it accepts, what unset
    /// means and its value now (--json); exits 1 on a value it can't take
    Env(Box<env::EnvArgs>),

    /// Pack a game folder into .flintpack bundles for the web player
    Bundle(Box<bundle::BundleArgs>),

    /// Export a game for elsewhere: `flint export web <game>` writes a
    /// folder any static server plays it from
    #[command(subcommand)]
    Export(Box<export::ExportCommands>),

    /// Music suites and rhythm charts: validate, play, render to WAV,
    /// calibrate, chart play / chart replay
    #[command(subcommand)]
    Music(Box<music::MusicCommands>),

    /// The rumble spike (ADR 0025), an example since #709:
    /// `cargo run -p flint-input-capture --example spike_rumble`
    #[command(hide = true)]
    SpikeRumble(Box<spike_rumble::SpikeRumbleArgs>),

    /// Render a `.goldens.toml` of scenes and gate each against its golden
    /// PNG (`--bless` rewrites them)
    Goldens(Box<render_goldens::GoldensArgs>),

    /// Prefab operations
    #[command(subcommand)]
    Prefab(Box<prefab::PrefabCommands>),

    /// Asset management operations
    #[command(subcommand)]
    Asset(Box<asset::AssetCommands>),

    /// Play a scene with first-person controls and physics
    Play(Box<play::PlayArgs>),

    /// Play a scene headless on a fixed clock with an input script and the
    /// scripts' test_* calls; a JSON report, exit 1 on a failure
    Playtest(Box<playtest::PlaytestArgs>),
    /// Capture a shot list as deterministic frame sequences (each take a
    /// stepped run of `flint play`), with --verify and --encode mp4
    Capture(Box<capture::CaptureArgs>),

    /// Preview a 3D model file (GLB/glTF) with orbit camera
    #[command(hide = true)]
    Preview(Box<preview::PreviewArgs>),

    /// Run a procedural generation spec
    Gen(Box<gen::GenArgs>),

    /// Interactive previewer for procedural generation specs
    #[command(hide = true)]
    GenPreview(Box<gen_preview::GenPreviewArgs>),

    /// Interactive terrain editor with procgen, sculpting, and painting
    #[command(hide = true)]
    TerrainEdit(Box<terrain_edit::TerrainEditArgs>),

    /// Visual node editor for texture pipeline specs
    #[command(hide = true)]
    TexEdit(Box<tex_edit::TexEditArgs>),

    /// Render a scene to a PNG image (headless)
    Render(Box<render::RenderArgs>),

    /// DX12's shader compiler, linked in: what DX12 compiles with here, and
    /// DXC's notices for a Windows build that ships
    #[command(subcommand)]
    Dxc(Box<dxc::DxcCommands>),

    /// User-authored styles: new (from a template), check (no GPU),
    /// prelude (the generated code), list
    #[command(subcommand)]
    Style(Box<style::StyleCommands>),
}

fn main() -> std::process::ExitCode {
    // The old names (`play-chart` is `music chart play`, #709;
    // `render-suite <x>.goldens.toml` is `goldens`, #327) still run
    let (argv, moved) = legacy_names::route(std::env::args_os().collect());
    if let Some(note) = moved {
        eprintln!("{note}");
    }
    let cli = match Cli::try_parse_from(&argv) {
        Ok(cli) => cli,
        Err(e) => {
            // A mistake on the command line under --json is still one
            // document on stdout; --help and --version print as always
            use clap::error::ErrorKind;
            let help = matches!(
                e.kind(),
                ErrorKind::DisplayHelp
                    | ErrorKind::DisplayVersion
                    | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            );
            if !help && argv.iter().any(|a| a == "--json") {
                output::document(&serde_json::json!({
                    "error": { "message": e.kind().to_string(), "usage": e.to_string() }
                }));
            }
            e.exit();
        }
    };
    // Installed once the command is known: the player says what
    // `flint-player` says, as its `--log-format` and `--quiet` ask (#716)
    match &cli.command {
        Commands::Play(play) => flint_player::logging::init(&play.player.log),
        // A playtest is the player with no window: the player's filter and
        // load summary, and the count of the engine's warnings its report
        // holds, whatever RUST_LOG shows (#714)
        Commands::Playtest(_) => {
            use tracing_subscriber::layer::SubscriberExt as _;
            use tracing_subscriber::util::SubscriberInitExt as _;
            use tracing_subscriber::Layer as _;
            let filter =
                tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                    tracing_subscriber::EnvFilter::new(
                        flint_player::logging::LogArgs::default().default_filter(),
                    )
                });
            tracing_subscriber::registry()
                .with(
                    tracing_subscriber::fmt::layer()
                        // Stdout is the report's, under --json
                        .with_writer(std::io::stderr)
                        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
                        .with_filter(filter),
                )
                .with(flint_player::logging::tally_layer())
                .with(flint_player::playtest_warning_layer())
                .init();
        }
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
    output::set_json(cli.json);
    output::finish(run(cli))
}

fn run(cli: Cli) -> Result<()> {
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

    if let Some(name) = cli.command.interactive() {
        output::refuse_interactive(name)?;
    }

    match cli.command {
        Commands::Init(args) => init::run(*args),
        Commands::Entity(cmd) => entity::run(*cmd),
        Commands::Scene(cmd) => scene::run(*cmd),
        Commands::Query(args) => query::run(*args),
        Commands::Schema(args) => schema::run(*args),
        Commands::Script(cmd) => script::run(*cmd),
        Commands::Validate(args) => validate::run(*args),
        Commands::Check(args) => check_cmd::run(*args),
        Commands::Frame(args) => frame::run(*args),
        Commands::Profile(args) => profile::run(*args),
        Commands::Examples(args) => examples::run(*args),
        Commands::Env(args) => env::run(*args),
        Commands::Bundle(args) => bundle::run(*args),
        Commands::Export(cmd) => export::run(*cmd),
        Commands::Dxc(cmd) => dxc::run(*cmd),
        Commands::Music(cmd) => music::run(*cmd),
        Commands::SpikeRumble(args) => spike_rumble::run(*args),
        Commands::Goldens(args) => render_goldens::run_command(*args),
        Commands::Play(args) => play::run(*args),
        Commands::Playtest(args) => playtest::run(*args),
        Commands::Capture(args) => capture::run(*args),
        Commands::Prefab(cmd) => prefab::run(*cmd),
        Commands::Asset(cmd) => asset::run(*cmd),
        Commands::Edit(args) => edit_router::run(*args),
        Commands::SplineEdit(args) => spline_edit::run(*args),
        Commands::Preview(args) => preview::run(*args),
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
        Commands::Gen(args) => gen::run(*args),
        Commands::GenPreview(args) => gen_preview::run(*args),
        Commands::TerrainEdit(args) => terrain_edit::run(*args),
        Commands::TexEdit(args) => tex_edit::run(*args),
        Commands::Render(mut args) => {
            // --variant builds each variant's command line on this one (#291)
            // --json is the global flag, not render's: a variant's words leave it out
            args.command_line = commands::variants::subcommand_words(std::env::args_os())
                .into_iter()
                .filter(|w| w != "--json")
                .collect();
            if output::json() && args.out.watch {
                anyhow::bail!(
                    "--watch renders until stopped and has no one JSON document: drop --json"
                );
            }
            // --json is --report json: the render's report is its document
            if output::json() && args.measure.report.is_none() {
                args.measure.report = Some("json".into());
            }
            render::run(*args)
        }
        Commands::Style(cmd) => style::run(*cmd),
    }
}

impl Commands {
    /// The command's name when it opens a window (or plays sound and reads
    /// a pad), which `--json` refuses. `flint edit` decides for itself: a
    /// model's or effect's `--render` snapshot answers with the file it wrote.
    fn interactive(&self) -> Option<&'static str> {
        Some(match self {
            Commands::Serve { .. } => "serve",
            Commands::SplineEdit(_) => "spline-edit",
            Commands::Music(cmd) => match &**cmd {
                music::MusicCommands::Play(_) => "music play",
                music::MusicCommands::Calibrate(_) => "music calibrate",
                music::MusicCommands::Chart(music::ChartCommands::Play(_)) => "music chart play",
                _ => return None,
            },
            Commands::SpikeRumble(_) => "spike-rumble",
            Commands::Play(_) => "play",
            Commands::Preview(args) if args.render.is_none() => "preview",
            Commands::GenPreview(_) => "gen-preview",
            Commands::TerrainEdit(_) => "terrain-edit",
            Commands::TexEdit(_) => "tex-edit",
            Commands::Prefab(_) => "prefab view",
            Commands::Frame(args) => match &args.command {
                frame::FrameCommands::Open(open) if open.interactive() => "frame open",
                _ => return None,
            },
            _ => return None,
        })
    }
}

#[cfg(test)]
mod stack_tests {
    use super::*;

    /// Parse `words` (after `flint`) on a thread of `bytes` of stack, the
    /// help or error text rendered as the binary would print it.
    fn parse_on_stack(words: &[&str], bytes: usize) -> String {
        let argv: Vec<String> = std::iter::once("flint")
            .chain(words.iter().copied())
            .map(String::from)
            .collect();
        std::thread::Builder::new()
            .stack_size(bytes)
            .spawn(move || match Cli::try_parse_from(argv) {
                Ok(cli) => format!("parsed ({} bytes)", std::mem::size_of_val(&cli)),
                Err(e) => e.render().to_string(),
            })
            .unwrap()
            .join()
            .unwrap()
    }

    /// Windows' default main-thread stack. `build.rs` links an 8 MB one
    /// however the crate is built (#990), but the parser alone should never
    /// need it: a debug build's
    /// derived `augment_args` is one frame of about 8 KB a flag, and #708
    /// measured 1346 KB to parse any command line while `flint render` was
    /// one struct of 130 flags (about 600 KB once each help heading was its own
    /// struct). A stack overflow aborts the test binary, naming the thread.
    const WINDOWS_DEFAULT_STACK: usize = 1 << 20;

    #[test]
    fn the_parser_fits_in_windows_default_stack() {
        for line in [
            "--help",
            "render --help",
            "render x.scene.toml --set post.exposure=2",
        ] {
            let words: Vec<&str> = line.split_whitespace().collect();
            let out = parse_on_stack(&words, WINDOWS_DEFAULT_STACK);
            assert!(!out.is_empty(), "{line}");
        }
    }

    /// The least stack each command line parses on, by bisection (#708):
    /// `cargo test -p flint-cli --bin flint stack_use -- --ignored --nocapture`.
    /// Each try is this test binary run again with `STACK_PROBE_KB` (a
    /// handshake with its own child, not a user switch, so not `FLINT_`),
    /// since an overflow aborts the process.
    #[test]
    #[ignore = "a measurement, run by hand"]
    fn stack_use() {
        if let Ok(kb) = std::env::var("STACK_PROBE_KB") {
            let line = std::env::var("STACK_PROBE_ARGS").unwrap();
            let words: Vec<&str> = line.split_whitespace().collect();
            parse_on_stack(&words, kb.parse::<usize>().unwrap() << 10);
            return;
        }
        let exe = std::env::current_exe().unwrap();
        let fits = |line: &str, kb: usize| {
            std::process::Command::new(&exe)
                .args(["--ignored", "--exact", "stack_tests::stack_use"])
                .env("STACK_PROBE_KB", kb.to_string())
                .env("STACK_PROBE_ARGS", line)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        println!(
            "size_of::<Commands>() = {} bytes",
            std::mem::size_of::<Commands>()
        );
        for line in [
            "--help",
            "render --help",
            "render x.scene.toml --set post.exposure=2",
        ] {
            let (mut lo, mut hi) = (16, 16384);
            assert!(fits(line, hi), "{line}: overflows {hi} KB");
            while hi - lo > 4 {
                let mid = (lo + hi) / 2;
                if fits(line, mid) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            println!("{line:<45} {hi} KB");
        }
    }
}
