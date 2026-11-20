//! #750's done-when, with the real binary on a small project:
//!
//! - a spline file with a mistake is named by file, line and column, by
//!   `flint check` (text and JSON) and by `flint render`'s warning;
//! - a `spline_mesh` with a non-rectangular `profile` sweeps, and draws;
//! - a profile that is no polygon is refused by name.
//!
//! The renders skip themselves on a machine without a GPU. That existing
//! tracks render pixel-identically was checked against main's binary on
//! FlintKart's six tracks (the PR has the table); nothing in this repo
//! sweeps a track to pin.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

/// A loop, with a mistake at line 10: the third point has no position.
const BAD_SPLINE: &str = "\
[spline]
closed = true

[[control_points]]
position = [0, 0, 6]

[[control_points]]
position = [6, 0, 0]

[[control_points]]
twist = 4.0

[[control_points]]
position = [-6, 0, 0]
";

const GOOD_SPLINE: &str = "\
[spline]
closed = true

[sampling]
spacing = 0.5

[[control_points]]
position = [0, 0, 6]

[[control_points]]
position = [6, 0, 0]

[[control_points]]
position = [0, 0, -6]

[[control_points]]
position = [-6, 0, 0]
";

fn scene(sweep: &str) -> String {
    format!(
        r#"[scene]
name = "Sweep"

[camera]
position = [0.0, 9.0, 11.0]
target = [0.0, 0.0, 0.0]
fov = 50.0

[entities.sun.light]
type = "directional"
direction = [0.3, 0.8, 0.4]
intensity = 2.5

[entities.ring.spline]
file = "splines/ring.spline.toml"
{sweep}"#
    )
}

/// A diamond, which no box can be.
const DIAMOND: &str = r#"
[entities.tube.spline_mesh]
spline = "ring"
profile = [[0.0, 0.6], [0.5, 0.0], [0.0, -0.6], [-0.5, 0.0]]
color = [0.9, 0.5, 0.2, 1.0]
"#;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A project of its own: the engine's schemas, a scene, a spline.
fn project(name: &str, spline: &str, sweep: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("spline_sweep_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("splines")).unwrap();
    let schemas = repo()
        .join("schemas")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
        .replace("//?/", "");
    std::fs::write(
        dir.join("flint.toml"),
        format!("[project]\nschemas = [\"{schemas}\"]\n"),
    )
    .unwrap();
    std::fs::write(dir.join("splines/ring.spline.toml"), spline).unwrap();
    std::fs::write(dir.join("sweep.scene.toml"), scene(sweep)).unwrap();
    dir
}

fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("flint runs")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Render `sweep.scene.toml` to `out`; `None` when there is no GPU.
fn render(dir: &Path, out: &str) -> Option<Output> {
    let o = flint(
        dir,
        &[
            "render",
            "sweep.scene.toml",
            "--output",
            out,
            "--width",
            "160",
            "--height",
            "120",
            "--no-grid",
        ],
    );
    if !o.status.success() && stderr(&o).contains("headless render context") {
        eprintln!("no GPU adapter: skipped");
        return None;
    }
    assert!(o.status.success(), "flint render failed: {}", stderr(&o));
    Some(o)
}

#[test]
fn flint_check_places_a_bad_spline_by_line_and_column() {
    let dir = project("check", BAD_SPLINE, DIAMOND);
    let o = flint(&dir, &["check", "sweep.scene.toml", "--json"]);
    assert!(!o.status.success(), "a bad spline is an error");
    let report: serde_json::Value = serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&o.stdout)));
    let found = report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "bad-spline")
        .unwrap_or_else(|| panic!("no bad-spline in {report:#}"));
    assert!(
        found["file"]
            .as_str()
            .unwrap()
            .ends_with("splines/ring.spline.toml"),
        "{found:#}"
    );
    assert_eq!(found["line"], 10, "{found:#}");
    assert!(found["column"].as_u64().is_some(), "{found:#}");
    assert!(
        found["message"].as_str().unwrap().contains("position"),
        "{found:#}"
    );

    // The text says the same, as file:line:col
    let o = flint(&dir, &["check", "sweep.scene.toml"]);
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(text.contains("error[bad-spline]"), "{text}");
    assert!(text.contains("10:"), "{text}");
}

#[test]
fn a_good_spline_and_profile_pass_flint_check() {
    let dir = project("clean", GOOD_SPLINE, DIAMOND);
    let o = flint(&dir, &["check", "sweep.scene.toml"]);
    assert!(
        o.status.success(),
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        stderr(&o)
    );
}

#[test]
fn flint_render_warns_with_the_file_line_and_column() {
    let dir = project("render_bad", BAD_SPLINE, DIAMOND);
    let Some(o) = render(&dir, "bad.png") else {
        return;
    };
    let err = stderr(&o);
    let line = err
        .lines()
        .find(|l| l.contains("ring.spline.toml:10:"))
        .unwrap_or_else(|| panic!("no file:line:col warning in:\n{err}"));
    assert!(line.contains("position"), "{line}");
}

#[test]
fn a_diamond_profile_sweeps_and_draws() {
    let bare = project("render_bare", GOOD_SPLINE, "");
    let swept = project("render_diamond", GOOD_SPLINE, DIAMOND);
    let (Some(_), Some(o)) = (render(&bare, "out.png"), render(&swept, "out.png")) else {
        return;
    };
    assert!(!stderr(&o).contains("spline_mesh"), "{}", stderr(&o));
    let a = image::open(bare.join("out.png")).unwrap().to_rgba8();
    let b = image::open(swept.join("out.png")).unwrap().to_rgba8();
    let changed = a
        .pixels()
        .zip(b.pixels())
        .filter(|(p, q)| p.0.iter().zip(q.0).any(|(x, y)| x.abs_diff(y) > 24))
        .count();
    // the ring covers a good share of a 160 x 120 frame
    assert!(changed > 1500, "only {changed} pixels changed");
}

#[test]
fn a_profile_that_is_no_polygon_is_refused_by_name() {
    let flat = r#"
[entities.tube.spline_mesh]
spline = "ring"
profile = [[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]]
"#;
    let dir = project("render_flat", GOOD_SPLINE, flat);
    let Some(o) = render(&dir, "flat.png") else {
        return;
    };
    let err = stderr(&o);
    assert!(
        err.contains("bad-spline") && err.contains("no area"),
        "the flat profile was not named:\n{err}"
    );
}

/// The ring with data along it (#855): ice from the first point to the
/// third, a gap after the second, speed 1 everywhere but from the third
/// point to the fourth.
const ATTR_SPLINE: &str = "\
[spline]
closed = true

[[control_points]]
position = [0, 0, 6]
attrs = { surface = \"ice\", speed = 1 }

[[control_points]]
position = [6, 0, 0]
gap_after = true
attrs = { surface = \"ice\", speed = 1 }

[[control_points]]
position = [0, 0, -6]
attrs = { speed = 0.5 }

[[control_points]]
position = [-6, 0, 0]
attrs = { speed = 1 }
";

/// A script that throws unless the spline's attributes read as written;
/// `surface` is what it expects at t = 0.1.
fn probe(surface: &str) -> String {
    format!(
        r#"fn on_update() {{
    let ring = get_entity("ring");
    if spline_point_attr(ring, "gap_after", 0.3) != true {{ throw "no gap at 0.3"; }}
    if spline_point_attr(ring, "gap_after", 0.6) != () {{ throw "a gap at 0.6"; }}
    let g = spline_point_attr_span(ring, "gap_after", 0.3);
    if g.start_t != 0.25 || g.end_t != 0.5 || g.value != true {{ throw `gap span ${{g}}`; }}
    if spline_point_attr(ring, "surface", 0.1) != "{surface}" {{ throw "surface at 0.1"; }}
    if spline_point_attr(ring, "surface", 0.6) != () {{ throw "a surface at 0.6"; }}
    let s = spline_point_attr_span(ring, "speed", 0.9);
    if s.value != 1 || s.start_t != 0.75 || s.end_t != 0.5 {{ throw `speed span ${{s}}`; }}
    if spline_point_attr(ring, "speed", 0.6) != 0.5 {{ throw "speed at 0.6"; }}
}}
"#
    )
}

/// `flint render --strict-scripts` on the probe; `None` with no GPU.
fn render_probe(name: &str, surface: &str) -> Option<Output> {
    let probe_entity = "\n[entities.probe.script]\nsource = \"probe.rhai\"\n";
    let dir = project(name, ATTR_SPLINE, probe_entity);
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(dir.join("scripts/probe.rhai"), probe(surface)).unwrap();
    let o = flint(
        &dir,
        &[
            "render",
            "sweep.scene.toml",
            "--output",
            "probe.png",
            "--width",
            "64",
            "--height",
            "48",
            "--no-grid",
            "--strict-scripts",
            "--time",
            "0.1",
        ],
    );
    if !o.status.success() && stderr(&o).contains("headless render context") {
        eprintln!("no GPU adapter: skipped");
        return None;
    }
    Some(o)
}

/// #855: a control point's `attrs`, and `gap_after`, reach a script through
/// the loader's `spline_data`, as spans that wrap through the first point;
/// a wrong expectation fails, so the probe is known to have run.
#[test]
fn a_script_reads_the_attributes_along_the_spline() {
    let Some(o) = render_probe("attrs", "ice") else {
        return;
    };
    assert!(o.status.success(), "the probe failed:\n{}", stderr(&o));
    let Some(o) = render_probe("attrs_wrong", "sand") else {
        return;
    };
    assert!(!o.status.success(), "a wrong expectation passed");
    assert!(stderr(&o).contains("surface at 0.1"), "{}", stderr(&o));
}
