//! The trace flags (#1097): record the engine's scopes and save the last
//! few seconds of them as a Chrome trace-event JSON file Perfetto opens, at
//! exit or after a hitch. `flint play` and the standalone `flint-player`
//! flatten the same flags, as they do `HitchArgs`.
//!
//! What is recorded and how it is written is `player_app/trace.rs` over
//! `flint_core::profile` (ADR 0128). The `save_trace` debug key (Shift+F3)
//! saves one mid-play, with or without these flags.

use crate::PlayerApp;

/// The trace flags, shared by `flint play` and `flint-player`
#[derive(clap::Args, Clone, Debug, Default)]
pub struct TraceArgs {
    /// Record the engine's scopes and write the last --trace-seconds of
    /// them to FILE when the run ends (#1097): Chrome trace-event JSON with
    /// a track per thread, the frames, the phases, the GPU's passes, the
    /// hitches and the engine's notes, which Perfetto (ui.perfetto.dev) and
    /// chrome://tracing open. Needs the recorder in the build: every debug
    /// build has it, a release build with --features trace. Desktop only
    #[arg(long, value_name = "FILE")]
    pub trace: Option<std::path::PathBuf>,

    /// How far back the recorder holds, in seconds (default 5): each
    /// thread's ring is sized for it, 32 768 scopes a second (1.3 MB a
    /// second a thread). 0 keeps the whole session, which grows without
    /// bound (about 112 bytes of JSON a scope). Starts the recorder
    #[arg(long, value_name = "SECS")]
    pub trace_seconds: Option<f64>,

    /// Write a trace eight frames after each of the run's first N hitches
    /// past the start (default 1), beside --trace's file or in `traces/` at
    /// the project's root. Hitches in the first 2 s of game time are passed
    /// over, as the exit report and --max-hitches set them apart. Times
    /// every script callback, as --hitch-log does
    #[arg(
        long,
        value_name = "N",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "1"
    )]
    pub trace_on_hitch: Option<u32>,
}

impl TraceArgs {
    /// Whether any flag asks the recorder for something.
    pub fn wanted(&self) -> bool {
        self.trace.is_some()
            || self.trace_seconds.is_some()
            || self.trace_on_hitch.is_some_and(|n| n > 0)
    }

    /// Start the recorder on `app` as the flags say, before `run_app`. A
    /// build without the recorder, the web, or a look-back that isn't a
    /// number of seconds is refused here, before a window opens.
    pub fn apply(&self, app: &mut PlayerApp) -> anyhow::Result<()> {
        if !self.wanted() {
            return Ok(());
        }
        let seconds = self
            .trace_seconds
            .unwrap_or(crate::player_app::trace::DEFAULT_SECONDS);
        if !seconds.is_finite() || seconds < 0.0 {
            anyhow::bail!(
                "--trace-seconds {seconds}: seconds, 0 or more (0 keeps the whole session)"
            );
        }
        if !flint_core::profile::COMPILED_IN {
            anyhow::bail!(
                "--trace: this build has no scope recorder. Every debug build has one; \
                 a release build needs --features trace (ADR 0128)"
            );
        }
        if cfg!(target_arch = "wasm32") {
            anyhow::bail!(
                "--trace is desktop only: on the web, ?perf=1's Save trace button offers one"
            );
        }
        if seconds == 0.0 {
            tracing::info!(
                target: "player",
                "[trace] --trace-seconds 0: recording the whole session, which grows without bound"
            );
        }
        app.trace_scopes(crate::player_app::trace::TraceSetup {
            exit_file: self.trace.clone(),
            seconds,
            on_hitch: self.trace_on_hitch.unwrap_or(0),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        trace: TraceArgs,
        #[arg(default_value = "")]
        scene: String,
    }

    #[test]
    fn the_flags_parse() {
        let cli = Cli::try_parse_from(["x"]).unwrap();
        assert!(!cli.trace.wanted());
        let cli = Cli::try_parse_from([
            "x",
            "--trace",
            "out.json",
            "--trace-seconds",
            "2.5",
            "--trace-on-hitch",
            "a.scene.toml",
        ])
        .unwrap();
        assert_eq!(
            cli.trace.trace.as_deref(),
            Some(std::path::Path::new("out.json"))
        );
        assert_eq!(cli.trace.trace_seconds, Some(2.5));
        assert_eq!(cli.trace.trace_on_hitch, Some(1));
        assert_eq!(
            cli.scene, "a.scene.toml",
            "a bare --trace-on-hitch takes no scene"
        );
        assert!(cli.trace.wanted());
        let cli = Cli::try_parse_from(["x", "--trace-on-hitch=3"]).unwrap();
        assert_eq!(cli.trace.trace_on_hitch, Some(3));
        let cli = Cli::try_parse_from(["x", "--trace-on-hitch=0"]).unwrap();
        assert!(!cli.trace.wanted(), "none of the hitches asks for nothing");
    }
}
