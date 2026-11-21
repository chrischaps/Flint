//! #526's first box, with the real binary: `flint play <scene> --exit-after 3`
//! ends on its own, exit 0, and says the exit report `flint-player` says (the
//! `[perf]` lines, target `report`). That the two take one set of flags and
//! build one player is `commands::play::tests`; this is the window opening
//! and closing again with nobody there.
//!
//! The player window is small and `--no-activate`, so it never takes the
//! keyboard. Skipped, said on stderr, on a machine with no GPU adapter.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

/// Long enough for a cold debug load on a busy machine; a run that reaches
/// it never ended on its own, which is the failure this file is about
const DEADLINE: Duration = Duration::from_secs(240);

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("play_cli_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("flint runs")
}

fn has_gpu(dir: &Path) -> bool {
    let out = flint(dir, &["render", "--list-gpus", "--json"]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    let any = v["adapters"].as_array().is_some_and(|a| !a.is_empty());
    if !any {
        eprintln!("no GPU adapter: play_cli is skipped");
    }
    any
}

/// `flint <args>` in `dir`, killed and failed past [`DEADLINE`]
fn flint_until_it_ends(dir: &Path, args: &[&str]) -> Output {
    let mut child = Command::new(FLINT)
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("flint runs");
    // Drain both pipes as it runs, so a chatty run can't fill one and stall
    let out = child.stdout.take().unwrap();
    let err = child.stderr.take().unwrap();
    let read = |mut pipe: Box<dyn std::io::Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).unwrap();
            bytes
        })
    };
    let (out, err) = (read(Box::new(out)), read(Box::new(err)));
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() > DEADLINE {
            let _ = child.kill();
            let _ = child.wait();
            let err = String::from_utf8_lossy(&err.join().unwrap()).into_owned();
            panic!(
                "flint {} didn't end on its own in {DEADLINE:?}\n{err}",
                args.join(" ")
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Output {
        status,
        stdout: out.join().unwrap(),
        stderr: err.join().unwrap(),
    }
}

#[test]
fn flint_play_exits_after_three_seconds_with_the_players_report() {
    let top = scratch("exit_after");
    if !has_gpu(&top) {
        return;
    }
    let init = flint(&top, &["init", "game"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let game = top.join("game");

    let run = flint_until_it_ends(
        &game,
        &[
            "play",
            "scenes/main.scene.toml",
            "--exit-after",
            "3",
            "--frame-step",
            "0.0166667",
            "--screenshot",
            "shot.png",
            "--screenshot-at",
            "2.9",
            "--width",
            "192",
            "--height",
            "108",
            "--music-volume",
            "0",
            "--sfx-volume",
            "0",
            "--no-activate",
            "--log-format",
            "json",
        ],
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "flint play failed:\n{stderr}");

    // One JSON object a line; the exit report is target `report`
    let report: Vec<Value> = stderr
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["target"] == "report")
        .collect();
    let said = |prefix: &str| {
        report
            .iter()
            .find(|v| v["message"].as_str().is_some_and(|m| m.starts_with(prefix)))
            .unwrap_or_else(|| panic!("no `{prefix}` line in the report:\n{stderr}"))
    };

    // Stepped at 1/60 s, the frames from t = 2 s to 3 s are 61 every run
    // (flint-player says the same of this scene with these flags)
    let average = report
        .iter()
        .find(|v| v["frames"].is_number())
        .unwrap_or_else(|| panic!("no frame-count line in the report:\n{stderr}"));
    assert!(
        average["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("[perf] ")),
        "{average}"
    );
    assert_eq!(average["frames"], 61, "{average}");
    assert_eq!(average["fps"], 60.0, "{average}");
    for line in [
        "[perf] dt:",
        "[perf] present to present:",
        "[perf] avg per frame:",
        "[perf] hitches:",
        "[script] no errors",
    ] {
        said(line);
    }

    // A capture flag beside --exit-after does its part too
    let shot = image::open(game.join("shot.png")).expect("the screenshot is written");
    assert_eq!((shot.width(), shot.height()), (192, 108));
}

/// #1093: `--perf-report` writes the exit report as JSON, and its numbers
/// are the printed `[perf]` lines' numbers
#[test]
fn a_perf_report_holds_the_numbers_the_lines_print() {
    let top = scratch("perf_report");
    if !has_gpu(&top) {
        return;
    }
    let init = flint(&top, &["init", "game"]);
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let game = top.join("game");

    // Refused without --exit-after, before a window opens
    let refused = flint(
        &game,
        &["play", "scenes/main.scene.toml", "--perf-report", "r.json"],
    );
    assert!(!refused.status.success());
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(said.contains("needs --exit-after"), "{said}");

    let run = flint_until_it_ends(
        &game,
        &[
            "play",
            "scenes/main.scene.toml",
            "--exit-after",
            "3",
            "--frame-step",
            "0.0166667",
            "--perf-report",
            "out/report.json",
            "--width",
            "192",
            "--height",
            "108",
            "--music-volume",
            "0",
            "--sfx-volume",
            "0",
            "--no-activate",
            "--log-format",
            "json",
        ],
    );
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run.status.success(), "flint play failed:\n{stderr}");
    let lines: Vec<String> = stderr
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["target"] == "report")
        .filter_map(|v| v["message"].as_str().map(str::to_string))
        .collect();
    let line = |prefix: &str| {
        lines
            .iter()
            .find(|m| m.starts_with(prefix))
            .unwrap_or_else(|| panic!("no `{prefix}` line:\n{stderr}"))
            .clone()
    };

    let text = std::fs::read_to_string(game.join("out/report.json")).expect("the report");
    let r: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(r["format"], "flint-perf-report");
    assert_eq!(r["written_by"], "flint-player");
    assert_eq!(r["run"]["frames"], 61, "{text}");
    assert!(line("[perf] 61 frames").contains(&format!(
        "avg {:.2} ms/frame",
        r["run"]["mean_ms"].as_f64().unwrap()
    )));

    // Each spread as printed, from the report's numbers
    let spread = |key: &str| {
        let s = &r[key];
        let f = |k: &str| s[k].as_f64().unwrap();
        format!(
            "p50 {:.1} p90 {:.1} p99 {:.1} max {:.1} ms, {} of {} over 1.5x p50",
            f("p50_ms"),
            f("p90_ms"),
            f("p99_ms"),
            f("max_ms"),
            s["lumps"],
            s["frames"]
        )
    };
    assert_eq!(line("[perf] dt:"), format!("[perf] dt: {}", spread("dt")));
    assert_eq!(
        line("[perf] present to present:"),
        format!("[perf] present to present: {}", spread("frame"))
    );
    assert!(line("[perf] avg per frame:").contains(&format!(
        "{:.0} draw calls",
        r["draws"]["draw_calls"].as_f64().unwrap()
    )));
    // The renderer's GPU objects a frame (#1091)
    let objects = &r["gpu_objects"];
    assert!(line("[perf] GPU objects a frame:").contains(&format!(
        "a frame: {:.2} (",
        objects["created"].as_f64().unwrap()
    )));
    assert!(line("[perf] GPU objects a frame:")
        .contains(&format!("in {} of 61 frames", objects["frames_creating"])));
    // GPU timing is on for a report, where the adapter has timestamps
    if let Some(gpu) = r["gpu"].as_object() {
        if gpu["timed_frames"].as_u64().unwrap_or(0) > 0 {
            assert!(line("[perf] avg GPU").contains(&format!(
                "avg GPU {:.2} ms/frame over {} timed frames",
                gpu["mean_ms"].as_f64().unwrap(),
                gpu["timed_frames"]
            )));
        }
    }
    // The run's phases, every one named
    let phases = r["cpu"]["phases"].as_array().unwrap();
    assert_eq!(phases.len(), 10, "{text}");
    assert!(r["cpu"]["frames"].as_u64().unwrap() > 0, "{text}");
    // The machine it ran on
    let m = &r["machine"];
    assert!(m["adapter"].is_string() && m["backend"].is_string(), "{m}");
    assert_eq!(
        (m["width"].as_u64(), m["height"].as_u64()),
        (Some(192), Some(108))
    );
    assert!(!m["build_profile"].as_str().unwrap().is_empty(), "{m}");
    line("[perf] report:");
}

/// #1157's scene: it authors blueprint; its director cuts to graphite at
/// 0.5 s and fades to ink wash over 2 s from 1 s, so 2 s is the fade's
/// midpoint and three looks are in play
const THREE_LOOKS_SCENE: &str = r#"
[scene]
name = "Three Looks"

[camera]
position = [0.0, 3.0, 4.0]
target = [0.0, 0.0, 0.0]
fov = 50.0

[post_process]
look = "blueprint"

[entities.key.transform]
position = [0.0, 2.0, 0.0]
[entities.key.light]
type = "point"
intensity = 30.0
range = 10.0

[entities.floor.transform]
position = [0.0, 0.0, 0.0]
[entities.floor.bounds]
min = [-5.0, -0.1, -5.0]
max = [5.0, 0.0, 5.0]
[entities.floor.material]
color = [0.8, 0.8, 0.8, 1.0]

[entities.director.script]
source = "director.rhai"
"#;

const THREE_LOOKS_SCRIPT: &str = r#"
let cut = false;
let faded = false;
fn on_update() {
    let t = total_time();
    if !cut && t >= 0.5 { set_look("graphite"); cut = true; }
    if !faded && t >= 1.0 { set_look("ink_wash", 2.0); faded = true; }
}
"#;

/// #1157: inside a look fade, `flint render --scripts --time` lays what the
/// player draws at that game time: every `[post_process]` value and every
/// style's blend in the two frame descriptions the same. The scene authors
/// one look, cuts to a second and fades to a third, so a capture that faded
/// from the authored look (blueprint) instead of the cut's (graphite) fails.
/// A step of 1/16 s is exact in binary, so both clocks reach 2 s exactly.
#[test]
fn a_capture_inside_a_look_fade_lays_what_the_player_draws() {
    let dir = scratch("look_fade");
    if !has_gpu(&dir) {
        return;
    }
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(dir.join("looks.scene.toml"), THREE_LOOKS_SCENE).unwrap();
    std::fs::write(dir.join("scripts/director.rhai"), THREE_LOOKS_SCRIPT).unwrap();
    let schemas = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let schemas = schemas.to_str().unwrap();
    let shared = ["--schemas", schemas, "--width", "160", "--height", "120"];
    let step = ["--frame-step", "0.0625"];

    let mut play = vec!["play", "looks.scene.toml", "--exit-after", "2.2"];
    play.extend(step);
    play.extend(shared);
    play.extend([
        "--capture-frame-at",
        "2",
        "--capture-dir",
        "caps",
        "--music-volume",
        "0",
        "--sfx-volume",
        "0",
        "--no-activate",
    ]);
    let run = flint_until_it_ends(&dir, &play);
    assert!(
        run.status.success(),
        "flint play failed:\n{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let mut render = vec![
        "render",
        "looks.scene.toml",
        "--scripts",
        "--time",
        "2",
        "-o",
        "capture.png",
        "--frame-dump",
        "capture.json",
    ];
    render.extend(step);
    render.extend(shared);
    let o = flint(&dir, &render);
    assert!(
        o.status.success(),
        "flint render failed:\n{}",
        String::from_utf8_lossy(&o.stderr)
    );

    let read = |path: PathBuf| -> Value {
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    };
    let player = read(dir.join("caps/looks-t2.000.flintframe/manifest.json"));
    let capture = read(dir.join("capture.json"));
    let (player, capture) = (&player["frame"], &capture["frame"]);
    assert_eq!(player["game_time"], 2.0, "{player}");
    assert_eq!(capture["game_time"], 2.0, "{capture}");
    assert_eq!(
        player["styles"], capture["styles"],
        "the style blends differ mid-fade"
    );
    let (ours, theirs) = (
        capture["post_process"].as_object().unwrap(),
        player["post_process"].as_object().unwrap(),
    );
    let differ: Vec<_> = ours
        .keys()
        .chain(theirs.keys())
        .filter(|k| ours.get(*k) != theirs.get(*k))
        .map(|k| format!("{k}: capture {:?}, player {:?}", ours.get(k), theirs.get(k)))
        .collect();
    assert!(
        differ.is_empty(),
        "post values differ mid-fade:\n{}",
        differ.join("\n")
    );
    // And it is mid-fade: a style on its way in or out at its middle
    let blends: Vec<f64> = capture["styles"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["blend"].as_f64())
        .collect();
    assert!(
        blends.iter().any(|b| (b - 0.5).abs() < 1e-6),
        "no style half way at the fade's midpoint: {}",
        capture["styles"]
    );
}
