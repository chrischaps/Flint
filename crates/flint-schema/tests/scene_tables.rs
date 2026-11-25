//! The engine's scene-table schemas (`schemas/scene/*.toml`, #686) against
//! the repository.
//!
//! A scene-level table (`[camera]`, `[post_process]`, `[wooze]`) is declared
//! as a `[table.<name>]`, beside the components and not among them:
//!
//! - `schemas/components/` holds only components, `schemas/scene/` only
//!   tables, and no name is both
//! - every scene in the repository writes only tables the schemas declare,
//!   and nothing in them the schemas reject, so turning the checks on
//!   rejected no scene that loaded before
//!
//! `flint-scene`'s own tests hold the schemas to the structs that read them.
#![allow(clippy::disallowed_methods)] // a test: reads its files off the disk, not through the vfs

use flint_schema::diagnostic::Severity;
use flint_schema::{validate_table, SchemaRegistry};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The engine's schema directories from the repository root: `schemas`,
/// then each package's (#1332), as the resolver brings them.
fn engine_dirs() -> Vec<String> {
    let mut dirs = vec!["schemas".to_string()];
    for (package, _) in flint_core::project::package_schema_dirs(&repo().join("schemas")) {
        dirs.push(format!("packages/{package}/schemas"));
    }
    dirs
}

/// The engine's set: `schemas` and its packages'.
fn engine_registry() -> SchemaRegistry {
    let dirs: Vec<PathBuf> = engine_dirs().iter().map(|d| repo().join(d)).collect();
    SchemaRegistry::load_from_directories(&dirs).unwrap()
}

/// Every `.toml` file of `kind` (`components`, `scene`) in the engine's
/// directories.
fn engine_files(kind: &str) -> Vec<(String, toml::Table)> {
    engine_dirs()
        .iter()
        .flat_map(|dir| toml_files(&format!("{dir}/{kind}")))
        .collect()
}

/// Every `.toml` directly under `dir` of the repository: `(name, parsed)`.
/// A directory that isn't there has none.
fn toml_files(dir: &str) -> Vec<(String, toml::Table)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(repo().join(dir)) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "toml") {
            let text = std::fs::read_to_string(&path).unwrap();
            let doc: toml::Table = text
                .parse()
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            out.push((
                format!("{dir}/{}", entry.file_name().to_string_lossy()),
                doc,
            ));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The ticket's third box. `wooze.toml` sat in `components/` declaring a
/// `[component.wooze]` it described as "not a component", to borrow the
/// component checks; nothing like it may come back.
#[test]
fn schemas_components_holds_only_components() {
    let mut wrong = Vec::new();
    let mut components = BTreeSet::new();
    let mut files = engine_files("components");
    files.extend(toml_files("schemas/examples/components"));
    for (file, doc) in files {
        for (key, value) in &doc {
            if key != "component" {
                wrong.push(format!("{file}: top-level `{key}` in a component file"));
                continue;
            }
            for (name, def) in value.as_table().into_iter().flatten() {
                components.insert(name.clone());
                let description = def.get("description").and_then(|d| d.as_str());
                if description.is_some_and(|d| d.to_lowercase().contains("not a component")) {
                    wrong.push(format!(
                        "{file}: `{name}` says it is not a component: declare it as a \
                         [table.{name}] under schemas/scene/"
                    ));
                }
            }
        }
    }
    let mut tables = BTreeSet::new();
    for (file, doc) in engine_files("scene") {
        for (key, value) in &doc {
            if key != "table" {
                wrong.push(format!("{file}: top-level `{key}` in a scene-table file"));
                continue;
            }
            tables.extend(
                value
                    .as_table()
                    .into_iter()
                    .flatten()
                    .map(|(n, _)| n.clone()),
            );
        }
    }
    for both in components.intersection(&tables) {
        wrong.push(format!("`{both}` is both a component and a scene table"));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(tables.contains("wooze") && tables.contains("post_process"));
    assert_eq!(
        tables,
        book_tables(),
        "the engine's scene tables against the ones the book's Scene-level tables \
         section names (docs/book/src/concepts/schemas.md): a new table needs its line there"
    );
    assert!(components.len() > 40 && !components.contains("wooze"));
}

/// The tables the book names in its one sentence listing them: every
/// `` `[name]` `` on the line that opens "A scene file has tables". Was a
/// count in this file, which #880's `panels` broke while the book's list
/// went on naming five (#928).
fn book_tables() -> BTreeSet<String> {
    let page = repo().join("docs/book/src/concepts/schemas.md");
    let text = std::fs::read_to_string(&page).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("A scene file has tables that belong to no entity:"))
        .unwrap_or_else(|| panic!("{}: no sentence listing the scene tables", page.display()));
    // The sentence alone: the next says each is a `[table.<name>]`
    let sentence = line.split_once(". ").map_or(line, |(first, _)| first);
    sentence
        .split("`[")
        .skip(1)
        .filter_map(|rest| rest.split_once("]`").map(|(name, _)| name.to_string()))
        .collect()
}

/// The registry loads them from `scene/`, with no lint, and each declares
/// fields (none is open: an open table checks nothing).
#[test]
fn the_registry_loads_the_engine_tables_clean() {
    let registry = engine_registry();
    for table in registry.tables() {
        assert!(!table.is_open(), "`{}` declares no fields", table.name);
        assert!(table.description.is_some(), "`{}`", table.name);
        for (name, field) in &table.fields {
            assert!(
                field.description.is_some(),
                "`{}.{name}` has no description",
                table.name
            );
        }
    }
    // Every table a file under `scene/` declares, and nothing else (#928:
    // was a count, six, which the next table would have broken)
    let loaded: BTreeSet<String> = registry.tables().iter().map(|t| t.name.clone()).collect();
    let declared: BTreeSet<String> = engine_files("scene")
        .iter()
        .flat_map(|(_, doc)| doc.get("table").and_then(|t| t.as_table()))
        .flat_map(|t| t.keys().cloned())
        .collect();
    assert_eq!(loaded, declared);
    let about_tables: Vec<String> = registry
        .lints()
        .iter()
        .filter(|d| d.table.is_some() || d.component.is_none())
        .map(|d| d.one_line())
        .collect();
    assert!(about_tables.is_empty(), "{}", about_tables.join("\n"));
    assert!(registry.get_component("wooze").is_none());
    assert!(registry.get_component("wooze_zone").is_none());
}

/// Every scene file of the repository: `(path from the root, parsed)`.
fn scenes() -> Vec<(String, toml::Table)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, toml::Table)>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !matches!(name.as_str(), "target" | "dist" | "node_modules" | ".git") {
                    walk(&path, root, out);
                }
                continue;
            }
            if !name.ends_with(".scene.toml") && !name.ends_with(".chunk.toml") {
                continue;
            }
            let Ok(doc) = std::fs::read_to_string(&path)
                .unwrap_or_default()
                .parse::<toml::Table>()
            else {
                continue;
            };
            let file = path.strip_prefix(root).unwrap().to_string_lossy();
            out.push((file.replace('\\', "/"), doc));
        }
    }
    let root = repo();
    let mut out = Vec::new();
    // ...and each game kept in the checkout: a top-level directory with a
    // `flint.toml`, found by it so the engine names no game (#760)
    let games = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|d| d.join("flint.toml").is_file());
    for dir in ["tests", "crates", "tools"]
        .map(|d| root.join(d))
        .into_iter()
        .chain(games)
    {
        walk(&dir, &root, &mut out);
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(
        out.len() > 30,
        "the repository's scenes were not found: {}",
        out.len()
    );
    out
}

/// Keys and tables in the repository's scenes that no code reads, each
/// pinned here so the list can't grow; repairing a scene takes its line off
/// (the test fails on a line that no longer applies).
///
/// Empty since #807. The first run (#686) found six examples: four 2D ones
/// wrote `[camera] orthographic = true` (the key is `projection =
/// "orthographic"`), so they were drawn in perspective; rolling_meadow wrote
/// `flint render`'s `distance / pitch / yaw` flags as `[camera]` keys; and
/// campfire's `[lights.*]` table lit nothing (a light is an entity with a
/// `light` component). Each scene now says what it meant.
const NOTHING_READS: &[&str] = &[];

/// No scene in the repository (the examples, the frozen fixtures the goldens
/// load, the showcase game) writes a top-level table the schemas don't
/// declare, or a field in one they reject or don't declare, but for
/// [`NOTHING_READS`]. The fixtures are frozen, so this is what says the new
/// checks changed nothing for them; and nothing anywhere is an error.
#[test]
fn the_repository_scenes_fit_the_table_schemas() {
    let registry = engine_registry();
    let mut problems = Vec::new();
    let mut checked = 0;
    for (file, doc) in scenes() {
        for (name, data) in &doc {
            if matches!(name.as_str(), "scene" | "prefabs" | "entities") {
                continue;
            }
            let Some(schema) = registry.get_table(name) else {
                problems.push(format!("{file}: [{name}] is no scene table"));
                continue;
            };
            checked += 1;
            for d in validate_table(schema, data) {
                let how = match d.severity {
                    Severity::Error => "rejected",
                    Severity::Warning => "not declared",
                };
                problems.push(format!("{file}: {how}: {d}"));
            }
        }
    }
    assert!(checked > 60, "only {checked} scene tables were checked");
    assert!(
        !problems.iter().any(|p| p.contains(": rejected: ")),
        "a table schema rejects a scene that loaded before it:\n  {}",
        problems.join("\n  ")
    );
    let new: Vec<&String> = problems
        .iter()
        .filter(|p| !NOTHING_READS.contains(&p.as_str()))
        .collect();
    assert!(
        new.is_empty(),
        "a scene writes what a table schema doesn't take: if the reader takes it, the schema \
         is wrong; if not, the scene is:\n  {new:#?}"
    );
    let repaired: Vec<&&str> = NOTHING_READS
        .iter()
        .filter(|known| !problems.iter().any(|p| p == **known))
        .collect();
    assert!(
        repaired.is_empty(),
        "these are repaired: take them off NOTHING_READS: {repaired:#?}"
    );
    assert!(
        !problems.iter().any(|p| p.starts_with("tests/")),
        "a frozen fixture is flagged"
    );
}
