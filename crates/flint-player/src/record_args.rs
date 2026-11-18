//! `--record DIR --record-fps N` (#583): the window's frames as shown,
//! written to a folder at a chosen rate while the game plays. `flint play`
//! and `flint-player` flatten the same flags. How a frame is read back
//! without the frame waiting for it is `player_app/frame_grab.rs`.

use crate::PlayerApp;

/// The recording flags, shared by `flint play` and `flint-player`
#[derive(clap::Args, Clone, Debug)]
pub struct RecordArgs {
    /// Record the window's frames, scene and HUD as shown, into this folder
    /// (#583): `frame_000001.png`..., `frames.jsonl` (each frame's game,
    /// wall and Unix time) and, at exit, `frames.ffconcat` for `ffmpeg -f
    /// concat -i frames.ffconcat out.mp4` at the times they were shown. The
    /// frame never waits for a read-back, so the scripts' clock is the same
    /// recorded or not. Made if missing; refused if it holds a recording.
    /// Desktop only
    #[arg(long, value_name = "DIR")]
    pub record: Option<std::path::PathBuf>,

    /// Frames a second for --record, on the wall clock (on game time under
    /// --frame-step, so a stepped run records the same frames every time)
    #[arg(long, value_name = "FPS", default_value_t = 8.0, requires = "record")]
    pub record_fps: f64,
}

impl Default for RecordArgs {
    fn default() -> Self {
        Self {
            record: None,
            record_fps: 8.0,
        }
    }
}

impl RecordArgs {
    /// Start the recording on `app`, before `run_app`: the folder made and
    /// checked and the rate refused here, before a window opens.
    pub fn apply(&self, app: &mut PlayerApp) -> anyhow::Result<()> {
        let Some(dir) = &self.record else {
            return Ok(());
        };
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (dir, app);
            anyhow::bail!("--record is desktop only: the web player writes no files");
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::player_app::frame_grab::check_record_dir(dir, self.record_fps)?;
            app.record_frames(dir.clone(), self.record_fps);
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
        record: RecordArgs,
        #[arg(default_value = "")]
        scene: String,
    }

    #[test]
    fn the_flags_parse() {
        let cli = Cli::try_parse_from(["x"]).unwrap();
        assert!(cli.record.record.is_none());
        let cli = Cli::try_parse_from(["x", "--record", "out", "a.scene.toml"]).unwrap();
        assert_eq!(
            cli.record.record.as_deref(),
            Some(std::path::Path::new("out"))
        );
        assert_eq!(cli.record.record_fps, 8.0, "8 a second unless asked");
        assert_eq!(cli.scene, "a.scene.toml");
        let cli = Cli::try_parse_from(["x", "--record", "out", "--record-fps", "24"]).unwrap();
        assert_eq!(cli.record.record_fps, 24.0);
        assert!(
            Cli::try_parse_from(["x", "--record-fps", "24"]).is_err(),
            "a rate needs a recording"
        );
    }
}
