# Architecture Explorer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a web-based interactive visualization of the Flint engine's crate structure, module hierarchy, and public API surface.

**Architecture:** A Rust binary (`flint-arch-analyzer`) parses the workspace to produce `arch-data.json`. A static web app (`tools/arch-viewer/`) loads this JSON and renders an interactive Cytoscape.js graph with search, filtering, path finding, and drill-down to struct fields.

**Tech Stack:** Rust (`syn`, `toml`, `serde_json`, `walkdir`), Cytoscape.js (CDN), vanilla HTML/CSS/JS.

**Spec:** `docs/superpowers/specs/2026-03-22-architecture-explorer-design.md`

---

## File Structure

```
tools/
  arch-analyzer/
    Cargo.toml
    src/
      main.rs               # CLI entry point, orchestration
      model.rs              # Serde data structures
      cargo_parser.rs       # Cargo.toml dependency extraction
      source_parser.rs      # syn-based module/item extraction
      metrics.rs            # Line counts, tier computation
  arch-viewer/
    index.html              # App shell, three-panel layout
    style.css               # Dark theme, tier colors
    app.js                  # Cytoscape graph + all UI logic
    arch-data.json          # Generated output (gitignored)
```

---

### Task 1: Scaffold Analyzer Crate

**Files:**
- Create: `tools/arch-analyzer/Cargo.toml`
- Create: `tools/arch-analyzer/src/main.rs`
- Create: `tools/arch-analyzer/src/model.rs`
- Modify: `Cargo.toml` (workspace root — add member)
- Modify: `.gitignore` (add `arch-data.json`)

- [ ] **Step 1: Create directory structure**

```bash
mkdir -p tools/arch-analyzer/src
```

- [ ] **Step 2: Create `tools/arch-analyzer/Cargo.toml`**

```toml
[package]
name = "flint-arch-analyzer"
version.workspace = true
edition.workspace = true
license.workspace = true
description = "Static analysis tool that generates architecture data for the Flint engine"

[[bin]]
name = "flint-arch-analyzer"
path = "src/main.rs"

[dependencies]
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
toml = { workspace = true }
syn = { version = "2", features = ["full", "visit"] }
walkdir = "2"
chrono = { version = "0.4", features = ["serde"] }
```

- [ ] **Step 3: Create `tools/arch-analyzer/src/model.rs`**

Define all serde-serializable data structures matching the spec's JSON schema:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct ArchData {
    pub generated_at: String,
    pub crates: Vec<CrateInfo>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CrateInfo {
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub lines: usize,
    pub external_deps: Vec<String>,
    pub internal_deps: Vec<String>,
    pub tier: u32,
    pub modules: Vec<ModuleInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ModuleInfo {
    pub name: String,
    pub path: String,
    pub lines: usize,
    pub public_items: Vec<PublicItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ModuleInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PublicItem {
    pub kind: ItemKind,
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<Member>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Struct,
    Enum,
    Trait,
    Fn,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
}
```

- [ ] **Step 4: Create `tools/arch-analyzer/src/main.rs`** (skeleton)

```rust
mod model;
mod cargo_parser;
mod source_parser;
mod metrics;

use model::ArchData;
use std::path::PathBuf;

fn main() {
    let workspace_root = find_workspace_root();
    let output_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root.join("tools/arch-viewer/arch-data.json"));

    let data = analyze(&workspace_root);

    let json = serde_json::to_string_pretty(&data).expect("Failed to serialize");
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&output_path, &json).expect("Failed to write output");
    println!("Wrote {} crates, {} edges to {}", data.crates.len(), data.edges.len(), output_path.display());
}

fn find_workspace_root() -> PathBuf {
    let mut dir = std::env::current_dir().expect("No current dir");
    loop {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            let content = std::fs::read_to_string(&cargo_toml).unwrap();
            if content.contains("[workspace]") {
                return dir;
            }
        }
        if !dir.pop() {
            panic!("Could not find workspace root (no Cargo.toml with [workspace] found)");
        }
    }
}

fn analyze(workspace_root: &std::path::Path) -> ArchData {
    let crate_dirs = cargo_parser::find_crate_dirs(workspace_root);
    let mut crates = Vec::new();
    let mut edges = Vec::new();

    for crate_dir in &crate_dirs {
        let (info, crate_edges) = cargo_parser::parse_crate(workspace_root, crate_dir);
        edges.extend(crate_edges);
        let modules = source_parser::parse_crate_sources(crate_dir);
        let lines = metrics::total_lines(&modules);
        crates.push(model::CrateInfo {
            name: info.name,
            path: info.path,
            description: info.description,
            lines,
            external_deps: info.external_deps,
            internal_deps: info.internal_deps,
            tier: 0, // computed below
            modules,
        });
    }

    metrics::compute_tiers(&mut crates);

    ArchData {
        generated_at: chrono::Utc::now().to_rfc3339(),
        crates,
        edges,
    }
}
```

Also create stub files so it compiles:

```rust
// tools/arch-analyzer/src/cargo_parser.rs
use crate::model;
use std::path::Path;

pub struct CrateMeta {
    pub name: String,
    pub path: String,
    pub description: Option<String>,
    pub external_deps: Vec<String>,
    pub internal_deps: Vec<String>,
}

pub fn find_crate_dirs(_workspace_root: &Path) -> Vec<std::path::PathBuf> {
    vec![]
}

pub fn parse_crate(_workspace_root: &Path, _crate_dir: &Path) -> (CrateMeta, Vec<model::Edge>) {
    todo!()
}
```

```rust
// tools/arch-analyzer/src/source_parser.rs
use crate::model::ModuleInfo;
use std::path::Path;

pub fn parse_crate_sources(_crate_dir: &Path) -> Vec<ModuleInfo> {
    vec![]
}
```

```rust
// tools/arch-analyzer/src/metrics.rs
use crate::model::{CrateInfo, ModuleInfo};

pub fn total_lines(modules: &[ModuleInfo]) -> usize {
    0
}

pub fn compute_tiers(crates: &mut [CrateInfo]) {
}
```

- [ ] **Step 5: Add to workspace**

In the root `Cargo.toml`, add `"tools/arch-analyzer"` to the `members` array. Do NOT add to `default-members`.

- [ ] **Step 6: Add `arch-data.json` to `.gitignore`**

Add `arch-data.json` to `.gitignore`.

- [ ] **Step 7: Verify it compiles**

Run: `cargo build -p flint-arch-analyzer`
Expected: successful build with no errors.

- [ ] **Step 8: Commit**

```bash
git add tools/arch-analyzer/ Cargo.toml .gitignore Cargo.lock
git commit -m "feat(arch): scaffold flint-arch-analyzer crate with data model"
```

---

### Task 2: Cargo Parser

**Files:**
- Modify: `tools/arch-analyzer/src/cargo_parser.rs`

- [ ] **Step 1: Write tests for `find_crate_dirs`**

Add to `cargo_parser.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_crate_dirs_finds_flint_crates() {
        let root = find_workspace_root_for_test();
        let dirs = find_crate_dirs(&root);
        // Should find all flint-* crates in crates/
        assert!(dirs.len() >= 20, "Expected at least 20 crates, found {}", dirs.len());
        // Each should contain a Cargo.toml
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
        // Should have edges for internal deps
        assert!(edges.iter().any(|e| e.from == "flint-ecs" && e.to == "flint-core"));
    }

    #[test]
    fn test_parse_crate_extracts_external_deps() {
        let root = find_workspace_root_for_test();
        let crate_dir = root.join("crates/flint-core");
        let (meta, _) = parse_crate(&root, &crate_dir);
        // flint-core should have external deps like toml, serde
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p flint-arch-analyzer -- cargo_parser`
Expected: FAIL (stubs return empty/todo).

- [ ] **Step 3: Implement `find_crate_dirs`**

Reads the workspace `Cargo.toml`, extracts `[workspace] members`, resolves each to a directory path.

```rust
use std::path::{Path, PathBuf};
use crate::model::Edge;

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
```


- [ ] **Step 4: Implement `parse_crate`**

Reads a single crate's `Cargo.toml`, extracts package name, description, and separates dependencies into internal (`flint-*`) and external.

```rust
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
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p flint-arch-analyzer -- cargo_parser`
Expected: all 3 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add tools/arch-analyzer/
git commit -m "feat(arch): implement cargo parser for dependency extraction"
```

---

### Task 3: Source Parser

**Files:**
- Modify: `tools/arch-analyzer/src/source_parser.rs`

- [ ] **Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_crate_sources_finds_modules() {
        let root = find_workspace_root_for_test();
        let crate_dir = root.join("crates/flint-core");
        let modules = parse_crate_sources(&crate_dir);
        assert!(!modules.is_empty(), "Expected modules in flint-core");
        // Should find lib.rs or top-level modules
        let names: Vec<&str> = modules.iter().map(|m| m.name.as_str()).collect();
        assert!(names.contains(&"lib"), "Expected lib module");
    }

    #[test]
    fn test_extracts_public_structs() {
        let source = r#"
            pub struct Foo {
                pub x: f32,
                pub y: String,
            }
            struct Private {
                z: i32,
            }
            pub fn bar(a: i32) -> bool { true }
        "#;
        let items = extract_public_items(source);
        assert_eq!(items.len(), 2); // Foo and bar, not Private
        let foo = items.iter().find(|i| i.name == "Foo").unwrap();
        assert!(matches!(foo.kind, crate::model::ItemKind::Struct));
        assert_eq!(foo.members.len(), 2);
        assert_eq!(foo.members[0].name, "x");
        assert_eq!(foo.members[0].ty, "f32");
    }

    #[test]
    fn test_extracts_enum_variants() {
        let source = r#"
            pub enum Color {
                Red,
                Green,
                Custom(u8, u8, u8),
            }
        "#;
        let items = extract_public_items(source);
        assert_eq!(items.len(), 1);
        let color = &items[0];
        assert!(matches!(color.kind, crate::model::ItemKind::Enum));
        assert_eq!(color.members.len(), 3);
        assert_eq!(color.members[0].name, "Red");
        assert_eq!(color.members[2].name, "Custom");
    }

    #[test]
    fn test_extracts_trait_methods() {
        let source = r#"
            pub trait Drawable {
                fn draw(&self, ctx: &Context);
                fn bounds(&self) -> Rect;
            }
        "#;
        let items = extract_public_items(source);
        assert_eq!(items.len(), 1);
        let drawable = &items[0];
        assert!(matches!(drawable.kind, crate::model::ItemKind::Trait));
        assert_eq!(drawable.members.len(), 2);
        assert_eq!(drawable.members[0].name, "draw");
    }

    #[test]
    fn test_extracts_function_params() {
        let source = r#"
            pub fn process(input: &str, count: usize) -> Result<Vec<u8>, Error> {
                todo!()
            }
        "#;
        let items = extract_public_items(source);
        assert_eq!(items.len(), 1);
        let func = &items[0];
        assert!(matches!(func.kind, crate::model::ItemKind::Fn));
        // members = params + return
        assert!(func.members.iter().any(|m| m.name == "input"));
        assert!(func.members.iter().any(|m| m.name == "return"));
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
            if !dir.pop() { panic!("Could not find workspace root"); }
        }
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p flint-arch-analyzer -- source_parser`
Expected: FAIL.

- [ ] **Step 3: Implement `extract_public_items`**

A helper that parses a source string with `syn` and extracts public items with their members:

```rust
use crate::model::{ItemKind, Member, ModuleInfo, PublicItem};
use std::path::{Path, PathBuf};
use syn::{self, Item, Visibility, Type, FnArg, ReturnType, Fields};

pub fn extract_public_items(source: &str) -> Vec<PublicItem> {
    let file = match syn::parse_file(source) {
        Ok(f) => f,
        Err(_) => return vec![],
    };

    let mut items = Vec::new();
    for item in &file.items {
        match item {
            Item::Struct(s) if matches!(s.vis, Visibility::Public(_)) => {
                let members = match &s.fields {
                    Fields::Named(fields) => fields.named.iter().map(|f| Member {
                        name: f.ident.as_ref().map(|i| i.to_string()).unwrap_or_default(),
                        ty: type_to_string(&f.ty),
                    }).collect(),
                    _ => vec![],
                };
                items.push(PublicItem {
                    kind: ItemKind::Struct,
                    name: s.ident.to_string(),
                    members,
                });
            }
            Item::Enum(e) if matches!(e.vis, Visibility::Public(_)) => {
                let members = e.variants.iter().map(|v| {
                    let ty = match &v.fields {
                        Fields::Unnamed(fields) => {
                            fields.unnamed.iter()
                                .map(|f| type_to_string(&f.ty))
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                        Fields::Named(fields) => {
                            fields.named.iter()
                                .map(|f| format!("{}: {}", f.ident.as_ref().unwrap(), type_to_string(&f.ty)))
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                        Fields::Unit => String::new(),
                    };
                    Member { name: v.ident.to_string(), ty }
                }).collect();
                items.push(PublicItem {
                    kind: ItemKind::Enum,
                    name: e.ident.to_string(),
                    members,
                });
            }
            Item::Trait(t) if matches!(t.vis, Visibility::Public(_)) => {
                let members = t.items.iter().filter_map(|item| {
                    if let syn::TraitItem::Fn(method) = item {
                        Some(Member {
                            name: method.sig.ident.to_string(),
                            ty: fn_signature_string(&method.sig),
                        })
                    } else {
                        None
                    }
                }).collect();
                items.push(PublicItem {
                    kind: ItemKind::Trait,
                    name: t.ident.to_string(),
                    members,
                });
            }
            Item::Fn(f) if matches!(f.vis, Visibility::Public(_)) => {
                let mut members: Vec<Member> = f.sig.inputs.iter().filter_map(|arg| {
                    if let FnArg::Typed(pat_type) = arg {
                        let name = pat_to_string(&pat_type.pat);
                        Some(Member { name, ty: type_to_string(&pat_type.ty) })
                    } else {
                        None // skip self params
                    }
                }).collect();
                if let ReturnType::Type(_, ty) = &f.sig.output {
                    members.push(Member { name: "return".to_string(), ty: type_to_string(ty) });
                }
                items.push(PublicItem {
                    kind: ItemKind::Fn,
                    name: f.sig.ident.to_string(),
                    members,
                });
            }
            _ => {}
        }
    }
    items
}

fn type_to_string(ty: &Type) -> String {
    quote::quote!(#ty).to_string().replace(" ", "")
        // Clean up common spacing issues from quote
        .replace(",", ", ")
}

fn pat_to_string(pat: &syn::Pat) -> String {
    quote::quote!(#pat).to_string()
}

fn fn_signature_string(sig: &syn::Signature) -> String {
    let params: Vec<String> = sig.inputs.iter().map(|arg| {
        quote::quote!(#arg).to_string()
    }).collect();
    let ret = match &sig.output {
        ReturnType::Default => String::new(),
        ReturnType::Type(_, ty) => format!(" -> {}", type_to_string(ty)),
    };
    format!("({}){}", params.join(", "), ret)
}
```

Note: add `quote = "1"` to `tools/arch-analyzer/Cargo.toml` dependencies (needed for `type_to_string`).

- [ ] **Step 4: Implement `parse_crate_sources`**

Walk the crate's `src/` directory, parse each `.rs` file, build the module tree:

```rust
pub fn parse_crate_sources(crate_dir: &Path) -> Vec<ModuleInfo> {
    let src_dir = crate_dir.join("src");
    if !src_dir.exists() {
        return vec![];
    }

    // Start from lib.rs or main.rs
    let entry = if src_dir.join("lib.rs").exists() {
        src_dir.join("lib.rs")
    } else if src_dir.join("main.rs").exists() {
        src_dir.join("main.rs")
    } else {
        return vec![];
    };

    let entry_name = entry.file_stem().unwrap().to_string_lossy().to_string();
    vec![parse_module_file(&entry, &entry_name, &src_dir)]
}

fn parse_module_file(path: &Path, name: &str, src_dir: &Path) -> ModuleInfo {
    let source = std::fs::read_to_string(path).unwrap_or_default();
    let lines = source.lines().count();
    let public_items = extract_public_items(&source);

    // Find child modules declared via `mod foo;`
    let children = find_child_modules(&source, path, src_dir);

    let rel_path = path.strip_prefix(src_dir.parent().unwrap_or(src_dir))
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();

    ModuleInfo {
        name: name.to_string(),
        path: rel_path,
        lines,
        public_items,
        children,
    }
}

fn find_child_modules(source: &str, parent_path: &Path, src_dir: &Path) -> Vec<ModuleInfo> {
    let file = match syn::parse_file(source) {
        Ok(f) => f,
        Err(_) => return vec![],
    };

    let parent_dir = if parent_path.file_name().map(|f| f == "mod.rs" || f == "lib.rs" || f == "main.rs").unwrap_or(false) {
        parent_path.parent().unwrap().to_path_buf()
    } else {
        // foo.rs -> look for foo/ directory
        let stem = parent_path.file_stem().unwrap().to_string_lossy().to_string();
        parent_path.parent().unwrap().join(&stem)
    };

    let mut children = Vec::new();
    for item in &file.items {
        if let Item::Mod(m) = item {
            if m.content.is_none() {
                // `mod foo;` — external module, resolve file
                let mod_name = m.ident.to_string();
                let mod_file = parent_dir.join(format!("{}.rs", mod_name));
                let mod_dir_file = parent_dir.join(&mod_name).join("mod.rs");

                let resolved = if mod_file.exists() {
                    Some(mod_file)
                } else if mod_dir_file.exists() {
                    Some(mod_dir_file)
                } else {
                    None
                };

                if let Some(path) = resolved {
                    children.push(parse_module_file(&path, &mod_name, src_dir));
                }
            }
        }
    }
    children
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p flint-arch-analyzer -- source_parser`
Expected: all 5 tests PASS.

- [ ] **Step 6: Commit**

```bash
git add tools/arch-analyzer/
git commit -m "feat(arch): implement source parser with syn-based AST extraction"
```

---

### Task 4: Metrics & Tier Computation

**Files:**
- Modify: `tools/arch-analyzer/src/metrics.rs`

- [ ] **Step 1: Write tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn make_crate(name: &str, deps: Vec<&str>) -> CrateInfo {
        CrateInfo {
            name: name.to_string(),
            path: format!("crates/{}", name),
            description: None,
            lines: 0,
            external_deps: vec![],
            internal_deps: deps.into_iter().map(String::from).collect(),
            tier: 0,
            modules: vec![],
        }
    }

    #[test]
    fn test_total_lines() {
        let modules = vec![
            ModuleInfo {
                name: "lib".into(), path: "src/lib.rs".into(), lines: 50,
                public_items: vec![], children: vec![
                    ModuleInfo {
                        name: "child".into(), path: "src/child.rs".into(), lines: 30,
                        public_items: vec![], children: vec![],
                    },
                ],
            },
        ];
        assert_eq!(total_lines(&modules), 80);
    }

    #[test]
    fn test_compute_tiers_leaf_is_zero() {
        let mut crates = vec![make_crate("flint-core", vec![])];
        compute_tiers(&mut crates);
        assert_eq!(crates[0].tier, 0);
    }

    #[test]
    fn test_compute_tiers_chain() {
        let mut crates = vec![
            make_crate("flint-core", vec![]),
            make_crate("flint-schema", vec!["flint-core"]),
            make_crate("flint-ecs", vec!["flint-core", "flint-schema"]),
        ];
        compute_tiers(&mut crates);
        let tier_of = |name: &str| crates.iter().find(|c| c.name == name).unwrap().tier;
        assert_eq!(tier_of("flint-core"), 0);
        assert_eq!(tier_of("flint-schema"), 1);
        assert_eq!(tier_of("flint-ecs"), 2);
    }

    #[test]
    fn test_compute_tiers_diamond() {
        // A depends on B and C, both depend on D
        let mut crates = vec![
            make_crate("d", vec![]),
            make_crate("b", vec!["d"]),
            make_crate("c", vec!["d"]),
            make_crate("a", vec!["b", "c"]),
        ];
        compute_tiers(&mut crates);
        let tier_of = |name: &str| crates.iter().find(|c| c.name == name).unwrap().tier;
        assert_eq!(tier_of("d"), 0);
        assert_eq!(tier_of("b"), 1);
        assert_eq!(tier_of("c"), 1);
        assert_eq!(tier_of("a"), 2);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p flint-arch-analyzer -- metrics`
Expected: FAIL.

- [ ] **Step 3: Implement `total_lines` and `compute_tiers`**

```rust
use crate::model::{CrateInfo, ModuleInfo};
use std::collections::HashMap;

pub fn total_lines(modules: &[ModuleInfo]) -> usize {
    modules.iter().map(|m| m.lines + total_lines(&m.children)).sum()
}

pub fn compute_tiers(crates: &mut [CrateInfo]) {
    // Build a map of crate name -> index
    let name_to_idx: HashMap<&str, usize> = crates.iter().enumerate()
        .map(|(i, c)| (c.name.as_str(), i))
        .collect();

    // Iterative longest-path computation
    // tier(c) = 0 if no internal deps, else max(tier(dep) for dep in deps) + 1
    let mut tiers = vec![0u32; crates.len()];
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..crates.len() {
            let max_dep_tier = crates[i].internal_deps.iter()
                .filter_map(|dep| name_to_idx.get(dep.as_str()))
                .map(|&idx| tiers[idx])
                .max();
            if let Some(max_t) = max_dep_tier {
                let new_tier = max_t + 1;
                if new_tier != tiers[i] {
                    tiers[i] = new_tier;
                    changed = true;
                }
            }
        }
    }

    for (i, krate) in crates.iter_mut().enumerate() {
        krate.tier = tiers[i];
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p flint-arch-analyzer -- metrics`
Expected: all 4 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add tools/arch-analyzer/src/metrics.rs
git commit -m "feat(arch): implement metrics with line counts and tier computation"
```

---

### Task 5: End-to-End Analyzer Test

**Files:**
- Modify: `tools/arch-analyzer/src/main.rs`

- [ ] **Step 1: Run the analyzer against the real workspace**

Run: `cargo run -p flint-arch-analyzer -- /tmp/test-arch-data.json`
Expected: writes JSON, prints summary like "Wrote 23 crates, 91 edges to /tmp/test-arch-data.json".

- [ ] **Step 2: Validate the output**

Run: `cat /tmp/test-arch-data.json | python3 -c "import json,sys; d=json.load(sys.stdin); print(f'{len(d[\"crates\"])} crates, {len(d[\"edges\"])} edges'); print('Tiers:', sorted(set(c['tier'] for c in d['crates'])))""`
Expected: 23+ crates, 90+ edges, tiers [0, 1, 2, ...].

- [ ] **Step 3: Spot-check data quality**

Run: `cat /tmp/test-arch-data.json | python3 -c "import json,sys; d=json.load(sys.stdin); core=[c for c in d['crates'] if c['name']=='flint-core'][0]; print(f'flint-core: tier={core[\"tier\"]}, {len(core[\"modules\"])} modules, {core[\"lines\"]} lines'); print('Modules:', [m['name'] for m in core['modules']])"`
Expected: flint-core at tier 0, with modules and reasonable line count.

- [ ] **Step 4: Fix any issues found, re-run**

- [ ] **Step 5: Commit any fixes**

```bash
git add tools/arch-analyzer/
git commit -m "fix(arch): end-to-end analyzer fixes"
```

---

### Task 6: Web Viewer — HTML Shell & CSS

**Files:**
- Create: `tools/arch-viewer/index.html`
- Create: `tools/arch-viewer/style.css`

- [ ] **Step 1: Create directory**

```bash
mkdir -p tools/arch-viewer
```

- [ ] **Step 2: Create `tools/arch-viewer/index.html`**

Three-panel layout loading Cytoscape.js from CDN:

```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Flint Architecture Explorer</title>
  <link rel="stylesheet" href="style.css">
  <script src="https://cdnjs.cloudflare.com/ajax/libs/cytoscape/3.30.4/cytoscape.min.js"></script>
  <script src="https://cdnjs.cloudflare.com/ajax/libs/dagre/0.8.5/dagre.min.js"></script>
  <script src="https://cdnjs.cloudflare.com/ajax/libs/cytoscape-dagre/2.5.0/cytoscape-dagre.min.js"></script>
</head>
<body>
  <div id="app">
    <aside id="toolbar">
      <div class="toolbar-section">
        <label class="toolbar-label">Search</label>
        <input type="text" id="search-input" placeholder="Find crate, module, type...">
      </div>
      <div class="toolbar-section">
        <label class="toolbar-label">Filter by Tier</label>
        <div id="tier-filters"></div>
      </div>
      <div class="toolbar-section">
        <label class="toolbar-label">Layout</label>
        <div id="layout-buttons">
          <button class="layout-btn active" data-layout="dagre">Hierarchical</button>
          <button class="layout-btn" data-layout="cose">Force-directed</button>
          <button class="layout-btn" data-layout="concentric">Concentric</button>
        </div>
      </div>
      <div class="toolbar-section">
        <label class="toolbar-label">Tools</label>
        <div id="tool-buttons">
          <button class="tool-btn" id="btn-path-finder">Path Finder</button>
          <button class="tool-btn" id="btn-metrics">Metrics Overlay</button>
          <button class="tool-btn" id="btn-dep-explorer">Dep Explorer</button>
        </div>
      </div>
      <div id="toolbar-footer"></div>
    </aside>
    <main id="graph-container"></main>
    <aside id="detail-panel">
      <div id="detail-content">
        <p class="detail-placeholder">Click a node or edge to see details</p>
      </div>
    </aside>
  </div>
  <script src="app.js"></script>
</body>
</html>
```

- [ ] **Step 3: Create `tools/arch-viewer/style.css`**

Full dark theme with tier colors matching the spec:

```css
* { margin: 0; padding: 0; box-sizing: border-box; }

:root {
  --bg: #0f0f1a;
  --panel-bg: #1a1a2e;
  --surface: #252540;
  --border: #333;
  --text: #ccc;
  --text-dim: #888;
  --accent: #5865F2;
  --tier0: #4ade80;
  --tier1: #60a5fa;
  --tier2: #a78bfa;
  --tier3: #fbbf24;
  --tier4: #f87171;
}

body {
  background: var(--bg);
  color: var(--text);
  font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
  font-size: 13px;
  overflow: hidden;
  height: 100vh;
}

#app {
  display: flex;
  height: 100vh;
}

/* Left Toolbar */
#toolbar {
  width: 220px;
  background: var(--panel-bg);
  border-right: 1px solid var(--border);
  padding: 12px;
  display: flex;
  flex-direction: column;
  gap: 16px;
  flex-shrink: 0;
  overflow-y: auto;
}

.toolbar-label {
  display: block;
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: 1px;
  color: var(--text-dim);
  margin-bottom: 8px;
}

.toolbar-section { }

#search-input {
  width: 100%;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 8px 10px;
  color: var(--text);
  font-size: 13px;
  outline: none;
}

#search-input:focus { border-color: var(--accent); }

.tier-toggle {
  display: inline-block;
  padding: 3px 8px;
  border-radius: 4px;
  font-size: 11px;
  cursor: pointer;
  margin: 2px;
  opacity: 1;
  transition: opacity 0.2s;
  border: none;
  font-weight: 500;
}

.tier-toggle.inactive { opacity: 0.3; }

.layout-btn, .tool-btn {
  display: block;
  width: 100%;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 4px;
  padding: 6px 10px;
  color: var(--text-dim);
  font-size: 12px;
  cursor: pointer;
  text-align: left;
  margin-bottom: 4px;
}

.layout-btn.active, .tool-btn.active {
  border-color: var(--accent);
  color: var(--text);
}

.layout-btn:hover, .tool-btn:hover { color: var(--text); }

#toolbar-footer {
  margin-top: auto;
  font-size: 11px;
  color: #666;
  border-top: 1px solid var(--border);
  padding-top: 8px;
}

/* Center Canvas */
#graph-container {
  flex: 1;
  background: var(--bg);
  position: relative;
}

/* Right Detail Panel */
#detail-panel {
  width: 260px;
  background: var(--panel-bg);
  border-left: 1px solid var(--border);
  padding: 12px;
  flex-shrink: 0;
  overflow-y: auto;
}

.detail-placeholder { color: var(--text-dim); text-align: center; margin-top: 40%; }

.detail-label {
  font-size: 11px;
  text-transform: uppercase;
  letter-spacing: 1px;
  color: var(--text-dim);
  margin-top: 12px;
  margin-bottom: 6px;
}

.detail-label:first-child { margin-top: 0; }

.detail-name { font-size: 16px; font-weight: bold; margin-bottom: 4px; }

.detail-path { font-size: 12px; color: var(--text-dim); margin-bottom: 4px; }

.detail-stat { font-size: 12px; color: #aaa; }

.item-card {
  background: var(--surface);
  padding: 6px 10px;
  border-radius: 4px;
  margin-bottom: 4px;
  cursor: pointer;
}

.item-card:hover { border-color: var(--accent); }

.item-kind { font-size: 10px; font-weight: 600; }
.item-name { font-size: 12px; color: var(--text); }

.item-kind.struct { color: var(--tier3); border-left: 3px solid var(--tier3); }
.item-kind.enum { color: var(--tier2); }
.item-kind.trait { color: var(--tier1); }
.item-kind.fn { color: var(--tier0); }

.item-card.struct { border-left: 3px solid var(--tier3); }
.item-card.enum { border-left: 3px solid var(--tier2); }
.item-card.trait { border-left: 3px solid var(--tier1); }
.item-card.fn { border-left: 3px solid var(--tier0); }

.member-row {
  background: #1e1e30;
  padding: 4px 8px;
  border-radius: 3px;
  font-family: monospace;
  font-size: 11px;
  margin-bottom: 3px;
}

.member-name { color: var(--text-dim); }
.member-type { color: var(--tier2); }

.dep-link {
  font-size: 12px;
  cursor: pointer;
  margin-bottom: 3px;
}

.dep-link:hover { text-decoration: underline; }

/* Path finder mode */
.path-finder-hint {
  background: var(--surface);
  border: 1px solid var(--accent);
  border-radius: 6px;
  padding: 8px;
  font-size: 12px;
  color: var(--text);
  margin-top: 8px;
}
```

- [ ] **Step 4: Verify the shell renders**

Open `tools/arch-viewer/index.html` in a browser.
Expected: three-panel layout visible with dark theme, toolbar sections, empty graph area, detail panel placeholder.

- [ ] **Step 5: Commit**

```bash
git add tools/arch-viewer/
git commit -m "feat(arch-viewer): create HTML shell and CSS theme"
```

---

### Task 7: Web Viewer — Graph Rendering

**Files:**
- Create: `tools/arch-viewer/app.js`

- [ ] **Step 1: Create `app.js` with data loading and basic graph**

Core application: load JSON, build Cytoscape elements, render crate-level graph with tier coloring and edges.

```javascript
// app.js — Flint Architecture Explorer

const TIER_COLORS = {
  0: '#4ade80', 1: '#60a5fa', 2: '#a78bfa',
  3: '#fbbf24', 4: '#f87171', 5: '#f87171',
  6: '#f87171', 7: '#f87171',
};

const TIER_BG = {
  0: '#1e3a2e', 1: '#1e2a3a', 2: '#2a1e3a',
  3: '#3a2a1e', 4: '#3a1e1e', 5: '#3a1e1e',
  6: '#3a1e1e', 7: '#3a1e1e',
};

const TIER_NAMES = {
  0: 'Core', 1: 'ECS', 2: 'Scene',
  3: 'Systems', 4: 'Integration', 5: 'Integration',
  6: 'Integration', 7: 'Aggregators',
};

let cy;
let archData = null;

async function init() {
  try {
    const resp = await fetch('arch-data.json');
    archData = await resp.json();
  } catch (e) {
    document.getElementById('detail-content').innerHTML =
      '<p style="color:#f87171;text-align:center;margin-top:40%">Failed to load arch-data.json.<br>Run flint-arch-analyzer first.</p>';
    return;
  }

  buildGraph();
  buildTierFilters();
  setupSearch();
  setupLayoutButtons();
  setupTools();
  updateFooter();
}

function buildGraph() {
  const elements = [];

  // Crate nodes
  for (const crate of archData.crates) {
    const dependentCount = archData.edges.filter(e => e.to === crate.name).length;
    elements.push({
      group: 'nodes',
      data: {
        id: crate.name,
        label: crate.name.replace('flint-', ''),
        tier: crate.tier,
        lines: crate.lines,
        dependentCount,
        type: 'crate',
        crateData: crate,
      },
    });
  }

  // Edges
  for (const edge of archData.edges) {
    elements.push({
      group: 'edges',
      data: {
        id: `${edge.from}->${edge.to}`,
        source: edge.from,
        target: edge.to,
      },
    });
  }

  cy = cytoscape({
    container: document.getElementById('graph-container'),
    elements,
    style: [
      {
        selector: 'node[type="crate"]',
        style: {
          'label': 'data(label)',
          'text-valign': 'center',
          'text-halign': 'center',
          'font-size': '11px',
          'color': ele => TIER_COLORS[ele.data('tier')] || '#ccc',
          'background-color': ele => TIER_BG[ele.data('tier')] || '#252540',
          'border-width': ele => Math.min(1 + ele.data('dependentCount') * 0.5, 4),
          'border-color': ele => TIER_COLORS[ele.data('tier')] || '#ccc',
          'shape': 'roundrectangle',
          'width': 'label',
          'height': 30,
          'padding': '12px',
          'text-wrap': 'none',
        },
      },
      {
        selector: 'node[type="module"]',
        style: {
          'label': 'data(label)',
          'text-valign': 'center',
          'text-halign': 'center',
          'font-size': '9px',
          'color': '#ccc',
          'background-color': '#252540',
          'border-width': 1,
          'border-color': '#444',
          'shape': 'roundrectangle',
          'width': 'label',
          'height': 22,
          'padding': '8px',
        },
      },
      {
        selector: ':parent',
        style: {
          'background-color': ele => {
            const color = TIER_COLORS[ele.data('tier')] || '#ccc';
            return color + '08';
          },
          'border-style': 'dashed',
          'border-width': 1,
          'border-color': ele => (TIER_COLORS[ele.data('tier')] || '#ccc') + '40',
          'text-valign': 'top',
          'text-halign': 'center',
          'font-size': '10px',
          'padding': '12px',
        },
      },
      {
        selector: 'edge',
        style: {
          'width': 1.5,
          'line-color': '#ffffff15',
          'target-arrow-color': '#ffffff30',
          'target-arrow-shape': 'triangle',
          'curve-style': 'bezier',
          'arrow-scale': 0.8,
        },
      },
      {
        selector: 'edge.highlighted',
        style: {
          'line-color': '#5865F2',
          'target-arrow-color': '#5865F2',
          'width': 2.5,
          'z-index': 10,
        },
      },
      {
        selector: 'node.highlighted',
        style: {
          'border-width': 3,
          'border-color': '#5865F2',
          'z-index': 10,
        },
      },
      {
        selector: 'node.dimmed, edge.dimmed',
        style: { 'opacity': 0.15 },
      },
      {
        selector: 'node.search-match',
        style: {
          'border-width': 3,
          'border-color': '#fff',
          'z-index': 10,
        },
      },
    ],
    layout: { name: 'dagre', rankDir: 'TB', spacingFactor: 1.2, nodeSep: 60, rankSep: 80 },
    wheelSensitivity: 0.3,
  });

  // Click handlers
  cy.on('tap', 'node[type="crate"]', onCrateClick);
  cy.on('tap', 'node[type="module"]', onModuleClick);
  cy.on('tap', 'edge', onEdgeClick);
  cy.on('tap', function(e) {
    if (e.target === cy) { clearSelection(); }
  });
}

function onCrateClick(e) {
  const node = e.target;
  const crateData = node.data('crateData');

  if (node.isParent()) {
    // Collapse: remove child module nodes
    collapseCrate(node);
  } else {
    // Expand: add module nodes as children
    expandCrate(node, crateData);
  }

  showCrateDetail(crateData);
}

function expandCrate(node, crateData) {
  if (!crateData.modules || crateData.modules.length === 0) return;

  const flatModules = flattenModules(crateData.modules, crateData.name);
  for (const mod of flatModules) {
    cy.add({
      group: 'nodes',
      data: {
        id: mod.id,
        label: mod.name,
        parent: crateData.name,
        type: 'module',
        moduleData: mod.data,
        parentCrate: crateData.name,
      },
    });
  }

  // Re-run layout just for the expanded children
  cy.layout({
    name: 'grid',
    fit: false,
    boundingBox: node.boundingBox(),
    rows: Math.ceil(Math.sqrt(flatModules.length)),
  }).run();
}

function collapseCrate(node) {
  const children = node.children();
  children.remove();
}

function flattenModules(modules, crateId, prefix = '') {
  const result = [];
  for (const mod of modules) {
    const id = `${crateId}::${prefix}${mod.name}`;
    result.push({ id, name: mod.name, data: mod });
    if (mod.children && mod.children.length > 0) {
      result.push(...flattenModules(mod.children, crateId, `${prefix}${mod.name}::`));
    }
  }
  return result;
}

function onModuleClick(e) {
  const node = e.target;
  showModuleDetail(node.data('moduleData'), node.data('parentCrate'));
}

function onEdgeClick(e) {
  const edge = e.target;
  showEdgeDetail(edge.data('source'), edge.data('target'));
}

function clearSelection() {
  cy.elements().removeClass('highlighted dimmed');
  document.getElementById('detail-content').innerHTML =
    '<p class="detail-placeholder">Click a node or edge to see details</p>';
}

// ---- Detail Panel Renderers ----

function showCrateDetail(crate) {
  const tierColor = TIER_COLORS[crate.tier] || '#ccc';
  let html = `
    <div class="detail-label">Crate</div>
    <div class="detail-name" style="color:${tierColor}">${crate.name}</div>
    <div class="detail-path">${crate.path}</div>
    <div class="detail-stat">${crate.lines.toLocaleString()} lines · Tier ${crate.tier}</div>
  `;

  if (crate.description) {
    html += `<div class="detail-stat" style="margin-top:4px">${crate.description}</div>`;
  }

  if (crate.internal_deps.length > 0) {
    html += '<div class="detail-label">Internal Dependencies</div>';
    for (const dep of crate.internal_deps) {
      const depColor = TIER_COLORS[archData.crates.find(c => c.name === dep)?.tier] || '#ccc';
      html += `<div class="dep-link" style="color:${depColor}" onclick="navigateTo('${dep}')">→ ${dep}</div>`;
    }
  }

  if (crate.external_deps.length > 0) {
    html += '<div class="detail-label">External Dependencies</div>';
    html += `<div class="detail-stat">${crate.external_deps.join(', ')}</div>`;
  }

  if (crate.modules.length > 0) {
    html += '<div class="detail-label">Modules</div>';
    for (const mod of crate.modules) {
      html += `<div class="item-card" onclick='showModuleDetail(${JSON.stringify(mod).replace(/'/g, "\\'")} , "${crate.name}")'><div class="item-name">${mod.name}</div><div class="detail-stat">${mod.lines} lines</div></div>`;
    }
  }

  document.getElementById('detail-content').innerHTML = html;
}

function showModuleDetail(mod, crateName) {
  const tierColor = TIER_COLORS[archData.crates.find(c => c.name === crateName)?.tier] || '#ccc';
  let html = `
    <div class="detail-label">Module</div>
    <div class="detail-name" style="color:${tierColor}">${mod.name}</div>
    <div class="detail-path">${mod.path}</div>
    <div class="detail-stat">${mod.lines} lines</div>
  `;

  if (mod.public_items && mod.public_items.length > 0) {
    html += '<div class="detail-label">Public API</div>';
    for (const item of mod.public_items) {
      html += `<div class="item-card ${item.kind}" onclick='showItemDetail(${JSON.stringify(item).replace(/'/g, "\\'")})'>
        <div class="item-kind ${item.kind}">${item.kind}</div>
        <div class="item-name">${item.name}</div>
      </div>`;
    }
  }

  if (mod.children && mod.children.length > 0) {
    html += '<div class="detail-label">Submodules</div>';
    for (const child of mod.children) {
      html += `<div class="item-card" onclick='showModuleDetail(${JSON.stringify(child).replace(/'/g, "\\'")} , "${crateName}")'>
        <div class="item-name">${child.name}</div>
        <div class="detail-stat">${child.lines} lines</div>
      </div>`;
    }
  }

  document.getElementById('detail-content').innerHTML = html;
}

function showItemDetail(item) {
  let html = `
    <div class="detail-label">${item.kind}</div>
    <div class="detail-name">${item.name}</div>
  `;

  if (item.members && item.members.length > 0) {
    const label = item.kind === 'fn' ? 'Signature' :
                  item.kind === 'trait' ? 'Methods' :
                  item.kind === 'enum' ? 'Variants' : 'Fields';
    html += `<div class="detail-label">${label}</div>`;
    for (const member of item.members) {
      html += `<div class="member-row">
        <span class="member-name">${member.name}:</span> <span class="member-type">${member.type}</span>
      </div>`;
    }
  }

  document.getElementById('detail-content').innerHTML = html;
}

function showEdgeDetail(source, target) {
  const srcCrate = archData.crates.find(c => c.name === source);
  const tgtCrate = archData.crates.find(c => c.name === target);
  const srcColor = TIER_COLORS[srcCrate?.tier] || '#ccc';
  const tgtColor = TIER_COLORS[tgtCrate?.tier] || '#ccc';

  const html = `
    <div class="detail-label">Dependency</div>
    <div class="dep-link" style="color:${srcColor}" onclick="navigateTo('${source}')">${source}</div>
    <div style="color:var(--text-dim);margin:4px 0">depends on</div>
    <div class="dep-link" style="color:${tgtColor}" onclick="navigateTo('${target}')">${target}</div>
  `;

  document.getElementById('detail-content').innerHTML = html;

  // Highlight the edge
  cy.elements().removeClass('highlighted dimmed');
  const edge = cy.getElementById(`${source}->${target}`);
  edge.addClass('highlighted');
  cy.getElementById(source).addClass('highlighted');
  cy.getElementById(target).addClass('highlighted');
}

function navigateTo(nodeId) {
  const node = cy.getElementById(nodeId);
  if (node.length > 0) {
    cy.animate({ center: { eles: node }, zoom: cy.zoom() }, { duration: 300 });
    node.addClass('highlighted');
    setTimeout(() => node.removeClass('highlighted'), 1500);
    const crateData = node.data('crateData');
    if (crateData) showCrateDetail(crateData);
  }
}

// ---- Toolbar: Search ----

function setupSearch() {
  const input = document.getElementById('search-input');
  input.addEventListener('input', () => {
    const query = input.value.toLowerCase().trim();
    cy.elements().removeClass('search-match dimmed');

    if (!query) return;

    // Search crate names, module names, and public item names
    const matchingNodes = cy.nodes().filter(node => {
      const crateData = node.data('crateData');
      if (node.data('label').toLowerCase().includes(query)) return true;
      if (crateData) {
        return searchInModules(crateData.modules, query);
      }
      return false;
    });

    if (matchingNodes.length > 0) {
      cy.elements().addClass('dimmed');
      matchingNodes.removeClass('dimmed').addClass('search-match');
      matchingNodes.connectedEdges().removeClass('dimmed');
    }
  });
}

function searchInModules(modules, query) {
  for (const mod of modules) {
    if (mod.name.toLowerCase().includes(query)) return true;
    for (const item of (mod.public_items || [])) {
      if (item.name.toLowerCase().includes(query)) return true;
    }
    if (mod.children && searchInModules(mod.children, query)) return true;
  }
  return false;
}

// ---- Toolbar: Layout ----

function setupLayoutButtons() {
  document.querySelectorAll('.layout-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      document.querySelectorAll('.layout-btn').forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      const layoutName = btn.dataset.layout;

      const layoutOpts = {
        dagre: { name: 'dagre', rankDir: 'TB', spacingFactor: 1.2, nodeSep: 60, rankSep: 80 },
        cose: { name: 'cose', animate: true, animationDuration: 500, nodeRepulsion: 8000, idealEdgeLength: 120 },
        concentric: {
          name: 'concentric',
          concentric: node => {
            const maxTier = Math.max(...archData.crates.map(c => c.tier));
            return maxTier - (node.data('tier') || 0);
          },
          levelWidth: () => 1,
          animate: true,
        },
      };

      cy.layout(layoutOpts[layoutName]).run();
    });
  });
}

// ---- Toolbar: Tier Filters ----

function buildTierFilters() {
  const tiers = [...new Set(archData.crates.map(c => c.tier))].sort((a, b) => a - b);
  const container = document.getElementById('tier-filters');

  for (const tier of tiers) {
    const btn = document.createElement('button');
    btn.className = 'tier-toggle';
    btn.style.backgroundColor = TIER_COLORS[tier] + '30';
    btn.style.color = TIER_COLORS[tier];
    btn.textContent = `T${tier}`;
    btn.title = TIER_NAMES[tier] || `Tier ${tier}`;
    btn.dataset.tier = tier;
    btn.addEventListener('click', () => {
      btn.classList.toggle('inactive');
      const hidden = btn.classList.contains('inactive');
      cy.nodes(`[tier = ${tier}]`).forEach(node => {
        if (hidden) {
          node.style('display', 'none');
        } else {
          node.style('display', 'element');
        }
      });
    });
    container.appendChild(btn);
  }
}

// ---- Toolbar: Tools ----

function setupTools() {
  setupPathFinder();
  setupMetrics();
  setupDepExplorer();
}

let pathFinderMode = false;
let pathFinderNodes = [];

function setupPathFinder() {
  const btn = document.getElementById('btn-path-finder');
  btn.addEventListener('click', () => {
    pathFinderMode = !pathFinderMode;
    btn.classList.toggle('active', pathFinderMode);
    pathFinderNodes = [];
    cy.elements().removeClass('highlighted dimmed');

    if (pathFinderMode) {
      cy.off('tap', 'node[type="crate"]', onCrateClick);
      cy.on('tap', 'node[type="crate"]', onPathFinderClick);
    } else {
      cy.off('tap', 'node[type="crate"]', onPathFinderClick);
      cy.on('tap', 'node[type="crate"]', onCrateClick);
    }
  });
}

function onPathFinderClick(e) {
  const node = e.target;
  pathFinderNodes.push(node);

  if (pathFinderNodes.length === 1) {
    node.addClass('highlighted');
    document.getElementById('detail-content').innerHTML =
      '<div class="path-finder-hint">Select a second node to find the shortest path.</div>';
  } else if (pathFinderNodes.length === 2) {
    const path = cy.elements().dijkstra(pathFinderNodes[0], () => 1, true);
    const pathTo = path.pathTo(pathFinderNodes[1]);

    cy.elements().addClass('dimmed');
    pathTo.removeClass('dimmed').addClass('highlighted');

    const names = pathTo.nodes().map(n => n.data('label')).join(' → ');
    document.getElementById('detail-content').innerHTML =
      `<div class="detail-label">Shortest Path</div><div class="detail-stat">${names}</div><div class="detail-stat" style="margin-top:8px">${pathTo.nodes().length} nodes, ${pathTo.edges().length} edges</div>`;

    pathFinderNodes = [];
  }
}

let metricsActive = false;

function setupMetrics() {
  const btn = document.getElementById('btn-metrics');
  btn.addEventListener('click', () => {
    metricsActive = !metricsActive;
    btn.classList.toggle('active', metricsActive);

    if (metricsActive) {
      const maxLines = Math.max(...archData.crates.map(c => c.lines));
      cy.nodes('[type="crate"]').forEach(node => {
        const lines = node.data('lines') || 0;
        const scale = 30 + (lines / maxLines) * 60;
        node.style({ 'width': scale + 'px', 'height': scale + 'px', 'font-size': '9px' });
      });
    } else {
      cy.nodes('[type="crate"]').forEach(node => {
        node.style({ 'width': '', 'height': '30px', 'font-size': '11px' });
      });
    }
  });
}

let depExplorerMode = false;

function setupDepExplorer() {
  const btn = document.getElementById('btn-dep-explorer');
  btn.addEventListener('click', () => {
    depExplorerMode = !depExplorerMode;
    btn.classList.toggle('active', depExplorerMode);
    cy.elements().removeClass('highlighted dimmed');

    if (depExplorerMode) {
      cy.off('tap', 'node[type="crate"]', onCrateClick);
      cy.on('tap', 'node[type="crate"]', onDepExplorerClick);
    } else {
      cy.off('tap', 'node[type="crate"]', onDepExplorerClick);
      cy.on('tap', 'node[type="crate"]', onCrateClick);
    }
  });
}

function onDepExplorerClick(e) {
  const node = e.target;
  cy.elements().addClass('dimmed');

  // Upstream: what this depends on (successors in dep graph = outgoing edges)
  const upstream = node.successors();
  // Downstream: what depends on this (predecessors = incoming edges)
  const downstream = node.predecessors();

  const all = upstream.union(downstream).union(node);
  all.removeClass('dimmed').addClass('highlighted');

  const upNames = upstream.nodes().map(n => n.data('label'));
  const downNames = downstream.nodes().map(n => n.data('label'));

  document.getElementById('detail-content').innerHTML = `
    <div class="detail-label">Dependency Explorer</div>
    <div class="detail-name" style="color:${TIER_COLORS[node.data('tier')]}">${node.data('label')}</div>
    <div class="detail-label">Depends On (${upNames.length})</div>
    <div class="detail-stat">${upNames.join(', ') || 'None'}</div>
    <div class="detail-label">Depended On By (${downNames.length})</div>
    <div class="detail-stat">${downNames.join(', ') || 'None'}</div>
  `;
}

// ---- Footer ----

function updateFooter() {
  const footer = document.getElementById('toolbar-footer');
  const date = new Date(archData.generated_at).toLocaleDateString();
  footer.textContent = `${archData.crates.length} crates · ${archData.edges.length} edges\nGenerated: ${date}`;
}

// ---- Start ----
init();
```

- [ ] **Step 2: Generate test data and verify the viewer**

Run: `cargo run -p flint-arch-analyzer -- tools/arch-viewer/arch-data.json`

Then open `tools/arch-viewer/index.html` in a browser.

Expected:
- Crate graph renders with tier coloring
- Nodes are draggable
- Clicking a crate shows details in right panel
- Clicking a crate in the graph expands to show modules
- Search highlights matching nodes
- Tier filter buttons toggle visibility
- Layout buttons switch between hierarchical/force/concentric

- [ ] **Step 3: Fix any rendering issues**

- [ ] **Step 4: Commit**

```bash
git add tools/arch-viewer/app.js
git commit -m "feat(arch-viewer): implement interactive graph with Cytoscape.js"
```

---

### Task 8: Polish & Final Integration

**Files:**
- Possibly modify: `tools/arch-viewer/app.js`, `tools/arch-viewer/style.css`
- Modify: `.gitignore`

- [ ] **Step 1: Test all interactive features end-to-end**

With `arch-data.json` generated, open the viewer and verify each feature:

1. **Graph renders** — all 23 crates visible, edges connecting them
2. **Click crate** — expands to show modules, detail panel populates
3. **Click module** — shows public items in detail panel
4. **Click item** — shows members/fields
5. **Search** — type "render", matching nodes highlight
6. **Tier filters** — toggle T0, crates disappear/reappear
7. **Layout switch** — try all three layouts
8. **Path Finder** — click two distant nodes, path highlights
9. **Metrics Overlay** — nodes resize by line count
10. **Dep Explorer** — click a node, upstream/downstream highlighted
11. **Edge click** — shows dependency info
12. **Navigate links** — click dep links in panel, graph navigates

- [ ] **Step 2: Fix any issues found**

- [ ] **Step 3: Ensure `arch-data.json` is gitignored**

Verify `.gitignore` contains `arch-data.json`.

- [ ] **Step 4: Final commit**

```bash
git add tools/
git commit -m "feat(arch): complete architecture explorer with analyzer and web viewer"
```
