//! Flint CLI - Command-line interface for the Flint engine
//!
//! The `flint` binary is [`run_with`] with no extensions. A game binary that
//! embeds the CLI calls it with its own [`CliExtension`]s, which add
//! subcommands beside the engine's, as `PlayerExtension` adds hooks to the
//! player (ADR 0133):
//!
//! ```ignore
//! #[derive(clap::Subcommand)]
//! enum GameCommands {
//!     /// Bake the level's navigation grid
//!     BakeNav(BakeNavArgs),
//! }
//!
//! fn main() -> std::process::ExitCode {
//!     flint_cli::run_with(vec![flint_cli::extension::subcommands(
//!         |cmd: GameCommands| match cmd {
//!             GameCommands::BakeNav(args) => bake_nav(args),
//!         },
//!     )])
//! }
//! ```

mod check;
mod commands;
pub mod extension;
mod output;

pub use extension::CliExtension;

use anyhow::Result;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use commands::{
    asset, build, bundle, capture, check as check_cmd, dxc, edit_router, entity, env, examples,
    export, frame, gen, init, package, play, playtest, prefab, preview, profile, query, render,
    render_goldens, scene, schema, script, spec_edit, spline_edit, style, terrain_edit, validate,
};
use std::ffi::OsString;

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

    /// Render a `.goldens.toml` of scenes and gate each against its golden
    /// PNG (`--bless` rewrites them)
    Goldens(Box<render_goldens::GoldensArgs>),

    /// Prefab operations
    #[command(subcommand)]
    Prefab(Box<prefab::PrefabCommands>),

    /// Asset management operations
    #[command(subcommand)]
    Asset(Box<asset::AssetCommands>),

    /// The engine's packages, from their manifests: `flint package list
    /// [--json]` gives each one's tier, version and what it owns
    #[command(subcommand)]
    Package(Box<package::PackageCommands>),

    /// Build the project's flint-player and flint with the packages its
    /// flint.toml lists (`--dry-run` prints the cargo command)
    Build(Box<build::BuildArgs>),

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
    GenPreview(Box<spec_edit::GenPreviewArgs>),

    /// Interactive terrain editor with procgen, sculpting, and painting
    #[command(hide = true)]
    TerrainEdit(Box<terrain_edit::TerrainEditArgs>),

    /// Visual node editor for texture pipeline specs
    #[command(hide = true)]
    TexEdit(Box<spec_edit::TexEditArgs>),

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

/// The whole `flint` command line, from the process's arguments to its exit
/// code: the engine's commands and, beside them, each extension's (ADR
/// 0133). `flint` itself is `run_with(Vec::new())`.
///
/// An extension's command parses with the same parser as the engine's, so
/// the global `--json` and `--project` reach it, `flint --help` lists it,
/// and it leaves through the same exit path (`extension::document`,
/// `extension::failed`). A name that is already the engine's or another
/// extension's is refused before anything runs. A game binary links its own main-thread stack on Windows
/// (book: guides/player-extensions.md); flint-cli's `build.rs` reaches only
/// flint-cli's own targets.
///
/// The packages' commands come first: `run_with` is [`stock`]'s
/// extensions, then `extensions`, so a game binary keeps every command the
/// stock `flint` has.
pub fn run_with(extensions: Vec<Box<dyn CliExtension>>) -> std::process::ExitCode {
    // What `flint build` set aside last time (#1335), if it has stopped
    flint_player::packages::sweep_set_aside();
    let argv: Vec<OsString> = std::env::args_os().collect();
    let mut all = stock();
    all.extend(extensions);
    // The run's registration points, before any command runs (#1557)
    flint_package::cli::points_or(|| points_of(&all));
    let mut added = match extension::Added::new(all, &Cli::command()) {
        Ok(added) => added,
        Err(e) => {
            output::set_json(argv.iter().any(|a| a == "--json"));
            return output::finish(Err(e));
        }
    };
    let matches = match parse_line(&added, &argv) {
        Ok(matches) => matches,
        Err(Refused::Clap(e)) => parse_failed(e, &argv),
        Err(Refused::Missing(m)) => {
            output::set_json(argv.iter().any(|a| a == "--json"));
            return output::finish(Err(m.into()));
        }
    };
    if added.owns(&matches) {
        default_logging();
        output::set_json(matches.get_flag("json"));
        let project = matches.get_one::<std::path::PathBuf>("project");
        return output::finish(set_project(project).and_then(|()| added.run(&matches)));
    }
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(e) => parse_failed(e.format(&mut Cli::command()), &argv),
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
        // The editors' windows say what they say through the player's
        // subscriber, and the scene viewer its one load summary (#1164)
        Commands::Edit(edit) => edit.log.init(),
        Commands::Serve { .. }
        | Commands::SplineEdit(_)
        | Commands::Preview(_)
        | Commands::GenPreview(_)
        | Commands::TerrainEdit(_)
        | Commands::TexEdit(_) => edit_router::EditLogArgs::default().init(),
        _ => default_logging(),
    }
    output::set_json(cli.json);
    output::finish(run(cli))
}

/// The CLI launcher's list (ADR 0137 section 4, #1553): the extension of
/// every package this build has, each the package's own `cli()`. The stock
/// `flint` is [`run_with`] over it and nothing else. A package that adds
/// no command has nothing here; one left out of the build has nothing in
/// the parser, and a line naming its command is told to add it
/// ([`commands::absent::missing_command`]).
///
/// `build.rs` writes the list from the manifests (#1554): each package
/// whose `[rust] cli` names a crate, when this build has its
/// `[build] features`, in the order packages register, so the source names
/// none (ai; music since #1555; rhythm, under music's `music`, since #1556).
pub fn stock() -> Vec<Box<dyn CliExtension>> {
    include!(concat!(env!("OUT_DIR"), "/stock_extensions.rs"))
}

/// The run's registration points (#1557, `flint_package::cli::Points`):
/// what the extensions of `extensions` register (procgen's spec commands),
/// then this binary's own parts of another package's commands (the ai
/// package's steps of procgen's `gen`, living's `--strip`), each under its
/// feature.
fn points_of(extensions: &[Box<dyn CliExtension>]) -> flint_package::cli::Points {
    let mut points = flint_package::cli::Points::new();
    for extension in extensions {
        extension.register(&mut points);
    }
    #[cfg(feature = "ai")]
    points.insert(flint_procgen_cli::gen::AiPoint(Box::new(
        commands::gen_ai::Ai,
    )));
    #[cfg(feature = "living")]
    points.insert(commands::gen_strip::point());
    points
}

/// The run's points: the launcher's ([`run_with`] makes them before any
/// command runs), else the stock list's (a unit test).
pub(crate) fn points() -> &'static flint_package::cli::Points {
    flint_package::cli::points_or(|| points_of(&stock()))
}

/// Whether this run opens procgen's spec editors (a window): a run without
/// the package refuses the command naming the package instead.
fn procgen_editors() -> bool {
    points().get::<flint_viewer::host::SpecEditors>().is_some()
}

/// The stock `flint`'s whole command tree: the engine's commands with
/// every [`stock`] extension's, as `flint --help` walks it.
#[cfg(test)]
pub(crate) fn stock_command() -> clap::Command {
    extension::Added::new(stock(), &Cli::command())
        .expect("the stock extensions name no engine command")
        .augment(Cli::command())
}

/// `words` after `flint`, parsed and run as the launcher would with
/// `extensions` (and no others): a command of a package missing from them
/// is the [`commands::absent::MissingPackage`] error, as the binary's exit
/// 2. Panics on any other line clap refuses.
#[cfg(test)]
pub(crate) fn run_line(extensions: Vec<Box<dyn CliExtension>>, words: &[&str]) -> Result<()> {
    let argv: Vec<OsString> = std::iter::once("flint")
        .chain(words.iter().copied())
        .map(OsString::from)
        .collect();
    let mut added = extension::Added::new(extensions, &Cli::command())?;
    let matches = match parse_line(&added, &argv) {
        Ok(matches) => matches,
        Err(Refused::Missing(missing)) => return Err(missing.into()),
        Err(Refused::Clap(e)) => panic!("{words:?} doesn't parse: {e}"),
    };
    if added.owns(&matches) {
        return added.run(&matches);
    }
    run(Cli::from_arg_matches(&matches)?)
}

/// Why a command line has no matches.
enum Refused {
    /// clap's own mistake, `--help` and `--version` too: printed as clap
    /// prints it.
    Clap(clap::Error),
    /// A command of a package this build was built without (#1553).
    Missing(commands::absent::MissingPackage),
}

/// The line's matches in the one parser: the engine's commands with every
/// extension's.
fn parse_line(added: &extension::Added, argv: &[OsString]) -> Result<clap::ArgMatches, Refused> {
    let mut parser = added.augment(Cli::command());
    parser.try_get_matches_from_mut(argv).map_err(|e| {
        match commands::absent::missing_command(&e, argv, &parser) {
            Some(missing) => Refused::Missing(missing),
            None => Refused::Clap(e),
        }
    })
}

/// A command line clap refused, or `--help` / `--version`: printed, and the
/// process exits. A mistake on the command line under --json is still one
/// document on stdout; --help and --version print as always.
fn parse_failed(e: clap::Error, argv: &[OsString]) -> ! {
    use clap::error::ErrorKind;
    // A per-effect flag `flint render` had until #1144 names its spelling
    let e = commands::render_retired::explained(e, argv);
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

/// The log every command but `play` and `playtest` writes, to stderr.
fn default_logging() {
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

/// `--project DIR`: the project every later `Project::current` returns,
/// for an engine command and an extension's alike.
fn set_project(dir: Option<&std::path::PathBuf>) -> Result<()> {
    let Some(dir) = dir else {
        return Ok(());
    };
    if !dir.is_dir() {
        anyhow::bail!("--project {}: not a directory", dir.display());
    }
    let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.clone());
    // Reads the directory's flint.toml: a bad manifest stops here.
    let project =
        flint_core::project::Project::at(dir).map_err(|e| anyhow::anyhow!("--project {e}"))?;
    flint_core::project::set_override(project);
    Ok(())
}

fn run(cli: Cli) -> Result<()> {
    set_project(cli.project.as_ref())?;

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
        Commands::Goldens(args) => render_goldens::run_command(*args),
        Commands::Play(args) => play::run(*args),
        Commands::Playtest(args) => playtest::run(*args),
        Commands::Capture(args) => capture::run(*args),
        Commands::Prefab(cmd) => prefab::run(*cmd),
        Commands::Asset(cmd) => asset::run(*cmd),
        Commands::Package(cmd) => package::run(*cmd),
        Commands::Build(args) => build::run(*args),
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
        Commands::GenPreview(args) => spec_edit::gen_preview(*args),
        Commands::TerrainEdit(args) => terrain_edit::run(*args),
        Commands::TexEdit(args) => spec_edit::tex_edit(*args),
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
            // `--list-keys` prints the debug-key table and exits (#717, #1184)
            Commands::Play(args) if !args.player.list_keys => "play",
            Commands::Preview(args) if args.render.is_none() => "preview",
            // A flint without the procgen package opens no window: the
            // command names the package, under --json too
            Commands::GenPreview(_) if procgen_editors() => "gen-preview",
            Commands::TerrainEdit(_) => "terrain-edit",
            Commands::TexEdit(_) if procgen_editors() => "tex-edit",
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
            // The parser the binary builds: the engine's commands with the
            // stock extensions' (#1553)
            .spawn(move || match stock_command().try_get_matches_from(argv) {
                Ok(m) => format!("parsed {:?}", m.subcommand_name()),
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
            "asset generate --help",
        ] {
            let words: Vec<&str> = line.split_whitespace().collect();
            let out = parse_on_stack(&words, WINDOWS_DEFAULT_STACK);
            assert!(!out.is_empty(), "{line}");
        }
    }

    /// The least stack each command line parses on, by bisection (#708):
    /// `cargo test -p flint-cli --lib stack_use -- --ignored --nocapture`.
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
            "asset generate --help",
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
