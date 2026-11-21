//! A game binary adds its own subcommands to `flint` (#1140, ADR 0133).
//!
//! This test is that game binary: `harness = false`, so its `main` is
//! ours. Run with `CLI_EXTENSION_GAME` set it is a game's `flint`,
//! `flint_cli::run_with` with a `hello` and a `peek` command of its own;
//! run plainly it spawns itself that way and checks what each command line
//! does. (`CLI_EXTENSION_GAME` is a handshake with its own child, not a
//! user switch, so not `FLINT_`.)

use clap::{ArgMatches, Command};
use flint_cli::extension::{self, CliExtension};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Output;

const GAME: &str = "CLI_EXTENSION_GAME";

/// The game's commands, derived.
#[derive(clap::Subcommand)]
enum Game {
    /// Greet someone, as the game would
    Hello(HelloArgs),
    /// Look at the level in a window
    Peek,
}

#[derive(clap::Args)]
struct HelloArgs {
    /// Who to greet
    #[arg(long)]
    name: String,
    /// Fail after answering, as a check with errors does
    #[arg(long)]
    fail: bool,
}

fn hello(args: HelloArgs) -> anyhow::Result<()> {
    // --project is applied before an extension runs
    let project = flint_core::project::Project::current()?;
    let greeting = format!("Hello, {}", args.name);
    extension::say(format!("(the project is {})", project.root.display()));
    extension::emit(
        &extension::Answer::new(
            greeting.clone(),
            json!({ "greeting": greeting, "project": project.root }),
        )
        .failed_if(args.fail),
    )
}

/// One written by hand, for a name the engine has.
struct Clash;

impl CliExtension for Clash {
    fn commands(&self) -> Vec<Command> {
        vec![Command::new("render").about("not the engine's")]
    }
    fn run(&mut self, _: &ArgMatches) -> anyhow::Result<()> {
        unreachable!("refused before parsing")
    }
}

fn main() {
    match std::env::var(GAME).as_deref() {
        Ok("game") => std::process::exit(game()),
        Ok("clash") => std::process::exit(code(flint_cli::run_with(vec![Box::new(Clash)]))),
        _ => checks(),
    }
}

fn game() -> i32 {
    code(flint_cli::run_with(vec![extension::subcommands(
        |cmd: Game| match cmd {
            Game::Hello(args) => hello(args),
            Game::Peek => unreachable!("refused under --json in these checks"),
        },
    )
    .opens_window_when(|cmd| matches!(cmd, Game::Peek))]))
}

/// `ExitCode` has no getter: success or not is all the checks need.
fn code(exit: std::process::ExitCode) -> i32 {
    i32::from(exit != std::process::ExitCode::SUCCESS)
}

fn run(mode: &str, dir: &Path, words: &[&str]) -> Output {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(words)
        .current_dir(dir)
        .env(GAME, mode)
        .env_remove("RUST_LOG")
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The whole of stdout as one JSON value.
fn document(out: &Output, line: &str) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{line}: stdout is not one JSON document ({e}):\n{}",
            text(&out.stdout)
        )
    })
}

fn checks() {
    let dir = scratch();
    let game = |words: &[&str]| run("game", &dir, words);
    let line = |words: &[&str]| words.join(" ");

    // The help lists the game's commands after the engine's, which keep
    // their order
    let words = ["--help"];
    let out = game(&words);
    let help = text(&out.stdout);
    assert!(out.status.success(), "{}", line(&words));
    let at = |command: &str, about: &str| {
        help.lines()
            .position(|l| l.trim_start().starts_with(command) && l.contains(about))
            .unwrap_or_else(|| panic!("`flint --help` lists {command}:\n{help}"))
    };
    let listed = [
        at("init", "Start a game project"),
        at("render", "Render a scene"),
        at("style", "User-authored styles"),
        at("hello", "Greet someone"),
        at("peek", "Look at the level"),
    ];
    assert!(listed.is_sorted(), "{listed:?}:\n{help}");
    let out = game(&["hello", "--help"]);
    assert!(
        text(&out.stdout).contains("--name <NAME>"),
        "{}",
        text(&out.stdout)
    );

    // The command runs, in text: the answer on stdout, the human line too
    let words = ["hello", "--name", "Ada"];
    let out = game(&words);
    assert!(
        out.status.success(),
        "{}: {}",
        line(&words),
        text(&out.stderr)
    );
    let stdout = text(&out.stdout);
    assert!(
        stdout.contains("Hello, Ada") && stdout.contains("the project is"),
        "{stdout}"
    );

    // The global --json, before or after the command: one document, the
    // human line moved to stderr
    for words in [
        ["--json", "hello", "--name", "Ada"],
        ["hello", "--name", "Ada", "--json"],
    ] {
        let out = game(&words);
        assert!(out.status.success(), "{}", line(&words));
        let doc = document(&out, &line(&words));
        assert_eq!(doc["greeting"], "Hello, Ada", "{doc}");
        assert!(text(&out.stderr).contains("the project is"));
    }

    // The global --project reaches the extension
    let elsewhere = dir.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::write(
        elsewhere.join("flint.toml"),
        "[project]\nid = \"elsewhere\"\n",
    )
    .unwrap();
    let words = ["--json", "--project", "elsewhere", "hello", "--name", "Ada"];
    let out = game(&words);
    let doc = document(&out, &line(&words));
    let root = PathBuf::from(doc["project"].as_str().unwrap());
    assert_eq!(
        root.canonicalize().unwrap(),
        elsewhere.canonicalize().unwrap(),
        "{doc}"
    );
    let out = game(&["--project", "nowhere-at-all", "hello", "--name", "Ada"]);
    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("not a directory"),
        "{}",
        text(&out.stderr)
    );

    // A failed answer is printed, then exit 1, and nothing more on stdout
    let words = ["--json", "hello", "--name", "Ada", "--fail"];
    let out = game(&words);
    assert_eq!(out.status.code(), Some(1), "{}", line(&words));
    assert_eq!(document(&out, &line(&words))["greeting"], "Hello, Ada");

    // A mistake on its command line is clap's, as the engine's are: exit 2,
    // and under --json an error document with the usage
    let words = ["--json", "hello"];
    let out = game(&words);
    assert_eq!(out.status.code(), Some(2), "{}", line(&words));
    let doc = document(&out, &line(&words));
    assert!(
        doc["error"]["usage"].as_str().unwrap().contains("--name"),
        "{doc}"
    );

    // A window under --json is refused with an error document, never opened
    let words = ["--json", "peek"];
    let out = game(&words);
    assert_eq!(out.status.code(), Some(1), "{}", line(&words));
    let doc = document(&out, &line(&words));
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(message.contains("flint peek opens a window"), "{doc}");

    // The engine's own commands still run in the game's binary
    let words = ["--json", "env"];
    let out = game(&words);
    assert!(
        out.status.success(),
        "{}: {}",
        line(&words),
        text(&out.stderr)
    );
    assert!(document(&out, &line(&words))["switches"].is_array());

    // A name the engine has is refused before anything runs, naming it
    let words = ["--json", "render", "x.scene.toml"];
    let out = run("clash", &dir, &words);
    assert_eq!(out.status.code(), Some(1));
    let doc = document(&out, &line(&words));
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("`render` is one of flint's own commands"),
        "{doc}"
    );

    std::fs::remove_dir_all(&dir).ok();
    println!("cli_extension: a game binary's own commands run beside flint's");
}

/// A folder of this run's own under the target dir, with a flint.toml so
/// `Project::current` finds it.
fn scratch() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("cli-extension-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("flint.toml"), "[project]\nid = \"scratch\"\n").unwrap();
    dir
}
