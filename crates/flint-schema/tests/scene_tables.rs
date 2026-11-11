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

use flint_schema::diagnostic::Severity;
use flint_schema::{validate_table, SchemaRegistry};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every `.toml` directly under `dir` of the repository: `(name, parsed)`.
fn toml_files(dir: &str) -> Vec<(String, toml::Table)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(repo().join(dir)).unwrap().flatten() {
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
    let component_dirs = ["schemas/components", "schemas/examples/components"];
    for (file, doc) in component_dirs.iter().flat_map(|dir| toml_files(dir)) {
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
    for (file, doc) in toml_files("schemas/scene") {
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
    assert_eq!(
        tables.into_iter().collect::<Vec<_>>(),
        ["camera", "environment", "living", "post_process", "wooze"],
        "the engine's scene tables; a new one needs a reader, and a line in the book"
    );
    assert!(components.len() > 40 && !components.contains("wooze"));
}

/// The registry loads them from `scene/`, with no lint, and each declares
/// fields (none is open: an open table checks nothing).
#[test]
fn the_registry_loads_the_engine_tables_clean() {
    let registry = SchemaRegistry::load_from_directory(repo().join("schemas")).unwrap();
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
    assert_eq!(registry.tables().len(), 5);
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
    for dir in ["examples", "tests", "showcase_game", "crates", "tools"] {
        walk(&root.join(dir), &root, &mut out);
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(out.len() > 100, "the repository's scenes were not found");
    out
}

/// What the checks found in the examples the day they were turned on
/// (#686): keys and tables that no code has ever read, so each of these
/// scenes has been playing without them. All are warnings, and none is in a
/// frozen fixture. They are named here so the list can't grow; repairing a
/// scene takes its lines off (the test fails on a line that no longer
/// applies).
///
/// - `[camera] orthographic = true` in four 2D examples: the key is
///   `projection = "orthographic"`, so they have been drawn in perspective.
/// - `[camera] distance / pitch / yaw` in rolling_meadow: those are
///   `flint render` flags, not scene keys.
/// - `[lights.*]` in campfire: lights are entities with a `light` component,
///   so the scene is lit by the default lights.
const NOTHING_READS: &[&str] = &[
    "examples/campfire.scene.toml: [lights] is no scene table",
    "examples/collision_2d_test/collision_2d_test.scene.toml: not declared: \
     warning[unknown-field] [camera] orthographic: `camera` has no field `orthographic`",
    "examples/rolling_meadow.scene.toml: not declared: warning[unknown-field] [camera] \
     distance: `camera` has no field `distance`",
    "examples/rolling_meadow.scene.toml: not declared: warning[unknown-field] [camera] pitch: \
     `camera` has no field `pitch`",
    "examples/rolling_meadow.scene.toml: not declared: warning[unknown-field] [camera] yaw: \
     `camera` has no field `yaw`",
    "examples/scrolling_test/scrolling_test.scene.toml: not declared: warning[unknown-field] \
     [camera] orthographic: `camera` has no field `orthographic`",
    "examples/ui_test/ui_fonts.scene.toml: not declared: warning[unknown-field] [camera] \
     orthographic: `camera` has no field `orthographic`",
    "examples/ui_test/ui_test.scene.toml: not declared: warning[unknown-field] [camera] \
     orthographic: `camera` has no field `orthographic`",
];

/// No scene in the repository (the examples, the frozen fixtures the goldens
/// load, the showcase game) writes a top-level table the schemas don't
/// declare, or a field in one they reject or don't declare, but for
/// [`NOTHING_READS`]. The fixtures are frozen, so this is what says the new
/// checks changed nothing for them; and nothing anywhere is an error.
#[test]
fn the_repository_scenes_fit_the_table_schemas() {
    let registry = SchemaRegistry::load_from_directory(repo().join("schemas")).unwrap();
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
    assert!(checked > 100, "only {checked} scene tables were checked");
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
