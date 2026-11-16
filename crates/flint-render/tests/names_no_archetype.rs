//! #793: the renderer knows no archetype by name. The room-builder
//! vocabulary (`room`, `door`, `furniture`, `wall`, `floor`, `ceiling`,
//! `pillar`) left the engine's schemas for `schemas/examples/` in #681, and
//! #751 moved the colours the renderer kept for them, and for `character`,
//! into each archetype's own `[archetype.<name>.visual]`. This holds it: no
//! source file of the crate (Rust or WGSL) spells one of those names as a
//! string literal, so a look can't come to depend on a name no schema has
//! to declare again.
//!
//! The crate's own tests are not scanned: a test may well name an entity
//! `floor`.

#![allow(clippy::disallowed_methods)] // a test reads the crate's sources from disk

use std::path::{Path, PathBuf};

/// The names whose look the renderer once kept in a table (#751).
const ARCHETYPES: &[&str] = &[
    "room",
    "door",
    "furniture",
    "wall",
    "floor",
    "ceiling",
    "pillar",
    "character",
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs" | "wgsl")
        ) {
            out.push(path);
        }
    }
}

#[test]
fn the_renderer_spells_no_archetype_name() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    assert!(files.len() > 50, "found only {} sources", files.len());

    let mut found = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        for (n, line) in text.lines().enumerate() {
            for name in ARCHETYPES {
                if line.contains(&format!("\"{name}\"")) {
                    let shown = file.strip_prefix(&src).unwrap().display();
                    let shown = shown.to_string().replace('\\', "/");
                    found.push(format!("src/{shown}:{}: \"{name}\"", n + 1));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "the renderer names an archetype; its look belongs in the \
         archetype's [archetype.<name>.visual] (#751, #793):\n{}",
        found.join("\n")
    );
}
