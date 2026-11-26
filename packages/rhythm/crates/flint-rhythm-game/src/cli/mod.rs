//! The `rhythm` package's commands, under the `music` package's `flint
//! music` (#709, #1555): `calibrate` and `chart play|replay`, a
//! `CliExtension` the launcher lists after music's, from the manifest's
//! `[rust] cli` (in this crate since #1556, so flint-cli names none of it).
//! `music validate`, `play` and `render` are the music package's own
//! (`flint_music::cli`), and this extension hands `validate` the rhythm
//! contract and the chart check ([`RULES`]).
//!
//! Each was its own top-level command until #709 (`validate-suite`,
//! `play-suite`, `render-suite`, `calibrate`, `play-chart`,
//! `replay-chart`). Those spellings ran for one release, rewritten onto
//! these before clap parsed; #1141 retired them, so each is an unknown
//! command now.

use anyhow::{Context, Result};
use clap::Subcommand;
use flint_music::{Issue, SuiteManifest};
use flint_package::cli::{subcommands, CliExtension};
use std::path::Path;

/// A human line through the one output path (stdout, or stderr once stdout
/// is a document's).
macro_rules! say {
    ($($arg:tt)*) => {
        flint_package::cli::say(format_args!($($arg)*))
    };
}

mod calibrate;
mod play_chart;
mod play_chart_window;
mod replay_chart;

#[derive(Subcommand)]
pub enum RhythmCommands {
    /// Tap-to-beat calibration: write the player's median offset to
    /// logs/latency/
    Calibrate(calibrate::CalibrateArgs),

    /// A suite against its beatmap chart: `play` it live with gamepad
    /// capture, or `replay` a session through judgment, headless
    #[command(subcommand)]
    Chart(Box<ChartCommands>),
}

#[derive(Subcommand)]
pub enum ChartCommands {
    /// Play a suite against its chart with live gamepad capture
    /// (`--window`, `--record`)
    Play(play_chart::PlayChartArgs),

    /// Replay a recorded or synthetic session through judgment, fully
    /// headless (`--render` mixes it to WAV too)
    Replay(replay_chart::ReplayChartArgs),
}

pub fn run(cmd: RhythmCommands) -> Result<()> {
    match cmd {
        RhythmCommands::Calibrate(args) => calibrate::run(args),
        RhythmCommands::Chart(chart) => match *chart {
            ChartCommands::Play(args) => play_chart::run(args),
            ChartCommands::Replay(args) => replay_chart::run(args),
        },
    }
}

/// The rhythm package's commands under `flint music`, and its rules handed
/// to `music validate`. `calibrate` and `chart play` read a pad and sound,
/// so `--json` refuses them.
pub fn extensions() -> Vec<Box<dyn CliExtension>> {
    flint_music::cli::judge_rhythm_suites_with(&RULES);
    vec![subcommands(run)
        .under(&["music"])
        .opens_window_when(|cmd| match cmd {
            RhythmCommands::Calibrate(_) => true,
            RhythmCommands::Chart(chart) => matches!(**chart, ChartCommands::Play(_)),
        })]
}

/// The rhythm layer's half of `music validate`: its contract and the chart.
pub struct Rules;

pub static RULES: Rules = Rules;

impl flint_music::cli::RhythmSuites for Rules {
    fn declared(&self, manifest: &SuiteManifest) -> bool {
        flint_rhythm::Reintegration::declared(manifest)
    }

    fn validate_manifest(&self, manifest: &SuiteManifest) -> Vec<Issue> {
        flint_rhythm::validate_manifest(manifest)
    }

    fn validate_chart(&self, path: &Path, manifest: &SuiteManifest) -> Result<Vec<Issue>> {
        let chart = flint_rhythm::Chart::load(path)
            .with_context(|| format!("loading chart {}", path.display()))?;
        Ok(flint_rhythm::validate_chart(&chart, manifest))
    }
}

/// The manifest/chart/base-dir trio shared by `music chart play` and
/// `music chart replay`.
#[derive(clap::Args)]
pub struct ChartCommonArgs {
    /// Path to the suite manifest (.suite.toml)
    pub manifest: String,

    /// Beatmap chart (.chart.toml) for the suite
    #[arg(long)]
    pub chart: String,

    /// Directory the manifest's file paths are relative to (default: cwd)
    #[arg(long)]
    pub base_dir: Option<String>,
}

/// Write a report into `<base_dir>/logs/latency/` as
/// `<prefix>-<host>-<epoch>.toml` — the shared naming scheme the offset
/// readers sort lexicographically (name embeds host + epoch, so max =
/// newest). One writer for `music calibrate` and the input spike
/// (`music chart play --spike-input-secs`); the rumble spike, an example
/// of flint-input-capture since #709, writes its own.
#[allow(clippy::disallowed_methods)] // a command writes its report to the disk
fn write_latency_report(
    base_dir: &std::path::Path,
    prefix: &str,
    contents: &str,
) -> anyhow::Result<std::path::PathBuf> {
    use anyhow::Context;
    let dir = base_dir.join("logs/latency");
    std::fs::create_dir_all(&dir).context("creating logs/latency")?;
    let host = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "host".into());
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("{prefix}-{host}-{epoch}.toml"));
    std::fs::write(&path, contents).with_context(|| format!("writing {path:?}"))?;
    Ok(path)
}
