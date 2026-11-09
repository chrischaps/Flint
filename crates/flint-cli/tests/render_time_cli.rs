//! End-to-end tests for `flint render --time`, `--sweep`, `--report json` and GIF
//! output (#216): run the real binary on a two-sapling scene and hold what it writes
//! to the ticket. Those that need a GPU skip themselves on a machine without one.
//!
//! What this can't test is the ticket's other half, that a capture matches the
//! player's at the same game time, since the player needs a window. The book's
//! headless guide says how that was measured.

use image::codecs::gif::GifDecoder;
use image::AnimationDecoder;
use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SPEC: &str = r#"
generator = "tree_v1"
[meta]
name = "sapling"
version = "1.0.0"
[seed]
mode = "fixed"
value = 1
[params]
branch_method = "lsystem"
lsystem_iterations = 2
leaf_density = 0.0
[living]
lineage_seed = 7
pose_ring = 4
echoes = { count = 2 }
"#;

const SCENE: &str = r#"
[scene]
name = "Two Saplings"

[wooze]
step = 0.5
glide = 0.7

[camera]
position = [0.0, 1.5, 6.0]
target = [0.0, 1.0, 0.0]
fov = 50.0

[entities.sun.directional_light]
direction = [0.6, -0.5, -0.4]
color = [1.0, 0.9, 0.8]
intensity = 2.5

[entities.first.transform]
position = [-1.5, 0.0, 0.0]
[entities.first.model]
asset = "sapling"
[entities.first.living_prop]
lineage_seed = 3

[entities.second.transform]
position = [1.5, 0.0, 0.0]
[entities.second.model]
asset = "sapling"
[entities.second.living_prop]
lineage_seed = 4
"#;

/// A project holding the scene and its spec, in a directory of its own.
fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("render_time_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("specs")).unwrap();
    std::fs::write(dir.join("specs/sapling.procgen.toml"), SPEC).unwrap();
    std::fs::write(dir.join("grove.scene.toml"), SCENE).unwrap();
    dir
}

fn schemas() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas")
}

fn render(dir: &Path, out: &str, args: &[&str]) -> Output {
    std::process::Command::new(FLINT)
        .arg("render")
        .arg(dir.join("grove.scene.toml"))
        .arg("--schemas")
        .arg(schemas())
        .arg("--width")
        .arg("64")
        .arg("--height")
        .arg("48")
        .arg("-o")
        .arg(dir.join(out))
        .args(args)
        .output()
        .expect("run flint")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Whether this run had no GPU to draw with, which isn't this test's business.
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

fn gif_frames(path: &Path) -> Vec<image::Frame> {
    let file = std::io::BufReader::new(std::fs::File::open(path).unwrap());
    GifDecoder::new(file)
        .unwrap()
        .into_frames()
        .collect_frames()
        .unwrap()
}

#[test]
fn the_same_time_gives_the_same_bytes_and_another_time_doesnt() {
    let dir = project("same_bytes");
    let first = render(&dir, "a.png", &["--time", "1.3"]);
    if no_gpu(&first) {
        return;
    }
    ok(&first);
    ok(&render(&dir, "b.png", &["--time", "1.3"]));
    ok(&render(&dir, "c.png", &["--time", "1.7"]));
    let read = |name: &str| std::fs::read(dir.join(name)).unwrap();
    assert_eq!(read("a.png"), read("b.png"));
    assert_ne!(read("a.png"), read("c.png"), "the props never moved");
}

#[test]
fn the_report_is_all_of_stdout_and_says_what_each_prop_was_doing() {
    let dir = project("report");
    let o = render(&dir, "a.png", &["--time", "1.3", "--report", "json"]);
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let report: serde_json::Value = serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("stdout isn't JSON ({e}): {:?}", o.stdout));
    assert_eq!(report["scene"], "Two Saplings");
    assert_eq!(report["wooze_field"], true);

    let capture = &report["captures"][0];
    assert_eq!(capture["time"], 1.3);
    let props = capture["props"].as_array().unwrap();
    assert_eq!(props.len(), 2, "every living prop");
    assert_eq!(props[0]["entity"], "first");
    assert_eq!(props[0]["lineage_seed"], 3);
    assert_eq!(props[1]["lineage_seed"], 4);
    for prop in props {
        assert!(prop["wooze"]["glide"].as_f64().unwrap() > 0.69);
        assert_eq!(prop["wooze"]["step"], 0.5);
        // 1.3 s at half a second a step is 2.6 steps past wherever its phase began.
        let index = prop["poses"]["index"].as_u64().unwrap();
        assert!((2..=3).contains(&index), "pose {index}");
        assert!(prop["poses"]["parts"].as_u64().unwrap() > 0);
    }
    assert!(dir.join("a.png").metadata().unwrap().len() > 0);
}

#[test]
fn a_glide_sweep_is_one_labelled_image_with_a_column_a_value() {
    let dir = project("sweep");
    let sweep = "wooze.glide=0,0.25,0.5,1";
    let o = render(
        &dir,
        "sweep.png",
        &["--time", "1.3", "--sweep", sweep, "--report", "json"],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let sheet = image::open(dir.join("sweep.png")).unwrap().into_rgb8();
    // Four framed columns and three gutters, between margins (style_sheet's
    // geometry since one sheet drew every sweep, #487).
    assert_eq!(sheet.width(), sheet_width(4, 64));
    assert!(sheet.height() > 48);

    let report: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(report["sweep"], "wooze.glide");
    let captures = report["captures"].as_array().unwrap();
    let columns: Vec<&str> = captures
        .iter()
        .map(|c| c["column"].as_str().unwrap())
        .collect();
    assert_eq!(columns, ["0", "0.25", "0.5", "1"]);
    let glide = captures[1]["props"][0]["wooze"]["glide"].as_f64().unwrap();
    assert!((glide - 0.25).abs() < 1e-6, "the column's value reached it");
}

#[test]
fn a_sweep_that_changes_nothing_says_so() {
    let dir = project("same_columns");
    let o = render(
        &dir,
        "sweep.png",
        &["--time", "1.3", "--sweep", "wooze.glide=0.5,0.5"],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    assert!(stderr(&o).contains("pixel-identical"), "{}", stderr(&o));
}

#[test]
fn a_span_makes_a_gif_of_every_frame() {
    let dir = project("gif");
    let o = render(&dir, "span.gif", &["--time", "0..1", "--fps", "4"]);
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let frames = gif_frames(&dir.join("span.gif"));
    assert_eq!(frames.len(), 4);
    assert_eq!(frames[0].buffer().width(), 64);
    assert_ne!(frames[0].buffer(), frames[2].buffer(), "nothing moved");
}

#[test]
fn a_swept_span_animates_the_whole_sheet() {
    let dir = project("sweep_gif");
    let o = render(
        &dir,
        "sweep.gif",
        &[
            "--time",
            "0..1",
            "--fps",
            "2",
            "--sweep",
            "wooze.preset=boil,morph",
        ],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let frames = gif_frames(&dir.join("sweep.gif"));
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].buffer().width(), sheet_width(2, 64));
}

/// A style sweep draws through the same stages as a wooze one when it needs
/// clocks (#487): one --sweep flag, a JSON report with each capture's column,
/// and a GIF of the sheet.
#[test]
fn a_style_sweep_reports_and_animates_as_a_wooze_one_does() {
    let dir = project("style_sweep");
    let o = render(
        &dir,
        "style.png",
        &["--sweep", "ascii.cell=6,8", "--report", "json"],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let report: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(report["sweep"], "ascii.cell");
    let columns: Vec<&str> = report["captures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["column"].as_str().unwrap())
        .collect();
    assert_eq!(columns, ["6", "8"]);
    let sheet = image::open(dir.join("style.png")).unwrap().into_rgb8();
    assert_eq!(sheet.width(), sheet_width(2, 64));

    let o = render(
        &dir,
        "style.gif",
        &["--time", "0..1", "--fps", "2", "--sweep", "ascii.cell=6,8"],
    );
    ok(&o);
    let frames = gif_frames(&dir.join("style.gif"));
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].buffer().width(), sheet_width(2, 64));
}

/// style_sheet's width for `columns` cells `cell` wide: each cell framed by a
/// pixel a side, 16 between them, 24 at each edge.
fn sheet_width(columns: u32, cell: u32) -> u32 {
    2 * 24 + columns * (cell + 2) + (columns - 1) * 16
}

#[test]
fn a_strip_can_be_a_gif() {
    let dir = project("strip_gif");
    let out = dir.join("strip.gif");
    let o = std::process::Command::new(FLINT)
        .arg("gen")
        .arg(dir.join("specs/sapling.procgen.toml"))
        .args(["--strip", "--frames", "3", "--cell-size", "64", "-o"])
        .arg(&out)
        .output()
        .expect("run flint");
    if !o.status.success() && stderr(&o).contains("headless render context") {
        eprintln!("no GPU adapter: skipped");
        return;
    }
    ok(&o);
    let frames = gif_frames(&out);
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0].buffer().width(), 64);
}

/// These fail before anything is loaded, so they need no GPU.
#[test]
fn what_cant_be_written_is_refused_and_says_why() {
    let dir = project("refused");
    let refused = |out: &str, args: &[&str], says: &str| {
        let o = render(&dir, out, args);
        assert!(!o.status.success(), "{args:?} was let through");
        assert!(stderr(&o).contains(says), "{args:?}: {}", stderr(&o));
    };
    refused("a.png", &["--time", "0..4"], ".gif");
    refused("a.gif", &["--time", "4"], "span");
    refused("a.gif", &[], "span");
    refused(
        "a.png",
        &["--sweep", "wooze.glide=0,2"],
        "glide = 2 must be",
    );
    refused("a.png", &["--sweep", "wooze.preset=boil,nope"], "linklater");
    refused("a.png", &["--sweep", "fog.density=1"], "at least two");
    refused("a.png", &["--time", "soon"], "'soon'");
    refused("a.png", &["--frame-step", "0.1"], "--time");
}
