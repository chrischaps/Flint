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
    #[serde(rename = "enum")]
    Enum { values: Vec<String> },
    Array { element: Box<FieldType> },
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
                field_type: parse_field_type(&d.field_type, d.values.as_ref(), d.element.as_deref()),
                default: d.default,
                required: d.required.unwrap_or(false),
                description: d.description,
                min: d.min,
                max: d.max,
            },
        }
    }
}

fn parse_field_type(
    type_str: &str,
    enum_values: Option<&Vec<String>>,
    array_element: Option<&str>,
) -> FieldType {
    match type_str {
        "bool" => FieldType::Bool,
        "i32" => FieldType::I32,
        "i64" => FieldType::I64,
        "f32" => FieldType::F32,
        "f64" => FieldType::F64,
        "string" => FieldType::String,
        "vec3" => FieldType::Vec3,
        "transform" => FieldType::Transform,
        "color" => FieldType::Color,
        "enum" => FieldType::Enum {
            values: enum_values.cloned().unwrap_or_default(),
        },
        "array" => {
            let element_type = array_element
                .map(|e| parse_field_type(e, None, None))
                .unwrap_or(FieldType::String);
            FieldType::Array {
                element: Box::new(element_type),
            }
        }
        _ => FieldType::String, // Default fallback
    }
}
