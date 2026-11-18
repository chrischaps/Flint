//! End-to-end tests for `flint render --time`, `--sweep`, `--report json` and GIF
//! output (#216), and the one-renderer flags the clocked path takes since #596:
//! run the real binary on a two-sapling scene and hold what it writes to the
//! ticket. Those that need a GPU skip themselves on a machine without one.
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

[entities.sun.light]
type = "directional"
direction = [-0.6, 0.5, 0.4]
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

/// Where `cell` sits in `sheet`, pixel for pixel: its top-left corner, or
/// `None` when no region of the sheet is that image.
fn find_cell(sheet: &image::RgbImage, cell: &image::RgbImage) -> Option<(u32, u32)> {
    let (w, h) = cell.dimensions();
    (0..=sheet.height().saturating_sub(h))
        .flat_map(|y| (0..=sheet.width().saturating_sub(w)).map(move |x| (x, y)))
        .find(|&(x, y)| {
            (0..h).all(|cy| {
                (0..w).all(|cx| sheet.get_pixel(x + cx, y + cy) == cell.get_pixel(cx, cy))
            })
        })
}

/// #531's done-when: one command draws a labelled spread comparison whose
/// columns are, pixel for pixel, renders of the scene with those `[living]
/// echoes`. The scene copies sit beside the original, so they find its spec.
#[test]
fn an_echoes_sweep_draws_what_scenes_with_those_echoes_draw() {
    let dir = project("echoes_sweep");
    let spreads = ["0", "0.9", "1.8"];
    let o = render(
        &dir,
        "sweep.png",
        &[
            "--time",
            "1.3",
            "--sweep",
            "living.echoes.spread=0,0.9,1.8",
            "--report",
            "json",
        ],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let report: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(report["sweep"], "living.echoes.spread");
    let sheet = image::open(dir.join("sweep.png")).unwrap().into_rgb8();
    assert_eq!(sheet.width(), sheet_width(3, 64));
    assert!(
        !stderr(&o).contains("pixel-identical"),
        "the spread showed nothing: {}",
        stderr(&o)
    );

    let mut corners = Vec::new();
    for (i, spread) in spreads.iter().enumerate() {
        // The scene with the column's spread in its own [living] table
        let scene = SCENE.replace(
            "[camera]",
            &format!("[living]\nechoes = {{ spread = {spread} }}\n\n[camera]"),
        );
        let name = format!("spread_{i}.scene.toml");
        std::fs::write(dir.join(&name), scene).unwrap();
        let alone = std::process::Command::new(FLINT)
            .arg("render")
            .arg(dir.join(&name))
            .arg("--schemas")
            .arg(schemas())
            .args(["--width", "64", "--height", "48", "--time", "1.3", "-o"])
            .arg(dir.join(format!("spread_{i}.png")))
            .output()
            .expect("run flint");
        ok(&alone);
        let cell = image::open(dir.join(format!("spread_{i}.png")))
            .unwrap()
            .into_rgb8();
        let at = find_cell(&sheet, &cell)
            .unwrap_or_else(|| panic!("spread {spread}: no cell of the sheet is its render"));
        corners.push(at);
    }
    // Each in its own column, left to right, on one row
    assert!(
        corners
            .windows(2)
            .all(|p| p[0].0 < p[1].0 && p[0].1 == p[1].1),
        "{corners:?}"
    );
    let first = image::open(dir.join("spread_0.png")).unwrap();
    let last = image::open(dir.join("spread_2.png")).unwrap();
    assert_ne!(first, last, "spread 0 and 1.8 drew the same");
}

/// An echoes sweep in a scene with nothing to echo is refused before a GPU wakes.
#[test]
fn an_echoes_sweep_needs_living_props() {
    let dir = project("echoes_none");
    let bare = "[scene]\nname = \"Bare\"\n[entities.sun.light]\ntype = \"directional\"\n";
    std::fs::write(dir.join("grove.scene.toml"), bare).unwrap();
    let o = render(&dir, "a.png", &["--sweep", "living.echoes.count=0,4"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("no living props"), "{}", stderr(&o));
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

/// #574's done-when: a span is one labelled contact sheet in one command,
/// and each tile is, pixel for pixel, `--time <its t>` rendered alone at the
/// tile's size.
#[test]
fn a_strip_is_a_sheet_of_its_moments_each_as_rendered_alone() {
    let dir = project("strip_sheet");
    let ascii = ["--style", "ascii"];
    let o = render(
        &dir,
        "strip.png",
        &[
            &["--time", "0..2", "--strip", "--every", "1", "--cols", "2"][..],
            &ascii,
        ]
        .concat(),
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let said = String::from_utf8_lossy(&o.stdout);
    assert!(said.contains("3 tiles of 64x48"), "{said}");
    let sheet = image::open(dir.join("strip.png")).unwrap().into_rgb8();
    assert_eq!(sheet.width(), sheet_width(2, 64));

    let mut corners = Vec::new();
    for t in ["0", "1", "2"] {
        let name = format!("alone_{t}.png");
        ok(&render(&dir, &name, &[&["--time", t][..], &ascii].concat()));
        let cell = image::open(dir.join(&name)).unwrap().into_rgb8();
        let at = find_cell(&sheet, &cell)
            .unwrap_or_else(|| panic!("t = {t}: no tile of the strip is its render alone"));
        corners.push(at);
    }
    // 0 and 1 along the first row, 2 under 0
    assert!(
        corners[0].1 == corners[1].1 && corners[0].0 < corners[1].0,
        "{corners:?}"
    );
    assert!(
        corners[2].0 == corners[0].0 && corners[2].1 > corners[0].1,
        "{corners:?}"
    );
    let read = |name: &str| std::fs::read(dir.join(name)).unwrap();
    assert_ne!(
        read("alone_0.png"),
        read("alone_2.png"),
        "the props never moved"
    );
}

/// `--label off`: the tiles alone, 4 pixels apart, so a tile is cut back out
/// by arithmetic. A list's moments make a strip as a span's do. The ASCII
/// style keeps a history (#277) that a GIF's frames follow on from; at this
/// size and 0.05 s apart, a capture that followed on from the one before
/// differs from the render alone (a `--time` list's stills do, measured
/// 2026-11-18), so this holds each tile to being drawn as a first frame.
#[test]
fn a_strip_without_labels_is_its_tiles_four_pixels_apart() {
    let dir = project("strip_plain");
    let run = |out: &str, args: &[&str]| {
        std::process::Command::new(FLINT)
            .arg("render")
            .arg(dir.join("grove.scene.toml"))
            .arg("--schemas")
            .arg(schemas())
            .args([
                "--width", "320", "--height", "240", "--style", "ascii", "-o",
            ])
            .arg(dir.join(out))
            .args(args)
            .output()
            .expect("run flint")
    };
    let times = ["0", "0.05", "0.1"];
    let o = run(
        "plain.png",
        &["--time", &times.join(","), "--strip", "--label", "off"],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let sheet = image::open(dir.join("plain.png")).unwrap().into_rgb8();
    // Three tiles are two across: the fewest that keep the sheet no taller than wide
    assert_eq!(sheet.dimensions(), (2 * 320 + 4, 2 * 240 + 4));
    for (i, t) in times.iter().enumerate() {
        let name = format!("alone_{i}.png");
        ok(&run(&name, &["--time", t]));
        let alone = image::open(dir.join(&name)).unwrap().into_rgb8();
        let (x, y) = ((i as u32 % 2) * 324, (i as u32 / 2) * 244);
        let tile = image::imageops::crop_imm(&sheet, x, y, 320, 240).to_image();
        assert!(tile == alone, "tile {i} isn't --time {t} alone");
    }
}

/// The flags the one-renderer path took and the clocked one refused until #596.
/// A grid of two wooze knobs: a stage a cell, each cell reporting both its
/// values, and each value reaching the props.
#[test]
fn two_sweeps_on_the_clock_make_a_grid() {
    let dir = project("clock_grid");
    let o = render(
        &dir,
        "grid.png",
        &[
            "--time",
            "1.3",
            "--sweep",
            "wooze.glide=0,1",
            "--sweep",
            "wooze.step=0.25,1",
            "--report",
            "json",
        ],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let report: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(report["sweep"], "wooze.glide x wooze.step");
    let captures = report["captures"].as_array().unwrap();
    let cells: Vec<(&str, &str)> = captures
        .iter()
        .map(|c| (c["column"].as_str().unwrap(), c["row"].as_str().unwrap()))
        .collect();
    assert_eq!(
        cells,
        [("0", "0.25"), ("1", "0.25"), ("0", "1"), ("1", "1")]
    );
    for capture in captures {
        let wooze = &capture["props"][0]["wooze"];
        let glide: f64 = capture["column"].as_str().unwrap().parse().unwrap();
        let step: f64 = capture["row"].as_str().unwrap().parse().unwrap();
        assert!((wooze["glide"].as_f64().unwrap() - glide).abs() < 1e-6);
        assert!((wooze["step"].as_f64().unwrap() - step).abs() < 1e-6);
    }
    // Two columns of 64 beside a row-label band, two rows down
    let sheet = image::open(dir.join("grid.png")).unwrap().into_rgb8();
    assert!(sheet.width() > sheet_width(2, 64), "{}", sheet.width());
    assert!(sheet.height() > 2 * 48);
}

/// A crop keeps a region of each full-size frame, 1:1.
#[test]
fn a_clocked_sweep_can_be_cropped() {
    let dir = project("clock_crop");
    let o = render(
        &dir,
        "crop.png",
        &[
            "--time",
            "1.3",
            "--sweep",
            "wooze.glide=0,1",
            "--sweep-crop",
            "16,12,32,24",
        ],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let sheet = image::open(dir.join("crop.png")).unwrap().into_rgb8();
    assert_eq!(sheet.width(), sheet_width(2, 32));

    // #501: the same region in fractions of the 64x48 frame is the same
    // sheet, locator and all
    let o = render(
        &dir,
        "fraction.png",
        &[
            "--time",
            "1.3",
            "--sweep",
            "wooze.glide=0,1",
            "--sweep-crop",
            "0.25,0.25,0.5,0.5",
        ],
    );
    ok(&o);
    let fraction = image::open(dir.join("fraction.png")).unwrap().into_rgb8();
    assert!(
        fraction == sheet,
        "a crop in fractions draws the pixel crop's sheet"
    );
}

/// A still of the moment is its own reference: distance 0, and in a sweep the
/// column that draws it is the nearest, in the measures file and the JSON.
#[test]
fn a_clocked_render_is_measured_against_a_reference() {
    let dir = project("clock_reference");
    let o = render(&dir, "ref.png", &["--time", "1.3"]);
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let reference = dir.join("ref.png");
    let reference = reference.to_str().unwrap();
    let measures = dir.join("m.json");
    let o = render(
        &dir,
        "sweep.png",
        &[
            "--time",
            "1.3",
            "--sweep",
            "wooze.glide=0,0.7",
            "--reference",
            reference,
            "--report",
            measures.to_str().unwrap(),
        ],
    );
    ok(&o);
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&measures).unwrap()).unwrap();
    assert_eq!(report["sweep"], "wooze.glide");
    assert_eq!(report["nearest"], 1, "{report}");
    let score = report["columns"][1]["distance"]["score"].as_f64().unwrap();
    assert!(
        score < 0.01,
        "the scene's own glide drew {score} from its still"
    );
    assert!(
        report["columns"][0]["image"]["mean_luminance"].is_number(),
        "{report}"
    );

    let o = render(
        &dir,
        "sweep.png",
        &[
            "--time",
            "1.3",
            "--sweep",
            "wooze.glide=0,0.7",
            "--reference",
            reference,
            "--report",
            "json",
        ],
    );
    ok(&o);
    let report: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let far = report["captures"][0]["distance"].as_f64().unwrap();
    let near = report["captures"][1]["distance"].as_f64().unwrap();
    assert!(near < far, "{near} {far}");
}

/// The ASCII grid as text: one file for a moment, a file a frame for a span.
#[test]
fn a_clocked_render_writes_its_ascii_grid() {
    let dir = project("clock_text");
    let text = dir.join("grid.txt");
    let o = render(
        &dir,
        "a.png",
        &[
            "--time",
            "1.3",
            "--style",
            "ascii",
            "--ascii-text",
            text.to_str().unwrap(),
        ],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let grid = std::fs::read_to_string(&text).unwrap();
    assert!(grid.lines().count() >= 2, "{grid:?}");

    let o = render(
        &dir,
        "span.gif",
        &[
            "--time",
            "0..1",
            "--fps",
            "2",
            "--style",
            "ascii",
            "--ascii-text",
            text.to_str().unwrap(),
        ],
    );
    ok(&o);
    for frame in ["grid_0000.txt", "grid_0001.txt"] {
        assert!(dir.join(frame).is_file(), "{frame} missing: {}", stderr(&o));
    }
}

/// `--frames` on the clock: a PNG a frame `--frame-dt` of game time apart, the
/// camera orbiting, and the ASCII flips counted.
#[test]
fn frames_on_the_clock_move_the_props_and_count_flips() {
    let dir = project("clock_frames");
    let flips = dir.join("flips.json");
    let o = render(
        &dir,
        "seq.png",
        &[
            "--frames",
            "3",
            "--frame-dt",
            "0.5",
            "--orbit",
            "20",
            "--style",
            "ascii",
            "--flip-json",
            flips.to_str().unwrap(),
        ],
    );
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let read = |name: &str| std::fs::read(dir.join(name)).unwrap();
    assert_ne!(read("seq_0000.png"), read("seq_0002.png"), "nothing moved");
    let stats: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&flips).unwrap()).unwrap();
    assert_eq!(stats["frames"], 3, "{stats}");
}

/// `--last-frame` (#478) writes only where a sequence ends, to `-o` itself,
/// and it is the numbered run's last frame byte for byte: on the clock (this
/// grove's living props) and on the still path (a plain fixture scene).
#[test]
fn last_frame_is_the_sequences_last_frame_alone() {
    let dir = project("last_frame");
    let seq = [
        "--frames",
        "3",
        "--frame-dt",
        "0.5",
        "--orbit",
        "20",
        "--style",
        "ascii",
    ];
    let o = render(&dir, "seq.png", &seq);
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    ok(&render(
        &dir,
        "last.png",
        &[&seq[..], &["--last-frame"]].concat(),
    ));
    let read = |name: &str| std::fs::read(dir.join(name)).unwrap();
    assert_eq!(read("last.png"), read("seq_0002.png"), "on the clock");
    assert!(!dir.join("last_0000.png").exists(), "a numbered frame too");

    let still = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/scenes/pbr_test.scene.toml");
    let plain = |out: &str, last: bool| {
        let o = std::process::Command::new(FLINT)
            .arg("render")
            .arg(&still)
            .args(["--width", "64", "--height", "48", "--no-grid"])
            .args(seq)
            .args(last.then_some("--last-frame"))
            .arg("-o")
            .arg(dir.join(out))
            .output()
            .expect("run flint");
        ok(&o);
    };
    plain("plain.png", false);
    plain("plain_last.png", true);
    assert_eq!(read("plain_last.png"), read("plain_0002.png"), "still path");
    assert!(!dir.join("plain_last_0000.png").exists());

    // Nothing to write and only the last frame to write contradict
    let o = render(
        &dir,
        "x.png",
        &[&seq[..], &["--last-frame", "--no-images"]].concat(),
    );
    assert!(!o.status.success());
    assert!(stderr(&o).contains("--no-images"), "{}", stderr(&o));
}

/// `--gpu-time` times the first cell's capture: a spread a pass (#292), and
/// with `--gpu-time-json` every kept frame's raw time beside it.
#[test]
fn a_clocked_render_can_be_timed_on_the_gpu() {
    let dir = project("clock_gpu_time");
    let json = dir.join("gpu.json");
    let o = render(
        &dir,
        "a.png",
        &[
            "--time",
            "1.3",
            "--gpu-time",
            "20",
            "--gpu-time-json",
            json.to_str().unwrap(),
        ],
    );
    if no_gpu(&o) {
        return;
    }
    if stderr(&o).contains("TIMESTAMP_QUERY") {
        eprintln!("no GPU timestamps: skipped");
        return;
    }
    ok(&o);
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("GPU time over 20 frames"), "{out}");
    assert!(
        out.contains("20 frames timed, the first 2 dropped"),
        "{out}"
    );
    let header = out.lines().find(|l| l.trim_start().starts_with("pass"));
    let header: Vec<&str> = header.expect(&out).split_whitespace().collect();
    assert_eq!(header, ["pass", "min", "p50", "p90", "max", "ms"]);

    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
    assert_eq!(doc["frames_drawn"], 20);
    assert_eq!(doc["frames_kept"], 18);
    assert!(doc["adapter"]["backend"].is_string(), "{doc}");
    let passes = doc["passes"].as_array().unwrap();
    assert!(passes.iter().any(|p| p["label"] == "Scene"), "{doc}");
    for p in passes.iter().chain([&doc["frame"]]) {
        let at = |k: &str| p[k].as_f64().unwrap();
        assert!(at("min_ms") <= at("p50_ms") && at("p50_ms") <= at("p90_ms"));
        assert!(at("p90_ms") <= at("max_ms"), "{p}");
        let samples = p["samples_ms"].as_array().unwrap().len();
        assert!((1..=18).contains(&samples), "{p}");
    }
    assert_eq!(doc["frame"]["samples_ms"].as_array().unwrap().len(), 18);
}

/// `--gpu-time-json` means nothing without `--gpu-time`; clap says so.
#[test]
fn gpu_time_json_needs_gpu_time() {
    let dir = project("gpu_time_json_alone");
    let o = render(&dir, "a.png", &["--gpu-time-json", "x.json"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("--gpu-time"), "{}", stderr(&o));
}

/// `--ai-refine` on the clock, with the free offline agent: each candidate is
/// the moment redrawn under its look.
#[test]
fn a_look_is_refined_at_a_moment() {
    let dir = project("clock_refine");
    let o = render(&dir, "ref.png", &["--time", "1.3"]);
    if no_gpu(&o) {
        return;
    }
    ok(&o);
    let reference = dir.join("ref.png");
    let o = render(
        &dir,
        "rounds.png",
        &[
            "--time",
            "1.3",
            "--look",
            "ascii_noir",
            "--reference",
            reference.to_str().unwrap(),
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
    assert!(dir.join("rounds.png").is_file());
    assert!(dir.join("ascii_noir_refined.look.toml").is_file());
    assert!(dir.join("ascii_noir_refined.look.json").is_file());
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

    // A strip (#574) is one PNG of several moments
    refused("a.gif", &["--time", "0..4", "--strip"], ".png");
    refused("f-%04d.png", &["--time", "0..4", "--strip"], "plain .png");
    refused("a.png", &["--time", "3", "--strip"], "is one");
    refused("a.png", &["--strip"], "--time");
    refused(
        "a.png",
        &["--time", "1,2", "--strip", "--every", "1"],
        "drop --every",
    );
    refused("a.png", &["--time", "0..900", "--strip"], "at most 100");
    refused(
        "a.png",
        &["--time", "0..2", "--strip", "--sweep", "wooze.glide=0,1"],
        "cannot be used with",
    );
    refused("a.png", &["--time", "0..2", "--every", "1"], "--strip");

    // What a clocked render can't mean (#596)
    let reference = dir.join("tiny.png");
    image::RgbImage::new(4, 4).save(&reference).unwrap();
    let reference = reference.to_str().unwrap();
    let span = ["--time", "0..1", "--reference", reference];
    refused("a.gif", &span, "one moment");
    refused(
        "a.png",
        &["--time", "1,2", "--report", "m.json"],
        "one moment",
    );
    refused(
        "a.png",
        &[
            "--time",
            "1,2",
            "--look",
            "ascii_noir",
            "--reference",
            reference,
            "--ai-refine",
        ],
        "one moment",
    );
    refused(
        "a.png",
        &["--sweep", "wooze.glide=0,1", "--ascii-text", "t.txt"],
        "drop --sweep",
    );
    refused(
        "a.png",
        &["--sweep", "wooze.glide=0,1", "--frames", "2"],
        "drop --sweep",
    );
}
