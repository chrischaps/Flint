//! `--load-report` (#1092): a table for each load (the start-up, each
//! scene transition, each chunk) of where its time went, stage by stage,
//! and the slowest assets by name. `flint play` and the standalone
//! `flint-player` flatten it, as they do `HitchArgs`; `--perf` turns it on
//! too, as the web page's `?perf=1` does.
//!
//! What is timed is `flint_core::load_report`: every timed frame note made
//! during the load, plus the stages the player names itself
//! (`player_app/load_timing.rs`).

use crate::PlayerApp;

/// The load-report flag, shared by `flint play` and `flint-player`.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct LoadReportArgs {
    /// Print a table for each load, the start-up, every scene transition
    /// and every chunk (#1092): each stage's ms, share and count (scene
    /// read, window, GPU device, renderer, models, textures, pipelines,
    /// scripts, on_init, physics, audio, the first frame...), what no stage
    /// named, and the five slowest assets by name. --perf prints it too
    #[arg(long)]
    pub load_report: bool,
}

impl LoadReportArgs {
    /// Whether to record the start-up: this flag or `--perf`.
    pub fn wanted(&self, perf: bool) -> bool {
        self.load_report || perf
    }

    /// Keep reporting each load after the start.
    pub fn apply(&self, app: &mut PlayerApp, perf: bool) {
        if self.wanted(perf) {
            app.launch.load_report = true;
        }
    }
}
