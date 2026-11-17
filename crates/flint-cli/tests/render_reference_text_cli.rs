//! `--reference-text` (#495): with the reference's glyph grid, a frame the
//! ASCII style drew is scored by its letters (mean colour, glyph coverage,
//! glyph layout), and every score lists its terms. Runs the real binary on
//! the frozen `stylize_props` scene; skips itself without a GPU.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn scene() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/scenes/stylize_props.scene.toml")
}

fn dir(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render_reference_text_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn render(out: &Path, args: &[&str]) -> Output {
    std::process::Command::new(FLINT)
        .arg("render")
        .arg(scene())
        .args([
            "--width", "320", "--height", "180", "--style", "ascii", "-o",
        ])
        .arg(out)
        .args(args)
        .output()
        .expect("run flint")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn no_gpu(o: &Output) -> bool {
    let missing = !o.status.success() && stderr(o).contains("headless render context");
    if missing {
        eprintln!("no GPU adapter: skipped");
    }
    missing
}

fn ok(o: &Output) {
    assert!(o.status.success(), "flint failed: {}", stderr(o));
}

fn json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// The reference: its picture, its grid and its measures in one run (the grid
/// is read back once a frame, so `--ascii-text` with `--report` used to find
/// it gone).
fn reference(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let (png, txt, m) = (
        dir.join("ref.png"),
        dir.join("ref.txt"),
        dir.join("ref.json"),
    );
    let o = render(&png, &["--ascii-text", s(&txt), "--report", s(&m)]);
    if no_gpu(&o) {
        return None;
    }
    ok(&o);
    assert!(std::fs::read_to_string(&txt).unwrap().lines().count() > 5);
    assert!(json(&m)["columns"][0]["ascii"]["cols"].as_u64().unwrap() > 0);
    Some((png, txt))
}

/// Glow changes pixels, not letters: on a glow sweep the glyph terms hold
/// still while the pixel histogram moves, and the first column, the
/// reference itself, is 0 by glyphs.
#[test]
fn a_glow_sweep_moves_pixels_and_not_glyphs() {
    let dir = dir("glow");
    let Some((png, txt)) = reference(&dir) else {
        return;
    };
    let m = dir.join("m.json");
    let o = render(
        &dir.join("sweep.png"),
        &[
            "--sweep",
            "ascii.glow=0,1.5",
            "--reference",
            s(&png),
            "--reference-text",
            s(&txt),
            "--report",
            s(&m),
        ],
    );
    ok(&o);
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("by glyphs (mean colour"), "{out}");
    let report = json(&m);
    let d = |i: usize| report["columns"][i]["distance"].clone();
    assert_eq!(d(0)["metric"], "glyphs", "{report}");
    let first = d(0);
    let terms: Vec<&str> = first["terms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["term"].as_str().unwrap())
        .collect();
    assert_eq!(terms, ["mean colour", "glyph coverage", "glyph layout"]);
    assert_eq!(d(0)["glyph"]["coverage"], 0.0, "{report}");
    assert_eq!(d(0)["glyph"]["layout"], 0.0, "{report}");
    assert_eq!(d(1)["glyph"], d(0)["glyph"], "glow changed the letters");
    let hist = |i: usize| d(i)["histogram"].as_f64().unwrap();
    assert!(hist(1) > hist(0) + 0.01, "{} {}", hist(0), hist(1));
    // A score is its terms' points
    let sum: f64 = d(1)["terms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["points"].as_f64().unwrap())
        .sum();
    assert!((sum - d(1)["score"].as_f64().unwrap()).abs() < 1e-9);
}

/// A refine on the clock (#596's Stage path) scores by glyphs too, and its
/// log says so with each candidate's terms.
#[test]
fn a_refine_on_the_clock_reads_glyphs() {
    let dir = dir("refine");
    let Some((png, txt)) = reference(&dir) else {
        return;
    };
    let o = render(
        &dir.join("rounds.png"),
        &[
            "--time",
            "0",
            "--look",
            "ascii_noir",
            "--reference",
            s(&png),
            "--reference-text",
            s(&txt),
            "--ai-refine",
            "--ai-provider",
            "mock",
            "--refine-rounds",
            "1",
            "--refine-candidates",
            "2",
        ],
    );
    ok(&o);
    let log = json(&dir.join("ascii_noir_refined.look.json"));
    assert_eq!(log["metric"], "glyphs", "{log}");
    assert_eq!(log["start"]["terms"][1]["term"], "glyph coverage");
    let tried = log["tried"].as_array().unwrap();
    assert!(!tried.is_empty());
    assert!(
        tried[0]["distance"]["glyph"]["coverage"].is_number(),
        "{log}"
    );
}

/// Without a reference there is nothing to read the grid against.
#[test]
fn reference_text_needs_a_reference() {
    let dir = dir("alone");
    let o = render(&dir.join("a.png"), &["--reference-text", "x.txt"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("--reference"), "{}", stderr(&o));
}
