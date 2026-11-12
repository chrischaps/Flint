//! The stylize goldens (#271): `flint goldens
//! tests/golden/stylize.goldens.toml`, through the real binary. Every
//! registered style (full-screen ASCII at 1x, 2x and MSAA 4, per-entity
//! ASCII, the entity style mask, invert, Kuwahara) is rendered
//! and compared with its checked-in PNG; the tolerance is the manifest's
//! (#265's: 2% of pixels past 48 levels, or a mean past 3).
//!
//! Without a GPU adapter the suite skips every case and passes, unless
//! `FLINT_REQUIRE_GPU=1`, which CI's WARP job sets (#314), so there a
//! skip is a failure. A golden may have a variant per adapter beside it
//! (`<stem>.warp.png`, ...); a case compares with its adapter's when there
//! is one. `FLINT_BLESS=1 cargo test -p flint-cli --test render_goldens`
//! rewrites the goldens after a deliberate change; look at them before
//! committing.
//!
//! Each run writes its renders, diffs, `report.json` and contact sheet to
//! its own folder, `$CARGO_TARGET_TMPDIR/stylize-goldens-<pid>` (#421: two
//! runs sharing one fixed folder read each other's half-written output and
//! failed on a tree where each passes alone). It is removed when the run
//! passes and kept, with its path printed, when it fails.
//! `FLINT_GOLDENS_OUT=<dir>` names the folder instead and always keeps it:
//! CI's WARP job sets it, so its summary and artifact have one path to read.

use std::path::{Path, PathBuf};
use std::process::Command;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

/// Where this run's goldens output goes, and whether to keep it on a pass.
fn goldens_out_dir() -> (PathBuf, bool) {
    match std::env::var_os("FLINT_GOLDENS_OUT") {
        Some(dir) if !dir.is_empty() => (PathBuf::from(dir), true),
        _ => (run_dir("stylize-goldens"), false),
    }
}

/// `$CARGO_TARGET_TMPDIR/<stem>-<pid>`, emptied first: a pid can come round
/// again, and a folder kept from a failed run must not mix with this one.
fn run_dir(stem: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{stem}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[test]
fn stylize_goldens_hold() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/stylize.goldens.toml");
    let (out, keep) = goldens_out_dir();
    eprintln!("goldens out-dir: {}", out.display());
    let mut cmd = Command::new(FLINT);
    cmd.arg("goldens").arg(&manifest).arg("--out-dir").arg(&out);
    if std::env::var_os("FLINT_BLESS").is_some() {
        cmd.arg("--bless");
    }
    let run = cmd.output().expect("run flint");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    eprintln!("{text}");
    assert!(
        run.status.success(),
        "stylize goldens drifted:\n{text}\nrenders, diffs and index.html kept in {}",
        out.display()
    );
    if !keep {
        let _ = std::fs::remove_dir_all(&out);
    }
}

fn help(command: &str) -> String {
    let run = Command::new(FLINT)
        .args([command, "--help"])
        .output()
        .expect("run flint");
    assert!(run.status.success(), "flint {command} --help failed");
    String::from_utf8_lossy(&run.stdout).into_owned()
}

/// #327: `goldens` and `render-suite` are two commands, and each one's
/// `--help` lists only its own flags.
#[test]
fn goldens_and_music_each_list_only_their_own_flags() {
    const GOLDEN: &[&str] = &[
        "--bless",
        "--bless-variant",
        "--bless-drifted",
        "--case",
        "--out-dir",
        "--require-gpu",
        "--isolate",
    ];
    const MUSIC: &[&str] = &[
        "--script",
        "--output",
        "--base-dir",
        "--duration-bars",
        "--duration-seconds",
        "--status-every",
        "--chunk-frames",
    ];
    let goldens = help("goldens");
    let music = help("render-suite");
    let has = |text: &str, flag: &str| text.split_whitespace().any(|w| w == flag);
    for flag in GOLDEN {
        assert!(
            has(&goldens, flag),
            "goldens --help lacks {flag}:\n{goldens}"
        );
        assert!(
            !has(&music, flag),
            "render-suite --help has {flag}:\n{music}"
        );
    }
    for flag in MUSIC {
        assert!(
            has(&music, flag),
            "render-suite --help lacks {flag}:\n{music}"
        );
        assert!(
            !has(&goldens, flag),
            "goldens --help has {flag}:\n{goldens}"
        );
    }
    assert!(!goldens.contains("Goldens:"), "{goldens}");
    assert!(music.contains("flint goldens"), "{music}");
}

/// #327: the old spelling, `render-suite <x>.goldens.toml` with golden
/// flags, still runs the goldens. A case filter that matches nothing stops
/// it before any GPU work, so this holds on any machine.
#[test]
fn render_suite_still_runs_a_goldens_manifest() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/stylize.goldens.toml");
    for command in ["render-suite", "goldens"] {
        let run = Command::new(FLINT)
            .arg(command)
            .arg("--case")
            .arg("no-case-is-named-this")
            .arg(&manifest)
            .output()
            .expect("run flint");
        let err = String::from_utf8_lossy(&run.stderr);
        assert!(!run.status.success(), "{command}: {err}");
        assert!(err.contains("no cases to run"), "{command}: {err}");
    }
    // A music suite still needs its WAV, now from clap
    let run = Command::new(FLINT)
        .args(["render-suite", "x.suite.toml"])
        .output()
        .expect("run flint");
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(!run.status.success() && err.contains("--output"), "{err}");
}

/// A user style that fails to compile (#417): `flint render` names its
/// file, line and column, and still renders (the suite's
/// `user-style-broken` case holds the frame to the one without it). The
/// warning comes before any GPU work, so it holds without an adapter too.
#[test]
fn broken_user_style_prints_where_it_failed() {
    let dir = run_dir("broken-style");
    std::fs::create_dir_all(&dir).expect("make the render's folder");
    let run = Command::new(FLINT)
        .args([
            "render",
            "crates/flint-cli/tests/golden/fixtures/broken_style.scene.toml",
            "--schemas",
            "schemas",
            "--schemas",
            "schemas/examples",
            "--no-grid",
            "--width",
            "64",
            "--height",
            "36",
            "--style",
            "broken",
            "--output",
        ])
        .arg(dir.join("broken-style.png"))
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .expect("run flint");
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(
        err.contains("broken.wgsl:9:9") && err.contains("expected to be"),
        "{err}"
    );
    let no_gpu = err.contains("Failed to get adapter");
    assert!(run.status.success() || no_gpu, "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn style_flag_rejects_what_it_cannot_do() {
    let run = Command::new(FLINT)
        .args([
            "render",
            "tests/fixtures/scenes/pbr_test.scene.toml",
            "--style",
            "ascii:colour=tint",
        ])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .output()
        .expect("run flint");
    assert!(!run.status.success());
    let err = String::from_utf8_lossy(&run.stderr);
    assert!(
        err.contains("no key 'colour'") && err.contains("color="),
        "{err}"
    );
}
