//! The `rhythm` package's commands, under the `music` package's `flint
//! music` (#709, #1555): `calibrate` and `chart play|replay`, a
//! `CliExtension` the launcher lists after music's. `music validate`,
//! `play` and `render` are the music package's own
//! (`flint_music::cli`), and this extension hands `validate` the rhythm
//! contract and the chart check ([`RULES`]). #1556 moves this file to the
//! rhythm package's crate.
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

use super::{calibrate, play_chart, replay_chart};

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
