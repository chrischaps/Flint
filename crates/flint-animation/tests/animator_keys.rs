//! The `animator` component's keys are spelled in one file,
//! `crates/flint-animation/src/animator.rs` (#733): three syncs once read
//! them each with their own defaults and rules, and drifted (#732). Every
//! other crate reads an animator through `Animator` / `animator::layers` and
//! writes one through `animator::write` / `animator::edit_layer`.
//!
//! This walks every crate's `src/` and fails on code that names a key next
//! to `comp::ANIMATOR`: `set_field(comp::ANIMATOR, "clip", ..)`,
//! `view(id, comp::ANIMATOR).f64("speed", ..)`,
//! `get(comp::ANIMATOR).and_then(|a| a.get("speed"))`. Test code is left
//! out (a `*tests.rs` file, and a file's text from its first `#[cfg(test)]`):
//! a test spells what a scene author writes.
#![allow(clippy::disallowed_methods)] // a test reading the source tree

use std::path::{Path, PathBuf};

const HOME: &str = "flint-animation/src/animator.rs";

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Where `text` names an animator key: a string literal right after
/// `ANIMATOR,` or `ANIMATOR).method(`, or inside the `.and_then(..)` chain
/// that follows `get(comp::ANIMATOR)`.
fn keys_named(text: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (at, _) in text.match_indices("comp::ANIMATOR") {
        let rest = &text[at + "comp::ANIMATOR".len()..];
        let line = text[..at].matches('\n').count() + 1;
        let trimmed = rest.trim_start();
        let literal = |s: &str| -> String { s.chars().take_while(|c| *c != '"').collect() };
        let key = if let Some(after) = trimmed.strip_prefix(',') {
            after.trim_start().strip_prefix('"').map(literal)
        } else if let Some(after) = trimmed.strip_prefix(')') {
            // `view(..).str("clip"` or `get(..).and_then(|a| a.get("clip"`:
            // a call on the component with a literal before the statement ends.
            let end = after.find(';').unwrap_or(after.len());
            let end = after
                .char_indices()
                .nth(160)
                .map_or(end, |(i, _)| i.min(end));
            let statement = &after[..end];
            if statement.trim_start().starts_with('.') {
                statement.find("(\"").map(|i| literal(&statement[i + 2..]))
            } else {
                None
            }
        } else {
            None
        };
        if let Some(key) = key {
            found.push((line, key));
        }
    }
    found
}

#[test]
fn only_animator_rs_names_an_animator_key() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&crates).unwrap().flatten() {
        rust_files(&entry.path().join("src"), &mut files);
    }
    assert!(files.len() > 100, "found only {} files", files.len());

    let mut offenders = Vec::new();
    for file in files {
        let shown = file
            .strip_prefix(&crates)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        if shown == HOME || shown.ends_with("tests.rs") || shown.contains("/tests/") {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        let code = text.split("#[cfg(test)]").next().unwrap_or("");
        for (line, key) in keys_named(code) {
            offenders.push(format!("{shown}:{line}: \"{key}\""));
        }
    }
    assert!(
        offenders.is_empty(),
        "animator keys named outside {HOME}; read with `Animator` / \
         `animator::layers`, write with `animator::write` / \
         `animator::edit_layer`:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_scan_finds_each_way_of_naming_a_key() {
    let code = r#"
        comps.set_field(comp::ANIMATOR, "clip", v);
        comps.set_field(
            comp::ANIMATOR,
            "playing",
            v,
        );
        let s = world.view(id, comp::ANIMATOR).f64("speed", 1.0);
        let l = c.get(comp::ANIMATOR).and_then(|a| a.get("layers"));
        let fine = world.entities_with_component(comp::ANIMATOR);
        let also_fine = Animator::of(world, id).speed;
    "#;
    let keys: Vec<String> = keys_named(code).into_iter().map(|(_, k)| k).collect();
    assert_eq!(keys, ["clip", "playing", "speed", "layers"]);
}
