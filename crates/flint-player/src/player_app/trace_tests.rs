#![allow(clippy::disallowed_methods)] // reads back the traces the run wrote to the disk
//! #1097, end to end through the player's own frames: a scene whose script
//! stalls 150 ms at t = 1 and 3 s, played headless on a stepped clock with
//! `--trace FILE --trace-on-hitch`. The hitch's trace holds the stalled
//! frame, with the script's callback a scope spanning it; the startup
//! stall is passed over; the exit trace is written too. Each frame ends as
//! the player's `RedrawRequested` ends it: `finish_frame`, then the saves.
//!
//! No GPU: the stall is the script's, and the GPU track stays empty. The
//! detector's floor is raised to 60 ms, as in the hitch log's tests, so a
//! busy machine's own hiccups don't count.

use super::scene_entry::SceneEntry;
use super::trace::TraceSetup;
use super::PlayerApp;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use serde_json::Value;
use std::sync::Arc;

const STEP: f64 = 1.0 / 60.0;

/// Stalls 150 ms in a helper on the frame game time crosses 1.01 s (the
/// start) and 3.01 s, and calls an engine function ten times a frame: what
/// #1100 puts inside the callback's scope.
const STALL: &str = r#"
fn stall(seconds) {
    let start = timestamp();
    while start.elapsed < seconds { }
}

fn on_update() {
    let t = total_time();
    let before = t - delta_time();
    let x = 0.0;
    for i in 0..10 { x += clamp(i * 0.1, 0.0, 0.5); }
    for mark in [1.01, 3.01] {
        if before < mark && t >= mark {
            stall(0.15);
        }
    }
}
"#;

fn game() -> MemorySource {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert(
        "game/scenes/a.scene.toml",
        "[scene]\nname = \"A\"\n\n[entities.controller]\n[entities.controller.script]\nsource = \"stall.rhai\"\n",
    );
    files.insert("game/scripts/stall.rhai", STALL.to_string());
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
            packages: None,
        },
    );
    app.enter_scene(SceneEntry::Start)
        .expect("the scene starts");
    app.core.clock.frame_step = Some(STEP);
    app.stats.hitches.thresholds.floor_ms = 60.0;
    app
}

/// One frame as the window's `RedrawRequested` runs it, without drawing.
fn frame(app: &mut PlayerApp) {
    app.tick();
    flint_core::profile::finish_frame();
    app.trace_frame_end();
}

fn events(trace: &Value) -> &Vec<Value> {
    trace["traceEvents"].as_array().expect("traceEvents")
}

/// The track (tid) named `name`.
fn track(trace: &Value, name: &str) -> Option<i64> {
    events(trace)
        .iter()
        .find(|e| e["ph"] == "M" && e["name"] == "thread_name" && e["args"]["name"] == name)
        .and_then(|e| e["tid"].as_i64())
}

fn span(e: &Value) -> (f64, f64) {
    let ts = e["ts"].as_f64().unwrap();
    (ts, ts + e["dur"].as_f64().unwrap_or(0.0))
}

#[test]
fn a_hitchs_trace_holds_the_stalled_frame_under_its_scripts_scope() {
    // It records and ends frames: no other test may meanwhile
    let _turn = super::trace::recorder_test_turn();
    let dir = std::env::temp_dir().join(format!("flint-trace-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let exit_file = dir.join("run.json");
    vfs::scoped(Arc::new(game()), || {
        let mut app = started();
        app.trace_scopes(TraceSetup {
            exit_file: Some(exit_file.clone()),
            seconds: 5.0,
            on_hitch: 1,
        });
        while app.core.clock.total_time < 3.5 {
            frame(&mut app);
        }
        app.finish_traces();
        flint_core::profile::set_recording(false);
        assert_eq!(app.trace_failure(), None);
    });

    let mut hitch_files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("run-hitch-t")
        })
        .collect();
    hitch_files.sort();
    assert_eq!(
        hitch_files.len(),
        1,
        "one hitch past the start, the startup stall passed over: {hitch_files:?}"
    );
    let name = hitch_files[0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(
        name.starts_with("run-hitch-t3.0"),
        "named for the hitch's time: {name}"
    );

    let trace: Value = serde_json::from_str(&std::fs::read_to_string(&hitch_files[0]).unwrap())
        .expect("the trace is JSON");
    let header = &trace["otherData"];
    assert_eq!(header["format"], "flint-trace");
    assert!(
        header["reason"]
            .as_str()
            .unwrap()
            .starts_with("hitch at t=3.0"),
        "{header}"
    );
    assert!(
        header["reason"]
            .as_str()
            .unwrap()
            .contains("stall.rhai:on_update"),
        "the reason names the script: {header}"
    );
    assert_eq!(header["seconds"], 5.0);

    // The script's callback is a scope of its own, and the stalled one
    // took the stall
    let stalled = events(&trace)
        .iter()
        .find(|e| e["name"] == "stall.rhai:on_update" && e["dur"].as_f64().unwrap() >= 150_000.0)
        .unwrap_or_else(|| panic!("no 150 ms stall.rhai:on_update scope: {header}"));
    let (s0, s1) = span(stalled);
    // ... inside the `tick` of its frame, on the same thread
    let tick = events(&trace)
        .iter()
        .find(|e| {
            e["name"] == "tick"
                && e["tid"] == stalled["tid"]
                && e["args"]["frame"] == stalled["args"]["frame"]
        })
        .expect("its frame's tick");
    let (t0, t1) = span(tick);
    assert!(
        t0 <= s0 && s1 <= t1,
        "tick {t0}..{t1} holds the script {s0}..{s1}"
    );

    // Inside the callback (#1100): the helper that took the stall, and
    // each call of the engine function, all in the callback's span
    let in_stalled_frame = |name: &str| -> Vec<&Value> {
        events(&trace)
            .iter()
            .filter(|e| {
                e["name"] == name
                    && e["tid"] == stalled["tid"]
                    && e["args"]["frame"] == stalled["args"]["frame"]
            })
            .collect()
    };
    let helper = in_stalled_frame("stall.rhai:stall()");
    assert_eq!(helper.len(), 1, "the helper, once: {header}");
    let (h0, h1) = span(helper[0]);
    assert!(
        s0 <= h0 && h1 <= s1,
        "on_update {s0}..{s1} holds stall() {h0}..{h1}"
    );
    assert!(h1 - h0 >= 150_000.0, "stall() took the stall: {h0}..{h1}");
    // The engine function's ten calls are one summed record (#1288), a
    // level inside the callback, laid from the first call for their total
    let clamps = in_stalled_frame("clamp()");
    assert_eq!(
        clamps.len(),
        1,
        "ten engine calls are one record: {clamps:?}"
    );
    let clamp = clamps[0];
    assert_eq!(clamp["args"]["calls"], 10, "{clamp}");
    assert_eq!(
        clamp["args"]["depth"].as_i64().unwrap(),
        stalled["args"]["depth"].as_i64().unwrap() + 1,
        "one level inside on_update: {clamp}"
    );
    let worst = clamp["args"]["worst_us"].as_f64().unwrap();
    let (c0, c1) = span(clamp);
    assert!(
        worst <= c1 - c0,
        "the longest call is in the total: {clamp}"
    );
    assert!(s0 <= c0 && c1 <= s1, "on_update holds clamp() {c0}..{c1}");

    // The header says the scopes cover the whole look-back
    assert!(header.get("covered_from_ms").is_none(), "{header}");
    assert_eq!(header["cut_threads"], "", "said, so no reader guesses");

    // The frames track has the stalled frame, and the hitches track marks
    // it, both spanning the script's scope (to a millisecond: the frame
    // history's clock is `Instant`, the scopes' the TSC)
    const SLACK: f64 = 1000.0;
    let frames = track(&trace, "frames").expect("a frames track");
    let long_frame = events(&trace)
        .iter()
        .find(|e| {
            e["tid"] == frames && e["ph"] == "X" && e["args"]["frame"] == stalled["args"]["frame"]
        })
        .expect("the stalled frame on the frames track");
    let (f0, f1) = span(long_frame);
    assert!(
        f0 - SLACK <= s0 && s1 <= f1 + SLACK,
        "frame {f0}..{f1}, script {s0}..{s1}"
    );
    assert!(long_frame["args"]["ms"].as_f64().unwrap() >= 150.0);
    let hitches = track(&trace, "hitches").expect("a hitches track");
    let hitch = events(&trace)
        .iter()
        .find(|e| e["tid"] == hitches && e["name"] == "hitch")
        .expect("the hitch is marked");
    let (h0, h1) = span(hitch);
    assert!(
        h0 - SLACK <= s0 && s1 <= h1 + SLACK,
        "hitch {h0}..{h1}, script {s0}..{s1}"
    );
    assert!(
        hitch["args"]["cause"]
            .as_str()
            .unwrap()
            .contains("stall.rhai:on_update"),
        "{hitch}"
    );
    // The phases track has the scripts' phase of that frame
    let phases = track(&trace, "phases").expect("a phases track");
    assert!(events(&trace).iter().any(|e| e["tid"] == phases
        && e["name"] == "scripts"
        && e["args"]["frame"] == stalled["args"]["frame"]
        && e["dur"].as_f64().unwrap() >= 150_000.0));

    // The frames after it are there too: it waited eight
    let after = events(&trace)
        .iter()
        .filter(|e| e["tid"] == frames && e["ph"] == "X" && span(e).0 > f1 - SLACK)
        .count();
    assert!(after >= 7, "the frames after the hitch: {after}");

    // The exit trace is written, and covers the same stall
    let exit: Value = serde_json::from_str(&std::fs::read_to_string(&exit_file).unwrap()).unwrap();
    assert_eq!(exit["otherData"]["reason"], "exit");
    assert!(events(&exit)
        .iter()
        .any(|e| e["name"] == "stall.rhai:on_update" && e["dur"].as_f64().unwrap() >= 150_000.0));
    let _ = std::fs::remove_dir_all(&dir);
}
