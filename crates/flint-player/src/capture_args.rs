//! The frame capture flags (#1086): capture a frame for the frame debugger
//! at a game time or on a hitch, into a folder. `flint play` and the
//! standalone `flint-player` flatten the same flags, as they do `HitchArgs`.
//!
//! What a capture holds and how it is taken is `player_app/frame_capture.rs`
//! (ADR 0126). The debug key (Shift+F2) and an input script's `capture
//! frame` line take one too, with or without these flags.

use crate::PlayerApp;

/// The frame capture flags, shared by `flint play` and `flint-player`
#[derive(clap::Args, Clone, Debug, Default)]
pub struct FrameCaptureArgs {
    /// Capture the frame at this game time for the frame debugger (#1086):
    /// every pass's image, its draws and timings, the frame history around
    /// it and what a replay needs, as `<scene>-t<secs>.flintframe/` under
    /// --capture-dir. Repeatable; deterministic with --frame-step. Desktop only
    #[arg(long, value_name = "SECS", action = clap::ArgAction::Append)]
    pub capture_frame_at: Vec<f64>,

    /// Capture the frame after each of the run's first N hitches past the
    /// start (default 1). Hitches in the first 2 s of game time (loading,
    /// first compiles) are passed over, as the exit report and
    /// --max-hitches set them apart; --capture-on-hitch-startup takes them.
    /// A hitch is over by the time it is found, so the capture is the next
    /// frame, with the frame history around the hitch (its cost by phase,
    /// its cause) in the manifest's `cpu`. Times every script callback, as
    /// --hitch-log does
    #[arg(
        long,
        value_name = "N",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "1"
    )]
    pub capture_on_hitch: Option<u32>,

    /// Let --capture-on-hitch take the hitches of the first 2 s of game
    /// time too, when it's the load itself being chased
    #[arg(long, requires = "capture_on_hitch")]
    pub capture_on_hitch_startup: bool,

    /// Where frame captures go. Default: `captures/` at the project's root
    #[arg(long, value_name = "DIR")]
    pub capture_dir: Option<std::path::PathBuf>,
}

impl FrameCaptureArgs {
    /// Set the captures up on `app`, before `run_app`. A negative or
    /// not-a-number time is refused here, before a window opens.
    pub fn apply(&self, app: &mut PlayerApp) -> anyhow::Result<()> {
        if let Some(t) = self
            .capture_frame_at
            .iter()
            .find(|t| !t.is_finite() || **t < 0.0)
        {
            anyhow::bail!("--capture-frame-at {t}: a game time in seconds, 0 or more");
        }
        let wanted = !self.capture_frame_at.is_empty()
            || self.capture_on_hitch.is_some_and(|n| n > 0)
            || self.capture_dir.is_some();
        if wanted && cfg!(target_arch = "wasm32") {
            anyhow::bail!("frame captures are desktop only: the web player writes no files");
        }
        let mut times = self.capture_frame_at.clone();
        times.sort_by(f64::total_cmp);
        app.capture_frames(
            times,
            self.capture_on_hitch.unwrap_or(0),
            self.capture_dir.clone(),
        );
        app.capture_startup_hitches(self.capture_on_hitch_startup);
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
        captures: FrameCaptureArgs,
        #[arg(default_value = "")]
        scene: String,
    }

    #[test]
    fn the_flags_parse() {
        let cli = Cli::try_parse_from(["x"]).unwrap();
        assert!(cli.captures.capture_frame_at.is_empty());
        assert_eq!(cli.captures.capture_on_hitch, None);
        let cli = Cli::try_parse_from([
            "x",
            "--capture-frame-at",
            "2",
            "--capture-frame-at",
            "0.5",
            "--capture-on-hitch",
            "--capture-dir",
            "out",
            "a.scene.toml",
        ])
        .unwrap();
        assert_eq!(cli.captures.capture_frame_at, vec![2.0, 0.5]);
        assert_eq!(cli.captures.capture_on_hitch, Some(1));
        assert_eq!(
            cli.scene, "a.scene.toml",
            "a bare --capture-on-hitch takes no scene"
        );
        assert!(!cli.captures.capture_on_hitch_startup, "off by default");
        let cli = Cli::try_parse_from(["x", "--capture-on-hitch=3"]).unwrap();
        assert_eq!(cli.captures.capture_on_hitch, Some(3));
        let cli =
            Cli::try_parse_from(["x", "--capture-on-hitch", "--capture-on-hitch-startup"]).unwrap();
        assert!(cli.captures.capture_on_hitch_startup);
        assert!(
            Cli::try_parse_from(["x", "--capture-on-hitch-startup"]).is_err(),
            "it says which hitches --capture-on-hitch takes, so it needs it"
        );
    }
}
