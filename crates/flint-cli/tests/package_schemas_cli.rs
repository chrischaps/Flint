//! A package's schemas move into its package and a game still gets them
//! (#1332, ADR 0136 decision 8): the real binary in a game laid out as one
//! is, the engine's tree copied in at `engine/` (`schemas/` and `packages/`,
//! as a subtree holds them) and a `flint.toml` that says only
//! `schemas = ["engine/schemas", "schemas"]`. `living_prop`,
//! `shallow_wooze`, `music_session`, `[living]` and `[wooze]` live in
//! `engine/packages/<name>/schemas/` now, and `engine/schemas` brings them:
//! the game's diagnostics are the engine checkout's own, word for word, and
//! its bundle packs and lists them.

use flint_core::bundle::Bundle;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}

/// One mistake for each moved schema to find, so a diagnostic says the
/// schema was loaded: with none, each would be an `unknown-component` or
/// `unknown-table` instead.
const GROVE: &str = r#"[scene]
name = "Grove"

[living]
shadowz = "blended"

[wooze]
glide = 0.4

[entities.oak]
[entities.oak.transform]
position = [0.0, 0.0, 0.0]
[entities.oak.living_prop]
pose_ring = 1

[entities.bench]
[entities.bench.transform]
position = [2.0, 0.0, 0.0]
[entities.bench.shallow_wooze]
amplitude = -1.0

[entities.band]
[entities.band.music_session]
lean_mode = "sideways"
"#;

/// A game under the system temp folder (one under the engine's `target/`
/// would be the engine's), with `engine/` holding the engine's `schemas/`
/// and, when `packages` is true, its `packages/`.
fn game(tag: &str, packages: bool) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "flint-package-schemas-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    copy_tree(&repo().join("schemas"), &dir.join("engine/schemas"));
    if packages {
        copy_tree(&repo().join("packages"), &dir.join("engine/packages"));
    }
    write(
        &dir,
        "flint.toml",
        "[project]\nid = \"grove\"\nschemas = [\"engine/schemas\", \"schemas\"]\n\
         scene = \"scenes/grove.scene.toml\"\n",
    );
    // The game's own layer, over the engine's and the packages'
    write(
        &dir,
        "schemas/components/keeper.toml",
        "[component.keeper]\ndescription = \"The grove's keeper\"\n\n\
         [component.keeper.fields]\npatience = { type = \"f32\", default = 1.0 }\n",
    );
    write(&dir, "scenes/grove.scene.toml", GROVE);
    dir
}

fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run flint")
}

/// `flint --json check <scene>`'s diagnostics, without the file each names
/// (the two runs spell the scene from different folders), sorted.
fn diagnostics(dir: &Path, flags: &[&str]) -> Vec<String> {
    let mut args = vec!["--json", "check", "scenes/grove.scene.toml"];
    args.extend(flags);
    let out = flint(dir, &args);
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "flint check printed no JSON ({e})\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    let mut found: Vec<String> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            let mut d = d.clone();
            d.as_object_mut().unwrap().remove("file");
            d.to_string()
        })
        .collect();
    found.sort();
    found
}

fn codes_of(found: &[String]) -> Vec<String> {
    found
        .iter()
        .map(|d| {
            let v: serde_json::Value = serde_json::from_str(d).unwrap();
            let place = v["component"]
                .as_str()
                .or(v["table"].as_str())
                .unwrap_or("-")
                .to_string();
            format!("{} {place}", v["code"].as_str().unwrap())
        })
        .collect()
}

#[test]
fn a_game_naming_engine_schemas_resolves_the_packages_schemas_as_before() {
    let dir = game("resolve", true);
    let found = diagnostics(&dir, &[]);
    let codes = codes_of(&found);
    for want in [
        "out-of-range living_prop",
        "out-of-range shallow_wooze",
        "bad-enum-value music_session",
        "unknown-field living",
    ] {
        assert!(codes.iter().any(|c| c == want), "no {want:?}: {codes:#?}");
    }
    assert!(
        !codes
            .iter()
            .any(|c| c.starts_with("unknown-component") || c.starts_with("unknown-table")),
        "a moved schema went missing: {codes:#?}"
    );

    // The engine checkout says the same of the same scene, from its own
    // `schemas/` (and its packages', which the flag brings)
    let engine = repo().join("schemas");
    let engine = engine.to_string_lossy();
    let here = diagnostics(&dir, &["--schemas", &engine, "--schemas", "schemas"]);
    assert_eq!(found, here);

    // `flint schema` finds each in its package's folder
    let out = flint(&dir, &["schema", "living_prop"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{text}");
    assert!(text.contains("lineage_seed"), "{text}");
    assert!(
        text.contains("engine/packages/living/schemas/components/living_prop.toml"),
        "{text}"
    );
    for name in ["wooze", "living", "music_session", "shallow_wooze"] {
        let out = flint(&dir, &["schema", name]);
        assert!(out.status.success(), "flint schema {name}: {out:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// What makes `engine/schemas` the engine's is the kernel's manifest beside
/// it: a folder of schemas with no `packages/` next to it brings nothing,
/// so a moved schema is unknown there.
#[test]
fn an_engine_folder_with_no_packages_brings_none() {
    let dir = game("bare", false);
    let codes = codes_of(&diagnostics(&dir, &[]));
    for want in [
        "unknown-component living_prop",
        "unknown-component music_session",
        "unknown-table living",
    ] {
        assert!(codes.iter().any(|c| c == want), "no {want:?}: {codes:#?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The bundle packs each package's schemas where the engine's tree keeps
/// them and lists them after `engine/schemas`, so the web player and
/// `flint-player --bundle` (which read the list as written) load them.
#[test]
fn a_bundle_packs_and_lists_the_packages_schemas() {
    let dir = game("bundle", true);
    // A bundle refuses a scene that doesn't load, and `[living] shadowz`
    // doesn't: pack the grove with its tables spelt right
    write(
        &dir,
        "scenes/grove.scene.toml",
        &GROVE.replace("shadowz", "shadows"),
    );
    let out = flint(&dir, &["bundle", ".", "--out-dir", "dist"]);
    assert!(
        out.status.success(),
        "flint bundle failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let bundle = Bundle::parse(std::fs::read(dir.join("dist/grove.flintpack")).unwrap()).unwrap();
    assert_eq!(
        bundle.manifest.schemas,
        [
            "engine/schemas",
            "engine/packages/living/schemas",
            "engine/packages/music/schemas",
            "schemas"
        ]
    );
    let files: Vec<String> = bundle.files().map(|(p, _)| p.to_string()).collect();
    for want in [
        "engine/packages/living/schemas/components/living_prop.toml",
        "engine/packages/living/schemas/components/shallow_wooze.toml",
        "engine/packages/living/schemas/scene/living.toml",
        "engine/packages/living/schemas/scene/wooze.toml",
        "engine/packages/music/schemas/components/music_session.toml",
    ] {
        assert!(
            files.iter().any(|f| f == want),
            "{want} not packed: {files:#?}"
        );
    }
    // Nothing else of the packages' folders: no manifest, no notes
    assert!(
        !files
            .iter()
            .any(|f| f.starts_with("engine/packages/") && !f.contains("/schemas/")),
        "{files:#?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
