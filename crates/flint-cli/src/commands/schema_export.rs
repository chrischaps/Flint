//! The loaded schemas as data (#684): `flint schema --format json` (the
//! registry as it is: every component, field and archetype) and
//! `--format json-schema` (a JSON Schema for a `.scene.toml`, which taplo and
//! Even Better TOML read for completion and inline errors).
//!
//! Both are built from a [`SchemaRegistry`], so they describe exactly the
//! schema directories the command resolved: the engine's set, and a game's
//! layer over it.

use flint_schema::{
    ArchetypeSchema, ComponentSchema, FieldSchema, FieldType, SchemaRegistry, OLD_TYPE_NAMES,
    TYPE_NAMES,
};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

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
        FieldType::Asset { kinds, unless } => {
            let labels: Vec<&str> = kinds.iter().map(|kind| kind.label()).collect();
            out.insert("kinds".into(), json!(labels));
            if let Some(unless) = unless {
                out.insert("unless".into(), json!(unless));
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
/// and nothing in it checked) and its fields by name.
pub fn component_json(component: &ComponentSchema) -> Value {
    json!({
        "kind": "component",
        "name": component.name,
        "description": component.description,
        "open": component.is_open(),
        "fields": fields_json(&component.fields),
    })
}

/// One archetype: its components, and the defaults it gives them.
pub fn archetype_json(archetype: &ArchetypeSchema) -> Value {
    let defaults: Map<String, Value> = sorted(&archetype.defaults)
        .into_iter()
        .map(|(component, table)| (component.clone(), to_json(table)))
        .collect();
    json!({
        "kind": "archetype",
        "name": archetype.name,
        "description": archetype.description,
        "components": archetype.components,
        "defaults": defaults,
    })
}

/// Everything loaded: the directories, the type names (and the old spellings
/// that still load), every component and every archetype.
pub fn registry_json(registry: &SchemaRegistry, dirs: &[String]) -> Value {
    let components: Map<String, Value> = registry
        .components()
        .into_iter()
        .map(|c| (c.name.clone(), component_json(c)))
        .collect();
    let archetypes: Map<String, Value> = registry
        .archetypes()
        .into_iter()
        .map(|a| (a.name.clone(), archetype_json(a)))
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
        "archetypes": archetypes,
    })
}

/// `flint schema --list --json`: every component and archetype by name,
/// with enough to choose one.
pub fn list_json(registry: &SchemaRegistry, dirs: &[String]) -> Value {
    let components: Vec<Value> = registry
        .components()
        .into_iter()
        .map(|c| {
            let mut fields: Vec<&str> = c.field_names();
            fields.sort_unstable();
            json!({
                "name": c.name,
                "description": c.description,
                "open": c.is_open(),
                "fields": fields,
            })
        })
        .collect();
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
        FieldType::Color => json!({
            "anyOf": [
                numbers(3, 4),
                {
                    "type": "object",
                    "properties": {
                        "r": { "type": "number" },
                        "g": { "type": "number" },
                        "b": { "type": "number" },
                        "a": { "type": "number" },
                    },
                    "required": ["r", "g", "b"],
                },
                { "type": "integer", "minimum": 0, "maximum": 0xFFFFFF },
            ]
        }),
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
        FieldType::Asset { kinds, .. } if !kinds.is_empty() => Some(format!(
            "Found by the project's asset search as: {}.",
            kinds
                .iter()
                .map(|kind| kind.label())
                .collect::<Vec<_>>()
                .join(", ")
        )),
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
    if let Some(min) = field.min {
        out.insert("minimum".into(), json!(min));
    }
    if let Some(max) = field.max {
        out.insert("maximum".into(), json!(max));
    }
    Value::Object(out)
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

/// A JSON Schema (draft-07) for a `.scene.toml` under `registry`:
///
/// - `[entities.<name>]` holds `archetype` (one of the loaded archetypes),
///   `parent`, and a table per component. A table that is no loaded
///   component is refused, as `flint validate` refuses it
///   (`unknown-component`).
/// - A component's table takes its fields, each by its type. A key the
///   component doesn't declare is allowed, as the engine allows it.
/// - `[scene]` and `[prefabs.<name>]` are described; any other top-level
///   table (`[camera]`, `[post_process]`, ...) is allowed and not described
///   (scene-level tables are #686).
pub fn scene_json_schema(registry: &SchemaRegistry, dirs: &[String]) -> Value {
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
    json!({
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
        },
    })
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
