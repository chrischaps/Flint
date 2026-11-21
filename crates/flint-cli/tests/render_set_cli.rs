//! `flint render --set` and the per-effect flags it replaced (#708), through
//! the real binary: an old flag is an unknown-flag error naming what to
//! write (#1144); a `--set` that names no key, or a value of the wrong kind,
//! fails before any GPU work; and `--help` is grouped, with no old flag in
//! it. That each old flag's spelling parses and builds a config is
//! `render_retired`'s unit tests.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn scene() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/scenes/stylize_props_plain.scene.toml")
}

fn out(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("render_set_cli");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn render(args: &[&str], name: &str) -> Output {
    std::process::Command::new(FLINT)
        .arg("render")
        .arg(scene())
        .args(["--width", "160", "--height", "90", "-o"])
        .arg(out(name))
        .args(args)
        .output()
        .expect("run flint")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn an_old_flag_is_an_error_naming_what_to_write() {
    let o = render(&["--exposure", "1"], "exposure.png");
    assert_eq!(o.status.code(), Some(2), "clap's exit for an unknown flag");
    let err = stderr(&o);
    assert!(err.contains("unexpected argument '--exposure'"), "{err}");
    assert!(err.contains("--set post.exposure=1"), "{err}");
    assert!(!out("exposure.png").exists(), "nothing is drawn");

    // Every old flag on the line, in order
    let o = render(&["--fog-density", "0.05", "--ascii-cell", "12"], "old.png");
    let err = stderr(&o);
    let fog = err
        .find("--fog-density 0.05  ->  --set post.fog_density=0.05 --set post.fog_enabled=true")
        .unwrap_or_else(|| panic!("{err}"));
    // (the old flags are a column, padded to the longest)
    let cell = err
        .find("--ascii-cell 12     ->  --set ascii.cell=12")
        .unwrap_or_else(|| panic!("{err}"));
    assert!(fog < cell, "{err}");

    // Under --json, the error document carries it
    let o = std::process::Command::new(FLINT)
        .args(["--json", "render"])
        .arg(scene())
        .args(["--exposure", "1"])
        .output()
        .unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&o.stdout).expect("one document");
    let usage = doc["error"]["usage"].as_str().unwrap_or_default();
    assert!(usage.contains("--set post.exposure=1"), "{doc}");
}

#[test]
fn a_set_naming_no_key_fails_before_the_gpu() {
    let o = render(&["--set", "post.exposur=2"], "typo.png");
    assert!(!o.status.success());
    let err = stderr(&o);
    assert!(err.contains("did you mean post.exposure"), "{err}");

    let o = render(&["--set", "post.fog_color=1,2"], "kind.png");
    assert!(!o.status.success());
    assert!(stderr(&o).contains("three numbers"), "{}", stderr(&o));

    // A style key is checked once the scene's user styles are in
    let o = render(&["--set", "ascii.cel=12"], "style.png");
    assert!(!o.status.success());
    assert!(stderr(&o).contains("--set ascii.cel=12"), "{}", stderr(&o));
}

#[test]
fn a_set_of_a_swept_key_is_refused() {
    let o = render(
        &["--set", "post.exposure=2", "--sweep", "post.exposure=1,2"],
        "swept.png",
    );
    assert!(!o.status.success());
    assert!(stderr(&o).contains("drop the --set"), "{}", stderr(&o));
}

#[test]
fn help_is_grouped_and_the_old_flags_are_hidden() {
    let o = std::process::Command::new(FLINT)
        .args(["render", "--help"])
        .output()
        .unwrap();
    assert!(o.status.success());
    let help = String::from_utf8_lossy(&o.stdout);
    for heading in [
        "Output:",
        "Camera:",
        "Styles and post-processing:",
        "Quality and lighting:",
        "Time:",
        "Sequences:",
        "Sweep and variants:",
        "Measure and report:",
        "Refine a look:",
        "Debug, lists, GPU and schemas:",
    ] {
        assert!(help.lines().any(|l| l == heading), "no {heading}\n{help}");
    }
    assert!(help.contains("--set <KEY=VALUE>"), "{help}");
    for hidden in [
        "--bloom-intensity",
        "--ascii-cell",
        "--contour-width",
        "--fxaa",
    ] {
        assert!(!help.contains(hidden), "{hidden} is listed\n{help}");
    }
}
