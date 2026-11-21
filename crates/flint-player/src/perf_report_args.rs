//! `--perf-report <file.json>` (#1093): what `--exit-after` prints as
//! `[perf]` lines, written as data, so two runs can be compared by a script
//! (`scripts/perf-compare.py`) rather than by eye. `flint play` and the
//! standalone `flint-player` flatten the same flag, as they do `HitchArgs`.
//!
//! The report itself is `player_app/perf_report.rs`.

use crate::PlayerApp;

/// The perf report flag, shared by `flint play` and `flint-player`
#[derive(clap::Args, Clone, Debug, Default)]
pub struct PerfReportArgs {
    /// Write the `--exit-after` report to FILE as JSON when the run ends
    /// (#1093): the frame and dt spreads, each phase's and script's mean and
    /// max, the GPU's passes, draws, hitches, and the machine (adapter,
    /// backend, present mode, size, build profile, engine commit).
    /// Needs --exit-after. Records every frame, times scripts and the GPU
    /// where the adapter can. `scripts/perf-compare.py` compares two
    #[arg(long, value_name = "FILE")]
    pub perf_report: Option<std::path::PathBuf>,
}

impl PerfReportArgs {
    /// Ask `app` for the report, before `run_app`. Refused without
    /// `--exit-after` (the numbers are its), and on the web.
    pub fn apply(&self, app: &mut PlayerApp, exit_after: Option<f64>) -> anyhow::Result<()> {
        let Some(path) = &self.perf_report else {
            return Ok(());
        };
        if exit_after.is_none() {
            anyhow::bail!(
                "--perf-report {} needs --exit-after: the report is that run's numbers",
                path.display()
            );
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = app;
            anyhow::bail!("--perf-report {}: no files on the web", path.display());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            app.report_perf_to(path.clone());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        report: PerfReportArgs,
    }

    #[test]
    fn the_flag_takes_a_file() {
        let cli = Cli::parse_from(["x", "--perf-report", "out/a.json"]);
        assert_eq!(
            cli.report.perf_report.as_deref(),
            Some(std::path::Path::new("out/a.json"))
        );
        assert!(Cli::parse_from(["x"]).report.perf_report.is_none());
    }
}
