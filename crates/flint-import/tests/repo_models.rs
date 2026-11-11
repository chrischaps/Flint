//! Every model checked into the repo imports.
//!
//! #779's `demo/` -> `examples/` rename rewrote the `asset.generator` string
//! inside all 18 tavern GLBs, four bytes longer, without the JSON chunk's
//! length. Every loader refused them ("unknown chunk type") and the tavern
//! drew as fog (found under #294). A text search-and-replace can't see a
//! binary file's framing, so this test does.

use std::path::{Path, PathBuf};

const DIRS: &[&str] = &[
    "examples/models",
    "showcase_game/models",
    "testModels",
    "tests/fixtures/scenes/models",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn every_checked_in_glb_imports() {
    let root = repo_root();
    let mut seen = 0;
    let mut broken = Vec::new();
    for dir in DIRS {
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
