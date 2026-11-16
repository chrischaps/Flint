#![allow(clippy::disallowed_methods)] // reads back the log the run wrote to the disk
//! #1079, end to end through the player's own frames: a scene whose script
//! stalls for 150 ms at t = 1, 3, 4 and 5 s, played headless on a stepped
//! clock as `--input-script` + `--exit-after` runs are. The hitch log gets
//! one line per stall naming the script, the first counted as `startup`;
//! the exit report totals them; `--max-hitches 0` fails the run and a clean
//! scene passes it.
//!
//! No GPU: the stalls are the script's. The detector's floor is raised to
//! 60 ms here so a busy machine's own hiccups (several agents build on
//! this one) don't count: the stalls are 150 ms.

use super::scene_entry::SceneEntry;
use super::PlayerApp;
use crate::{HitchClass, HitchGate};
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use std::sync::Arc;

const STEP: f64 = 1.0 / 60.0;

/// Stalls 150 ms on the frame game time crosses each mark: once in the
/// start (1 s), three times after it. Marks sit between steps, so each is
/// crossed by exactly one frame.
const STALL: &str = r#"
fn on_update() {
    let t = total_time();
    let before = t - delta_time();
    for mark in [1.01, 3.01, 4.01, 5.01] {
        if before < mark && t >= mark {
            let start = timestamp();
            while start.elapsed < 0.15 { }
        }
    }
}
"#;

const CALM: &str = "fn on_update() { let t = total_time(); }";

fn game(script: &str) -> MemorySource {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert(
        "game/scenes/a.scene.toml",
        "[scene]\nname = \"A\"\n\n[entities.controller]\n[entities.controller.script]\nsource = \"stall.rhai\"\n",
    );
    files.insert("game/scripts/stall.rhai", script.to_string());
    files
}

fn started() -> PlayerApp {
    let path = "game/scenes/a.scene.toml";
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(path, &registry).expect("the scene loads");
    let mut app = PlayerApp::new(world, path.into(), false, None, None);
    app.apply_scene_settings(&file);
    app.set_schemas(
        registry,
        SchemaDirs {
            dirs: Vec::new(),
            source: SchemaSource::Flags,
            dropped: Vec::new(),
        },
    );
    app.enter_scene(SceneEntry::Start)
        .expect("the scene starts");
    app.core.clock.frame_step = Some(STEP);
    app.stats.hitches.thresholds.floor_ms = 60.0;
    app
}

/// Play until game time passes `secs`, plus the frames a log line waits for.
fn play_to(app: &mut PlayerApp, secs: f64) {
    while app.core.clock.total_time < secs {
        app.tick();
    }
    for _ in 0..super::hitch_watch::AROUND + 2 {
        app.tick();
    }
}

fn log_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("flint-hitch-{}-{name}.jsonl", std::process::id()))
}

#[test]
fn a_stalling_script_leaves_a_line_per_stall_and_fails_the_gate() {
    let log = log_path("stall");
    vfs::scoped(Arc::new(game(STALL)), || {
        let mut app = started();
        app.log_hitches_to(&log).expect("the log opens");
        app.gate_hitches(HitchGate {
            max: Some(0),
            ..Default::default()
        });
        play_to(&mut app, 5.5);
        app.print_exit_report();

        let text = std::fs::read_to_string(&log).unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let stalls: Vec<&serde_json::Value> = lines
            .iter()
            .filter(|l| l["script"] == "stall.rhai:on_update")
            .collect();
        assert_eq!(
            stalls.len(),
            4,
            "one line a stall, the script named: {text}"
        );
        for l in &stalls {
            assert_eq!(l["class"], "player", "{l}");
            assert_eq!(l["phase"], "scripts", "{l}");
            assert!(l["ms"].as_f64().unwrap() >= 150.0, "{l}");
            assert!(l["script_ms"].as_f64().unwrap() >= 150.0, "{l}");
            assert_eq!(l["before_ms"].as_array().unwrap().len(), 8, "{l}");
            assert_eq!(l["after_ms"].as_array().unwrap().len(), 8, "{l}");
            assert!(
                l["cause"]
                    .as_str()
                    .unwrap()
                    .starts_with("script stall.rhai:on_update "),
                "{l}"
            );
        }
        // At the frame each mark was crossed, the first in the start
        let times: Vec<f64> = stalls
            .iter()
            .map(|l| l["game_time"].as_f64().unwrap())
            .collect();
        for (t, mark) in times.iter().zip([1.01, 3.01, 4.01, 5.01]) {
            assert!(*t >= mark && *t < mark + 1.5 * STEP, "{times:?}");
        }
        let startup: Vec<bool> = stalls
            .iter()
            .map(|l| l["startup"].as_bool().unwrap())
            .collect();
        assert_eq!(startup, [true, false, false, false]);

        // The stalls exactly; the totals at least, since a machine busy
        // enough can add a hitch of its own past the 60 ms floor
        let watch = &app.stats.watch;
        let stall = |t: &super::hitch_watch::Tally| {
            t.causes
                .iter()
                .find(|(k, _)| k == "script stall.rhai:on_update")
                .map_or(0, |(_, n)| *n)
        };
        assert_eq!(stall(&watch.run), 3, "{:?}", watch.run);
        assert_eq!(stall(&watch.startup), 1, "{:?}", watch.startup);
        let (run, start) = (watch.run.clone(), watch.startup.clone());
        let report = watch.report();
        assert!(
            report[0].starts_with(&format!("[perf] hitches: {} (", run.hitches)),
            "{report:?}"
        );
        assert!(
            report[0].contains("causes: 3 script stall.rhai:on_update"),
            "{report:?}"
        );
        assert!(
            report[1].starts_with(&format!(
                "[perf] startup hitches (before t=2 s, not gated): {},",
                start.hitches
            )),
            "{report:?}"
        );
        assert!(
            report[2].contains(&format!("({} lines)", lines.len())),
            "{report:?}"
        );

        let why = app.run_failure().expect("--max-hitches 0 fails the run");
        assert!(
            why.starts_with(&format!(
                "{} hitches after the first 2 s, over --max-hitches 0",
                run.hitches
            )),
            "{why}"
        );
        // As many as there were is not over; the start's stall isn't held
        // against it
        app.gate_hitches(HitchGate {
            max: Some(run.hitches),
            ..Default::default()
        });
        assert_eq!(app.run_failure(), None);
        // Unless asked
        app.gate_hitches(HitchGate {
            max: Some(run.hitches),
            max_startup: Some(0),
            of: HitchClass::All,
        });
        assert!(app
            .run_failure()
            .unwrap()
            .contains("--max-hitches-startup 0"));
        // The player's: the stalls are, so a gate on them fails
        app.gate_hitches(HitchGate {
            max: Some(run.player - 1),
            max_startup: None,
            of: HitchClass::Player,
        });
        assert!(app.run_failure().unwrap().contains("player hitches"));
    });
    let _ = std::fs::remove_file(&log);
}

#[test]
fn a_calm_scene_passes_the_gate_with_an_empty_log() {
    let log = log_path("calm");
    vfs::scoped(Arc::new(game(CALM)), || {
        let mut app = started();
        app.log_hitches_to(&log).expect("the log opens");
        app.gate_hitches(HitchGate {
            max: Some(0),
            ..Default::default()
        });
        play_to(&mut app, 3.0);
        app.print_exit_report();
        assert_eq!(app.run_failure(), None);
        assert!(app.stats.watch.report()[0].starts_with("[perf] hitches: 0 in "));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "");
    });
    let _ = std::fs::remove_file(&log);
}
