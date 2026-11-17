//! `flint frame open` (#1088), end to end on the frozen captures in
//! `tests/fixtures/frames/`: a pass name it doesn't have is refused before
//! any window, and `--screenshot` draws the window into a PNG for a
//! `flint render` capture, the same capture made to look like the player's
//! (CPU phases and a frame history), and a description with no images and
//! no GPU times, each without a panel breaking. The screenshots skip
//! themselves without a GPU.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn two() -> PathBuf {
    repo().join("tests/fixtures/frames/two_crates.flintframe")
}

fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("frame_open_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn flint(args: &[&dyn AsRef<std::ffi::OsStr>]) -> Output {
    let mut cmd = std::process::Command::new(FLINT);
    for a in args {
        cmd.arg(a);
    }
    cmd.output().expect("run flint")
}

fn text(o: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// A screenshot of the window, or `None` with no GPU to draw it.
fn screenshot(capture: &Path, out: &Path, extra: &[&str]) -> Option<image::RgbaImage> {
    let mut cmd = std::process::Command::new(FLINT);
    cmd.args(["frame", "open"])
        .arg(capture)
        .arg("--screenshot")
        .arg(out)
        .args(["--width", "1280", "--height", "800"])
        .args(extra);
    let o = cmd.output().expect("run flint");
    if !o.status.success() && String::from_utf8_lossy(&o.stderr).contains("starting the GPU") {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter for the screenshot");
        return None;
    }
    assert!(o.status.success(), "{}", text(&o));
    let img = image::open(out).expect("a PNG").to_rgba8();
    assert_eq!(img.dimensions(), (1280, 800));
    Some(img)
}

/// The image pane's middle, where a pass's picture is drawn.
fn middle(img: &image::RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    for y in 300..600 {
        for x in 420..820 {
            out.extend_from_slice(&img.get_pixel(x, y).0);
        }
    }
    out
}

/// How many distinct colours a strip of the screenshot has: a panel that
/// drew rows of text has many, an empty one few.
fn colours(img: &image::RgbaImage, x: std::ops::Range<u32>, y: std::ops::Range<u32>) -> usize {
    let mut seen = std::collections::HashSet::new();
    for yy in y {
        for xx in x.clone() {
            seen.insert(img.get_pixel(xx, yy).0);
        }
    }
    seen.len()
}

#[test]
fn a_pass_it_does_not_have_is_refused_before_any_window() {
    let o = flint(&[&"frame", &"open", &two(), &"--pass", &"Blom"]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("no pass named \"Blom\""), "{err}");
    assert!(err.contains("Bloom"), "the nearest is named: {err}");
}

#[test]
fn the_window_refuses_json_and_a_screenshot_answers_it() {
    let o = flint(&[&"--json", &"frame", &"open", &two()]);
    assert!(!o.status.success(), "{}", text(&o));
    let dir = tmp("json");
    let out = dir.join("shot.png");
    let o = flint(&[&"--json", &"frame", &"open", &two(), &"--screenshot", &out]);
    if !o.status.success() && String::from_utf8_lossy(&o.stderr).contains("starting the GPU") {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter for the screenshot");
        return;
    }
    assert!(o.status.success(), "{}", text(&o));
    let doc: Value = serde_json::from_slice(&o.stdout).expect("one JSON document");
    assert!(
        doc["written"][0].as_str().unwrap().ends_with("shot.png"),
        "{doc}"
    );
    assert!(out.is_file());
}

#[test]
fn each_pass_shows_its_own_image() {
    let dir = tmp("passes");
    let Some(last) = screenshot(&two(), &dir.join("last.png"), &[]) else {
        return;
    };
    let shadow = screenshot(
        &two(),
        &dir.join("shadow.png"),
        &["--pass", "Shadow cascade 0"],
    )
    .unwrap();
    let scene = screenshot(
        &two(),
        &dir.join("scene.png"),
        &[
            "--pass", "Scene", "--target", "depth", "--entity", "crate", "--wipe", "previous",
        ],
    )
    .unwrap();
    // The finished frame, a shadow map and the scene's depth over its wipe
    // are three different pictures in the pane
    assert_ne!(middle(&last), middle(&shadow));
    assert_ne!(middle(&shadow), middle(&scene));
    assert_ne!(middle(&last), middle(&scene));
    // The tree and the draw table drew their rows
    assert!(
        colours(&scene, 0..250, 120..500) > 40,
        "the pass tree is empty"
    );
    assert!(
        colours(&scene, 1000..1270, 150..300) > 40,
        "the draw table is empty"
    );
}

#[test]
fn a_player_capture_and_a_bare_description_open_without_broken_panels() {
    let dir = tmp("shapes");
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(two().join("manifest.json")).unwrap())
            .unwrap();

    // The player's shape: CPU phases, scripts, a history with a hitch, the
    // images beside it
    let player = dir.join("player.flintframe");
    std::fs::create_dir_all(player.join("targets")).unwrap();
    for f in std::fs::read_dir(two().join("targets")).unwrap() {
        let f = f.unwrap();
        std::fs::copy(f.path(), player.join("targets").join(f.file_name())).unwrap();
    }
    let mut m = manifest.clone();
    let frame = |i: u64, ms: f64, hitch: bool| {
        serde_json::json!({
            "index": i, "game_time": i as f64 / 60.0, "ms": ms, "judged_ms": ms,
            "phases_ms": {"input": 0.2, "scripts": if hitch { 100.0 } else { 1.0 }, "render": 3.0, "present": 2.0},
            "scripts_ms": {"stall.rhai on_update": if hitch { 99.5 } else { 0.4 }},
            "fixed_steps": 1, "hitch": hitch
        })
    };
    let window: Vec<Value> = (0..90)
        .map(|i| frame(i, if i == 60 { 110.0 } else { 16.7 }, i == 60))
        .collect();
    m["cpu"] = serde_json::json!({
        "frame": window[61],
        "window": window,
        "scripts_timed": true,
        "hitch": {"frame": 60, "game_time": 1.0, "ms": 110.0, "judged_ms": 110.0, "median_ms": 16.7,
                  "class": "stall", "cause": "script", "script": "stall.rhai on_update", "script_ms": 99.5}
    });
    m["trigger"] = serde_json::json!({"kind": "hitch", "detail": "the hitch at frame 60"});
    m["written_by"] = Value::from("flint-player");
    std::fs::write(
        player.join("manifest.json"),
        serde_json::to_string(&m).unwrap(),
    )
    .unwrap();
    let Some(shot) = screenshot(&player, &dir.join("player.png"), &[]) else {
        return;
    };
    assert!(
        colours(&shot, 100..1270, 680..790) > 12,
        "the bottom panel drew the bars and the history"
    );

    // A description alone: no images, no GPU times, no CPU
    let mut bare = manifest;
    fn strip(passes: &mut Value) {
        for p in passes.as_array_mut().into_iter().flatten() {
            p["images"] = Value::Array(vec![]);
            p["gpu_ms"] = Value::Null;
            strip(&mut p["passes"]);
        }
    }
    strip(&mut bare["passes"]);
    bare["frame"]["gpu_frame_ms"] = Value::Null;
    let desc = dir.join("frame.json");
    std::fs::write(&desc, serde_json::to_string(&bare).unwrap()).unwrap();
    let shot = screenshot(&desc, &dir.join("bare.png"), &["--pass", "Scene"]).unwrap();
    assert!(
        colours(&shot, 0..250, 120..500) > 40,
        "the pass tree still lists the passes"
    );
    assert!(
        colours(&shot, 1000..1270, 150..300) > 40,
        "the scene's draws are still listed"
    );
}
