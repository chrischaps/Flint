//! What the crate's layout promises (#697), held: `unsafe` only in
//! `context.rs`, where the raw pointers a host lends live, and no source
//! file grown past about 1.5k lines.

use std::path::{Path, PathBuf};

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn crate_sources() -> Vec<PathBuf> {
    let mut out = Vec::new();
    sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
    assert!(out.len() > 20, "{out:?}");
    out
}

/// A line's code, without a `//` comment.
fn code(line: &str) -> &str {
    line.split("//").next().unwrap_or("")
}

#[test]
fn unsafe_lives_in_context_rs_alone() {
    let mut found = Vec::new();
    for path in crate_sources() {
        // context.rs holds the pointers; this file names the word
        if path
            .file_name()
            .is_some_and(|n| n == "context.rs" || n == "layout_tests.rs")
        {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for (n, line) in text.lines().enumerate() {
            let code = code(line);
            if code.contains("unsafe ") || code.contains("unsafe{") {
                found.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(
        found.is_empty(),
        "unsafe outside context.rs (reach the world through `call.world()` and \
         `ScriptCallContext::try_world*`):\n{}",
        found.join("\n")
    );
}

#[test]
fn no_source_file_is_over_1500_lines() {
    let long: Vec<String> = crate_sources()
        .into_iter()
        .filter_map(|path| {
            let lines = std::fs::read_to_string(&path).unwrap().lines().count();
            (lines > 1500).then(|| format!("{} ({lines} lines)", path.display()))
        })
        .collect();
    assert!(
        long.is_empty(),
        "split these along their seams: {}",
        long.join(", ")
    );
}
