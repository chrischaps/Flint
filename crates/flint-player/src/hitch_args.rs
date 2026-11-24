//! The hitch flags (#1079): a log of every hitch as it happens, and a gate
//! that fails a scripted run with too many. `flint play` and the standalone
//! `flint-player` flatten the same flags, as they do `WindowArgs`.
//!
//! What a hitch is, and why it was one, is the frame history's and the
//! hitch detector's (#1076, #1077; `player_app/hitch.rs`). These flags only
//! say where the record goes and when a run fails.

use crate::PlayerApp;

/// Which hitches a gate counts: the player's own work (a script, physics,
/// the renderer), time spent elsewhere (the compositor, the driver, the
/// present wait), both, or a scene loaded behind a script's cover (#1125),
/// which the other three leave out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum HitchClass {
    #[default]
    All,
    Player,
    Elsewhere,
    Loading,
}

impl HitchClass {
    /// The words a report says it with: `` (all), `player `, `elsewhere `,
    /// `loading `.
    pub(crate) fn words(self) -> &'static str {
        match self {
            HitchClass::All => "",
            HitchClass::Player => "player ",
            HitchClass::Elsewhere => "elsewhere ",
            HitchClass::Loading => "loading ",
        }
    }
}

/// When a run fails for its hitches: more than `max` after the start
/// (game time past [`STARTUP_SECS`]), or more than `max_startup` before it.
/// `None` is no limit. `of` narrows both to one class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HitchGate {
    pub max: Option<u64>,
    pub max_startup: Option<u64>,
    pub of: HitchClass,
}

/// Game seconds counted as the start: loading's last uploads and the first
/// pipeline compiles. Hitches before it are counted apart and not held
/// against `--max-hitches`. The same two seconds `--exit-after`'s `[perf]`
/// averages leave out.
pub const STARTUP_SECS: f64 = 2.0;

/// The hitch flags, shared by `flint play` and `flint-player`
#[derive(clap::Args, Clone, Debug, Default)]
pub struct HitchArgs {
    /// Write a JSON line for every hitch to FILE as the run goes (#1079):
    /// its frame, game and wall time, ms, the median it stood out from,
    /// missed refreshes, player or elsewhere, the worst phase and script,
    /// the engine's notes and the eight frames either side. Times every
    /// script callback, as --perf does. Each hitch is also marked on a
    /// sampling profiler's timeline (#1250): samply's marker-<pid>.txt
    /// beside FILE on Linux, ETW events from the Flint.Player provider on
    /// Windows. Desktop only
    #[arg(long, value_name = "FILE")]
    pub hitch_log: Option<std::path::PathBuf>,

    /// Exit non-zero when the run has more than N hitches after its first
    /// two seconds of game time (#1079), for --input-script and
    /// --exit-after runs. A wall-clock gate: a busy machine hitches too
    #[arg(long, value_name = "N")]
    pub max_hitches: Option<u64>,

    /// Exit non-zero when the run's first two seconds (loading, pipeline
    /// warm-up) have more than N hitches. Without it they are counted and
    /// reported but never fail a run
    #[arg(long, value_name = "N")]
    pub max_hitches_startup: Option<u64>,

    /// Which hitches the limits count: `player` (the engine's own work),
    /// `elsewhere` (the compositor, the driver, the present wait), `all`
    /// (both), or `loading`: a scene loaded behind a script's fade, which
    /// the others leave out (#1125)
    #[arg(long, value_name = "CLASS", value_enum, default_value_t = HitchClass::All)]
    pub max_hitches_of: HitchClass,
}

impl HitchArgs {
    /// The gate the flags ask for, if any.
    pub fn gate(&self) -> Option<HitchGate> {
        (self.max_hitches.is_some() || self.max_hitches_startup.is_some()).then_some(HitchGate {
            max: self.max_hitches,
            max_startup: self.max_hitches_startup,
            of: self.max_hitches_of,
        })
    }

    /// Open the log and set the gate on `app`, before `run_app`. A log
    /// that can't be created fails here, before a window opens.
    pub fn apply(&self, app: &mut PlayerApp) -> anyhow::Result<()> {
        if let Some(path) = &self.hitch_log {
            #[cfg(not(target_arch = "wasm32"))]
            {
                use anyhow::Context;
                app.log_hitches_to(path)
                    .with_context(|| format!("--hitch-log {}", path.display()))?;
            }
            #[cfg(target_arch = "wasm32")]
            anyhow::bail!("--hitch-log {}: no files on the web", path.display());
        }
        if let Some(gate) = self.gate() {
            app.gate_hitches(gate);
        }
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
        hitches: HitchArgs,
    }

    #[test]
    fn the_flags_parse_into_a_gate() {
        let cli = Cli::try_parse_from(["x"]).unwrap();
        assert_eq!(cli.hitches.gate(), None);
        assert_eq!(cli.hitches.hitch_log, None);
        let cli = Cli::try_parse_from([
            "x",
            "--hitch-log",
            "h.jsonl",
            "--max-hitches",
            "0",
            "--max-hitches-of",
            "player",
        ])
        .unwrap();
        assert_eq!(
            cli.hitches.gate(),
            Some(HitchGate {
                max: Some(0),
                max_startup: None,
                of: HitchClass::Player
            })
        );
        assert_eq!(
            cli.hitches.hitch_log.as_deref(),
            Some(std::path::Path::new("h.jsonl"))
        );
        let cli = Cli::try_parse_from(["x", "--max-hitches-startup", "2"]).unwrap();
        assert_eq!(cli.hitches.gate().unwrap().max, None);
        assert_eq!(cli.hitches.gate().unwrap().max_startup, Some(2));
        assert!(Cli::try_parse_from(["x", "--max-hitches-of", "gpu"]).is_err());
        let cli = Cli::try_parse_from(["x", "--max-hitches", "0", "--max-hitches-of", "loading"])
            .unwrap();
        assert_eq!(cli.hitches.gate().unwrap().of, HitchClass::Loading);
    }
}
