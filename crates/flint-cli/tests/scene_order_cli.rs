//! A scene's ids are the same in every process (#688).
//!
//! They came from a process-wide counter in `HashMap` order, so the same
//! entity could be id 3 in one run and id 41 in the next. Now a world counts
//! its own ids and a scene spawns by name: two separate `flint query` runs
//! print the same name-to-id map, and the ids are 1, 2, 3... in name order.

use std::path::{Path, PathBuf};
use std::process::Command;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// `flint query entities` on the scene, in a process of its own, as
/// (name, id) pairs in the order printed.
fn ids_in_a_new_process(scene: &str) -> Vec<(String, u64)> {
    let out = Command::new(FLINT)
        .current_dir(repo())
        .args(["query", "entities", "--scene", scene, "--format", "json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "flint query failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    json.as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap().to_string(),
                e["id"].as_u64().unwrap(),
            )
        })
        .collect()
}

#[test]
fn two_processes_give_a_scene_the_same_ids() {
    // A grove of parents and archetypes written out of name order, and a
    // scene made of a prefab (frozen copies since #984)
    for (scene, at_least) in [
        (
            "tests/fixtures/scenes/woozy_grove/woozy_grove.scene.toml",
            30,
        ),
        ("tests/fixtures/scenes/stylize_props.scene.toml", 5),
    ] {
        let first = ids_in_a_new_process(scene);
        let second = ids_in_a_new_process(scene);
        assert!(
            first.len() > at_least,
            "expected a real scene, got {first:?}"
        );
        assert_eq!(first, second);

        // The ids are the names' ranks: nothing else decides them
        let mut by_name = first.clone();
        by_name.sort();
        let ranks: Vec<u64> = (1..=by_name.len() as u64).collect();
        let ids: Vec<u64> = by_name.iter().map(|(_, id)| *id).collect();
        assert_eq!(ids, ranks, "{scene}");
    }
}
