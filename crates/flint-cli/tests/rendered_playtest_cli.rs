//! `flint playtest --render` (#1191), through the binary: a playtest that
//! draws every frame offscreen, with no window, writes screenshots at game
//! times (a flag and an input script's `screenshot` lines) and checks what
//! the frame showed (`frame hud`, `frame style`, `frame draws`). Two runs
//! write the same report and the same PNGs. Without `--render`, the HUD's
//! draw list is still read and the renderer's checks fail saying why.
//!
//! The GPU tests skip without an adapter, unless `FLINT_REQUIRE_GPU=1`.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run flint")
}

fn document(out: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is one JSON document ({e}):\n{stdout}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// A red box in front of the camera and a ghost (a card model) behind it,
/// which the camera culls; the HUD says
/// `Lap 1`, then `Lap 2` and the frame turns to `invert` once a second has
/// gone by.
const SCENE: &str = "[scene]\nname = \"Lap\"\n\n\
    [camera]\nposition = [0.0, 1.0, 5.0]\ntarget = [0.0, 0.5, 0.0]\nfov = 50.0\n\n\
    [entities.crate]\n[entities.crate.transform]\nposition = [0.0, 0.0, 0.0]\n\
    [entities.crate.bounds]\nmin = [-1.0, 0.0, -1.0]\nmax = [1.0, 1.0, 1.0]\n\
    [entities.crate.material]\ncolor = [0.9, 0.1, 0.1, 1.0]\nunlit = true\n\n\
    [entities.ghost]\n[entities.ghost.transform]\nposition = [0.0, 0.0, 30.0]\n\
    [entities.ghost.model]\nasset = \"card\"\n\n\
    [entities.director]\n[entities.director.script]\nsource = \"lap.rhai\"\n";

const SCRIPT: &str = "let turned = false;\n\
    fn on_update() {\n\
    \x20   if total_time() > 1.0 && !turned {\n\
    \x20       turned = true;\n\
    \x20       set_post_style(\"invert\", true);\n\
    \x20   }\n\
    }\n\
    fn on_draw_ui() {\n\
    \x20   let lap = if total_time() > 1.0 { 2 } else { 1 };\n\
    \x20   draw_text(16.0, 16.0, \"Lap \" + lap + \" of 3\", 28.0, #{ color: [1.0, 1.0, 1.0, 1.0] });\n\
    }\n";

const INPUT: &str = "0.5 expect frame hud contains \"Lap 1 of 3\"\n\
    0.5 expect frame style invert off\n\
    0.5 expect frame draws crate\n\
    0.5 expect frame draws no ghost\n\
    0.5 screenshot shots/lap1.png\n\
    1.0 wait frame hud contains \"Lap 2\" within 1\n\
    1.5 expect frame hud lacks \"Lap 1\"\n\
    1.5 expect frame style invert on\n\
    1.5 screenshot shots/lap2.png\n";

fn project(name: &str, input: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("rendered_playtest_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(dir.join("lap.scene.toml"), SCENE).unwrap();
    std::fs::write(dir.join("scripts/lap.rhai"), SCRIPT).unwrap();
    std::fs::write(dir.join("lap.input.txt"), input).unwrap();
    // The ghost is a model, so it has bounds to cull by (a procedural box
    // has none and always draws, ADR 0074); the playtest loads it only
    // with --render
    std::fs::create_dir_all(dir.join("models")).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/models/DoubleSidedCard.glb"),
        dir.join("models/card.glb"),
    )
    .unwrap();
    dir
}

fn schemas() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas")
        .to_string_lossy()
        .into_owned()
}

fn playtest(dir: &Path, extra: &[&str]) -> (Output, Value) {
    let schemas = schemas();
    let mut args = vec![
        "--json",
        "playtest",
        "lap.scene.toml",
        "--input-script",
        "lap.input.txt",
        "--schemas",
        &schemas,
        "--exit-after",
        "2",
    ];
    args.extend(extra);
    let out = flint(dir, &args);
    let d = document(&out);
    (out, d)
}

/// A rendered run, or `None` (skipped) on a machine with no GPU.
fn rendered(dir: &Path, extra: &[&str]) -> Option<(Output, Value)> {
    let mut args = vec!["--render", "--width", "320", "--height", "180"];
    args.extend(extra);
    let (out, d) = playtest(dir, &args);
    let no_gpu = d["failures"].as_array().into_iter().flatten().any(|f| {
        f["message"]
            .as_str()
            .unwrap_or("")
            .contains("no GPU to draw on")
    });
    if no_gpu {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter for a rendered playtest");
        return None;
    }
    Some((out, d))
}

fn rgb(path: &Path, x: u32, y: u32) -> [u8; 3] {
    image::open(path).unwrap().into_rgb8().get_pixel(x, y).0
}

/// The HUD's lettering: pixels near white in the top-left corner.
fn lettering(path: &Path) -> usize {
    let img = image::open(path).unwrap().into_rgb8();
    let mut n = 0;
    for y in 10..50 {
        for x in 10..150 {
            if img.get_pixel(x, y).0.iter().all(|c| *c > 200) {
                n += 1;
            }
        }
    }
    n
}

/// The issue's done-when: screenshots at game times with no window, and
/// checks that read what was drawn
#[test]
fn a_rendered_playtest_writes_screenshots_and_checks_the_frame() {
    let dir = project("rendered", INPUT);
    let Some((out, d)) = rendered(
        &dir,
        &["--screenshot", "shots/flag.png", "--screenshot-at", "0.25"],
    ) else {
        return;
    };
    assert!(out.status.success(), "{d:#}");
    let checks = d["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 7, "{d:#}");
    assert!(checks.iter().all(|c| c["passed"] == true), "{d:#}");
    let found = |text: &str| {
        checks
            .iter()
            .find(|c| c["text"] == text)
            .map(|c| c["found"].as_str().unwrap().to_string())
            .unwrap()
    };
    assert_eq!(found("frame hud contains \"Lap 1 of 3\""), "\"Lap 1 of 3\"");
    assert_eq!(found("frame draws crate"), "drawn");
    assert_eq!(found("frame draws no ghost"), "culled");

    // Three PNGs, each at its time, at the target's size
    let shots = d["screenshots"].as_array().unwrap();
    let files: Vec<&str> = shots.iter().map(|s| s["file"].as_str().unwrap()).collect();
    assert_eq!(
        files,
        ["shots/flag.png", "shots/lap1.png", "shots/lap2.png"],
        "{d:#}"
    );
    for s in shots {
        assert_eq!(
            (s["width"].as_u64(), s["height"].as_u64()),
            (Some(320), Some(180))
        );
    }
    assert!(shots[1]["at"].as_f64().unwrap() >= 0.5);
    assert!(shots[2]["at"].as_f64().unwrap() >= 1.5);
    assert_eq!(d["render"]["frames"], d["frames"]);
    assert!(d["render"]["draw_calls"].as_u64().unwrap() > 0);

    // What the pictures hold: the red box in the middle, then inverted;
    // the HUD's lettering in both
    let lap1 = dir.join("shots/lap1.png");
    let lap2 = dir.join("shots/lap2.png");
    let [r, g, b] = rgb(&lap1, 160, 90);
    assert!(
        r > 150 && r > g + 80 && r > b + 80,
        "a red box at t=0.5: {r},{g},{b}"
    );
    let [r, g, b] = rgb(&lap2, 160, 90);
    assert!(g > r + 80 && b > r + 80, "inverted at t=1.5: {r},{g},{b}");
    assert!(lettering(&lap1) > 40, "the HUD is drawn");
    assert!(lettering(&dir.join("shots/flag.png")) > 40);
}

/// A rendered run is the same bytes twice, its pictures included
#[test]
fn two_rendered_runs_write_the_same_report_and_pictures() {
    let mut runs = Vec::new();
    for n in 0..2 {
        let dir = project(&format!("twice{n}"), INPUT);
        let Some((out, _)) = rendered(&dir, &["--report", "report.json"]) else {
            return;
        };
        assert!(out.status.success());
        runs.push((
            std::fs::read(dir.join("report.json")).unwrap(),
            std::fs::read(dir.join("shots/lap1.png")).unwrap(),
            std::fs::read(dir.join("shots/lap2.png")).unwrap(),
        ));
    }
    assert!(runs[0].0 == runs[1].0, "the reports differ");
    assert!(runs[0].1 == runs[1].1, "lap1.png differs");
    assert!(runs[0].2 == runs[1].2, "lap2.png differs");
}

/// Without `--render` the HUD's draw list is still read, the renderer's
/// checks fail saying there was none, and a screenshot is a failure
#[test]
fn without_render_the_frame_checks_say_why_and_shots_fail() {
    let dir = project("unrendered", INPUT);
    let (out, d) = playtest(&dir, &[]);
    assert_eq!(out.status.code(), Some(1), "{d:#}");
    let checks = d["checks"].as_array().unwrap();
    let passed = |text: &str| checks.iter().find(|c| c["text"] == text).unwrap()["passed"] == true;
    assert!(passed("frame hud contains \"Lap 1 of 3\""), "{d:#}");
    assert!(passed("frame hud lacks \"Lap 1\""), "{d:#}");
    let style = checks
        .iter()
        .find(|c| c["text"] == "frame style invert off")
        .unwrap();
    assert_eq!(style["passed"], false);
    assert!(
        style["found"].as_str().unwrap().contains("--render"),
        "{style}"
    );
    let shots: Vec<&Value> = d["failures"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["kind"] == "screenshot")
        .collect();
    assert_eq!(shots.len(), 2, "{d:#}");
    assert!(shots[0]["message"]
        .as_str()
        .unwrap()
        .contains("no frame was drawn"));
    assert!(!dir.join("shots").exists());

    // --screenshot asks for a frame no run without --render draws
    let (out, d) = playtest(&dir, &["--screenshot", "x.png"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(d["ended"], "start_failed");
    assert!(
        d["failures"][0]["message"]
            .as_str()
            .unwrap()
            .contains("--screenshot needs --render"),
        "{d:#}"
    );
}
