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
    // With `/` however the folder was walked (#567: `styles\typo.wgsl`
    // on Windows before)
    assert!(
        report.contains(&format!("styles/typo.wgsl:{line}:")),
        "{report}"
    );
    assert!(report.contains("ok: styles/ok_one.shader.toml"), "{report}");
    assert!(!report.contains('\\'), "{report}");
    assert!(report.contains("1 of 2 styles failed"), "{report}");
}

/// #567: `--json` is one document an agent reads instead of the text: a
/// style that checks with its counts, one that doesn't with each error's
/// file, line, column, message, quoted line and hints.
#[test]
fn check_json_has_each_styles_counts_and_errors_in_parts() {
    let dir = game("json");
    assert!(flint(&dir, &["style", "new", "calm"]).status.success());
    assert!(flint(&dir, &["style", "new", "inky"]).status.success());
    let wgsl = dir.join("styles/inky.wgsl");
    let source = std::fs::read_to_string(&wgsl).unwrap();
    let line = source
        .lines()
        .position(|l| l.trim() == "let c = p.color;")
        .unwrap()
        + 1;
    // A mask helper in a style that doesn't read the mask: an error with a hint
    std::fs::write(
        &wgsl,
        source.replace(
            "let c = p.color;",
            "let c = p.color * flint_style_blend(p.uv);",
        ),
    )
    .unwrap();

    let out = flint(&dir, &["style", "check", "styles", "--json"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}\n{}", String::from_utf8_lossy(&out.stdout)));
    assert_eq!(doc["valid"], false, "{doc:#}");
    assert_eq!(doc["failed"], 1, "{doc:#}");
    let styles = doc["styles"].as_array().unwrap();
    assert_eq!(styles.len(), 2, "{doc:#}");

    let calm = &styles[0];
    assert_eq!(calm["file"], "styles/calm.shader.toml", "{doc:#}");
    assert_eq!(calm["ok"], true);
    assert_eq!(calm["loops"], 0);
    assert!(calm["texture_reads"].as_u64().is_some(), "{doc:#}");
    assert_eq!(calm["warnings"], serde_json::json!([]));
    assert_eq!(calm["errors"], serde_json::json!([]));

    let inky = &styles[1];
    assert_eq!(inky["file"], "styles/inky.shader.toml", "{doc:#}");
    assert_eq!(inky["ok"], false);
    let errors = inky["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{doc:#}");
    let e = &errors[0];
    assert_eq!(e["file"], "styles/inky.wgsl", "{doc:#}");
    assert_eq!(e["line"], line, "{doc:#}");
    assert!(e["col"].as_u64().is_some_and(|c| c > 0), "{doc:#}");
    assert!(
        e["message"].as_str().is_some_and(|m| !m.is_empty()),
        "{doc:#}"
    );
    assert!(
        e["source"]
            .as_str()
            .is_some_and(|s| s.contains("flint_style_blend")),
        "{doc:#}"
    );
    assert_eq!(
        e["hints"],
        serde_json::json!(["add \"mask\" to inputs in the .shader.toml"]),
        "{doc:#}"
    );
    // The text a host shows, for a reader that wants it whole
    assert!(
        inky["error"]
            .as_str()
            .is_some_and(|t| t.starts_with(&format!("styles/inky.wgsl:{line}:{}: ", e["col"]))),
        "{doc:#}"
    );
}

/// #567: `flint style new --dir a/b` names what it wrote with the
/// separators it was given, not `a/b\x.wgsl`.
#[test]
fn new_names_its_files_with_forward_slashes() {
    let dir = game("dir");
    let out = flint(&dir, &["style", "new", "dusk", "--dir", "demo/styles"]);
    assert!(out.status.success(), "{}", text(&out));
    let said = text(&out);
    assert!(
        said.contains("wrote demo/styles/dusk.shader.toml"),
        "{said}"
    );
    assert!(said.contains("wrote demo/styles/dusk.wgsl"), "{said}");
    assert!(
        said.contains("flint style check demo/styles/dusk.shader.toml"),
        "{said}"
    );
    assert!(!said.contains('\\'), "{said}");
}
