//! A package's commands on the `flint` command line (ADR 0133, ADR 0137
//! section 4, #1553).
//!
//! A package's CLI side builds against this module, never against
//! flint-cli: a package's crate depends on `flint-package`, and the stock
//! `flint` on the package's crate, so Cargo would refuse the other way
//! round. flint-cli's `extension` module re-exports it all, so a game
//! binary written against ADR 0133 compiles unchanged.
//!
//! A [`CliExtension`] names the subcommands it adds as `clap::Command`s,
//! beside the engine's own or [`under`](CliExtension::under) one of them
//! (`flint asset generate` is the ai package's, under the kernel's
//! `asset`), and runs the one chosen. They parse with the engine's
//! commands in one parser, so the global `--json` and `--project` reach
//! them, `flint --help` lists them, and they leave through the engine's
//! exit path:
//!
//! - under `--json`, print exactly one document ([`document`], [`emit`]),
//!   human lines through [`say`];
//! - a failure is an `Err` (an answer already printed that is a failure:
//!   `Err(failed())`), never `std::process::exit`;
//! - a command that opens a window says so in
//!   [`CliExtension::opens_window`], and `--json` refuses it.
//!
//! [`subcommands`] turns a derived `clap::Subcommand` enum into an
//! extension; implement the trait directly for anything else.
//!
//! The output state (`--json`, whether stdout is a document's, whether one
//! was printed) lives here, once a process, so an engine command and a
//! package's answer the same way: flint-cli's `output` module is these
//! functions and the exit path (#710).

use anyhow::Result;
use clap::{ArgMatches, Command, Subcommand};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

pub mod asset;
mod clock;
pub mod flags;
pub mod gen;
#[cfg(feature = "gif")]
pub mod gif;
pub mod look;
mod points;
pub mod render;

pub use clock::{random_seed, rfc3339, utc_now};
pub use flags::Flags;
pub use points::{
    points, points_or, Points, SpecChecks, SpecGen, SpecIssue, SpecParams, Stages, TableCheck,
    TableChecks,
};

/// Subcommands a package or an embedding game adds to `flint`, given to
/// flint-cli's `run_with`. The engine's own commands keep their names: an
/// extension naming one at the same place is refused, as are two
/// extensions naming the same.
pub trait CliExtension {
    /// The subcommands this extension adds, each a `clap::Command` with its
    /// own name, flags and help. Asked once a run, before parsing.
    fn commands(&self) -> Vec<Command>;

    /// The words of the engine command these commands go under: `["asset"]`
    /// puts `generate` at `flint asset generate`. Empty (the default): beside
    /// the engine's own top-level commands.
    fn under(&self) -> &[&'static str] {
        &[]
    }

    /// Run the command the line chose. `matches` are the matches of the
    /// command yours sit under (the whole line's for a top-level command):
    /// `matches.subcommand()` is one of [`commands`](Self::commands) with
    /// its own matches, and the global flags (`json`, `project`) are on it
    /// too. `--project` is already applied: `Project::current()` returns it.
    fn run(&mut self, matches: &ArgMatches) -> Result<()>;

    /// Fill in the run's registration points ([`Points`], #1557): what
    /// another command asks this package for (procgen's [`SpecGen`] for the
    /// kernel's `gen`). Called once, before parsing.
    fn register(&self, _points: &mut Points) {}

    /// The flag groups this package adds to commands it doesn't own
    /// ([`flags`], #1572): each a derived `clap::Args` and the command it
    /// goes on, which declares a [`flags::PackageFlags`] slot for them.
    /// Asked once a run, before parsing.
    fn flags(&self) -> Vec<Flags> {
        Vec::new()
    }

    /// The chosen command opens a window (or plays sound, or reads a pad),
    /// which `--json` refuses, as it does `flint play`. No by default.
    /// `matches` are [`run`](Self::run)'s.
    fn opens_window(&self, _matches: &ArgMatches) -> bool {
        false
    }
}

/// An extension from a derived `clap::Subcommand` enum: each variant is a
/// subcommand, and `run` gets the one the line chose.
///
/// ```ignore
/// #[derive(clap::Subcommand)]
/// enum GameCommands {
///     /// Bake the level's navigation grid
///     BakeNav(BakeNavArgs),
/// }
/// flint_cli::run_with(vec![subcommands(|cmd: GameCommands| match cmd {
///     GameCommands::BakeNav(args) => bake_nav(args),
/// })])
/// ```
///
/// [`Derived::opens_window_when`] names the variants that open a window,
/// [`Derived::under`] the engine command they go under.
pub fn subcommands<S, F>(run: F) -> Box<Derived<S, F>>
where
    S: Subcommand + 'static,
    F: FnMut(S) -> Result<()> + 'static,
{
    Box::new(Derived {
        run,
        window: None,
        under: &[],
        points: None,
        _commands: std::marker::PhantomData,
    })
}

/// What [`subcommands`] makes.
pub struct Derived<S, F> {
    run: F,
    #[allow(clippy::type_complexity)]
    window: Option<Box<dyn Fn(&S) -> bool>>,
    under: &'static [&'static str],
    points: Option<fn(&mut Points)>,
    _commands: std::marker::PhantomData<fn() -> S>,
}

impl<S: Subcommand, F> Derived<S, F> {
    /// Which of the enum's commands open a window, refused under `--json`.
    pub fn opens_window_when(
        mut self: Box<Self>,
        window: impl Fn(&S) -> bool + 'static,
    ) -> Box<Self> {
        self.window = Some(Box::new(window));
        self
    }

    /// The engine command the enum's commands go under, by its words
    /// (`&["asset"]`).
    pub fn under(mut self: Box<Self>, words: &'static [&'static str]) -> Box<Self> {
        self.under = words;
        self
    }

    /// What the extension puts in the run's registration points
    /// ([`CliExtension::register`]).
    pub fn registering(mut self: Box<Self>, points: fn(&mut Points)) -> Box<Self> {
        self.points = Some(points);
        self
    }
}

impl<S, F> CliExtension for Derived<S, F>
where
    S: Subcommand + 'static,
    F: FnMut(S) -> Result<()> + 'static,
{
    fn commands(&self) -> Vec<Command> {
        S::augment_subcommands(Command::new("flint"))
            .get_subcommands()
            .cloned()
            .collect()
    }

    fn under(&self) -> &[&'static str] {
        self.under
    }

    fn register(&self, points: &mut Points) {
        if let Some(register) = self.points {
            register(points);
        }
    }

    fn run(&mut self, matches: &ArgMatches) -> Result<()> {
        let command = S::from_arg_matches(matches)?;
        (self.run)(command)
    }

    fn opens_window(&self, matches: &ArgMatches) -> bool {
        match (&self.window, S::from_arg_matches(matches)) {
            (Some(window), Ok(command)) => window(&command),
            _ => false,
        }
    }
}

/// `--json` was given: stdout is one JSON document.
static JSON: AtomicBool = AtomicBool::new(false);
/// Stdout holds a document (`--json`, or a command's own `--report json`),
/// so the human lines go to stderr.
static STDOUT_RESERVED: AtomicBool = AtomicBool::new(false);
/// A document has been printed: an error after it adds none.
static EMITTED: AtomicBool = AtomicBool::new(false);

/// Turns `--json` on for the process (the launcher does, from the global
/// flag).
pub fn set_json(on: bool) {
    JSON.store(on, Ordering::Relaxed);
    if on {
        reserve_stdout();
    }
}

/// Whether `--json` was given.
pub fn json() -> bool {
    JSON.load(Ordering::Relaxed)
}

/// A command's own `--format`, which `--json` overrides: `json` under
/// `--json`, else what was asked.
pub fn format_or_json(format: &str) -> &str {
    if json() {
        "json"
    } else {
        format
    }
}

/// Stdout belongs to a document from here on: [`say`] writes to stderr.
/// `flint render --report json` reserves it without `--json`.
pub fn reserve_stdout() {
    STDOUT_RESERVED.store(true, Ordering::Relaxed);
}

/// Whether the human lines go to stderr.
pub fn stdout_reserved() -> bool {
    STDOUT_RESERVED.load(Ordering::Relaxed)
}

/// A human line, as the engine's commands write theirs: on stdout, or on
/// stderr when stdout is the `--json` document.
pub fn say(line: impl std::fmt::Display) {
    if stdout_reserved() {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
}

/// A line about work in flight ("Generating ..."): always stderr, so it
/// never mixes with an answer.
pub fn progress(line: impl std::fmt::Display) {
    eprintln!("{line}");
}

/// A command's answer, in both forms.
pub trait Report {
    /// What a person reads, printed as is (a final newline is added if
    /// missing).
    fn text(&self) -> String;
    /// The same answer as one JSON document.
    fn json(&self) -> Value;
    /// The answer is a failure: the run exits 1 after printing it.
    fn failed(&self) -> bool {
        false
    }
}

/// A report built in place: the text and the document side by side.
pub struct Answer {
    pub text: String,
    pub json: Value,
    pub failed: bool,
}

impl Answer {
    pub fn new(text: impl Into<String>, json: Value) -> Self {
        Self {
            text: text.into(),
            json,
            failed: false,
        }
    }

    /// The same answer, exiting 1 when `failed`.
    pub fn failed_if(mut self, failed: bool) -> Self {
        self.failed = failed;
        self
    }
}

impl Report for Answer {
    fn text(&self) -> String {
        self.text.clone()
    }
    fn json(&self) -> Value {
        self.json.clone()
    }
    fn failed(&self) -> bool {
        self.failed
    }
}

/// Prints `report` in the form asked for. `Err(Failed)` when the report is
/// a failure: return it from the command.
pub fn emit(report: &impl Report) -> Result<()> {
    if json() {
        document(&report.json());
    } else {
        let text = report.text();
        if text.ends_with('\n') || text.is_empty() {
            print!("{text}");
        } else {
            println!("{text}");
        }
    }
    if report.failed() {
        Err(Failed.into())
    } else {
        Ok(())
    }
}

/// Prints `value` as a JSON document on stdout. A command prints one.
pub fn document(value: &Value) {
    EMITTED.store(true, Ordering::Relaxed);
    // A Value always serialises.
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| "null".into())
    );
}

/// Under `--json`, prints the document `f` builds; otherwise nothing (the
/// command said it in [`say`] lines). For a command whose text is written
/// as it goes.
pub fn when_json(f: impl FnOnce() -> Value) {
    if json() {
        document(&f());
    }
}

/// The error to return when the command's answer, already printed, is a
/// failure: exit 1 and nothing more said.
pub fn failed() -> anyhow::Error {
    Failed.into()
}

/// Prints an already-serialised document (a command whose report is a
/// typed struct with its own serialiser).
pub fn document_text(text: &str) {
    EMITTED.store(true, Ordering::Relaxed);
    println!("{}", text.trim_end());
}

/// Whether a document has been printed this run.
pub fn emitted() -> bool {
    EMITTED.load(Ordering::Relaxed)
}

/// The command has said why it failed (its report is the answer); the
/// launcher exits 1 and prints nothing more.
#[derive(Debug)]
pub struct Failed;

impl std::fmt::Display for Failed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the command reported a failure")
    }
}

impl std::error::Error for Failed {}

/// A command (or a flag of one) needs a package this binary was built
/// without (#1333). The launcher exits 2 on it, naming the package, and
/// under `--json` its error document carries `package`. A package's
/// command returns it for a part another package brings (`flint music
/// validate` on a rhythm suite without `rhythm`, #1555).
#[derive(Debug)]
pub struct MissingPackage {
    /// What was asked for: `flint music chart play`, `--ai-refine`.
    pub what: String,
    /// The package (and the flint-cli feature) it needs.
    pub package: &'static str,
}

impl std::fmt::Display for MissingPackage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} needs the `{}` package, and this flint was built without it: \
             list `{} = \"*\"` under [packages] in flint.toml and run `flint build` \
             (`flint package list --built` names the packages this one has)",
            self.what, self.package, self.package
        )
    }
}

impl std::error::Error for MissingPackage {}

/// The error a command returns for a part of it whose package this binary
/// lacks.
pub fn missing(what: impl Into<String>, package: &'static str) -> anyhow::Error {
    MissingPackage {
        what: what.into(),
        package,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_report_is_the_failed_error() {
        let answer = Answer::new("x", serde_json::json!({})).failed_if(true);
        assert!(answer.failed());
        let e: anyhow::Error = Failed.into();
        assert!(e.downcast_ref::<Failed>().is_some());
    }

    #[derive(clap::Subcommand)]
    enum Two {
        /// One
        One,
        /// Two
        Two,
    }

    #[test]
    fn a_derived_enum_says_where_it_goes() {
        let top = subcommands(|_: Two| Ok(()));
        assert!(CliExtension::under(&*top).is_empty());
        let names: Vec<_> = top
            .commands()
            .iter()
            .map(|c| c.get_name().to_string())
            .collect();
        assert_eq!(names, ["one", "two"]);
        let under = subcommands(|_: Two| Ok(())).under(&["asset"]);
        assert_eq!(CliExtension::under(&*under), ["asset"]);
    }
}
