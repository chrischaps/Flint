//! The environment table is the whole list (#715): no `FLINT_*` variable
//! is read outside `flint_core::env`, and every `"FLINT_..."` name a Rust
//! file spells (a test setting one for a child, a doc constant) is a
//! switch the table lists, so `flint env` and the book can't miss one.
#![allow(clippy::disallowed_methods)] // walks the repository's sources off the disk

use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name != "target" && !name.starts_with('.') {
                rust_files(&path, out);
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

fn sources() -> Vec<(String, String)> {
    let mut files = Vec::new();
    for top in ["crates", "tools"] {
        rust_files(&repo().join(top), &mut files);
    }
    assert!(files.len() > 100, "found only {} Rust files", files.len());
    files
        .into_iter()
        .map(|p| {
            let shown = p
                .strip_prefix(repo())
                .unwrap_or(&p)
                .display()
                .to_string()
                .replace('\\', "/");
            (shown, std::fs::read_to_string(&p).unwrap_or_default())
        })
        .collect()
}

/// Each `"FLINT_<UPPER>"` string literal in `text`, by line.
fn flint_literals(text: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (n, line) in text.lines().enumerate() {
        // A build-time variable (`option_env!`) is not the runtime's
        if line.contains("option_env!") || line.contains("env!(") {
            continue;
        }
        let mut rest = line;
        while let Some(at) = rest.find("\"FLINT_") {
            let name: String = rest[at + 1..]
                .chars()
                .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                .collect();
            let after = &rest[at + 1 + name.len()..];
            if after.starts_with('"') && name != "FLINT_" {
                found.push((n + 1, name.clone()));
            }
            rest = &rest[at + 1 + name.len()..];
        }
    }
    found
}

/// Whether `line` calls `var` or `var_os` on a literal `FLINT_` name: a read, not
/// `set_var` / `remove_var` (a test setting one up for its child).
fn reads_flint_var(line: &str) -> bool {
    let squeezed: String = line.split_whitespace().collect();
    ["var(\"FLINT_", "var_os(\"FLINT_"].iter().any(|read| {
        squeezed.match_indices(read).any(|(at, _)| {
            let before = squeezed[..at].chars().next_back();
            !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    })
}

#[test]
fn every_flint_variable_is_read_through_the_table() {
    let mut offenders = Vec::new();
    for (file, text) in sources() {
        if file == "crates/flint-core/src/env.rs" {
            continue;
        }
        for (n, line) in text.lines().enumerate() {
            if reads_flint_var(line) {
                offenders.push(format!("{file}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "read these through flint_core::env (a switch's getter, or add it to the table):\n{}",
        offenders.join("\n")
    );
}

#[test]
fn every_flint_name_in_the_tree_is_a_listed_switch() {
    let mut unknown = Vec::new();
    let mut seen = 0;
    for (file, text) in sources() {
        for (line, name) in flint_literals(&text) {
            seen += 1;
            if flint_core::env::find(&name).is_none() {
                unknown.push(format!("{file}:{line}: {name}"));
            }
        }
    }
    assert!(seen > 20, "found only {seen} names: is the scan broken?");
    assert!(
        unknown.is_empty(),
        "these name no switch in flint_core::env's table; add each (name, kind, default, help, group):\n{}",
        unknown.join("\n")
    );
}

#[test]
fn a_read_is_told_from_a_write() {
    assert!(reads_flint_var("let v = std::env::var(\"FLINT_X\");"));
    assert!(reads_flint_var("var_os( \"FLINT_X\")"));
    assert!(!reads_flint_var("std::env::set_var(\"FLINT_X\", \"1\");"));
    assert!(!reads_flint_var("std::env::remove_var(\"FLINT_X\");"));
}

#[test]
fn the_literal_scan_finds_names_and_skips_build_time_ones() {
    let text =
        "let a = \"FLINT_VSYNC\";\nlet b = option_env!(\"FLINT_APK_VERSION\");\n\"FLINT_X_{}\"";
    assert_eq!(flint_literals(text), vec![(1, "FLINT_VSYNC".to_string())]);
}
