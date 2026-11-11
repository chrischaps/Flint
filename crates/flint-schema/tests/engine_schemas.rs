//! The engine's schema set against the engine's code (ADR 0115 decision 3, #681).
//!
//! Three things have to agree: the constants in `flint_core::components`,
//! the files in `schemas/components/`, and the code that reads a component.
//! When they don't, a scene's mistake passes in silence: `flint schema light`
//! printed nothing for years, so a `directional_light` table validated and
//! was lit by the defaults.
//!
//! - every constant has a schema ([`NO_SCHEMA`] names the exceptions)
//! - every constant is used by code outside tests
//! - every schema is read by code outside tests ([`NO_READER`] names the
//!   exceptions)
//! - the engine's set holds none of the examples' room-builder vocabulary,
//!   which lives in the `schemas/examples` layer
//! - the scenes in this repository write nothing the schemas in
//!   [`CHECKED_AGAINST_SCENES`] don't describe, and no example
//!   carries a light table that nothing reads
//!
//! "Read by code" is a search of `crates/*/src` with the test modules cut
//! off: the constant used as `comp::NAME` / `components::NAME`, or the name
//! as a string literal. It can't tell a reader from a writer, so it catches
//! a schema nobody mentions, not one nobody honours.

use flint_schema::{validate_component_data, SchemaRegistry};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Constants with no schema file, and why. An entry that gains a schema
/// fails `exemptions_are_still_needed`, so the list can't go stale.
const NO_SCHEMA: &[(&str, &str)] = &[
    (
        "spline_data",
        "written by the engine at load (the samples of a `spline`); no scene authors it",
    ),
    (
        "spline_chunk",
        "written by the engine on the child entities a chunked `spline_mesh` spawns",
    ),
];

/// Schemas no code outside tests mentions, and why they stay.
/// (`audio_listener` was the last: nothing read it, the listener is the
/// camera, and #792 deleted it.)
const NO_READER: &[(&str, &str)] = &[];

/// The examples' room-builder vocabulary (#681): in `schemas/examples`,
/// never in the engine's set.
const EXAMPLE_ARCHETYPES: &[&str] = &[
    "ceiling",
    "door",
    "floor",
    "furniture",
    "pillar",
    "room",
    "wall",
];
const EXAMPLE_COMPONENTS: &[&str] = &["door"];

/// The components written first (#681) and the ten whose fields once sat
/// outside `fields` (#792), checked field by field against every scene in
/// the repository.
const CHECKED_AGAINST_SCENES: &[&str] = &[
    "model",
    "light",
    "spline",
    "spline_mesh",
    "collider",
    "rigidbody",
    "character_controller",
    "audio_source",
    "audio_trigger",
    "joint",
    "parallax",
    "screen_anchor",
    "ui_fill",
    "ui_text",
];

/// Tables that look like a light and that nothing reads.
const STALE_LIGHT_TABLES: &[&str] = &["directional_light", "point_light", "spot_light"];

/// Where a stale light table may stay: the frozen scenes the goldens load
/// (`tests/fixtures/scenes/README.md`). They are lit by the defaults, their
/// pixels are pinned, and making the table a real light would move them.
const FROZEN: &[&str] = &["tests/fixtures/", "crates/flint-cli/tests/golden/fixtures/"];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `(IDENT, "value")` for every `pub const IDENT: &str = "value";` in
/// `flint_core::components`.
fn constants() -> Vec<(String, String)> {
    let text = read(&repo().join("crates/flint-core/src/components.rs"));
    let found: Vec<(String, String)> = text
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("pub const ")?;
            let (ident, rest) = rest.split_once(':')?;
            let value = rest.split('"').nth(1)?;
            Some((ident.trim().to_string(), value.to_string()))
        })
        .collect();
    assert!(
        found.len() > 20,
        "components.rs no longer parses: {} constants found",
        found.len()
    );
    found
}

/// Component name -> the file that declares it, for one schema directory.
fn schema_names(dir: &str, kind: &str, header: &str) -> BTreeMap<String, String> {
    let folder = repo().join(dir).join(kind);
    let mut names = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(&folder) else {
        return names;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "toml") {
            let doc: toml::Table = read(&path)
                .parse()
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let file = path.file_name().unwrap().to_string_lossy().into_owned();
            for name in doc
                .get(header)
                .and_then(|t| t.as_table())
                .into_iter()
                .flat_map(|t| t.keys())
            {
                names.insert(name.clone(), file.clone());
            }
        }
    }
    names
}

fn engine_components() -> BTreeMap<String, String> {
    let names = schema_names("schemas", "components", "component");
    assert!(names.len() > 30, "schemas/components not found");
    names
}

/// Every `.rs` under `crates/*/src`, with what only tests compile cut off:
/// `*_tests.rs` and `tests.rs` files, `tests/` folders, and the inline
/// `#[cfg(test)] mod tests {` that ends a file.
fn engine_source() -> Vec<(String, String)> {
    fn walk(dir: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if name != "tests" {
                    walk(&path, out);
                }
            } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") && name != "tests.rs" {
                let text = read(&path).replace("\r\n", "\n");
                let code = match text.find("#[cfg(test)]\nmod tests {") {
                    Some(at) => text[..at].to_string(),
                    None => text,
                };
                out.push((path.to_string_lossy().replace('\\', "/"), code));
            }
        }
    }
    let mut out = Vec::new();
    for krate in std::fs::read_dir(repo().join("crates")).unwrap().flatten() {
        walk(&krate.path().join("src"), &mut out);
    }
    assert!(out.len() > 100, "crates/*/src not found");
    out
}

/// Whether `code` holds `needle` as a whole identifier path (`comp::LIGHT`
/// is not found in `comp::LIGHTS`).
fn has_path(code: &str, needle: &str) -> bool {
    code.match_indices(needle).any(|(at, _)| {
        !code[at + needle.len()..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
    })
}

/// Whether code outside tests uses the constant `ident`.
fn constant_is_used(source: &[(String, String)], ident: &str) -> bool {
    source
        .iter()
        .filter(|(path, _)| !path.ends_with("flint-core/src/components.rs"))
        .any(|(_, code)| {
            has_path(code, &format!("comp::{ident}"))
                || has_path(code, &format!("components::{ident}"))
        })
}

/// Whether code outside tests writes `name` as a string literal.
fn literal_is_used(source: &[(String, String)], name: &str) -> bool {
    let literal = format!("\"{name}\"");
    source
        .iter()
        .filter(|(path, _)| !path.ends_with("flint-core/src/components.rs"))
        .any(|(_, code)| code.contains(&literal))
}

fn exempt(list: &[(&str, &str)], name: &str) -> bool {
    list.iter().any(|(n, _)| *n == name)
}

#[test]
fn every_component_constant_has_a_schema() {
    let schemas = engine_components();
    let missing: Vec<String> = constants()
        .into_iter()
        .filter(|(_, value)| !schemas.contains_key(value) && !exempt(NO_SCHEMA, value))
        .map(|(ident, value)| format!("comp::{ident} (\"{value}\")"))
        .collect();
    assert!(
        missing.is_empty(),
        "no schemas/components file declares these: write `[component.<name>]` with the fields \
         its reader takes, or name it in NO_SCHEMA with the reason:\n  {}",
        missing.join("\n  ")
    );
}

#[test]
fn every_component_constant_is_used() {
    let source = engine_source();
    let unused: Vec<String> = constants()
        .into_iter()
        .filter(|(ident, _)| !constant_is_used(&source, ident))
        .map(|(ident, _)| format!("comp::{ident}"))
        .collect();
    assert!(
        unused.is_empty(),
        "no code outside tests uses these constants: delete them (and the schema, if nothing \
         else reads the component):\n  {}",
        unused.join("\n  ")
    );
}

#[test]
fn every_engine_schema_has_a_reader() {
    let source = engine_source();
    let constants = constants();
    let unread: Vec<String> = engine_components()
        .into_iter()
        .filter(|(name, _)| {
            let by_constant = constants
                .iter()
                .any(|(ident, value)| value == name && constant_is_used(&source, ident));
            !by_constant && !literal_is_used(&source, name) && !exempt(NO_READER, name)
        })
        .map(|(name, file)| format!("{name} (schemas/components/{file})"))
        .collect();
    assert!(
        unread.is_empty(),
        "no code outside tests mentions these components: a schema nobody reads promises \
         fields that do nothing. Delete it, move it to the layer of whoever reads it, or name \
         it in NO_READER with the reason:\n  {}",
        unread.join("\n  ")
    );
}

#[test]
fn exemptions_are_still_needed() {
    let source = engine_source();
    let schemas = engine_components();
    let constants = constants();
    let mut stale = Vec::new();
    for (name, _) in NO_SCHEMA {
        if schemas.contains_key(*name) {
            stale.push(format!("NO_SCHEMA: {name} has a schema now"));
        }
        if !constants.iter().any(|(_, value)| value == name) {
            stale.push(format!("NO_SCHEMA: {name} is no constant"));
        }
    }
    for (name, _) in NO_READER {
        if !schemas.contains_key(*name) {
            stale.push(format!("NO_READER: {name} has no schema"));
        } else if literal_is_used(&source, name)
            || constants
                .iter()
                .any(|(ident, value)| value == name && constant_is_used(&source, ident))
        {
            stale.push(format!("NO_READER: {name} is read now"));
        }
    }
    assert!(stale.is_empty(), "drop these:\n  {}", stale.join("\n  "));
}

#[test]
fn the_engine_set_has_no_room_builder_vocabulary() {
    let engine = SchemaRegistry::load_from_directory(repo().join("schemas")).unwrap();
    for name in EXAMPLE_ARCHETYPES {
        assert!(
            engine.get_archetype(name).is_none(),
            "archetype `{name}` is back in schemas/archetypes: it belongs to schemas/examples"
        );
    }
    for name in EXAMPLE_COMPONENTS {
        assert!(
            engine.get_component(name).is_none(),
            "component `{name}` is back in schemas/components: it belongs to schemas/examples"
        );
    }
    // No engine archetype may lean on the layer either
    for (name, _) in schema_names("schemas", "archetypes", "archetype") {
        for component in &engine.get_archetype(&name).unwrap().components {
            assert!(
                engine.get_component(component).is_some(),
                "engine archetype `{name}` lists `{component}`, which the engine's set lacks"
            );
        }
    }

    // The layer over the engine's set is what the examples load
    // (flint.toml at the repository root)
    let layered = SchemaRegistry::load_from_directories(&[
        repo().join("schemas"),
        repo().join("schemas/examples"),
    ])
    .unwrap();
    for name in EXAMPLE_ARCHETYPES {
        let archetype = layered
            .get_archetype(name)
            .unwrap_or_else(|| panic!("schemas/examples/archetypes lacks `{name}`"));
        for component in &archetype.components {
            assert!(
                layered.get_component(component).is_some(),
                "example archetype `{name}` lists `{component}`, which no layer declares"
            );
        }
    }
    let manifest: toml::Table = read(&repo().join("flint.toml")).parse().unwrap();
    let listed: Vec<&str> = manifest["project"]["schemas"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(
        listed,
        ["schemas", "schemas/examples"],
        "the repository's flint.toml is how the examples get their archetypes"
    );
}

/// Every entity table in the repository's scenes and prefabs:
/// `(file, entity, table)`.
fn scene_entities() -> Vec<(String, String, toml::Table)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String, toml::Table)>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !matches!(name.as_str(), "target" | "dist" | "node_modules" | ".git") {
                    walk(&path, root, out);
                }
                continue;
            }
            if !name.ends_with(".toml") {
                continue;
            }
            let Ok(doc) = read(&path).parse::<toml::Table>() else {
                continue;
            };
            let file = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let groups = [
                doc.get("entities"),
                doc.get("prefab").and_then(|p| p.get("entities")),
            ];
            for entities in groups.into_iter().flatten().filter_map(|e| e.as_table()) {
                for (entity, table) in entities {
                    if let Some(table) = table.as_table() {
                        out.push((file.clone(), entity.clone(), table.clone()));
                    }
                }
            }
        }
    }
    let root = repo();
    let mut out = Vec::new();
    for dir in ["examples", "tests", "showcase_game", "crates"] {
        walk(&root.join(dir), &root, &mut out);
    }
    assert!(out.len() > 500, "the repository's scenes were not found");
    out
}

#[test]
fn the_repository_scenes_fit_the_new_schemas() {
    let registry = SchemaRegistry::load_from_directory(repo().join("schemas")).unwrap();
    let mut problems = BTreeSet::new();
    let mut checked = 0;
    for (file, entity, table) in scene_entities() {
        for name in CHECKED_AGAINST_SCENES {
            let Some(data) = table.get(*name) else {
                continue;
            };
            let schema = registry
                .get_component(name)
                .unwrap_or_else(|| panic!("no schema for `{name}`"));
            checked += 1;
            if let Err(e) = validate_component_data(schema, data) {
                problems.insert(format!("{file}: [entities.{entity}.{name}]: {e}"));
            }
            for field in data.as_table().into_iter().flat_map(|t| t.keys()) {
                if !schema.fields.contains_key(field) {
                    problems.insert(format!(
                        "{file}: [entities.{entity}.{name}] writes `{field}`, which the schema \
                         doesn't declare"
                    ));
                }
            }
        }
    }
    assert!(
        checked > 300,
        "only {checked} component tables were checked"
    );
    assert!(
        problems.is_empty(),
        "a scene writes what a schema rejects: if the reader takes it, the schema is wrong; \
         if not, the scene is:\n  {}",
        problems.into_iter().collect::<Vec<_>>().join("\n  ")
    );
}

#[test]
fn no_example_carries_a_light_table_nothing_reads() {
    let stale: BTreeSet<String> = scene_entities()
        .into_iter()
        .filter(|(file, _, _)| !FROZEN.iter().any(|frozen| file.starts_with(frozen)))
        .flat_map(|(file, entity, table)| {
            STALE_LIGHT_TABLES
                .iter()
                .filter(|name| table.contains_key(**name))
                .map(|name| format!("{file}: [entities.{entity}.{name}]"))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        stale.is_empty(),
        "nothing reads these tables, so each scene is lit by the built-in defaults. A light is \
         `[entities.<name>.light]` with `type = \"directional\"` and a `direction` that points \
         TOWARD the light (`flint schema light`):\n  {}",
        stale.into_iter().collect::<Vec<_>>().join("\n  ")
    );
}

#[test]
fn the_four_schemas_say_what_their_readers_take() {
    let registry = SchemaRegistry::load_from_directory(repo().join("schemas")).unwrap();
    let fields = |name: &str| -> BTreeSet<String> {
        registry
            .get_component(name)
            .unwrap_or_else(|| panic!("no schema for `{name}`"))
            .fields
            .keys()
            .cloned()
            .collect()
    };
    let set =
        |names: &[&str]| -> BTreeSet<String> { names.iter().map(|n| n.to_string()).collect() };

    // What `SceneRenderer::extract_lights_from_world` reads
    assert_eq!(
        fields("light"),
        set(&[
            "type",
            "color",
            "intensity",
            "direction",
            "angular_size",
            "range",
            "radius",
            "source_radius",
            "inner_angle",
            "outer_angle",
            "volumetric_intensity",
            "volumetric_color",
        ])
    );
    // `model_loader` and `is_runtime_model`
    assert_eq!(fields("model"), set(&["asset", "runtime"]));
    // `spline_gen::load_splines`
    assert_eq!(fields("spline"), set(&["file"]));
    // `spline_gen::parse_spline_mesh_component`
    assert_eq!(
        fields("spline_mesh"),
        set(&[
            "spline",
            "width",
            "height",
            "offset",
            "color",
            "stripe_color",
            "stripe_width",
            "friction",
            "restitution",
            "metallic",
            "roughness",
            "chunk_size",
            "texture",
            "normal_texture",
            "texture_length",
            "collider",
        ])
    );

    // A default is written into the component at load (`populate_world`),
    // so only a field whose reader falls back to that very value may carry
    // one: `range` with a default would beat an authored `radius`, and
    // `volumetric_color` would stop following `color`.
    let with_default = |name: &str| -> BTreeSet<String> {
        registry
            .get_component(name)
            .unwrap()
            .fields
            .iter()
            .filter(|(_, f)| f.default.is_some())
            .map(|(k, _)| k.clone())
            .collect()
    };
    assert_eq!(with_default("light"), set(&["type", "color", "intensity"]));
    for name in ["model", "spline", "spline_mesh"] {
        assert_eq!(with_default(name), set(&[]), "{name}");
    }
}
