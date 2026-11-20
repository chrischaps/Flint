//! `flint examples` (#763): every scene beside an index.toml is in it with a
//! topic and a purpose, and every one passes what `flint validate` checks.
//! Since #984 the examples are their own repository (flint-examples: the
//! scenes and the index at its root, the engine at `engine/`), whose CI runs
//! `flint examples --check`; these tests build a project of that shape. No
//! GPU: validation loads scenes only.

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

/// A project shaped like flint-examples: `flint.toml` naming the engine's
/// schemas, the index and two scenes at the root (one in a folder), and an
/// engine checkout at `engine/` with a scene of its own nobody indexes.
fn examples_project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let schemas = std::fs::canonicalize(repo().join("schemas"))
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let schemas = schemas.trim_start_matches("//?/");
    let scene = "[scene]\nname = \"x\"\nversion = \"1.0\"\n";
    for (file, text) in [
        (
            "flint.toml",
            format!("[project]\nschemas = [\"{schemas}\"]\n"),
        ),
        (
            "index.toml",
            "[[example]]\nscene = \"pbr_test.scene.toml\"\ntopic = \"rendering\"\npurpose = \"p\"\n\
             [[example]]\nscene = \"film/beat.scene.toml\"\ntopic = \"stylize\"\npurpose = \"q\"\n"
                .to_string(),
        ),
        ("pbr_test.scene.toml", scene.to_string()),
        ("film/beat.scene.toml", scene.to_string()),
        ("engine/crates/flint-core/Cargo.toml", String::new()),
        ("engine/tests/fixture.scene.toml", scene.to_string()),
    ] {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

/// In a flint-examples checkout no flag is needed: the index is the
/// project root's, and the engine's own scenes under `engine/` are not
/// examples.
#[test]
fn every_example_is_indexed_and_valid() {
    let dir = examples_project("examples-root");
    let (ok, out) = flint(&["examples", "--check"], &dir);
    assert!(ok, "flint examples --check failed:\n{out}");
    assert!(
        out.contains("2 examples in .: every one indexed and valid"),
        "{out}"
    );
}

#[test]
fn the_list_has_each_topic_and_the_json_parses() {
    let dir = examples_project("examples-list");
    let (ok, out) = flint(&["examples"], &dir);
    assert!(ok, "{out}");
    for topic in ["rendering", "stylize"] {
        assert!(
            out.lines().any(|l| l == topic),
            "no {topic} heading:\n{out}"
        );
    }
    assert!(
        out.lines()
            .any(|l| l.trim_start().starts_with("pbr_test.scene.toml")),
        "{out}"
    );

    let (ok, json) = flint(&["examples", "--topic", "stylize", "--json"], &dir);
    assert!(ok, "{json}");
    let list: Vec<serde_json::Value> = serde_json::from_str(&json).unwrap();
    assert_eq!(list.len(), 1, "{json}");
    assert_eq!(list[0]["scene"], "film/beat.scene.toml", "{json}");

    let (ok, err) = flint(&["examples", "--topic", "nope"], &dir);
    assert!(!ok && err.contains("topics: rendering, stylize"), "{err}");
}

/// The engine's checkout has no examples of its own any more: the command
/// says where they are.
#[test]
fn the_engine_checkout_points_at_flint_examples() {
    let (ok, err) = flint(&["examples"], &repo());
    assert!(!ok, "{err}");
    assert!(err.contains("no index.toml at the project root"), "{err}");
    assert!(err.contains("flint-examples"), "{err}");
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
