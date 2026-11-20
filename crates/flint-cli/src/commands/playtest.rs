//! `flint playtest <scene>` (#714): play a scene headless on a fixed clock,
//! with an input script and the scripts' own `test_*` calls, and say
//! whether it passed: one report, the same bytes run to run, exit 1 on a
//! failure.
//!
//! ```bash
//! flint playtest flint-examples/phase4_runtime.scene.toml \
//!     --input-script flint-examples/playtests/phase4_runtime.input --exit-after 6
//! flint playtest scenes/level1.scene.toml --timeout 30 --report out.json
//! flint playtest scenes/level1.scene.toml --strict --dump player --json
//! ```
//!
//! The run is `flint_player::playtest` (the player's frames with no window,
//! GPU or sound device); this command adds the terminal's text, the
//! `--report` file and, under `--strict`, `flint check`'s findings on the
//! scene before it plays, which catch what a run with no renderer can't
//! (a model that isn't there).

use crate::check::{self, Checks, Subject};
use crate::output::{self, say};
use anyhow::{Context, Result};
use flint_player::{PlaytestFailure, PlaytestOptions, PlaytestReport};
use std::path::{Path, PathBuf};

#[derive(clap::Args)]
pub struct PlaytestArgs {
    /// The scene to play
    pub scene: String,

    /// A timed input script: keys, pads, the pointer, and `expect` / `wait`
    /// lines that check the game (the format: flint-player's
    /// `--input-script`)
    #[arg(long, value_name = "FILE")]
    pub input_script: Option<String>,

    /// Game seconds a frame
    #[arg(long, default_value_t = flint_player::PLAYTEST_STEP, value_name = "S")]
    pub frame_step: f64,

    /// `random()`'s seed (default: the player's stepped-run seed)
    #[arg(long)]
    pub seed: Option<u64>,

    /// End at this game time, passing if nothing failed (default 10 when
    /// neither this nor --timeout is given)
    #[arg(long, value_name = "SECONDS")]
    pub exit_after: Option<f64>,

    /// Fail at this game time: the run should have ended itself by then
    /// with test_pass(), test_fail() or quit_game()
    #[arg(long, value_name = "SECONDS")]
    pub timeout: Option<f64>,

    /// A warning while the scene loads fails the run, and so does any
    /// error or warning `flint check` finds in the scene first
    #[arg(long)]
    pub strict: bool,

    /// Put this entity's components at the end in the report (repeatable)
    #[arg(long, value_name = "ENTITY", action = clap::ArgAction::Append)]
    pub dump: Vec<String>,

    /// Write the JSON report to this file as well
    #[arg(long, value_name = "FILE")]
    pub report: Option<PathBuf>,

    /// Put wall-clock times in the report (two runs then differ)
    #[arg(long)]
    pub timings: bool,

    /// The screen scripts see, in points
    #[arg(long, default_value_t = 1280)]
    pub width: u32,
    #[arg(long, default_value_t = 720)]
    pub height: u32,

    /// Schema directories, later ones overriding earlier (repeatable).
    /// Without it: the project's flint.toml `schemas`, else discovery
    #[arg(long, action = clap::ArgAction::Append)]
    pub schemas: Vec<String>,
}

/// `flint check` on the scene, under `--strict`: its findings, and a
/// failure for each error or warning.
fn preflight(args: &PlaytestArgs, report: &mut PlaytestReport) -> Result<serde_json::Value> {
    let mut subject = Subject::scene(Path::new(&args.scene), &args.schemas);
    subject.strict = true;
    subject.script_api = Some(check::scripts::ScriptApiArgs::default().api()?);
    let found = check::run(&subject, Checks::ALL);
    for d in &found.diagnostics {
        report.failures.push(PlaytestFailure::new(
            "preflight",
            format!("{}[{}]: {}", d.severity, d.code, d.message),
            d.file
                .as_ref()
                .map(|f| f.to_string_lossy().replace('\\', "/")),
            d.line,
        ));
    }
    report.passed = report.failures.is_empty();
    Ok(check::report::json(&found))
}

fn summary(report: &PlaytestReport) -> String {
    let mut out = format!(
        "{} {}: ended by {} at {:.3} s, {} frames\n",
        if report.passed { "PASS" } else { "FAIL" },
        report.scene,
        serde_json::to_value(report.ended)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default(),
        report.game_time,
        report.frames
    );
    let held = report.checks.iter().filter(|c| c.passed).count();
    let failed_places = report.assertions.iter().filter(|a| a.failed > 0).count();
    out.push_str(&format!(
        "  checks {held} of {} held; assertions {} places, {failed_places} failed; script errors {}; warnings {} at load, {} while running{}\n",
        report.checks.len(),
        report.assertions.len(),
        report.script_errors.len(),
        report.warnings.at_load,
        report.warnings.while_running,
        if report.warnings.counted { "" } else { " (not counted)" }
    ));
    for line in report.failure_lines() {
        out.push_str(&format!("  {line}\n"));
    }
    out
}

pub fn run(args: PlaytestArgs) -> Result<()> {
    let mut options = PlaytestOptions::new(args.scene.clone());
    options.schemas = args.schemas.clone();
    options.input_script = args.input_script.clone();
    options.frame_step = args.frame_step;
    options.seed = args.seed;
    options.exit_after = args.exit_after;
    options.timeout = args.timeout;
    options.strict = args.strict;
    options.dump = args.dump.clone();
    options.timings = args.timings;
    options.screen = (args.width as f32, args.height as f32);

    output::progress!("playtest: {}", args.scene);
    let mut report = flint_player::playtest(&options);
    let mut document = serde_json::to_value(&report)?;
    if args.strict {
        let found = preflight(&args, &mut report)?;
        document = serde_json::to_value(&report)?;
        document["preflight"] = found;
    }
    let text = serde_json::to_string_pretty(&document)?;
    if let Some(path) = &args.report {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("--report: cannot create {}", parent.display()))?;
        }
        std::fs::write(path, format!("{text}\n"))
            .with_context(|| format!("--report: cannot write {}", path.display()))?;
    }
    if output::json() {
        output::document_text(&text);
    } else {
        say!("{}", summary(&report).trim_end());
        if let Some(path) = &args.report {
            say!("  report: {}", flint_core::project::shown(path));
        }
    }
    if report.passed {
        Ok(())
    } else {
        Err(output::failed())
    }
}
