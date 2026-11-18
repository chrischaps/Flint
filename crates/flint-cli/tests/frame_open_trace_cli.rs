//! `flint frame open` on a scope trace (#1099), end to end: a trace alone
//! opens on the profiler tab with its hitch chosen; `--compare` tints the
//! stall's path vermillion; a capture that holds a trace opens with both
//! tabs; flags of the wrong kind are refused before any window. The trace
//! is written here, in the player's shape (ADR 0128), so no fixture is
//! kept. The screenshots skip themselves without a GPU.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tmp(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("frame_open_trace_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn text(o: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// Two seconds at 60 fps on a `main` thread, every frame a `tick` over a
/// script, physics and a render; frame 60 stalls 100 ms in its script.
fn trace() -> String {
    let mut e: Vec<String> = Vec::new();
    for (tid, name) in [(1, "main"), (2, "frames"), (3, "phases"), (4, "hitches")] {
        e.push(format!(
            r#"{{"ph":"M","name":"thread_name","pid":1,"tid":{tid},"args":{{"name":"{name}"}}}}"#
        ));
    }
    let x = |tid: u32, name: &str, ts: f64, dur: f64, args: String| {
        format!(
            r#"{{"ph":"X","name":"{name}","pid":1,"tid":{tid},"ts":{ts:.3},"dur":{dur:.3},"args":{{{args}}}}}"#
        )
    };
    let mut t = 0.0;
    for f in 0..120u32 {
        let stall = if f == 60 { 100_000.0 } else { 0.0 };
        let len = 16_667.0 + stall;
        let fa = |depth: u32| format!(r#""frame":{f},"depth":{depth}"#);
        e.push(x(
            1,
            "stall.rhai:on_update",
            t + 600.0,
            300.0 + stall,
            fa(2),
        ));
        e.push(x(1, "script::update", t + 500.0, 500.0 + stall, fa(1)));
        e.push(x(1, "physics::step", t + 1_200.0 + stall, 2_000.0, fa(1)));
        e.push(x(1, "render", t + 3_400.0 + stall, 9_000.0, fa(1)));
        e.push(x(1, "tick", t + 100.0, len - 300.0, fa(0)));
        e.push(x(2, "frame", t, len, format!(r#""frame":{f}"#)));
        e.push(x(
            3,
            "scripts",
            t + 400.0,
            700.0 + stall,
            format!(r#""frame":{f}"#),
        ));
        e.push(x(
            3,
            "physics",
            t + 1_150.0 + stall,
            2_100.0,
            format!(r#""frame":{f}"#),
        ));
        e.push(x(
            3,
            "render",
            t + 3_300.0 + stall,
            9_200.0,
            format!(r#""frame":{f}"#),
        ));
        if f == 60 {
            e.push(x(
                4,
                "hitch",
                t,
                len,
                r#""history_frame":60,"judged_ms":116.7,"median_ms":16.7,"cause":"script stall.rhai:on_update 100.3 ms""#.into(),
            ));
        }
        t += len;
    }
    format!(
        "{{\"traceEvents\":[\n{}\n],\"otherData\":{{\"format\":\"flint-trace\",\"scene\":\"stall.scene.toml\"}}}}\n",
        e.join(",\n")
    )
}

/// A screenshot of the window on `path`, or `None` with no GPU.
fn screenshot(path: &Path, out: &Path, extra: &[&str]) -> Option<image::RgbaImage> {
    let o = std::process::Command::new(FLINT)
        .args(["frame", "open"])
        .arg(path)
        .arg("--screenshot")
        .arg(out)
        .args(["--width", "1280", "--height", "800"])
        .args(extra)
        .output()
        .expect("run flint");
    if !o.status.success() && String::from_utf8_lossy(&o.stderr).contains("starting the GPU") {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter for the screenshot");
        return None;
    }
    assert!(o.status.success(), "{}", text(&o));
    let img = image::open(out).expect("a PNG").to_rgba8();
    assert_eq!(img.dimensions(), (1280, 800));
    Some(img)
}

/// Pixels near the hitch vermillion (213, 94, 0) in the flame chart.
fn vermillion(img: &image::RgbaImage) -> usize {
    let mut n = 0;
    for y in 260..560 {
        for x in 20..800 {
            let [r, g, b, _] = img.get_pixel(x, y).0;
            if (r as i32 - 213).abs() < 14 && (g as i32 - 94).abs() < 14 && b < 20 {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn a_trace_alone_opens_on_its_hitch_and_compare_tints_the_stall() {
    let dir = tmp("alone");
    let file = dir.join("stall.json");
    std::fs::write(&file, trace()).unwrap();
    let Some(plain) = screenshot(&file, &dir.join("plain.png"), &[]) else {
        return;
    };
    let compared = screenshot(&file, &dir.join("compare.png"), &["--compare"]).unwrap();
    let (a, b) = (vermillion(&plain), vermillion(&compared));
    assert!(
        b > 20_000 && b > a * 4,
        "--compare paints the stall's path vermillion: {a} pixels plain, {b} compared"
    );
    // A scope chosen, the plot under the table
    let chosen = screenshot(&file, &dir.join("chosen.png"), &["--scope", "stall"]).unwrap();
    assert_ne!(chosen.as_raw(), plain.as_raw());
    // The JSON answer names the file
    let out = dir.join("json.png");
    let o = std::process::Command::new(FLINT)
        .args(["--json", "frame", "open"])
        .arg(&file)
        .arg("--screenshot")
        .arg(&out)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", text(&o));
    let doc: Value = serde_json::from_slice(&o.stdout).expect("one JSON document");
    assert!(
        doc["written"][0].as_str().unwrap().ends_with("json.png"),
        "{doc}"
    );
}

#[test]
fn a_capture_holding_a_trace_opens_with_both_tabs() {
    let dir = tmp("capture");
    let from = repo().join("tests/fixtures/frames/two_crates.flintframe");
    let cap = dir.join("two_crates.flintframe");
    std::fs::create_dir_all(cap.join("targets")).unwrap();
    for entry in std::fs::read_dir(from.join("targets")).unwrap() {
        let p = entry.unwrap().path();
        std::fs::copy(&p, cap.join("targets").join(p.file_name().unwrap())).unwrap();
    }
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(from.join("manifest.json")).unwrap())
            .unwrap();
    manifest["trace"] = Value::String("trace.json".into());
    std::fs::write(cap.join("manifest.json"), manifest.to_string()).unwrap();
    std::fs::write(cap.join("trace.json"), trace()).unwrap();
    let Some(frame) = screenshot(&cap, &dir.join("frame.png"), &[]) else {
        return;
    };
    let profile = screenshot(&cap, &dir.join("profile.png"), &["--profile", "--compare"]).unwrap();
    assert_ne!(frame.as_raw(), profile.as_raw());
    assert!(
        vermillion(&profile) > 20_000,
        "the profiler tab drew the trace"
    );
    // The frame tab is the frame debugger's: its passes on the left
    assert!(vermillion(&frame) < 20_000);
}

#[test]
fn flags_of_the_wrong_kind_are_refused_before_any_window() {
    let dir = tmp("refused");
    let file = dir.join("stall.json");
    std::fs::write(&file, trace()).unwrap();
    let o = std::process::Command::new(FLINT)
        .args(["frame", "open"])
        .arg(&file)
        .args(["--pass", "Scene"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("is a scope trace"),
        "{}",
        text(&o)
    );

    let capture = repo().join("tests/fixtures/frames/two_crates.flintframe");
    let o = std::process::Command::new(FLINT)
        .args(["frame", "open"])
        .arg(&capture)
        .arg("--compare")
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("holds no trace"),
        "{}",
        text(&o)
    );

    let o = std::process::Command::new(FLINT)
        .args(["frame", "open"])
        .arg(&file)
        .args(["--trace-frame", "soon"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("hitch"),
        "{}",
        text(&o)
    );

    // Not a trace, not a capture
    let junk = dir.join("junk.json");
    std::fs::write(&junk, "{\"hello\": 1}").unwrap();
    let o = std::process::Command::new(FLINT)
        .args(["frame", "open"])
        .arg(&junk)
        .output()
        .unwrap();
    assert!(!o.status.success(), "{}", text(&o));
}
