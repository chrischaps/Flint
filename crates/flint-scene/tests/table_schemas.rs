//! The engine's scene-table schemas (`schemas/scene/*.toml`, #686) against
//! the structs of this crate that read the tables (`[living]` and `[wooze]`
//! are held to theirs in flint-procgen, which owns them, and `[panels]` to
//! `PanelLayout` in flint-render's `tests/panels_schema.rs`, #976).
//!
//! The schema is what `flint schema`, an editor and `flint validate` go by;
//! the struct is what runs. A field in one and not the other is a check that
//! lies: a key the schema takes and serde drops, or one serde reads and the
//! schema calls unknown.

use flint_scene::{PostProcessDef, SceneFile};
use flint_schema::{FieldSchema, FieldType, SchemaRegistry, TableSchema};
use std::collections::{BTreeSet, HashMap};

fn schemas() -> SchemaRegistry {
    SchemaRegistry::load_from_directory("../../schemas").unwrap()
}

/// A value the type takes.
fn sample(field_type: &FieldType) -> toml::Value {
    use toml::Value;
    match field_type {
        FieldType::Bool => Value::Boolean(true),
        FieldType::I32 | FieldType::I64 => Value::Integer(1),
        FieldType::F32 | FieldType::F64 => Value::Float(0.5),
        FieldType::String | FieldType::Entity | FieldType::Asset { .. } => "x".into(),
        FieldType::Vec2 => vec![0.1, 0.2].into(),
        FieldType::Vec3 | FieldType::Color => vec![0.1, 0.2, 0.3].into(),
        FieldType::Vec4 => vec![0.1, 0.2, 0.3, 0.4].into(),
        FieldType::Transform | FieldType::Table => Value::Table(toml::Table::new()),
        FieldType::Fields { fields } => Value::Table(sample_table(fields)),
        FieldType::Enum { values } => values[0].clone().into(),
        FieldType::Array { element } => Value::Array(vec![sample(element)]),
        FieldType::Any => Value::Float(0.5),
    }
}

/// A table with every one of `fields` written.
fn sample_table(fields: &HashMap<String, FieldSchema>) -> toml::Table {
    fields
        .iter()
        .map(|(name, field)| (name.clone(), sample(&field.field_type)))
        .collect()
}

fn table<'a>(registry: &'a SchemaRegistry, name: &str) -> &'a TableSchema {
    registry
        .get_table(name)
        .unwrap_or_else(|| panic!("schemas/scene/{name}.toml"))
}

/// A scene whose `[<name>]` writes every field the schema declares.
fn scene_writing_all_of(schema: &TableSchema) -> String {
    let mut doc = toml::Table::new();
    let mut scene = toml::Table::new();
    scene.insert("name".into(), "s".into());
    doc.insert("scene".into(), scene.into());
    doc.insert(schema.name.clone(), sample_table(&schema.fields).into());
    toml::to_string(&doc).unwrap()
}

/// `[post_process]`: the schema's fields are exactly the struct's
/// ([`PostProcessDef::field_names`], read back from a serialized one), and
/// each default the schema states is the one serde fills in.
#[test]
fn post_process_schema_is_the_struct() {
    let registry = schemas();
    let schema = table(&registry, "post_process");
    let declared: BTreeSet<String> = schema.fields.keys().cloned().collect();
    let read: BTreeSet<String> = PostProcessDef::field_names().into_iter().collect();
    assert_eq!(
        declared.difference(&read).collect::<Vec<_>>(),
        [] as [&String; 0],
        "the schema declares what PostProcessDef does not read"
    );
    assert_eq!(
        read.difference(&declared).collect::<Vec<_>>(),
        [] as [&String; 0],
        "PostProcessDef reads what schemas/scene/post_process.toml does not declare"
    );

    let empty: PostProcessDef = toml::from_str("").unwrap();
    let defaults = toml::Table::try_from(&empty).unwrap();
    let near = |a: &toml::Value, b: &toml::Value| match (a, b) {
        (toml::Value::Array(a), toml::Value::Array(b)) => {
            a.len() == b.len()
                && a.iter().zip(b).all(|(a, b)| {
                    let (a, b) = (number(a), number(b));
                    (a - b).abs() < 1e-6
                })
        }
        (a, b) if a.is_float() || b.is_float() => (number(a) - number(b)).abs() < 1e-6,
        (a, b) => a == b,
    };
    for (name, field) in &schema.fields {
        match (&field.default, defaults.get(name)) {
            (Some(stated), Some(filled)) => assert!(
                near(stated, filled),
                "post_process.{name}: the schema says {stated}, the struct fills in {filled}"
            ),
            (Some(stated), None) => panic!("post_process.{name} = {stated}: no such default"),
            (None, Some(filled)) => {
                panic!("post_process.{name} defaults to {filled}, and the schema does not say so")
            }
            (None, None) => {}
        }
    }
}

fn number(value: &toml::Value) -> f64 {
    match value {
        toml::Value::Integer(i) => *i as f64,
        toml::Value::Float(f) => *f,
        other => panic!("{other} is no number"),
    }
}

/// #741: no scene or look in the repo writes a built-in effect value past
/// the range its descriptor gives, so the ranges are what the shipped
/// content is tuned within and the `post-range` warning is news.
#[test]
#[allow(clippy::disallowed_methods)] // walks the repo's files on the disk, not a game's through the vfs
fn every_shipped_post_value_lies_in_its_range() {
    fn walk(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !(name.starts_with("target") || name.starts_with('.') || name == "node_modules")
                {
                    walk(&path, found);
                }
            } else if name.ends_with(".scene.toml") || name.ends_with(".look.toml") {
                found.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(std::path::Path::new("../.."), &mut files);
    assert!(files.len() > 100, "found {} scenes and looks", files.len());
    let mut past = Vec::new();
    for file in &files {
        let Ok(doc) = toml::from_str::<toml::Table>(&std::fs::read_to_string(file).unwrap()) else {
            continue;
        };
        let Some(toml::Value::Table(post)) = doc.get("post_process") else {
            continue;
        };
        for warning in flint_schema::post_effects::range_warnings(post) {
            past.push(format!("{}: {warning}", file.display()));
        }
    }
    assert_eq!(past, [] as [String; 0]);
}

/// `[camera]` and `[environment]`: a table writing every declared field
/// parses, and every one of them reaches the struct (it serializes back).
#[test]
fn camera_and_environment_schemas_declare_only_what_is_read() {
    let registry = schemas();
    for name in ["camera", "environment"] {
        let schema = table(&registry, name);
        let scene: SceneFile = toml::from_str(&scene_writing_all_of(schema))
            .unwrap_or_else(|e| panic!("[{name}] with every declared field: {e}"));
        let back = toml::Table::try_from(&scene).unwrap();
        let kept: BTreeSet<&String> = back[name].as_table().unwrap().keys().collect();
        let declared: BTreeSet<&String> = schema.fields.keys().collect();
        assert_eq!(
            declared.difference(&kept).collect::<Vec<_>>(),
            [] as [&&String; 0],
            "[{name}]: declared, and dropped by the struct"
        );
    }
    // ...and the count is the struct's, so a field added to one is added
    // to the other
    assert_eq!(table(&registry, "camera").fields.len(), 7);
    assert_eq!(table(&registry, "environment").fields.len(), 9);
}
