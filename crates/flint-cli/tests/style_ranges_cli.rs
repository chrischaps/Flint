//! #492: a scene entity's per-entity style field outside its range is
//! named by `flint check` and `flint validate`, with the entity, the field
//! and the range: a built-in style's (`ascii_style`, colours too) through
//! the component's schema file, a user style's (`hologram_style`, whose
//! schema file is open) through the schema its `.shader.toml` makes (#556),
//! which also names a misspelt field and a value of the wrong type, and
//! which `flint render` warns with when it binds the scene.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SCENE: &str = r#"[scene]
name = "Ranges"

[entities.bench]
[entities.bench.ascii_style]
cell = 400
tint = [3.0, 0.5, 0.2]

[entities.barrel]
[entities.barrel.hologram_style]
glow = 90.0
lines = 6.0

[entities.drum]
[entities.drum.hologram_style]
glwo = 2
flicker = "lots"

[entities.crate]
[entities.crate.ascii_style]
cell = 12
tint = [0.35, 1.0, 0.45]
"#;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A project naming the engine's schemas (and the examples' layer, which
/// declares `hologram_style` open), with the hologram user style and the
/// scene: one folder a test, since they run side by side.
fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("style_ranges_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let schemas = |p: &str| {
        let path = std::fs::canonicalize(repo().join(p)).unwrap();
        path.to_string_lossy()
            .trim_start_matches(r"\\?\")
            .replace('\\', "/")
    };
    let manifest = format!(
        "[project]\nid = \"ranges\"\nschemas = [\"{}\", \"{}\"]\n",
        schemas("schemas"),
        schemas("schemas/examples")
    );
    let style = |name: &str| {
        std::fs::read_to_string(repo().join("tests/fixtures/scenes/styles").join(name))
    };
    let files = [
        ("flint.toml".to_string(), manifest),
        ("scenes/ranges.scene.toml".into(), SCENE.into()),
        (
            "scenes/styles/hologram.shader.toml".into(),
            style("hologram.shader.toml").unwrap(),
        ),
        (
            "scenes/styles/hologram.wgsl".into(),
            style("hologram.wgsl").unwrap(),
        ),
    ];
    for (path, text) in files {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    dir
}

fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run flint")
}

fn json(o: &Output) -> serde_json::Value {
    let text = String::from_utf8_lossy(&o.stdout);
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}):\n{text}"))
}

/// Each out-of-range diagnostic as `entity component.field: message`.
fn out_of_range(doc: &serde_json::Value) -> Vec<String> {
    let mut found: Vec<String> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "out-of-range")
        .map(|d| {
            format!(
                "{} {}.{}: {}",
                d["entity"].as_str().unwrap_or("?"),
                d["component"].as_str().unwrap_or("?"),
                d["field"].as_str().unwrap_or("?"),
                d["message"].as_str().unwrap_or("?"),
            )
        })
        .collect();
    found.sort();
    found
}

#[test]
fn a_style_field_past_its_range_is_named_with_its_range() {
    let dir = project("ranges");
    let check = flint(&dir, &["check", "scenes/ranges.scene.toml", "--json"]);
    let doc = json(&check);
    assert!(!check.status.success(), "{doc:#}");
    let found = out_of_range(&doc);
    assert!(
        found
            .iter()
            .any(|d| d.starts_with("barrel hologram_style.glow: 90 is above the maximum 2")),
        "the user style's range: {found:#?}"
    );
    assert!(
        found
            .iter()
            .any(|d| d.starts_with("bench ascii_style.cell: 400 is above the maximum 32")),
        "the built-in's, from its schema: {found:#?}"
    );
    assert!(
        found
            .iter()
            .any(|d| d.starts_with("bench ascii_style.tint[0]: 3 is above the maximum 1")),
        "a colour's channel: {found:#?}"
    );
    assert_eq!(found.len(), 3, "the crate is in range: {found:#?}");

    // validate says the same, and fails on it
    let validate = flint(
        &dir,
        &["validate", "scenes/ranges.scene.toml", "--format", "json"],
    );
    assert!(!validate.status.success());
    assert_eq!(out_of_range(&json(&validate)), found);
}

/// Every diagnostic of `code` as `entity component.field`.
fn placed(doc: &serde_json::Value, code: &str) -> Vec<String> {
    let mut found: Vec<String> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == code)
        .map(|d| {
            format!(
                "{} {}.{}",
                d["entity"].as_str().unwrap_or("?"),
                d["component"].as_str().unwrap_or("?"),
                d["field"].as_str().unwrap_or("?"),
            )
        })
        .collect();
    found.sort();
    found
}

/// #556: a user style's component is held to the schema its `.shader.toml`
/// makes, so a misspelt field is a warning naming it and a wrong type an
/// error, as for a built-in's. Before, both were dropped without a word.
#[test]
fn a_user_style_field_misspelt_or_of_the_wrong_type_is_named() {
    let dir = project("misspelt");
    let check = flint(&dir, &["check", "scenes/ranges.scene.toml", "--json"]);
    let doc = json(&check);
    assert_eq!(
        placed(&doc, "unknown-field"),
        ["drum hologram_style.glwo"],
        "{doc:#}"
    );
    let glwo = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["field"] == "glwo")
        .unwrap();
    assert_eq!(glwo["severity"], "warning");
    assert!(
        glwo["hint"].as_str().unwrap_or("").contains("glow"),
        "the nearest field: {glwo:#}"
    );
    assert_eq!(placed(&doc, "bad-type"), ["drum hologram_style.flicker"]);
    // The component is known: no unknown-component, though its schema file
    // declares no fields
    assert!(placed(&doc, "unknown-component").is_empty(), "{doc:#}");

    let validate = flint(
        &dir,
        &["validate", "scenes/ranges.scene.toml", "--format", "json"],
    );
    let doc = json(&validate);
    assert_eq!(placed(&doc, "unknown-field"), ["drum hologram_style.glwo"]);
    assert_eq!(placed(&doc, "bad-type"), ["drum hologram_style.flicker"]);
}

/// The issue's own test: `hologram_style { glwo = 2 }` in a scene says so,
/// naming the field, when a drawing host loads it (`bind_scene`).
#[test]
fn rendering_the_scene_warns_about_the_misspelt_field() {
    let dir = project("render");
    let run = Command::new(FLINT)
        .current_dir(&dir)
        .args(["render", "scenes/ranges.scene.toml", "-o", "ranges.png"])
        .args(["--width", "64", "--height", "64", "--no-grid"])
        .output()
        .expect("run flint");
    let err = String::from_utf8_lossy(&run.stderr);
    if !run.status.success() && err.contains("headless render context") {
        eprintln!("skipped: no GPU adapter");
        return;
    }
    assert!(run.status.success(), "{err}");
    assert!(
        err.contains("warning[unknown-field] entity 'drum' hologram_style.glwo"),
        "{err}"
    );
    assert!(
        err.contains("error[bad-type] entity 'drum' hologram_style.flicker"),
        "{err}"
    );
    assert!(
        err.contains("error[out-of-range] entity 'barrel' hologram_style.glow"),
        "{err}"
    );
}
