//! Every model checked into the repo imports.
//!
//! #779's rename of `demo/` rewrote the `asset.generator` string
//! inside all 18 tavern GLBs, four bytes longer, without the JSON chunk's
//! length. Every loader refused them ("unknown chunk type") and the tavern
//! drew as fog (found under #294). A text search-and-replace can't see a
//! binary file's framing, so this test does.
#![allow(clippy::disallowed_methods)] // a test: reads its files off the disk, not through the vfs

use std::path::{Path, PathBuf};

const DIRS: &[&str] = &[
    // The frozen fixtures' models (their own since #984), then the ones
    // only the fixtures use
    "tests/fixtures/scenes/models",
    "tests/fixtures/models",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// [`DIRS`], then the `models/` of each game kept in the checkout: a
/// top-level directory with a `flint.toml`, found by it so the engine names
/// no game (#760).
fn model_dirs(root: &Path) -> Vec<String> {
    let mut games: Vec<String> = std::fs::read_dir(root)
        .unwrap()
        .flatten()
        .filter(|e| e.path().join("flint.toml").is_file() && e.path().join("models").is_dir())
        .map(|e| format!("{}/models", e.file_name().to_string_lossy()))
        .collect();
    games.sort();
    DIRS.iter().map(|d| d.to_string()).chain(games).collect()
}

#[test]
fn every_checked_in_glb_imports() {
    let root = repo_root();
    let mut seen = 0;
    let mut broken = Vec::new();
    for dir in &model_dirs(&root) {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("glb") {
                continue;
            }
            seen += 1;
            if let Err(e) = flint_import::import_gltf(&path) {
                broken.push(format!(
                    "{dir}/{}: {e}",
                    entry.file_name().to_string_lossy()
                ));
            }
        }
    }
    assert!(seen > 0, "no GLBs found under {}", root.display());
    assert!(
        broken.is_empty(),
        "{} of {seen} GLBs don't import:\n  {}",
        broken.len(),
        broken.join("\n  ")
    );
}
