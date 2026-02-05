//! Schema introspection command

use anyhow::{Context, Result};
use flint_schema::{FieldType, SchemaRegistry};
use std::path::Path;

pub fn run(name: &str, schemas_path: &str) -> Result<()> {
    if !Path::new(schemas_path).exists() {
        anyhow::bail!("Schemas directory not found: {}", schemas_path);
    }

    let registry =
        SchemaRegistry::load_from_directory(schemas_path).context("Failed to load schemas")?;

    // Try component first, then archetype
    if let Some(component) = registry.get_component(name) {
        println!("Component: {}", component.name);
        if let Some(desc) = &component.description {
            println!("Description: {}", desc);
        }
        println!("");
        println!("Fields:");

        let mut fields: Vec<_> = component.fields.iter().collect();
        fields.sort_by_key(|(name, _)| *name);

        for (field_name, field_schema) in fields {
            let type_str = format_field_type(&field_schema.field_type);
            let required = if field_schema.required { " (required)" } else { "" };

            print!("  {} : {}{}", field_name, type_str, required);

            if let Some(default) = &field_schema.default {
                print!(" = {}", format_toml_value(default));
            }

            println!();

            if let Some(desc) = &field_schema.description {
                println!("    # {}", desc);
            }

            if field_schema.min.is_some() || field_schema.max.is_some() {
                let min = field_schema.min.map(|v| v.to_string()).unwrap_or_default();
                let max = field_schema.max.map(|v| v.to_string()).unwrap_or_default();
                println!("    # Range: {} .. {}", min, max);
            }
        }

        return Ok(());
    }

    if let Some(archetype) = registry.get_archetype(name) {
        println!("Archetype: {}", archetype.name);
        if let Some(desc) = &archetype.description {
            println!("Description: {}", desc);
        }
        println!("");
        println!("Components:");
        for comp_name in &archetype.components {
            let exists = if registry.get_component(comp_name).is_some() {
                ""
            } else {
                " (not found)"
            };
            println!("  - {}{}", comp_name, exists);
        }

        if !archetype.defaults.is_empty() {
            println!("");
            println!("Defaults:");
            for (comp_name, defaults) in &archetype.defaults {
                println!("  [{}]", comp_name);
                if let Some(table) = defaults.as_table() {
                    for (k, v) in table {
                        println!("    {} = {}", k, format_toml_value(v));
                    }
                } else {
                    println!("    {}", format_toml_value(defaults));
                }
            }
        }

        return Ok(());
    }

    // List available schemas
    println!("Schema '{}' not found.", name);
    println!("");

    let components = registry.component_names();
    let archetypes = registry.archetype_names();

    if !components.is_empty() {
        println!("Available components:");
        for c in components {
            println!("  - {}", c);
        }
    }

    if !archetypes.is_empty() {
        println!("");
        println!("Available archetypes:");
        for a in archetypes {
            println!("  - {}", a);
        }
    }

    Ok(())
}

fn format_field_type(ft: &FieldType) -> String {
    match ft {
        FieldType::Bool => "bool".to_string(),
        FieldType::I32 => "i32".to_string(),
        FieldType::I64 => "i64".to_string(),
        FieldType::F32 => "f32".to_string(),
        FieldType::F64 => "f64".to_string(),
        FieldType::String => "string".to_string(),
        FieldType::Vec3 => "vec3".to_string(),
        FieldType::Transform => "transform".to_string(),
        FieldType::Color => "color".to_string(),
        FieldType::Enum { values } => format!("enum[{}]", values.join(", ")),
        FieldType::Array { element } => format!("array<{}>", format_field_type(element)),
    }
}

fn format_toml_value(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => format!("\"{}\"", s),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(format_toml_value).collect();
            format!("[{}]", items.join(", "))
        }
        toml::Value::Table(t) => {
            let items: Vec<String> = t
                .iter()
                .map(|(k, v)| format!("{} = {}", k, format_toml_value(v)))
                .collect();
            format!("{{ {} }}", items.join(", "))
        }
        toml::Value::Datetime(d) => d.to_string(),
    }
}
