//! End-to-end tests for `flint gen --validate` (#30): run the real binary on
//! the shipped specs and the tavern style guide, and hold the report and the
//! exit code to the ticket's acceptance criteria.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn spec(name: &str) -> String {
    repo_root()
        .join("specs")
        .join(name)
        .to_str()
        .unwrap()
        .to_string()
}

fn tavern() -> String {
    repo_root()
        .join("styles/medieval_tavern.style.toml")
        .to_str()
        .unwrap()
        .to_string()
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("gen_validate_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn gen(args: &[&str]) -> Output {
    Command::new(FLINT)
        .arg("gen")
        .args(args)
        .output()
        .expect("run flint")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// The report's check lines with this status, e.g. `[ WARN] LOD0 ...`.
fn lines_with<'a>(out: &'a str, status: &str) -> Vec<&'a str> {
    out.lines()
        .filter(|l| l.trim_start().starts_with(&format!("[ {status}]")))
        .collect()
}

#[test]
fn a_well_configured_spec_passes_cleanly() {
    let dir = scratch("pass");
    let out = dir.join("tree.glb");
    let o = gen(&[
        &spec("oak_tree.procgen.toml"),
        "-o",
        out.to_str().unwrap(),
        "--validate",
    ]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("Validation: "), "{t}");
    assert!(t.contains(" 0 warnings, 0 failed"), "{t}");
    // One spec target per LOD, read from the spec's [[lod]] table.
    for lod in 0..3 {
        assert!(t.contains(&format!("LOD{lod} spec triangle target")), "{t}");
    }
}

#[test]
fn without_validate_nothing_is_checked() {
    let dir = scratch("off");
    let out = dir.join("tree.glb");
    let o = gen(&[&spec("oak_tree.procgen.toml"), "-o", out.to_str().unwrap()]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(!t.contains("Validation:"), "{t}");
}

#[test]
fn a_style_guide_needs_validate() {
    let o = gen(&[&spec("oak_tree.procgen.toml"), "--style-guide", &tavern()]);
    assert!(!o.status.success());
    assert!(text(&o).contains("--validate"), "{}", text(&o));
}

/// The oak's LOD0 is ~8000 triangles and the tavern allows 5000: within the
/// 2x tolerance, so a warning, and `--strict` turns it into exit 1.
#[test]
fn over_budget_warns_and_strict_fails() {
    let dir = scratch("strict");
    let out = dir.join("tree.glb");
    let base = [
        spec("oak_tree.procgen.toml"),
        "-o".into(),
        out.to_str().unwrap().into(),
        "--validate".into(),
        "--style-guide".into(),
        tavern(),
    ];
    let args: Vec<&str> = base.iter().map(String::as_str).collect();

    let o = gen(&args);
    let t = text(&o);
    assert!(o.status.success(), "warnings alone must exit 0: {t}");
    let warns = lines_with(&t, "WARN");
    assert!(
        warns
            .iter()
            .any(|l| l.contains("LOD0 style triangle budget")),
        "{t}"
    );
    assert!(lines_with(&t, "FAIL").is_empty(), "{t}");

    let mut strict = args.clone();
    strict.push("--strict");
    let o = gen(&strict);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(t.contains("Strict mode"), "{t}");
}

/// A ceiling far below the output is a failure, and fails without --strict.
#[test]
fn far_over_budget_fails() {
    let dir = scratch("fail");
    let style = dir.join("tiny.style.toml");
    std::fs::write(
        &style,
        "[style]\nname = \"tiny\"\n[style.geometry]\nmax_triangles = 100\n",
    )
    .unwrap();
    let out = dir.join("tree.glb");
    let o = gen(&[
        &spec("oak_tree.procgen.toml"),
        "-o",
        out.to_str().unwrap(),
        "--validate",
        "--style-guide",
        style.to_str().unwrap(),
    ]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(
        lines_with(&t, "FAIL")
            .iter()
            .any(|l| l.contains("style triangle budget")),
        "{t}"
    );
}

/// The beetle's ellipsoids used to carry 16 zero-area slivers at each pole.
#[test]
fn the_beetle_has_no_degenerate_triangles() {
    let dir = scratch("beetle");
    let out = dir.join("beetle.glb");
    let o = gen(&[
        &spec("beetle.procgen.toml"),
        "-o",
        out.to_str().unwrap(),
        "--validate",
    ]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("[ PASS] skinned degenerate triangles"), "{t}");
}

/// A texture against the tavern: dimensions, palette and roughness-map
/// lines all appear, and the summary counts them.
#[test]
fn textures_are_checked_against_the_style() {
    let dir = scratch("texture");
    let src = std::fs::read_to_string(repo_root().join("specs/stone_wall.procgen.toml")).unwrap();
    let small = src
        .replace("height = 512", "height = 64")
        .replace("width = 512", "width = 64");
    let spec_path = dir.join("wall.procgen.toml");
    std::fs::write(&spec_path, small).unwrap();
    let out = dir.join("wall.png");
    let o = gen(&[
        spec_path.to_str().unwrap(),
        "-o",
        out.to_str().unwrap(),
        "--validate",
        "--style-guide",
        &tavern(),
    ]);
    let t = text(&o);
    assert!(t.contains("color texture dimensions: 64x64"), "{t}");
    assert!(t.contains("color palette: "), "{t}");
    assert!(t.contains("roughness style range: "), "{t}");
    assert!(t.contains("Validation: "), "{t}");
}
