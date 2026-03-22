use crate::model::Edge;
use std::path::{Path, PathBuf};

pub struct CrateMeta {
    pub name: String,
    pub path: String,
    pub description: Option<String>,
    pub external_deps: Vec<String>,
    pub internal_deps: Vec<String>,
}

pub fn find_crate_dirs(workspace_root: &Path) -> Vec<PathBuf> {
    let cargo_toml = workspace_root.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_toml).expect("Failed to read workspace Cargo.toml");
    let parsed: toml::Value = content.parse().expect("Failed to parse workspace Cargo.toml");

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
    let parsed: toml::Value = content.parse()
        .unwrap_or_else(|_| panic!("Failed to parse {:?}", cargo_toml));

    let package = &parsed["package"];
    let name = package["name"].as_str().unwrap().to_string();
    let description = package.get("description").and_then(|d| d.as_str()).map(|s| s.to_string());

    let rel_path = crate_dir.strip_prefix(workspace_root)
        .unwrap_or(crate_dir)
        .to_string_lossy()
        .to_string();

    let mut internal_deps = Vec::new();
    let mut external_deps = Vec::new();
    let mut edges = Vec::new();

    if let Some(deps) = parsed.get("dependencies").and_then(|d| d.as_table()) {
        for (dep_name, _) in deps {
            if dep_name.starts_with("flint-") {
                internal_deps.push(dep_name.clone());
                edges.push(Edge {
                    from: name.clone(),
                    to: dep_name.clone(),
                });
            } else {
                external_deps.push(dep_name.clone());
            }
        }
    }

    internal_deps.sort();
    external_deps.sort();

    (CrateMeta { name, path: rel_path, description, external_deps, internal_deps }, edges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_crate_dirs_finds_flint_crates() {
        let root = find_workspace_root_for_test();
        let dirs = find_crate_dirs(&root);
        assert!(dirs.len() >= 20, "Expected at least 20 crates, found {}", dirs.len());
        for dir in &dirs {
            assert!(dir.join("Cargo.toml").exists(), "Missing Cargo.toml in {:?}", dir);
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
        assert!(edges.iter().any(|e| e.from == "flint-ecs" && e.to == "flint-core"));
    }

    #[test]
    fn test_parse_crate_extracts_external_deps() {
        let root = find_workspace_root_for_test();
        let crate_dir = root.join("crates/flint-core");
        let (meta, _) = parse_crate(&root, &crate_dir);
        assert!(!meta.external_deps.is_empty(), "Expected external deps for flint-core");
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
