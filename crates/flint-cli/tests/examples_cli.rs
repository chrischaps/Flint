//! `flint examples` (#763): every scene under examples/ is in
//! examples/index.toml with a topic and a purpose, and every one passes
//! what `flint validate` checks. No GPU: validation loads scenes only.

use std::path::{Path, PathBuf};
use std::process::Command;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn flint(args: &[&str], dir: &Path) -> (bool, String) {
    let out = Command::new(FLINT)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run flint");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.success(), text)
}

#[test]
fn every_example_is_indexed_and_valid() {
    let (ok, out) = flint(&["examples", "--check"], &repo());
    assert!(ok, "flint examples --check failed:\n{out}");
    assert!(out.contains("every one indexed and valid"), "{out}");
}

#[test]
fn the_list_has_each_topic_and_the_json_parses() {
    let (ok, out) = flint(&["examples"], &repo());
    assert!(ok, "{out}");
    for topic in ["showcase", "rendering", "stylize", "living", "film"] {
        assert!(
            out.lines().any(|l| l == topic),
            "no {topic} heading:\n{out}"
        );
    }
    assert!(out.contains("examples/pbr_test.scene.toml"), "{out}");

    let (ok, json) = flint(&["examples", "--topic", "stylize", "--json"], &repo());
    assert!(ok, "{json}");
    let list: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
    assert!(!list.is_empty());
    assert!(list.iter().all(|e| e["topic"] == "stylize"), "{json}");

    let (ok, err) = flint(&["examples", "--topic", "nope"], &repo());
    assert!(!ok && err.contains("topics: showcase"), "{err}");
}

#[test]
fn check_names_a_scene_missing_from_the_index_and_one_missing_from_disk() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("examples-check");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let scene = "[scene]\nname = \"x\"\nversion = \"1.0\"\n";
    std::fs::write(dir.join("listed.scene.toml"), scene).unwrap();
    std::fs::write(dir.join("sub/stray.scene.toml"), scene).unwrap();
    std::fs::write(
        dir.join("index.toml"),
        "[[example]]\nscene = \"listed.scene.toml\"\ntopic = \"t\"\npurpose = \"p\"\n\
         [[example]]\nscene = \"gone.scene.toml\"\ntopic = \"t\"\npurpose = \"p\"\n",
    )
    .unwrap();
    let schemas = repo().join("schemas");
    let (ok, out) = flint(
        &[
            "examples",
            "--check",
            "--dir",
            dir.to_str().unwrap(),
            "--schemas",
            schemas.to_str().unwrap(),
        ],
        &dir,
    );
    assert!(!ok, "{out}");
    assert!(
        out.contains("sub/stray.scene.toml: not in index.toml"),
        "{out}"
    );
    assert!(
        out.contains("gone.scene.toml: in index.toml but not on disk"),
        "{out}"
    );
    assert!(!out.contains("listed.scene.toml:"), "{out}");
}

/// An example with an error diagnostic fails the check, named with its
/// entity and the suggestion (ADR 0115, #683); a field no schema declares is
/// a warning and does not.
#[test]
fn check_fails_on_an_error_diagnostic_and_not_on_a_warning() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("examples-diagnostics");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("typo.scene.toml"),
        "[scene]\nname = \"x\"\nversion = \"1.0\"\n\n[entities.lamp.lihgt]\nintensity = 1.0\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("extra_field.scene.toml"),
        "[scene]\nname = \"y\"\nversion = \"1.0\"\n\n[entities.box.transform]\npostion = [0, 1, 0]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("index.toml"),
        "[[example]]\nscene = \"typo.scene.toml\"\ntopic = \"t\"\npurpose = \"p\"\n\
         [[example]]\nscene = \"extra_field.scene.toml\"\ntopic = \"t\"\npurpose = \"p\"\n",
    )
    .unwrap();
    let schemas = repo().join("schemas");
    let (ok, out) = flint(
        &[
            "examples",
            "--check",
            "--dir",
            dir.to_str().unwrap(),
            "--schemas",
            schemas.to_str().unwrap(),
        ],
        &dir,
    );
    assert!(!ok, "{out}");
    assert!(
        out.contains(
            "typo.scene.toml: error[unknown-component] entity 'lamp' component 'lihgt': \
             no component named `lihgt`, so nothing checks it (did you mean `light`?)"
        ),
        "{out}"
    );
    assert!(!out.contains("extra_field.scene.toml:"), "{out}");
}
