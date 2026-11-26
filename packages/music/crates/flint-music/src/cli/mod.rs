//! The `music` package's commands (#1555, ADR 0137 section 4): `flint music
//! validate`, `play` and `render`, a `CliExtension` the CLI launcher lists
//! (flint-cli's build script names [`extensions`] when its `music` feature
//! turns this crate's `cli` feature on).
//!
//! The `music` command is this package's. The `rhythm` package's commands
//! (`music calibrate`, `music chart play|replay`) go under it, by
//! `CliExtension::under(&["music"])`. A flint without this package has no
//! `music` command at all, and a line asking for one is answered from the
//! manifests compiled in: exit 2, naming `music`.
//!
//! `music validate` holds a rhythm suite to the rhythm layer's contract too,
//! which this crate can't name (flint-rhythm depends on it): the rhythm
//! package hands its rules in with [`judge_rhythm_suites_with`], a
//! registration point until #1572 gives the CLI typed ones.

use crate::{Issue, SuiteManifest};
use flint_package::cli::{subcommands, CliExtension};
use std::path::Path;
use std::sync::OnceLock;

/// A human line through the one output path (stdout, or stderr once stdout
/// is a document's).
macro_rules! say {
    ($($arg:tt)*) => {
        flint_package::cli::say(format_args!($($arg)*))
    };
}

mod play;
mod render;
mod validate;

pub use play::PlaySuiteArgs;
pub use render::RenderSuiteArgs;
pub use validate::ValidateSuiteArgs;

/// `flint music`, the package's command.
#[derive(clap::Subcommand)]
enum Top {
    /// Music suites and rhythm charts: validate, play, render to WAV,
    /// calibrate, chart play / chart replay
    #[command(subcommand)]
    Music(MusicCommands),
}

/// The package's subcommands of `flint music` (#709).
#[derive(clap::Subcommand)]
pub enum MusicCommands {
    /// Validate a music suite manifest (`.suite.toml`), its stems, and an
    /// optional beatmap chart
    Validate(ValidateSuiteArgs),

    /// Play a suite's stems sample-locked, no judgment
    Play(PlaySuiteArgs),

    /// Render a suite (and an event script) to WAV, offline and
    /// deterministically
    Render(RenderSuiteArgs),
}

/// Run one of the package's commands.
pub fn run(command: MusicCommands) -> anyhow::Result<()> {
    match command {
        MusicCommands::Validate(args) => validate::run(args),
        MusicCommands::Play(args) => play::run(args),
        MusicCommands::Render(args) => render::run(args),
    }
}

/// The package's commands, as the launcher lists them. `music play` sounds,
/// so `--json` refuses it.
pub fn extensions() -> Vec<Box<dyn CliExtension>> {
    vec![subcommands(|Top::Music(command)| run(command))
        .opens_window_when(|Top::Music(command)| matches!(command, MusicCommands::Play(_)))]
}

/// What the rhythm package checks of a suite (flint-rhythm's contract and
/// its charts), handed to `music validate` by the rhythm package's CLI.
pub trait RhythmSuites: Sync {
    /// Whether the suite is a rhythm suite (it declares `[reintegration]`).
    fn declared(&self, manifest: &SuiteManifest) -> bool;
    /// The suite held to the rhythm contract, after the general pass.
    fn validate_manifest(&self, manifest: &SuiteManifest) -> Vec<Issue>;
    /// A beatmap chart, read and held to the suite.
    fn validate_chart(&self, chart: &Path, manifest: &SuiteManifest) -> anyhow::Result<Vec<Issue>>;
}

static RHYTHM: OnceLock<&'static dyn RhythmSuites> = OnceLock::new();

/// The rhythm package's rules for `music validate`, given once a run before
/// the line runs; a second call keeps the first.
pub fn judge_rhythm_suites_with(rules: &'static dyn RhythmSuites) {
    let _ = RHYTHM.set(rules);
}

fn rhythm_rules() -> Option<&'static dyn RhythmSuites> {
    RHYTHM.get().copied()
}
