//! #492: a scene entity's per-entity style field outside its range is
//! named by `flint check` and `flint validate`, with the entity, the field
//! and the range: a built-in style's (`ascii_style`, colours too) through
//! the component's schema, a user style's (`hologram_style`, whose schema
//! is open) through the style's own `StyleParam` ranges.

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
/// scene.
fn project() -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("style_ranges_cli");
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
    let style = |name: &str| std::fs::read_to_string(repo().join("examples/styles").join(name));
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
    let dir = project();
    let check = flint(&dir, &["check", "scenes/ranges.scene.toml", "--json"]);
    let doc = json(&check);
    assert!(!check.status.success(), "{doc:#}");
    let found = out_of_range(&doc);
    assert!(
        found
            .iter()
            .any(|d| d.starts_with("barrel hologram_style.glow: 90 is outside its range 0..2")),
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
