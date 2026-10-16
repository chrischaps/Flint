//! Component schema definitions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The type of a field in a component schema
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Bool,
    I32,
    I64,
    F32,
    F64,
    String,
    Vec3,
    Transform,
    Color,
    /// A TOML table with no fixed shape (one animator layer, say). Validated
    /// only as "is a table"; the component that reads it checks its keys.
    Table,
    #[serde(rename = "enum")]
    Enum {
        values: Vec<String>,
    },
    Array {
        element: Box<FieldType>,
    },
}

impl FieldType {
    pub fn type_name(&self) -> &'static str {
        match self {
            FieldType::Bool => "bool",
            FieldType::I32 => "i32",
            FieldType::I64 => "i64",
            FieldType::F32 => "f32",
            FieldType::F64 => "f64",
            FieldType::String => "string",
            FieldType::Vec3 => "vec3",
            FieldType::Transform => "transform",
            FieldType::Color => "color",
            FieldType::Table => "table",
            FieldType::Enum { .. } => "enum",
            FieldType::Array { .. } => "array",
        }
    }
}

/// Schema for a single field within a component
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldSchema {
    #[serde(rename = "type")]
    pub field_type: FieldType,
    #[serde(default)]
    pub default: Option<toml::Value>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
}

/// Schema definition for a component
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentSchema {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub fields: HashMap<String, FieldSchema>,
}

impl ComponentSchema {
    /// Get a field schema by name
    pub fn get_field(&self, name: &str) -> Option<&FieldSchema> {
        self.fields.get(name)
    }

    /// List all field names
    pub fn field_names(&self) -> Vec<&str> {
        self.fields.keys().map(|s| s.as_str()).collect()
    }

    /// Check if a field is required
    pub fn is_field_required(&self, name: &str) -> bool {
        self.fields.get(name).map(|f| f.required).unwrap_or(false)
    }
}

/// TOML file format for component schemas
#[derive(Debug, Deserialize)]
pub struct ComponentSchemaFile {
    pub component: HashMap<String, ComponentSchemaDefinition>,
}

#[derive(Debug, Deserialize)]
pub struct ComponentSchemaDefinition {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub fields: HashMap<String, FieldSchemaDefinition>,
}

/// Field definition as it appears in TOML files
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum FieldSchemaDefinition {
    Simple(String),
    Detailed(DetailedFieldSchema),
}

#[derive(Debug, Clone, Deserialize)]
pub struct DetailedFieldSchema {
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub values: Option<Vec<String>>,
    #[serde(default)]
    pub default: Option<toml::Value>,
    #[serde(default)]
    pub required: Option<bool>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub element: Option<String>,
}

impl FieldSchemaDefinition {
    pub fn to_field_schema(self) -> FieldSchema {
        match self {
            FieldSchemaDefinition::Simple(type_str) => FieldSchema {
                field_type: parse_field_type(&type_str, None, None),
                default: None,
                required: false,
                description: None,
                min: None,
                max: None,
            },
            FieldSchemaDefinition::Detailed(d) => FieldSchema {
                field_type: parse_field_type(
                    &d.field_type,
                    d.values.as_ref(),
                    d.element.as_deref(),
                ),
                default: d.default,
                required: d.required.unwrap_or(false),
                description: d.description,
                min: d.min,
                max: d.max,
            },
        }
    }
}

impl FieldSchemaDefinition {
    /// The type name this field writes that the parser doesn't know, if any:
    /// its own type, or its array element's. An unknown name still loads (as
    /// `String`), so this is how the loader and tests catch a typo.
    pub fn unknown_type_name(&self) -> Option<&str> {
        let (type_str, element) = match self {
            FieldSchemaDefinition::Simple(t) => (t.as_str(), None),
            FieldSchemaDefinition::Detailed(d) => (d.field_type.as_str(), d.element.as_deref()),
        };
        if known_field_type(type_str, None, None).is_none() {
            return Some(type_str);
        }
        if type_str == "array" {
            if let Some(e) = element {
                if known_field_type(e, None, None).is_none() {
                    return Some(e);
                }
            }
        }
        None
    }
}

fn parse_field_type(
    type_str: &str,
    enum_values: Option<&Vec<String>>,
    array_element: Option<&str>,
) -> FieldType {
    // Unknown names fall back to String. The registry warns about them at
    // load, naming the schema and field (see `unknown_type_name`).
    known_field_type(type_str, enum_values, array_element).unwrap_or(FieldType::String)
}

fn known_field_type(
    type_str: &str,
    enum_values: Option<&Vec<String>>,
    array_element: Option<&str>,
) -> Option<FieldType> {
    Some(match type_str {
        // `boolean` / `integer` are the TOML spec's names; schemas write them.
        "bool" | "boolean" => FieldType::Bool,
        "i32" => FieldType::I32,
        "i64" => FieldType::I64,
        // `float` / `int` are what most engine schemas write; without these
        // arms they fell through to String and every numeric field warned.
        "f32" | "float" => FieldType::F32,
        "f64" => FieldType::F64,
        "int" | "integer" => FieldType::I32,
        // An entity is referred to by name; no variant checks it exists yet.
        "string" | "entity_ref" => FieldType::String,
        "vec3" => FieldType::Vec3,
        // vec2/vec4 have no dedicated variant; validate as a float array so
        // schema-driven validation doesn't misreport them as strings.
        "vec2" | "vec4" => FieldType::Array {
            element: Box::new(FieldType::F32),
        },
        "transform" => FieldType::Transform,
        "color" => FieldType::Color,
        "table" => FieldType::Table,
        "enum" => FieldType::Enum {
            values: enum_values.cloned().unwrap_or_default(),
        },
        "array" => {
            // An array with no `element` is an array of strings.
            let element_type = array_element
                .map(|e| parse_field_type(e, None, None))
                .unwrap_or(FieldType::String);
            FieldType::Array {
                element: Box::new(element_type),
            }
        }
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(type_str: &str, element: Option<&str>) -> FieldSchemaDefinition {
        FieldSchemaDefinition::Detailed(DetailedFieldSchema {
            field_type: type_str.into(),
            values: None,
            default: None,
            required: None,
            description: None,
            min: None,
            max: None,
            element: element.map(Into::into),
        })
    }

    #[test]
    fn toml_spelled_names_parse() {
        assert_eq!(
            def("integer", None).to_field_schema().field_type,
            FieldType::I32
        );
        assert_eq!(
            def("boolean", None).to_field_schema().field_type,
            FieldType::Bool
        );
        assert_eq!(
            def("table", None).to_field_schema().field_type,
            FieldType::Table
        );
        assert_eq!(
            def("array", Some("table")).to_field_schema().field_type,
            FieldType::Array {
                element: Box::new(FieldType::Table)
            }
        );
    }

    #[test]
    fn unknown_names_are_reported() {
        assert_eq!(def("intger", None).unknown_type_name(), Some("intger"));
        assert_eq!(
            def("array", Some("flaot")).unknown_type_name(),
            Some("flaot")
        );
        assert_eq!(
            FieldSchemaDefinition::Simple("nope".into()).unknown_type_name(),
            Some("nope")
        );
        assert_eq!(def("array", None).unknown_type_name(), None);
        assert_eq!(def("string", None).unknown_type_name(), None);
    }
}
