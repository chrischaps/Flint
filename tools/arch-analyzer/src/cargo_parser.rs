use crate::model::Edge;
use std::path::{Path, PathBuf};

pub struct CrateMeta {
    pub name: String,
    pub path: String,
    pub description: Option<String>,
    pub external_deps: Vec<String>,
    pub internal_deps: Vec<String>,
    /// Each engine crate this one depends on, with how: optional, or only
    /// for some targets. What the book's table prints (`book.rs`).
    pub internal_dep_notes: Vec<InternalDep>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct InternalDep {
    pub name: String,
    /// `optional = true`: behind a feature.
    pub optional: bool,
    /// From a `[target.'cfg(..)'.dependencies]` table: the cfg, shortened
    /// to what it names ("wasm32", "not wasm32", "android").
    pub target: Option<String>,
}

/// `cfg(not(target_arch = "wasm32"))` -> `not wasm32`.
fn short_cfg(cfg: &str) -> String {
    let inner = cfg.trim().trim_start_matches("cfg(").trim_end_matches(')');
    let (neg, inner) = match inner.strip_prefix("not(") {
        Some(rest) => (true, rest.trim_end_matches(')')),
        None => (false, inner),
    };
    let value = inner
        .split('=')
        .nth(1)
        .map(|v| v.trim().trim_matches('"').to_string())
        .unwrap_or_else(|| inner.to_string());
    if neg {
        format!("not {value}")
    } else {
        value
    }
}

pub fn find_crate_dirs(workspace_root: &Path) -> Vec<PathBuf> {
    let cargo_toml = workspace_root.join("Cargo.toml");
    let content =
        std::fs::read_to_string(&cargo_toml).expect("Failed to read workspace Cargo.toml");
    let parsed: toml::Value = content
        .parse()
        .expect("Failed to parse workspace Cargo.toml");

    let members = parsed["workspace"]["members"]
        .as_array()
        .expect("No workspace.members array");

    let mut dirs = Vec::new();
    for member in members {
        let member_path = member.as_str().unwrap();
        let path = workspace_root.join(member_path);
        if path.join("Cargo.toml").exists() {
            dirs.push(path);
        }
    }
    dirs.sort();
    dirs
}

pub fn parse_crate(workspace_root: &Path, crate_dir: &Path) -> (CrateMeta, Vec<Edge>) {
    let cargo_toml = crate_dir.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_toml)
        .unwrap_or_else(|_| panic!("Failed to read {:?}", cargo_toml));
    let parsed: toml::Value = content
        .parse()
        .unwrap_or_else(|_| panic!("Failed to parse {:?}", cargo_toml));

    let package = &parsed["package"];
    let name = package["name"].as_str().unwrap().to_string();
    let description = package
        .get("description")
        .and_then(|d| d.as_str())
        .map(|s| s.to_string());

    let rel_path = crate_dir
        .strip_prefix(workspace_root)
        .unwrap_or(crate_dir)
        .to_string_lossy()
        .to_string();

    let mut internal_deps = Vec::new();
    let mut external_deps = Vec::new();
    let mut edges = Vec::new();

    let mut internal_dep_notes = Vec::new();

    // `[dependencies]`, then each `[target.'cfg(..)'.dependencies]`.
    let mut tables: Vec<(Option<String>, &toml::Table)> = Vec::new();
    if let Some(deps) = parsed.get("dependencies").and_then(|d| d.as_table()) {
        tables.push((None, deps));
    }
    if let Some(targets) = parsed.get("target").and_then(|t| t.as_table()) {
        for (cfg, body) in targets {
            if let Some(deps) = body.get("dependencies").and_then(|d| d.as_table()) {
                tables.push((Some(short_cfg(cfg)), deps));
            }
        }
    }

    for (target, deps) in tables {
        for (dep_name, spec) in deps {
            if dep_name.starts_with("flint-") {
                let optional = spec
                    .get("optional")
                    .and_then(|o| o.as_bool())
                    .unwrap_or(false);
                internal_dep_notes.push(InternalDep {
                    name: dep_name.clone(),
                    optional,
                    target: target.clone(),
                });
                if !internal_deps.contains(dep_name) {
                    internal_deps.push(dep_name.clone());
                    edges.push(Edge {
                        from: name.clone(),
                        to: dep_name.clone(),
                    });
                }
            } else if !external_deps.contains(dep_name) {
                external_deps.push(dep_name.clone());
            }
        }
    }

    internal_deps.sort();
    external_deps.sort();
    internal_dep_notes.sort();

    (
        CrateMeta {
            name,
            path: rel_path,
            description,
            external_deps,
            internal_deps,
            internal_dep_notes,
        },
        edges,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_crate_dirs_finds_flint_crates() {
        let root = find_workspace_root_for_test();
        let dirs = find_crate_dirs(&root);
        assert!(
            dirs.len() >= 20,
            "Expected at least 20 crates, found {}",
            dirs.len()
        );
        for dir in &dirs {
            assert!(
                dir.join("Cargo.toml").exists(),
                "Missing Cargo.toml in {:?}",
                dir
            );
        }
    }

    #[test]
    fn test_parse_crate_extracts_deps() {
        let root = find_workspace_root_for_test();
        let crate_dir = root.join("crates/flint-ecs");
        let (meta, edges) = parse_crate(&root, &crate_dir);
        assert_eq!(meta.name, "flint-ecs");
        assert!(meta.internal_deps.contains(&"flint-core".to_string()));
        assert!(meta.internal_deps.contains(&"flint-schema".to_string()));
        assert!(edges
            .iter()
            .any(|e| e.from == "flint-ecs" && e.to == "flint-core"));
    }

    #[test]
    fn test_parse_crate_extracts_external_deps() {
        let root = find_workspace_root_for_test();
        let crate_dir = root.join("crates/flint-core");
        let (meta, _) = parse_crate(&root, &crate_dir);
        assert!(
            !meta.external_deps.is_empty(),
            "Expected external deps for flint-core"
        );
    }

    #[test]
    fn test_short_cfg() {
        assert_eq!(short_cfg(r#"cfg(target_arch = "wasm32")"#), "wasm32");
        assert_eq!(
            short_cfg(r#"cfg(not(target_arch = "wasm32"))"#),
            "not wasm32"
        );
        assert_eq!(short_cfg(r#"cfg(target_os = "android")"#), "android");
    }

    #[test]
    fn test_optional_dep_is_noted() {
        let root = find_workspace_root_for_test();
        let (meta, _) = parse_crate(&root, &root.join("crates/flint-player"));
        let debug_ui = meta
            .internal_dep_notes
            .iter()
            .find(|d| d.name == "flint-debug-ui")
            .expect("flint-player depends on flint-debug-ui");
        assert!(debug_ui.optional);
    }

    fn find_workspace_root_for_test() -> std::path::PathBuf {
        let mut dir = std::env::current_dir().unwrap();
        loop {
            if dir.join("Cargo.toml").exists() {
                let content = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
                if content.contains("[workspace]") {
                    return dir;
                }
            }
            if !dir.pop() {
                panic!("Could not find workspace root");
            }
        }
    }
}
