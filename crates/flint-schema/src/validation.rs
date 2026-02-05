//! Validation of component data against schemas

use crate::component::{ComponentSchema, FieldType};
use flint_core::{FlintError, Result};

/// Validate component data against its schema
pub fn validate_component_data(
    schema: &ComponentSchema,
    data: &toml::Value,
) -> Result<()> {
    let table = data
        .as_table()
        .ok_or_else(|| FlintError::ValidationError("Component data must be a table".to_string()))?;

    // Check all required fields are present
    for (field_name, field_schema) in &schema.fields {
        if field_schema.required && !table.contains_key(field_name) {
            return Err(FlintError::MissingRequiredField(field_name.clone()));
        }
    }

    // Validate each provided field
    for (field_name, value) in table {
        if let Some(field_schema) = schema.fields.get(field_name) {
            validate_field_value(field_name, &field_schema.field_type, value, field_schema.min, field_schema.max)?;
        }
        // Unknown fields are allowed for flexibility
    }

    Ok(())
}

fn validate_field_value(
    field_name: &str,
    field_type: &FieldType,
    value: &toml::Value,
    min: Option<f64>,
    max: Option<f64>,
) -> Result<()> {
    match (field_type, value) {
        (FieldType::Bool, toml::Value::Boolean(_)) => Ok(()),
        (FieldType::I32 | FieldType::I64, toml::Value::Integer(n)) => {
            let n = *n as f64;
            validate_range(field_name, n, min, max)
        }
        (FieldType::F32 | FieldType::F64, toml::Value::Float(n)) => {
            validate_range(field_name, *n, min, max)
        }
        (FieldType::F32 | FieldType::F64, toml::Value::Integer(n)) => {
            // Allow integers where floats are expected
            validate_range(field_name, *n as f64, min, max)
        }
        (FieldType::String, toml::Value::String(_)) => Ok(()),
        (FieldType::Vec3, toml::Value::Table(t)) => {
            // Vec3 can be {x, y, z} or an array [x, y, z]
            if t.contains_key("x") && t.contains_key("y") && t.contains_key("z") {
                Ok(())
            } else {
                Err(FlintError::ValidationError(format!(
                    "Field '{}': Vec3 must have x, y, z fields",
                    field_name
                )))
            }
        }
        (FieldType::Vec3, toml::Value::Array(arr)) => {
            if arr.len() == 3 {
                Ok(())
            } else {
                Err(FlintError::ValidationError(format!(
                    "Field '{}': Vec3 array must have exactly 3 elements",
                    field_name
                )))
            }
        }
        (FieldType::Transform, toml::Value::Table(t)) => {
            // Transform needs position, rotation, scale - all optional with defaults
            for key in ["position", "rotation", "scale"] {
                if let Some(v) = t.get(key) {
                    validate_field_value(&format!("{}.{}", field_name, key), &FieldType::Vec3, v, None, None)?;
                }
            }
            Ok(())
        }
        (FieldType::Color, toml::Value::Table(t)) => {
            // Color can be {r, g, b, a}
            if t.contains_key("r") && t.contains_key("g") && t.contains_key("b") {
                Ok(())
            } else {
                Err(FlintError::ValidationError(format!(
                    "Field '{}': Color must have r, g, b fields",
                    field_name
                )))
            }
        }
        (FieldType::Color, toml::Value::Integer(hex)) => {
            // Allow hex color like 0xFF8844
            if *hex >= 0 && *hex <= 0xFFFFFF {
                Ok(())
            } else {
                Err(FlintError::ValidationError(format!(
                    "Field '{}': Hex color must be in range 0x000000-0xFFFFFF",
                    field_name
                )))
            }
        }
        (FieldType::Enum { values }, toml::Value::String(s)) => {
            if values.contains(s) {
                Ok(())
            } else {
                Err(FlintError::InvalidEnumValue {
                    value: s.clone(),
                    allowed: values.clone(),
                })
            }
        }
        (FieldType::Array { element }, toml::Value::Array(arr)) => {
            for (i, item) in arr.iter().enumerate() {
                validate_field_value(&format!("{}[{}]", field_name, i), element, item, None, None)?;
            }
            Ok(())
        }
        _ => Err(FlintError::InvalidFieldType {
            expected: field_type.type_name().to_string(),
            got: value_type_name(value).to_string(),
        }),
    }
}

fn validate_range(field_name: &str, value: f64, min: Option<f64>, max: Option<f64>) -> Result<()> {
    if let Some(min_val) = min {
        if value < min_val {
            return Err(FlintError::ValueOutOfRange {
                field: field_name.to_string(),
                min: min_val,
                max: max.unwrap_or(f64::MAX),
                value,
            });
        }
    }
    if let Some(max_val) = max {
        if value > max_val {
            return Err(FlintError::ValueOutOfRange {
                field: field_name.to_string(),
                min: min.unwrap_or(f64::MIN),
                max: max_val,
                value,
            });
        }
    }
    Ok(())
}

fn value_type_name(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "string",
        toml::Value::Integer(_) => "integer",
        toml::Value::Float(_) => "float",
        toml::Value::Boolean(_) => "bool",
        toml::Value::Datetime(_) => "datetime",
        toml::Value::Array(_) => "array",
        toml::Value::Table(_) => "table",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::component::FieldSchema;

    fn make_test_schema() -> ComponentSchema {
        let mut fields = HashMap::new();
        fields.insert(
            "locked".to_string(),
            FieldSchema {
                field_type: FieldType::Bool,
                default: None,
                required: false,
                description: None,
                min: None,
                max: None,
            },
        );
        fields.insert(
            "angle".to_string(),
            FieldSchema {
                field_type: FieldType::F32,
                default: None,
                required: false,
                description: None,
                min: Some(0.0),
                max: Some(180.0),
            },
        );
        fields.insert(
            "style".to_string(),
            FieldSchema {
                field_type: FieldType::Enum {
                    values: vec!["hinged".to_string(), "sliding".to_string()],
                },
                default: None,
                required: true,
                description: None,
                min: None,
                max: None,
            },
        );

        ComponentSchema {
            name: "door".to_string(),
            description: None,
            fields,
        }
    }

    #[test]
    fn test_valid_data() {
        let schema = make_test_schema();
        let data: toml::Value = toml::from_str(
            r#"
            locked = true
            angle = 90.0
            style = "hinged"
            "#,
        )
        .unwrap();

        assert!(validate_component_data(&schema, &data).is_ok());
    }

    #[test]
    fn test_missing_required_field() {
        let schema = make_test_schema();
        let data: toml::Value = toml::from_str(
            r#"
            locked = true
            "#,
        )
        .unwrap();

        assert!(matches!(
            validate_component_data(&schema, &data),
            Err(FlintError::MissingRequiredField(_))
        ));
    }

    #[test]
    fn test_invalid_enum_value() {
        let schema = make_test_schema();
        let data: toml::Value = toml::from_str(
            r#"
            style = "rotating"
            "#,
        )
        .unwrap();

        assert!(matches!(
            validate_component_data(&schema, &data),
            Err(FlintError::InvalidEnumValue { .. })
        ));
    }

    #[test]
    fn test_value_out_of_range() {
        let schema = make_test_schema();
        let data: toml::Value = toml::from_str(
            r#"
            style = "hinged"
            angle = 200.0
            "#,
        )
        .unwrap();

        assert!(matches!(
            validate_component_data(&schema, &data),
            Err(FlintError::ValueOutOfRange { .. })
        ));
    }
}
