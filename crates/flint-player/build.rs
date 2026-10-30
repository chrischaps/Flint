//! Embeds the engine's schema TOML for the web entry (`src/bin/flint_web.rs`).
//!
//! A browser can't list a directory over HTTP, so until game bundles land
//! (#98) the web player carries `schemas/components` and
//! `schemas/archetypes` in the binary (about 130 KB of text). Writes
//! `$OUT_DIR/engine_schemas.rs`; the desktop player never includes it.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let mut out = String::new();
    for kind in ["components", "archetypes"] {
        let dir = root.join(kind);
        println!("cargo:rerun-if-changed={}", dir.display());
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| rd.filter_map(|e| Some(e.ok()?.path())).collect())
            .unwrap_or_default();
        files.retain(|p| p.extension().is_some_and(|e| e == "toml"));
        files.sort();
        writeln!(out, "pub const {}: &[&str] = &[", kind.to_uppercase()).unwrap();
        for file in files {
            writeln!(out, "    include_str!({:?}),", file.display().to_string()).unwrap();
        }
        writeln!(out, "];").unwrap();
    }
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("engine_schemas.rs");
    std::fs::write(dest, out).unwrap();
}
