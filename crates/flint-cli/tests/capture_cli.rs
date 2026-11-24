//! #722's done-when, with the real binary: a game project captures a
//! three-shot list with one command and no Python, and `--verify` says
//! whether two runs made the same frames.
//!
//! Each take opens a small player window (`--no-activate`, so it never takes
//! the keyboard). Skipped, said on stderr, on a machine with no GPU adapter.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("capture_cli_{name}"));
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
        eprintln!("no GPU adapter: capture_cli is skipped");
    }
    any
}

const SHOTS: &str = r#"
[capture]
title = "Three shots"
fps = 10
width = 192
height = 108

[[shot]]
id = "open"
scene = "scenes/main.scene.toml"
seconds = 0.5

[[shot]]
id = "styled"
scene = "scenes/main.scene.toml"
seconds = 0.5
take = [
    { name = "main" },
    { name = "graphite", look = "graphite" },
]

[[shot]]
id = "walk"
scene = "scenes/main.scene.toml"
seconds = 0.8
input = "walk.input.txt"
"#;

#[test]
fn a_game_captures_three_shots_with_one_command_and_verifies_them() {
    let top = scratch("three");
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
    std::fs::write(game.join("shots.toml"), SHOTS).unwrap();
    std::fs::write(game.join("walk.input.txt"), "0.1 down KeyW\n0.6 up KeyW\n").unwrap();

    let out = flint(&game, &["capture", "shots.toml", "--verify", "--json"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not one JSON document ({e}):\n{stdout}\n--- stderr\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert!(
        out.status.success(),
        "{report:#}\n--- stderr\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(report["failed"], false);

    let takes = report["takes"].as_array().unwrap();
    let labels: Vec<String> = takes
        .iter()
        .map(|t| {
            format!(
                "{}/{}",
                t["shot"].as_str().unwrap(),
                t["take"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(
        labels,
        ["open/main", "styled/main", "styled/graphite", "walk/main"]
    );
    for (take, frames) in takes.iter().zip([6, 6, 6, 9]) {
        assert_eq!(take["frames"], frames, "{take:#}");
        assert_eq!(take["ok"], true);
        assert_eq!(take["verify"]["same"], true, "{take:#}");
        assert_eq!(take["verify"]["differing"], serde_json::json!([]));
    }

    // On disk: the frames, take.json, the verify report; the matching
    // second run given back
    let folder = game.join("captures/shots/walk/main");
    for n in 1..=9 {
        assert!(
            folder.join(format!("frame_{n:04}.png")).is_file(),
            "frame {n}"
        );
    }
    assert!(!folder.join("frame_0010.png").exists());
    let record: Value =
        serde_json::from_str(&std::fs::read_to_string(folder.join("take.json")).unwrap()).unwrap();
    assert_eq!(
        (record["width"].clone(), record["fps"].clone()),
        (192.into(), 10.into())
    );
    assert!(folder.join("verify.json").is_file());
    assert!(folder.join("input.txt").is_file());
    assert!(!game.join("captures/shots/walk/main.again").exists());
    assert!(game.join("captures/shots/verify.json").is_file());

    // The looks are what the list asked for: the graphite take isn't the
    // plain one, and walking moved the camera
    let frame = |shot: &str, take: &str, n: u32| {
        std::fs::read(game.join(format!("captures/shots/{shot}/{take}/frame_{n:04}.png"))).unwrap()
    };
    assert_ne!(frame("styled", "main", 1), frame("styled", "graphite", 1));
    assert_eq!(frame("styled", "main", 1), frame("open", "main", 1));
    assert_ne!(frame("walk", "main", 1), frame("walk", "main", 9));
}

#[test]
fn a_list_naming_a_missing_scene_stops_before_any_take() {
    let dir = scratch("missing");
    std::fs::write(
        dir.join("shots.toml"),
        "[[shot]]\nid = \"a\"\nscene = \"nope.scene.toml\"\nseconds = 1\n",
    )
    .unwrap();
    let out = flint(&dir, &["capture", "shots.toml"]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("a/main: no scene"), "{err}");
    assert!(!dir.join("captures").exists());
}

/// #1196's done-when: a run whose estimate passes the free space stops
/// before any take, and says why. Two hours of 100,000-pixel-square frames
/// at 30 fps, two shots of it, is about 2,000 TB at 8 MB a 4K frame: past
/// any disk.
#[test]
fn a_run_that_wont_fit_on_the_disk_stops_before_any_take() {
    let dir = scratch("too_big");
    std::fs::write(dir.join("a.scene.toml"), "[scene]\nname = \"a\"\n").unwrap();
    let list = "[capture]\nwidth = 100000\nheight = 100000\n\n\
                [[shot]]\nid = \"one\"\nscene = \"a.scene.toml\"\nseconds = 3600\n\n\
                [[shot]]\nid = \"two\"\nscene = \"a.scene.toml\"\nseconds = 3600\n";
    std::fs::write(dir.join("shots.toml"), list).unwrap();

    let out = flint(&dir, &["capture", "shots.toml"]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    for words in [
        "this capture won't fit: it needs ~",
        "GB free",
        "--shot <id>       fewer shots (the largest, one, is ~",
        "--out <dir>",
        "--no-disk-check",
    ] {
        assert!(err.contains(words), "no {words:?} in:\n{err}");
    }
    assert!(!dir.join("captures").exists(), "a take was started");

    // Under --json the refusal is the error document, and nothing ran
    let out = flint(&dir, &["--json", "capture", "shots.toml"]);
    assert_eq!(out.status.code(), Some(1));
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(message.contains("won't fit"), "{doc:#}");
    assert!(!dir.join("captures").exists());

    // A dry run says so and still prints the plan
    let out = flint(&dir, &["--json", "capture", "shots.toml", "--dry-run"]);
    assert!(out.status.success());
    let plan: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(plan["disk"]["fits"], false, "{plan:#}");
    assert!(plan["disk"]["free_bytes"].as_u64().unwrap() > 0);
    assert_eq!(plan["takes"].as_array().unwrap().len(), 2);
}
