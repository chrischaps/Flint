//! `flint style new | check | prelude | list` (#416): the real binary
//! writes a style from the template that checks clean, refuses to clobber
//! it, reports a broken shader as `file:line:col` with exit 1, prints the
//! prelude its inputs give, and lists it. No GPU anywhere.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// A throwaway game folder: `levels/` beside where `styles/` will go.
fn game(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("style-cli-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("levels")).unwrap();
    std::fs::write(
        dir.join("levels/a.scene.toml"),
        "[scene]\nname = \"a\"\nversion = \"1.0\"\n",
    )
    .unwrap();
    dir
}

#[test]
fn new_then_check_exits_zero_and_lists_it() {
    let dir = game("new");
    let out = flint(&dir, &["style", "new", "dusk", "--inputs", "depth,mask"]);
    assert!(out.status.success(), "{}", text(&out));
    let wgsl = std::fs::read_to_string(dir.join("styles/dusk.wgsl")).unwrap();
    assert!(wgsl.contains("flint_view_depth") && wgsl.contains("flint_style_blend"));
    assert!(!wgsl.contains("flint_edge("), "edges wasn't asked for");

    let out = flint(&dir, &["style", "check", "styles/dusk.shader.toml"]);
    assert!(out.status.success(), "{}", text(&out));

    // Never over a style that is there
    let out = flint(&dir, &["style", "new", "dusk"]);
    assert!(!out.status.success());
    assert!(text(&out).contains("--force"), "{}", text(&out));
    // Nor under a built-in's name
    let out = flint(&dir, &["style", "new", "ascii"]);
    assert!(text(&out).contains("built-in"), "{}", text(&out));

    let out = flint(&dir, &["style", "list", "levels/a.scene.toml"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        text(&out).contains("dusk (Dusk) [display, user, per-entity: dusk_style, reads mask]"),
        "{}",
        text(&out)
    );

    let out = flint(&dir, &["style", "prelude", "styles/dusk.shader.toml"]);
    let prelude = text(&out);
    assert!(out.status.success(), "{prelude}");
    assert!(prelude.contains("amount: f32") && prelude.contains("fn flint_view_depth"));
    assert!(!prelude.contains("fn flint_edges"), "{prelude}");
    let out = flint(
        &dir,
        &["style", "prelude", "styles/dusk.shader.toml", "--composed"],
    );
    assert!(text(&out).contains("fn fs_main"), "{}", text(&out));
}

#[test]
fn a_broken_shader_fails_with_its_line() {
    let dir = game("broken");
    assert!(flint(&dir, &["style", "new", "ok_one"]).status.success());
    assert!(flint(&dir, &["style", "new", "typo", "--stage", "hdr"])
        .status
        .success());
    let wgsl = dir.join("styles/typo.wgsl");
    let source = std::fs::read_to_string(&wgsl).unwrap();
    let line = source
        .lines()
        .position(|l| l.trim() == "let c = p.color;")
        .unwrap()
        + 1;
    std::fs::write(
        &wgsl,
        source.replace("let c = p.color;", "let c = p.colour;"),
    )
    .unwrap();

    // A folder checks every style in it, and one error fails the run
    let out = flint(&dir, &["style", "check", "styles"]);
    let report = text(&out);
    assert_eq!(out.status.code(), Some(1), "{report}");
    assert!(report.contains("ok: styles"), "{report}");
    assert!(report.contains(&format!("typo.wgsl:{line}:")), "{report}");
    assert!(report.contains("1 of 2 styles failed"), "{report}");
}
