//! #1256's done-when, held: the styles' hash and value noise are
//! `noise.wgsl`'s, and the style mask and slot table are read only by
//! `mask_read.wgsl` and `mask_slot.wgsl`, which the built-in styles and the
//! user prelude (`custom/compose.rs`) include. Read from disk, so a new copy
//! in any file under `stylize/` is seen.

use std::path::Path;

/// Every `.wgsl` and `.rs` file under `src/stylize`, by file name and text.
#[allow(clippy::disallowed_methods)] // a test reading the crate's own source tree
fn stylize_files() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("stylize");
    let mut out = Vec::new();
    let mut dirs = vec![root.clone()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "wgsl" || e == "rs") {
                let name = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((name, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    out
}

fn wgsl_files() -> Vec<(String, String)> {
    stylize_files()
        .into_iter()
        .filter(|(name, _)| name.ends_with(".wgsl"))
        .collect()
}

/// The WGSL functions a file declares, with the text of each up to the next.
fn functions(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("fn ") {
            let name = rest.split('(').next().unwrap_or("").trim().to_string();
            out.push((name, String::new()));
        } else if let Some((_, body)) = out.last_mut() {
            body.push_str(line);
            body.push('\n');
        }
    }
    out
}

/// Functions that are named for noise but aren't a hash or value noise of
/// their own, and why.
const NOT_COPIES: &[(&str, &str, &str)] = &[(
    "reveal.wgsl",
    "ragged_noise",
    "the reveal's edge is the tear's noise (../tear_mask.wgsl's fbm), the frame's, not a style's",
)];

#[test]
fn every_style_hashes_and_boils_with_the_one_noise() {
    let shared = [
        "fn flint_pcg(",
        "fn flint_unit(",
        "fn flint_hash1(",
        "fn flint_hash3(",
        "fn flint_vnoise(",
        "fn flint_hash(",
    ];
    let files = wgsl_files();
    for decl in shared {
        let found: Vec<&str> = files
            .iter()
            .filter(|(_, t)| t.lines().any(|l| l.trim_start().starts_with(decl)))
            .map(|(n, _)| n.as_str())
            .collect();
        assert_eq!(
            found,
            ["noise.wgsl"],
            "`{decl}` should be declared once, in noise.wgsl"
        );
    }
    for (file, text) in &files {
        if file == "noise.wgsl" {
            continue;
        }
        // A sin() hash reads differently GPU to GPU
        assert!(
            !text.contains("43758.5453"),
            "{file} hashes with sin(); use noise.wgsl's flint_hash1"
        );
        for (name, body) in functions(text) {
            let lower = name.to_lowercase();
            if !(lower.contains("hash") || lower.contains("noise") || lower.contains("pcg")) {
                continue;
            }
            if NOT_COPIES.iter().any(|(f, n, _)| f == file && *n == name) {
                continue;
            }
            // A style may name the shared noise under its own seed (watercolor's
            // `vnoise`): its body calls the shared one
            assert!(
                body.contains("flint_vnoise(") || body.contains("flint_hash"),
                "{file}: `fn {name}` is a hash or noise of its own; use noise.wgsl's \
                 (or add it to NOT_COPIES with the reason it keeps its own)"
            );
        }
    }
}

#[test]
fn the_mask_and_its_slots_are_read_in_one_place() {
    for (file, text) in stylize_files() {
        if file == "shared_wgsl_tests.rs" {
            continue;
        }
        if file != "mask_read.wgsl" {
            assert!(
                !text.contains("textureLoad(flint_mask_texture"),
                "{file} reads the style mask itself; use mask_read.wgsl's flint_mask_load / flint_mask_id"
            );
        }
        if file != "mask_slot.wgsl" {
            assert!(
                !text.contains("arrayLength(&flint_slots)"),
                "{file} reads the slot table itself; use mask_slot.wgsl's flint_slot"
            );
        }
        if !file.ends_with(".wgsl") {
            continue;
        }
        // Every mask a style binds is declared under the one name the
        // readers use
        for line in text.lines() {
            let t = line.trim();
            if t.starts_with("//") || !t.contains("texture_2d<u32>") {
                continue;
            }
            let Some(var) = t.split("var ").nth(1) else {
                continue;
            };
            let name = var.split(':').next().unwrap().trim();
            assert!(
                !name.contains("mask") || name == "flint_mask_texture",
                "{file}: the style mask is bound as `{name}`; declare it `flint_mask_texture` and include mask_read.wgsl"
            );
        }
    }
}

/// The user prelude is the same text, not a copy that could drift.
#[test]
fn the_user_prelude_includes_the_shared_chunks() {
    let compose = include_str!("custom/compose.rs");
    for chunk in [
        "include_str!(\"../noise.wgsl\")",
        "include_str!(\"../mask_read.wgsl\")",
        "include_str!(\"../mask_slot.wgsl\")",
    ] {
        assert!(compose.contains(chunk), "custom/compose.rs should {chunk}");
    }
}
