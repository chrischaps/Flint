//! Schema-to-descriptor parser for procedural generator parameter UI.
//!
//! Reads a generator's `param_schema()` JSON Schema and produces a list of
//! structured field descriptors (`ParamFieldSpec`) that a UI layer can consume
//! directly — keeping JSON Schema interpretation out of the rendering code.

use serde_json::Value;

/// Describes a single parameter field for UI rendering.
#[derive(Debug, Clone)]
pub struct ParamFieldSpec {
    /// The parameter name (matches the TOML key).
    pub name: String,
    /// The UI-appropriate type with constraints.
    pub field_type: ParamFieldType,
    /// The schema-declared default value, if any.
    pub default: Option<Value>,
}

/// The widget type and constraints for a parameter field.
#[derive(Debug, Clone)]
pub enum ParamFieldType {
    /// Floating-point slider/drag.
    Float { min: Option<f64>, max: Option<f64> },
    /// Integer slider/drag.
    Integer { min: Option<i64>, max: Option<i64> },
    /// Checkbox.
    Bool,
    /// Hex color string (e.g. "#573214").
    HexColor,
    /// Dropdown with fixed choices.
    Enum { values: Vec<String> },
    /// Free-form text input.
    String,
    /// Array of strings, optionally constrained to enum choices (multi-checkbox).
    StringArray { item_enum: Option<Vec<String>> },
    /// Array of objects (e.g. bones, body_parts, limb_chains).
    ObjectArray,
}

/// Parse a generator's JSON Schema into a list of UI field descriptors.
///
/// Expects a schema of the form:
/// ```json
/// {
///   "type": "object",
///   "properties": {
///     "field_name": { "type": "number", "minimum": 0.0, "default": 5.0 },
///     ...
///   }
/// }
/// ```
pub fn parse_param_schema(schema: &Value) -> Vec<ParamFieldSpec> {
    let properties = match schema.get("properties").and_then(|p| p.as_object()) {
        Some(props) => props,
        None => return Vec::new(),
    };

    let mut fields: Vec<ParamFieldSpec> = properties
        .iter()
        .map(|(name, prop)| {
            let field_type = classify_field(prop);
            let default = prop.get("default").cloned();
            ParamFieldSpec {
                name: name.clone(),
                field_type,
                default,
            }
        })
        .collect();

    // Sort by name for stable ordering
    fields.sort_by(|a, b| a.name.cmp(&b.name));
    fields
}

/// Classify a single property schema into a `ParamFieldType`.
fn classify_field(prop: &Value) -> ParamFieldType {
    let type_str = prop.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match type_str {
        "number" => {
            let min = get_f64(prop, "minimum").or_else(|| get_f64(prop, "exclusiveMinimum"));
            let max = get_f64(prop, "maximum").or_else(|| get_f64(prop, "exclusiveMaximum"));
            ParamFieldType::Float { min, max }
        }
        "integer" => {
            let min = get_i64(prop, "minimum").or_else(|| get_i64(prop, "exclusiveMinimum"));
            let max = get_i64(prop, "maximum").or_else(|| get_i64(prop, "exclusiveMaximum"));
            ParamFieldType::Integer { min, max }
        }
        "boolean" => ParamFieldType::Bool,
        "string" => {
            // Check for enum first (dropdown)
            if let Some(values) = extract_string_enum(prop) {
                return ParamFieldType::Enum { values };
            }
            // Check for hex color pattern
            if let Some(pattern) = prop.get("pattern").and_then(|p| p.as_str()) {
                if pattern.contains("[0-9a-fA-F]") {
                    return ParamFieldType::HexColor;
                }
            }
            ParamFieldType::String
        }
        "array" => {
            // Check if items are objects (e.g. bones, body_parts, limb_chains)
            if let Some(items) = prop.get("items") {
                if items.get("type").and_then(|t| t.as_str()) == Some("object") {
                    return ParamFieldType::ObjectArray;
                }
            }
            let item_enum = prop.get("items").and_then(extract_string_enum);
            ParamFieldType::StringArray { item_enum }
        }
        _ => ParamFieldType::String,
    }
}

/// Extract an enum array of strings from a schema node.
fn extract_string_enum(prop: &Value) -> Option<Vec<String>> {
    prop.get("enum")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .filter(|v: &Vec<String>| !v.is_empty())
}

fn get_f64(prop: &Value, key: &str) -> Option<f64> {
    prop.get(key).and_then(|v| v.as_f64())
}

fn get_i64(prop: &Value, key: &str) -> Option<i64> {
    prop.get(key).and_then(|v| v.as_i64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_float_field() {
        let schema = json!({
            "type": "object",
            "properties": {
                "height": { "type": "number", "minimum": 0.01, "default": 5.0 }
            }
        });
        let fields = parse_param_schema(&schema);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "height");
        match &fields[0].field_type {
            ParamFieldType::Float { min, max } => {
                assert_eq!(*min, Some(0.01));
                assert_eq!(*max, None);
            }
            _ => panic!("expected Float"),
        }
        assert_eq!(fields[0].default, Some(json!(5.0)));
    }

    #[test]
    fn test_integer_field() {
        let schema = json!({
            "type": "object",
            "properties": {
                "segments": { "type": "integer", "minimum": 2, "default": 10 }
            }
        });
        let fields = parse_param_schema(&schema);
        assert_eq!(fields.len(), 1);
        match &fields[0].field_type {
            ParamFieldType::Integer { min, max } => {
                assert_eq!(*min, Some(2));
                assert_eq!(*max, None);
            }
            _ => panic!("expected Integer"),
        }
    }

    #[test]
    fn test_bool_field() {
        let schema = json!({
            "type": "object",
            "properties": {
                "seamless": { "type": "boolean", "default": true }
            }
        });
        let fields = parse_param_schema(&schema);
        assert_eq!(fields.len(), 1);
        assert!(matches!(fields[0].field_type, ParamFieldType::Bool));
    }

    #[test]
    fn test_hex_color_field() {
        let schema = json!({
            "type": "object",
            "properties": {
                "bark_color": {
                    "type": "string",
                    "pattern": "^#?[0-9a-fA-F]{6,8}$",
                    "default": "#573214"
                }
            }
        });
        let fields = parse_param_schema(&schema);
        assert_eq!(fields.len(), 1);
        assert!(matches!(fields[0].field_type, ParamFieldType::HexColor));
    }

    #[test]
    fn test_enum_field() {
        let schema = json!({
            "type": "object",
            "properties": {
                "method": {
                    "type": "string",
                    "enum": ["lsystem", "space_colonization"],
                    "default": "lsystem"
                }
            }
        });
        let fields = parse_param_schema(&schema);
        assert_eq!(fields.len(), 1);
        match &fields[0].field_type {
            ParamFieldType::Enum { values } => {
                assert_eq!(values, &["lsystem", "space_colonization"]);
            }
            _ => panic!("expected Enum"),
        }
    }

    #[test]
    fn test_string_array_with_enum() {
        let schema = json!({
            "type": "object",
            "properties": {
                "output_maps": {
                    "type": "array",
                    "items": { "type": "string", "enum": ["albedo", "normal", "roughness"] },
                    "default": ["albedo", "normal", "roughness"]
                }
            }
        });
        let fields = parse_param_schema(&schema);
        assert_eq!(fields.len(), 1);
        match &fields[0].field_type {
            ParamFieldType::StringArray { item_enum } => {
                assert_eq!(
                    item_enum.as_ref().unwrap(),
                    &["albedo", "normal", "roughness"]
                );
            }
            _ => panic!("expected StringArray"),
        }
    }

    #[test]
    fn test_exclusive_minimum() {
        let schema = json!({
            "type": "object",
            "properties": {
                "falloff": { "type": "number", "exclusiveMinimum": 0.0, "maximum": 1.0, "default": 0.7 }
            }
        });
        let fields = parse_param_schema(&schema);
        match &fields[0].field_type {
            ParamFieldType::Float { min, max } => {
                assert_eq!(*min, Some(0.0));
                assert_eq!(*max, Some(1.0));
            }
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn test_empty_properties() {
        let schema = json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        });
        let fields = parse_param_schema(&schema);
        assert!(fields.is_empty());
    }

    #[test]
    fn test_object_array_field() {
        let schema = json!({
            "type": "object",
            "properties": {
                "bones": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "parent": { "type": "string" },
                            "position": { "type": "array" }
                        }
                    }
                }
            }
        });
        let fields = parse_param_schema(&schema);
        assert_eq!(fields.len(), 1);
        assert!(matches!(fields[0].field_type, ParamFieldType::ObjectArray));
    }

    #[test]
    fn test_string_array_without_enum_still_works() {
        let schema = json!({
            "type": "object",
            "properties": {
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "default": ["fast", "heavy"]
                }
            }
        });
        let fields = parse_param_schema(&schema);
        assert_eq!(fields.len(), 1);
        match &fields[0].field_type {
            ParamFieldType::StringArray { item_enum } => {
                assert!(item_enum.is_none());
            }
            _ => panic!("expected StringArray"),
        }
    }

    #[test]
    fn test_fields_sorted_by_name() {
        let schema = json!({
            "type": "object",
            "properties": {
                "zebra": { "type": "number" },
                "alpha": { "type": "number" },
                "middle": { "type": "number" }
            }
        });
        let fields = parse_param_schema(&schema);
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, &["alpha", "middle", "zebra"]);
    }

    #[test]
    fn test_full_tree_schema() {
        // Approximate the tree generator schema
        let schema = json!({
            "type": "object",
            "properties": {
                "trunk_height": { "type": "number", "minimum": 0.01, "default": 5.0 },
                "trunk_radius_base": { "type": "number", "minimum": 0.01, "default": 0.3 },
                "bark_color_base": { "type": "string", "pattern": "^#?[0-9a-fA-F]{6,8}$", "default": "#573214" },
                "branch_method": { "type": "string", "enum": ["lsystem", "space_colonization"], "default": "lsystem" },
                "branch_levels": { "type": "integer", "minimum": 1, "default": 3 },
                "leaf_style": { "type": "string", "enum": ["billboard", "mesh", "none"], "default": "billboard" },
                "seamless": { "type": "boolean", "default": false }
            }
        });
        let fields = parse_param_schema(&schema);
        assert_eq!(fields.len(), 7);

        // Verify each type is correctly classified
        let by_name: std::collections::HashMap<&str, &ParamFieldSpec> =
            fields.iter().map(|f| (f.name.as_str(), f)).collect();

        assert!(matches!(
            by_name["trunk_height"].field_type,
            ParamFieldType::Float { .. }
        ));
        assert!(matches!(
            by_name["trunk_radius_base"].field_type,
            ParamFieldType::Float { .. }
        ));
        assert!(matches!(
            by_name["bark_color_base"].field_type,
            ParamFieldType::HexColor
        ));
        assert!(matches!(
            by_name["branch_method"].field_type,
            ParamFieldType::Enum { .. }
        ));
        assert!(matches!(
            by_name["branch_levels"].field_type,
            ParamFieldType::Integer { .. }
        ));
        assert!(matches!(
            by_name["leaf_style"].field_type,
            ParamFieldType::Enum { .. }
        ));
        assert!(matches!(
            by_name["seamless"].field_type,
            ParamFieldType::Bool
        ));
    }
}
