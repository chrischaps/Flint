//! `flint music`: the music and rhythm tools under one command (#709).
//!
//! Each was its own top-level command until #709 (`validate-suite`,
//! `play-suite`, `render-suite`, `calibrate`, `play-chart`,
//! `replay-chart`); those spellings still run, rewritten onto these by
//! [`super::legacy_names::route`] before clap parses, for one release.

use anyhow::Result;
use clap::Subcommand;

use super::{calibrate, play_chart, play_suite, render_suite, replay_chart, validate_suite};

#[derive(Subcommand)]
pub enum MusicCommands {
    /// Validate a music suite manifest (`.suite.toml`), its stems, and an
    /// optional beatmap chart
    Validate(validate_suite::ValidateSuiteArgs),

    /// Play a suite's stems sample-locked, no judgment
    Play(play_suite::PlaySuiteArgs),

    /// Render a suite (and an event script) to WAV, offline and
    /// deterministically
    Render(render_suite::RenderSuiteArgs),

    /// Tap-to-beat calibration: write the player's median offset to
    /// logs/latency/
    Calibrate(calibrate::CalibrateArgs),

    /// A suite against its beatmap chart: `play` it live with gamepad
    /// capture, or `replay` a session through judgment, headless
    #[command(subcommand)]
    Chart(ChartCommands),
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

pub fn run(cmd: MusicCommands) -> Result<()> {
    match cmd {
        MusicCommands::Validate(args) => validate_suite::run(args),
        MusicCommands::Play(args) => play_suite::run(args),
        MusicCommands::Render(args) => render_suite::run(args),
        MusicCommands::Calibrate(args) => calibrate::run(args),
        MusicCommands::Chart(ChartCommands::Play(args)) => play_chart::run(args),
        MusicCommands::Chart(ChartCommands::Replay(args)) => replay_chart::run(args),
    }
}
