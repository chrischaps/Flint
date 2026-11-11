//! `schemas/scene/living.toml` and `schemas/scene/wooze.toml` against the
//! types that read those tables (#686; flint-scene's `tests/table_schemas.rs`
//! does the same for the tables it reads itself).
//!
//! Both types refuse a key they don't know, so a table that writes every
//! field its schema declares, and parses, shows the schema declares nothing
//! the reader would refuse.

use flint_procgen::LivingTables;
use flint_scene::SceneFile;
use flint_schema::{FieldSchema, FieldType, SchemaRegistry};
use std::collections::HashMap;

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
    }
}

/// A table with every one of `fields` written.
fn sample_table(fields: &HashMap<String, FieldSchema>) -> toml::Table {
    fields
        .iter()
        .map(|(name, field)| (name.clone(), sample(&field.field_type)))
        .collect()
}

#[test]
fn living_and_wooze_schemas_declare_only_what_is_read() {
    let registry = SchemaRegistry::load_from_directory("../../schemas").unwrap();
    let mut scene = SceneFile::new("s");
    for name in ["living", "wooze"] {
        let schema = registry
            .get_table(name)
            .unwrap_or_else(|| panic!("schemas/scene/{name}.toml"));
        scene
            .tables
            .insert(name.to_string(), sample_table(&schema.fields).into());
    }
    let tables = LivingTables::of(&scene)
        .unwrap_or_else(|e| panic!("a table with every declared field: {e}\n{scene:#?}"));
    // Every field reached its type: none was dropped on the way
    let living = tables.living.expect("[living]");
    assert!(living.shadows.is_some() && living.stagger.is_some() && living.softness.is_some());
    let echoes = living.echoes.expect("echoes");
    assert!(echoes.count.is_some() && echoes.spread.is_some() && echoes.alpha.is_some());
    let variance = living.variance.expect("variance");
    assert!(variance.amount.is_some() && variance.steady.is_some());
    assert!(variance.keep_size.is_some());
    let wooze = tables.wooze.expect("[wooze]");
    assert!(wooze.preset.is_some() && wooze.amplitude.is_some() && wooze.step.is_some());
    assert!(wooze.glide.is_some() && wooze.jitter.is_some());
    assert_eq!(wooze.zone.len(), 1);
    // ...and the count is the type's, so a field added to one is added to
    // the other
    assert_eq!(registry.get_table("living").unwrap().fields.len(), 6);
    assert_eq!(registry.get_table("wooze").unwrap().fields.len(), 6);
    // A zone's fields are the keys the field itself takes
    let FieldType::Array { element } = &registry
        .get_table("wooze")
        .unwrap()
        .get_field("zone")
        .unwrap()
        .field_type
    else {
        panic!("wooze.zone is an array");
    };
    let FieldType::Fields { fields } = &**element else {
        panic!("a zone is a table with fields");
    };
    let mut declared: Vec<&str> = fields.keys().map(String::as_str).collect();
    declared.sort_unstable();
    let mut keys = flint_core::wooze::ZONE_KEYS.to_vec();
    keys.sort_unstable();
    assert_eq!(declared, keys);
}
