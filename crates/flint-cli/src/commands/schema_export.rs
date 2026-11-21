//! The loaded schemas as data (#684): `flint schema --format json` (the
//! registry as it is: every component, field and archetype) and
//! `--format json-schema` (a JSON Schema for a `.scene.toml`, which taplo and
//! Even Better TOML read for completion and inline errors).
//!
//! Both are built from a [`SchemaRegistry`], so they describe exactly the
//! schema directories the command resolved: the engine's set, and a game's
//! layer over it.

use flint_schema::{
    ArchetypeOrigin, ArchetypeSchema, ComponentSchema, FieldSchema, FieldType, PanelRole,
    PanelSpec, SchemaOrigin, SchemaRegistry, OLD_TYPE_NAMES, TYPE_NAMES,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::Path;

/// A schema file as `flint schema` names it: from the working directory
/// where it is under it, so the output doesn't carry one machine's folders.
pub fn shown_file(path: &Path) -> String {
    let cwd = std::env::current_dir().ok();
    let from_here = cwd.as_deref().and_then(|cwd| path.strip_prefix(cwd).ok());
    flint_core::project::shown(from_here.unwrap_or(path))
}

fn to_json(value: &toml::Value) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn sorted<V>(map: &HashMap<String, V>) -> Vec<(&String, &V)> {
    let mut entries: Vec<(&String, &V)> = map.iter().collect();
    entries.sort_unstable_by_key(|(name, _)| *name);
    entries
}

// --- `--format json`: the registry as it is --------------------------------

/// A field type: `{ "type": "f32" }`, with `values` for an enum, `element`
/// for an array, `kinds` (and `unless`) for an asset, `fields` for a table
/// that declares them.
fn type_json(field_type: &FieldType) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("type".into(), json!(field_type.type_name()));
    match field_type {
        FieldType::Enum { values } => {
            out.insert("values".into(), json!(values));
        }
        FieldType::Array { element } => {
            out.insert("element".into(), Value::Object(type_json(element)));
        }
        FieldType::Asset {
            kinds,
            unless,
            builtins,
        } => {
            let labels: Vec<&str> = kinds.iter().map(|kind| kind.label()).collect();
            out.insert("kinds".into(), json!(labels));
            if let Some(unless) = unless {
                out.insert("unless".into(), json!(unless));
            }
            if !builtins.is_empty() {
                out.insert("builtins".into(), json!(builtins));
            }
        }
        FieldType::Fields { fields } => {
            out.insert("fields".into(), fields_json(fields));
        }
        _ => {}
    }
    out
}

fn field_json(field: &FieldSchema) -> Value {
    let mut out = type_json(&field.field_type);
    out.insert("required".into(), json!(field.required));
    if let Some(default) = &field.default {
        out.insert("default".into(), to_json(default));
    }
    if let Some(description) = &field.description {
        out.insert("description".into(), json!(description));
    }
    if let Some(min) = field.min {
        out.insert("min".into(), json!(min));
    }
    if let Some(max) = field.max {
        out.insert("max".into(), json!(max));
    }
    Value::Object(out)
}

fn fields_json(fields: &HashMap<String, FieldSchema>) -> Value {
    Value::Object(
        sorted(fields)
            .into_iter()
            .map(|(name, field)| (name.clone(), field_json(field)))
            .collect(),
    )
}

/// One component: its description, whether it is open (no fields: known,
/// and nothing in it checked), its fields by name, and where it and each
/// field was declared (`declared_in`, `extended_by`, each field's `file`;
/// #794), when it was read from a file.
pub fn component_json(component: &ComponentSchema, origin: Option<&SchemaOrigin>) -> Value {
    fielded_json("component", component, origin)
}

/// One scene-level table (#686): what a component has, under
/// `"kind": "table"`.
pub fn table_json(table: &ComponentSchema, origin: Option<&SchemaOrigin>) -> Value {
    fielded_json("table", table, origin)
}

fn fielded_json(kind: &str, schema: &ComponentSchema, origin: Option<&SchemaOrigin>) -> Value {
    let mut fields = fields_json(&schema.fields);
    let mut out = json!({
        "kind": kind,
        "name": schema.name,
        "description": schema.description,
        "open": schema.is_open(),
    });
    if let Some(origin) = origin {
        for (name, field) in fields.as_object_mut().expect("fields are an object") {
            if let Some(file) = origin.field_file(name) {
                field["file"] = json!(shown_file(file));
            }
        }
        out["declared_in"] = json!(origin.declared_in.as_deref().map(shown_file));
        out["extended_by"] = json!(origin
            .extended_by
            .iter()
            .map(|file| shown_file(file))
            .collect::<Vec<_>>());
    }
    out["fields"] = fields;
    if let Some(panel) = &schema.panel {
        out["panel"] = panel_json(panel);
    }
    out
}

/// A component's F3 panel (#744, #1020): its `title` and its `rows` in the
/// panel's order, each with `field`, `role` and `label`, and the keys that
/// say more than the defaults: `labels`, `press` (a trigger's), `step`,
/// `min`, `max`, `slider` and `presets` (a tune row's editor), `format`,
/// `scale`, `unset`, `from` and `commit`.
pub fn panel_json(panel: &PanelSpec) -> Value {
    let rows: Vec<Value> = panel
        .rows
        .iter()
        .map(|row| {
            let mut out = Map::new();
            out.insert("field".into(), json!(row.field));
            out.insert("role".into(), json!(row.role.name()));
            out.insert("label".into(), json!(row.label));
            if !row.labels.is_empty() {
                out.insert("labels".into(), json!(row.labels));
            }
            let tune = row.role == PanelRole::Tune;
            if row.role == PanelRole::Trigger {
                out.insert("press".into(), to_json(&row.press));
            }
            if tune {
                for (key, value) in [("step", row.step), ("min", row.min), ("max", row.max)] {
                    if let Some(value) = value {
                        out.insert(key.into(), json!(value));
                    }
                }
                if row.slider {
                    out.insert("slider".into(), json!(true));
                }
                if !row.presets.is_empty() {
                    let presets: Vec<Value> = row
                        .presets
                        .iter()
                        .map(|(name, n)| json!([name, n]))
                        .collect();
                    out.insert("presets".into(), json!(presets));
                }
            }
            if let Some(format) = &row.format {
                out.insert("format".into(), json!(format));
            }
            if row.scale != 1.0 {
                out.insert("scale".into(), json!(row.scale));
            }
            if let Some(unset) = row.unset {
                out.insert("unset".into(), json!(unset));
            }
            if let Some(from) = &row.from {
                out.insert("from".into(), json!(from));
            }
            if tune && !row.commit {
                out.insert("commit".into(), json!(false));
            }
            Value::Object(out)
        })
        .collect();
    json!({ "title": panel.title, "rows": rows })
}

/// One archetype: its components, and the defaults it gives them. Read
/// from files (`origin`, #1054), it also names `declared_in`, `extended_by`,
/// the file that listed each component (`component_files`) and the one
/// that gave each default (`default_files`, by component, then field).
pub fn archetype_json(archetype: &ArchetypeSchema, origin: Option<&ArchetypeOrigin>) -> Value {
    let defaults: Map<String, Value> = sorted(&archetype.defaults)
        .into_iter()
        .map(|(component, table)| (component.clone(), to_json(table)))
        .collect();
    let mut out = json!({
        "kind": "archetype",
        "name": archetype.name,
        "description": archetype.description,
        "components": archetype.components,
        "defaults": defaults,
    });
    // Its placeholder box (#751), only when the archetype gives one
    if let Some(visual) = &archetype.visual {
        // Each channel as the file spells it (0.76, not 0.7599999904632568)
        let color = visual
            .color
            .map(|c| c.to_string().parse::<f64>().unwrap_or(c.into()));
        out["visual"] = json!({ "color": color, "wireframe": visual.wireframe });
    }
    if let Some(origin) = origin {
        out["declared_in"] = json!(origin.declared_in.as_deref().map(shown_file));
        out["extended_by"] = json!(origin
            .extended_by
            .iter()
            .map(|file| shown_file(file))
            .collect::<Vec<_>>());
        let components: Map<String, Value> = archetype
            .components
            .iter()
            .filter_map(|c| Some((c.clone(), json!(shown_file(origin.component_file(c)?)))))
            .collect();
        out["component_files"] = Value::Object(components);
        let defaults: Map<String, Value> = sorted(&archetype.defaults)
            .into_iter()
            .map(|(component, table)| {
                let fields: Map<String, Value> = table
                    .as_table()
                    .into_iter()
                    .flatten()
                    .filter_map(|(field, _)| {
                        let file = origin.default_file(component, field)?;
                        Some((field.clone(), json!(shown_file(file))))
                    })
                    .collect();
                (component.clone(), Value::Object(fields))
            })
            .collect();
        out["default_files"] = Value::Object(defaults);
    }
    out
}

/// Everything loaded: the directories, the type names (and the old spellings
/// that still load), every component, every scene table and every
/// archetype.
pub fn registry_json(registry: &SchemaRegistry, dirs: &[String]) -> Value {
    let components: Map<String, Value> = registry
        .components()
        .into_iter()
        .map(|c| {
            let origin = registry.component_origin(&c.name);
            (c.name.clone(), component_json(c, origin))
        })
        .collect();
    let tables: Map<String, Value> = registry
        .tables()
        .into_iter()
        .map(|t| {
            (
                t.name.clone(),
                table_json(t, registry.table_origin(&t.name)),
            )
        })
        .collect();
    let archetypes: Map<String, Value> = registry
        .archetypes()
        .into_iter()
        .map(|a| {
            (
                a.name.clone(),
                archetype_json(a, registry.archetype_origin(&a.name)),
            )
        })
        .collect();
    let types: Vec<Value> = TYPE_NAMES
        .iter()
        .map(|(name, value)| json!({ "name": name, "value": value }))
        .collect();
    let old: Map<String, Value> = OLD_TYPE_NAMES
        .iter()
        .map(|(old, new)| (old.to_string(), json!(new)))
        .collect();
    json!({
        "schema_dirs": dirs,
        "types": types,
        "old_type_names": old,
        "components": components,
        "tables": tables,
        "archetypes": archetypes,
    })
}

/// `flint schema --list --json`: every component, scene table and archetype
/// by name, with enough to choose one.
pub fn list_json(registry: &SchemaRegistry, dirs: &[String]) -> Value {
    let brief = |c: &ComponentSchema| {
        let mut fields: Vec<&str> = c.field_names();
        fields.sort_unstable();
        json!({
            "name": c.name,
            "description": c.description,
            "open": c.is_open(),
            "fields": fields,
        })
    };
    let components: Vec<Value> = registry.components().into_iter().map(brief).collect();
    let tables: Vec<Value> = registry.tables().into_iter().map(brief).collect();
    let archetypes: Vec<Value> = registry
        .archetypes()
        .into_iter()
        .map(|a| {
            json!({
                "name": a.name,
                "description": a.description,
                "components": a.components,
            })
        })
        .collect();
    json!({
        "schema_dirs": dirs,
        "components": components,
        "tables": tables,
        "archetypes": archetypes,
    })
}

// --- `--format json-schema`: a JSON Schema for a scene file ----------------

const DRAFT: &str = "http://json-schema.org/draft-07/schema#";

fn numbers(min: usize, max: usize) -> Value {
    json!({
        "type": "array",
        "items": { "type": "number" },
        "minItems": min,
        "maxItems": max,
    })
}

fn vec3_schema() -> Value {
    json!({
        "anyOf": [
            numbers(3, 3),
            {
                "type": "object",
                "properties": {
                    "x": { "type": "number" },
                    "y": { "type": "number" },
                    "z": { "type": "number" },
                },
                "required": ["x", "y", "z"],
            },
        ]
    })
}

/// The JSON Schema of one value of `field_type`: what
/// `flint_schema::validate_component` accepts, as far as JSON Schema can
/// say it. Whether an `entity` or an `asset` leads anywhere is
/// `flint validate`'s to say.
fn type_schema(field_type: &FieldType) -> Map<String, Value> {
    let value = match field_type {
        FieldType::Bool => json!({ "type": "boolean" }),
        FieldType::I32 | FieldType::I64 => json!({ "type": "integer" }),
        FieldType::F32 | FieldType::F64 => json!({ "type": "number" }),
        FieldType::String | FieldType::Entity | FieldType::Asset { .. } => {
            json!({ "type": "string" })
        }
        FieldType::Vec2 => numbers(2, 2),
        FieldType::Vec3 => vec3_schema(),
        FieldType::Vec4 => numbers(4, 4),
        // [r, g, b] or [r, g, b, a] and nothing else: what the readers take (#804)
        FieldType::Color => numbers(3, 4),
        FieldType::Transform => json!({
            "type": "object",
            "properties": {
                "position": vec3_schema(),
                "rotation": vec3_schema(),
                "scale": vec3_schema(),
            },
        }),
        FieldType::Table => json!({ "type": "object" }),
        FieldType::Fields { fields } => table_schema(fields, &|_| false),
        FieldType::Enum { values } => json!({ "enum": values }),
        FieldType::Array { element } => json!({
            "type": "array",
            "items": Value::Object(type_schema(element)),
        }),
        // Any value: the component's reader judges its form (#1051)
        FieldType::Any => json!({}),
    };
    match value {
        Value::Object(map) => map,
        _ => Map::new(),
    }
}

fn field_schema(field: &FieldSchema) -> Value {
    let mut out = type_schema(&field.field_type);
    let mut description = field.description.clone().unwrap_or_default();
    let note = match &field.field_type {
        FieldType::Entity => Some("The name of an entity in this scene.".to_string()),
        FieldType::Asset {
            kinds, builtins, ..
        } if !kinds.is_empty() => {
            let found = format!(
                "Found by the project's asset search as: {}.",
                kinds
                    .iter()
                    .map(|kind| kind.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            Some(if builtins.is_empty() {
                found
            } else {
                format!("{found} Built in, with no file: {}.", builtins.join(", "))
            })
        }
        _ => None,
    };
    if let Some(note) = note {
        if !description.is_empty() {
            if !description.ends_with(['.', '!', '?', ')']) {
                description.push('.');
            }
            description.push(' ');
        }
        description.push_str(&note);
    }
    if !description.is_empty() {
        out.insert("description".into(), json!(description));
    }
    if let Some(default) = &field.default {
        out.insert("default".into(), to_json(default));
    }
    let mut out = Value::Object(out);
    match field.field_type {
        // A table's range (none says one) stays on the table
        FieldType::Fields { .. } | FieldType::Table => {
            if let Value::Object(map) = &mut out {
                if let Some(min) = field.min {
                    map.insert("minimum".into(), json!(min));
                }
                if let Some(max) = field.max {
                    map.insert("maximum".into(), json!(max));
                }
            }
        }
        _ => put_range(&mut out, field.min, field.max),
    }
    out
}

/// A field's range on every number its schema takes (#492): a scalar's
/// own, and each member of a vector or colour (`tint = [r, g, b]`), as
/// `flint validate` checks them; JSON Schema's `minimum` on an array would
/// say nothing. A number that has a range of its own (a hex colour's) keeps
/// it.
fn put_range(value: &mut Value, min: Option<f64>, max: Option<f64>) {
    match value {
        Value::Object(map) => {
            let number = matches!(
                map.get("type").and_then(Value::as_str),
                Some("number" | "integer")
            );
            if number && !map.contains_key("minimum") && !map.contains_key("maximum") {
                if let Some(min) = min {
                    map.insert("minimum".into(), json!(min));
                }
                if let Some(max) = max {
                    map.insert("maximum".into(), json!(max));
                }
            }
            for v in map.values_mut() {
                put_range(v, min, max);
            }
        }
        Value::Array(items) => {
            for v in items {
                put_range(v, min, max);
            }
        }
        _ => {}
    }
}

/// A table of `fields`. A key no field declares is allowed, as the engine
/// allows it (an `unknown-field` warning; games add fields scripts read).
/// `supplied(field)` says a required field is given some other way.
fn table_schema(fields: &HashMap<String, FieldSchema>, supplied: &dyn Fn(&str) -> bool) -> Value {
    let properties: Map<String, Value> = sorted(fields)
        .into_iter()
        .map(|(name, field)| (name.clone(), field_schema(field)))
        .collect();
    let required: Vec<&String> = sorted(fields)
        .into_iter()
        .filter(|(name, field)| field.required && field.default.is_none() && !supplied(name))
        .map(|(name, _)| name)
        .collect();
    let mut out = Map::new();
    out.insert("type".into(), json!("object"));
    out.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        out.insert("required".into(), json!(required));
    }
    Value::Object(out)
}

/// The JSON Schema of one component's table. A field is `required` only
/// when nothing else can supply it: it has no default of its own and no
/// archetype gives it one. An open component is any table.
pub fn component_schema(component: &ComponentSchema, registry: &SchemaRegistry) -> Value {
    let supplied = |field: &str| {
        registry
            .archetypes()
            .iter()
            .any(|a| a.get_default(&component.name, field).is_some())
    };
    let mut out = match table_schema(&component.fields, &supplied) {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    if component.is_open() {
        out.remove("properties");
    }
    if let Some(description) = &component.description {
        out.insert("description".into(), json!(description));
    }
    Value::Object(out)
}

/// The JSON Schema of one scene-level table (#686): its fields, each by its
/// type, a required one required (nothing else supplies a table's field).
/// A key the table doesn't declare is allowed, as in a component. An open
/// table is any table.
pub fn scene_table_schema(table: &ComponentSchema) -> Value {
    let mut out = match table_schema(&table.fields, &|_| false) {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    if table.is_open() {
        out.remove("properties");
    }
    if let Some(description) = &table.description {
        out.insert("description".into(), json!(description));
    }
    Value::Object(out)
}

/// A JSON Schema (draft-07) for a `.scene.toml` under `registry`:
///
/// - `[entities.<name>]` holds `archetype` (one of the loaded archetypes),
///   `parent`, and a table per component. A table that is no loaded
///   component is refused, as `flint validate` refuses it
///   (`unknown-component`).
/// - A component's table takes its fields, each by its type. A key the
///   component doesn't declare is allowed, as the engine allows it.
/// - `[scene]` and `[prefabs.<name>]` are described, and so is every
///   scene-level table a loaded `[table.<name>]` declares (`[camera]`,
///   `[post_process]`, a game's `[weather]`: #686), each a `$ref` into
///   `definitions.tables`. A top-level table none declares is allowed, as
///   the engine allows it (`unknown-table` is a warning).
pub fn scene_json_schema(registry: &SchemaRegistry, dirs: &[String]) -> Value {
    let mut tables = Map::new();
    let mut table_properties = Map::new();
    for table in registry.tables() {
        tables.insert(table.name.clone(), scene_table_schema(table));
        table_properties.insert(
            table.name.clone(),
            json!({ "$ref": format!("#/definitions/tables/{}", table.name) }),
        );
    }
    let mut components = Map::new();
    let mut entity_properties = Map::new();
    let archetype_names: Vec<&str> = registry
        .archetypes()
        .iter()
        .map(|a| a.name.as_str())
        .collect();
    entity_properties.insert(
        "archetype".into(),
        json!({
            "description": "The archetype whose components and defaults the entity starts from.",
            "enum": archetype_names,
        }),
    );
    entity_properties.insert(
        "parent".into(),
        json!({
            "type": "string",
            "description": "The name of the entity this one is a child of.",
        }),
    );
    for component in registry.components() {
        components.insert(
            component.name.clone(),
            component_schema(component, registry),
        );
        entity_properties.insert(
            component.name.clone(),
            json!({ "$ref": format!("#/definitions/components/{}", component.name) }),
        );
    }
    let mut schema = json!({
        "$schema": DRAFT,
        "title": "Flint scene",
        "description": format!(
            "A Flint .scene.toml. Generated by `flint schema --format json-schema` from {}; \
             generate it again when a schema changes.",
            dirs.join(", ")
        ),
        "type": "object",
        "required": ["scene"],
        "properties": {
            "scene": {
                "type": "object",
                "description": "What the scene is called, and how it starts.",
                "required": ["name"],
                "properties": {
                    "name": { "type": "string" },
                    "version": { "type": "string" },
                    "description": { "type": "string" },
                    "input_config": {
                        "type": "string",
                        "description": "An input config file for this scene.",
                    },
                    "preload_audio": { "type": "boolean" },
                },
            },
            "prefabs": {
                "type": "object",
                "description": "Prefab instances: each expands a prefabs/<template>.prefab.toml \
                                into entities named <prefix>_<name>.",
                "additionalProperties": {
                    "type": "object",
                    "required": ["template", "prefix"],
                    "properties": {
                        "template": { "type": "string" },
                        "prefix": { "type": "string" },
                        "overrides": { "type": "object" },
                    },
                },
            },
            "entities": {
                "type": "object",
                "description": "The scene's entities, by name.",
                "additionalProperties": { "$ref": "#/definitions/entity" },
            },
        },
        "definitions": {
            "entity": {
                "type": "object",
                "properties": entity_properties,
                "additionalProperties": false,
            },
            "components": components,
            "tables": tables,
        },
    });
    if let Some(properties) = schema["properties"].as_object_mut() {
        // `scene`, `prefabs` and `entities` are the format's own: a table
        // schema of one of those names would not be a scene table
        for (name, reference) in table_properties {
            properties.entry(name).or_insert(reference);
        }
    }
    schema
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> SchemaRegistry {
        let mut registry = SchemaRegistry::new();
        registry
            .load_component_string(
                r#"
[component.rig]
description = "A rig"

[component.rig.fields]
target = { type = "entity", default = "" }
mesh = { type = "asset", kind = ["models", "specs"], required = true }
weight = { type = "f32", default = 1.0, min = 0.0, max = 1.0 }
limits = "vec2"
tint = "color"
mode = { type = "enum", values = ["a", "b"] }
shear = { type = "table", fields = { xy = "f32" } }

[component.bag]
description = "Read by scripts"
"#,
            )
            .unwrap();
        registry
            .load_table_string(
                r#"
[table.weather]
description = "What the sky is doing"

[table.weather.fields]
rain = { type = "f32", min = 0.0, max = 1.0, default = 0.0 }
kind = { type = "enum", values = ["clear", "storm"], required = true }
fronts = { type = "array", element = { type = "table", fields = { at = "f32" } } }
"#,
            )
            .unwrap();
        registry
            .load_archetype_string(
                "[archetype.arm]\ncomponents = [\"rig\"]\n\n[archetype.arm.defaults.rig]\nweight = 0.5\n",
            )
            .unwrap();
        registry
    }

    #[test]
    fn the_registry_as_json_says_every_type_in_its_one_spelling() {
        let all = registry_json(&registry(), &["schemas".to_string()]);
        let rig = &all["components"]["rig"];
        assert_eq!(rig["open"], false);
        assert_eq!(rig["fields"]["target"]["type"], "entity");
        assert_eq!(rig["fields"]["target"]["default"], "");
        assert_eq!(rig["fields"]["mesh"]["kinds"], json!(["models", "specs"]));
        assert_eq!(rig["fields"]["mesh"]["required"], true);
        assert_eq!(rig["fields"]["weight"]["max"], 1.0);
        assert_eq!(rig["fields"]["mode"]["values"], json!(["a", "b"]));
        assert_eq!(rig["fields"]["shear"]["fields"]["xy"]["type"], "f32");
        assert_eq!(all["components"]["bag"]["open"], true);
        assert_eq!(all["archetypes"]["arm"]["defaults"]["rig"]["weight"], 0.5);
        assert_eq!(all["old_type_names"]["float"], "f32");
        assert_eq!(all["types"].as_array().unwrap().len(), TYPE_NAMES.len());

        let list = list_json(&registry(), &[]);
        let names: Vec<&str> = list["components"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["bag", "rig"]);
        assert_eq!(list["archetypes"][0]["name"], "arm");
    }

    /// #686: a scene table is in every form of the export, as a component
    /// is, and the scene's JSON Schema describes it at the top level.
    #[test]
    fn scene_tables_are_exported_beside_the_components() {
        let registry = registry();
        let all = registry_json(&registry, &[]);
        let weather = &all["tables"]["weather"];
        assert_eq!(weather["kind"], "table");
        assert_eq!(weather["open"], false);
        assert_eq!(weather["fields"]["rain"]["max"], 1.0);
        assert_eq!(weather["fields"]["kind"]["required"], true);
        assert!(all["components"].get("weather").is_none());

        let list = list_json(&registry, &[]);
        assert_eq!(list["tables"][0]["name"], "weather");
        assert_eq!(
            list["tables"][0]["fields"],
            json!(["fronts", "kind", "rain"])
        );

        let schema = scene_json_schema(&registry, &[]);
        assert_eq!(
            schema["properties"]["weather"]["$ref"],
            "#/definitions/tables/weather"
        );
        assert_eq!(schema["properties"]["scene"]["type"], "object");
        let weather = &schema["definitions"]["tables"]["weather"];
        assert_eq!(weather["required"], json!(["kind"]));
        assert_eq!(weather["properties"]["rain"]["maximum"], 1.0);
        assert_eq!(
            weather["properties"]["fronts"]["items"]["properties"]["at"]["type"],
            "number"
        );
        assert_eq!(
            scene_table_schema(registry.get_table("weather").unwrap()),
            *weather
        );
        // A table no schema declares is still allowed at the top level
        assert!(schema.get("additionalProperties").is_none());
    }

    #[test]
    fn the_scene_schema_describes_entities_by_component() {
        let schema = scene_json_schema(&registry(), &["schemas".to_string()]);
        let entity = &schema["definitions"]["entity"];
        assert_eq!(entity["additionalProperties"], false);
        assert_eq!(entity["properties"]["archetype"]["enum"], json!(["arm"]));
        assert_eq!(
            entity["properties"]["rig"]["$ref"],
            "#/definitions/components/rig"
        );
        let rig = &schema["definitions"]["components"]["rig"];
        assert_eq!(rig["required"], json!(["mesh"]));
        assert_eq!(rig["properties"]["weight"]["maximum"], 1.0);
        assert_eq!(rig["properties"]["limits"]["minItems"], 2);
        assert_eq!(rig["properties"]["mode"]["enum"], json!(["a", "b"]));
        assert_eq!(
            rig["properties"]["shear"]["properties"]["xy"]["type"],
            "number"
        );
        assert!(rig["properties"]["target"]["description"]
            .as_str()
            .unwrap()
            .contains("entity in this scene"));
        // An open component is any table
        let bag = &schema["definitions"]["components"]["bag"];
        assert_eq!(bag["type"], "object");
        assert!(bag.get("properties").is_none());
    }
}
