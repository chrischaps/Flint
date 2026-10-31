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
    /// When the field is read only for some values of another field
    /// (`"x-when"` in the schema); `None` means always.
    pub when: Option<ParamCondition>,
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
                when: parse_condition(prop),
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

// ─── Conditional fields and spec linting ───────────────────────────────────

/// A field that only applies when another field holds one of some values,
/// declared in the schema as `"x-when": { "pattern": ["voronoi_brick"] }`.
///
/// JSON Schema ignores keywords it doesn't know, so validators still accept
/// the schema; the UI hides the field and [`lint_params`] reports it when the
/// condition is false.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamCondition {
    /// The controlling field (e.g. `"pattern"`).
    pub key: String,
    /// The values of `key` under which this field is read.
    pub any_of: Vec<String>,
}

fn parse_condition(prop: &Value) -> Option<ParamCondition> {
    let when = prop.get("x-when")?.as_object()?;
    let (key, values) = when.iter().next()?;
    let any_of = values
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    Some(ParamCondition {
        key: key.clone(),
        any_of,
    })
}

/// The value a string field has in `params`, or its schema default.
fn effective_string(
    schema: &Value,
    params: &toml::map::Map<String, toml::Value>,
    key: &str,
) -> Option<String> {
    if let Some(v) = params.get(key) {
        return v.as_str().map(str::to_string);
    }
    schema
        .get("properties")?
        .get(key)?
        .get("default")?
        .as_str()
        .map(str::to_string)
}

impl ParamFieldSpec {
    /// Whether this field is read, given the spec's current params. Fields
    /// without an `x-when` condition always apply.
    pub fn applies(&self, schema: &Value, params: &toml::map::Map<String, toml::Value>) -> bool {
        match &self.when {
            None => true,
            Some(c) => {
                effective_string(schema, params, &c.key).is_some_and(|v| c.any_of.contains(&v))
            }
        }
    }
}

/// Something in a spec's `[params]` that the generator will not read.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamIssue {
    /// The key is not in the schema at all (usually a typo). `suggestion` is
    /// the closest schema key, when one is close.
    Unknown {
        key: String,
        suggestion: Option<String>,
    },
    /// The key exists but only applies when `controller` is one of `needs`;
    /// the spec has `controller = actual`, so the value is ignored.
    Inapplicable {
        key: String,
        controller: String,
        actual: String,
        needs: Vec<String>,
    },
}

impl ParamIssue {
    /// The params key the issue is about.
    pub fn key(&self) -> &str {
        match self {
            Self::Unknown { key, .. } | Self::Inapplicable { key, .. } => key,
        }
    }
}

impl std::fmt::Display for ParamIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown {
                key,
                suggestion: Some(s),
            } => write!(f, "`{key}` is not a parameter of this generator (did you mean `{s}`?)"),
            Self::Unknown { key, .. } => {
                write!(f, "`{key}` is not a parameter of this generator")
            }
            Self::Inapplicable {
                key,
                controller,
                actual,
                needs,
            } => write!(
                f,
                "`{key}` is ignored: it only applies when {controller} is {}, and this spec has {controller} = \"{actual}\"",
                needs
                    .iter()
                    .map(|n| format!("\"{n}\""))
                    .collect::<Vec<_>>()
                    .join(" or ")
            ),
        }
    }
}

/// Report every key in `params` that the generator described by `schema`
/// will silently ignore: keys missing from the schema (when it sets
/// `"additionalProperties": false`) and keys whose `x-when` condition the
/// spec doesn't meet. Returned in key order; empty means every key is read.
pub fn lint_params(schema: &Value, params: &toml::Value) -> Vec<ParamIssue> {
    let Some(table) = params.as_table() else {
        return Vec::new();
    };
    let properties = schema.get("properties").and_then(|p| p.as_object());
    let closed = schema.get("additionalProperties") == Some(&Value::Bool(false));

    let mut keys: Vec<&String> = table.keys().collect();
    keys.sort();
    let mut issues = Vec::new();
    for key in keys {
        match properties.and_then(|p| p.get(key.as_str())) {
            None if closed => issues.push(ParamIssue::Unknown {
                key: key.clone(),
                suggestion: properties.and_then(|p| closest(key, p.keys())),
            }),
            None => {}
            Some(prop) => {
                let Some(c) = parse_condition(prop) else {
                    continue;
                };
                let actual = effective_string(schema, table, &c.key).unwrap_or_default();
                if !c.any_of.contains(&actual) {
                    issues.push(ParamIssue::Inapplicable {
                        key: key.clone(),
                        controller: c.key,
                        actual,
                        needs: c.any_of,
                    });
                }
            }
        }
    }
    issues
}

/// The candidate within edit distance 2 (or a third of the key's length,
/// whichever is larger) of `key`, nearest first.
fn closest<'a>(key: &str, candidates: impl Iterator<Item = &'a String>) -> Option<String> {
    let limit = (key.chars().count() / 3).max(2);
    candidates
        .map(|c| (edit_distance(key, c), c))
        .filter(|(d, _)| *d <= limit)
        .min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .map(|(_, c)| c.clone())
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != *cb);
            cur[j + 1] = sub.min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
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

    // ── x-when and lint_params ──────────────────────────────────────────────

    fn shape_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "shape": { "type": "string", "enum": ["circle", "square"], "default": "circle" },
                "radius": { "type": "number", "x-when": { "shape": ["circle"] } },
                "side": { "type": "number", "x-when": { "shape": ["square"] } },
                "color": { "type": "string" }
            },
            "additionalProperties": false
        })
    }

    fn params(src: &str) -> toml::Value {
        toml::from_str(src).unwrap()
    }

    #[test]
    fn when_is_parsed_and_follows_the_controller_or_its_default() {
        let schema = shape_schema();
        let fields = parse_param_schema(&schema);
        let side = fields.iter().find(|f| f.name == "side").unwrap();
        assert_eq!(
            side.when,
            Some(ParamCondition {
                key: "shape".into(),
                any_of: vec!["square".into()]
            })
        );
        let radius = fields.iter().find(|f| f.name == "radius").unwrap();
        let color = fields.iter().find(|f| f.name == "color").unwrap();
        let empty = toml::map::Map::new();
        // No `shape` given: the default (circle) decides.
        assert!(radius.applies(&schema, &empty));
        assert!(!side.applies(&schema, &empty));
        let square = params("shape = \"square\"");
        assert!(side.applies(&schema, square.as_table().unwrap()));
        assert!(!radius.applies(&schema, square.as_table().unwrap()));
        assert!(color.applies(&schema, square.as_table().unwrap()));
    }

    #[test]
    fn lint_is_quiet_when_every_key_is_read() {
        let p = params("shape = \"square\"\nside = 2.0\ncolor = \"red\"");
        assert!(lint_params(&shape_schema(), &p).is_empty());
    }

    #[test]
    fn lint_reports_keys_for_another_mode() {
        let issues = lint_params(&shape_schema(), &params("side = 2.0"));
        assert_eq!(
            issues,
            vec![ParamIssue::Inapplicable {
                key: "side".into(),
                controller: "shape".into(),
                actual: "circle".into(),
                needs: vec!["square".into()],
            }]
        );
        assert_eq!(
            issues[0].to_string(),
            "`side` is ignored: it only applies when shape is \"square\", and this spec has shape = \"circle\""
        );
    }

    #[test]
    fn lint_reports_typos_with_a_suggestion() {
        let issues = lint_params(&shape_schema(), &params("radus = 1.0\nwobble = 3"));
        assert_eq!(
            issues,
            vec![
                ParamIssue::Unknown {
                    key: "radus".into(),
                    suggestion: Some("radius".into())
                },
                ParamIssue::Unknown {
                    key: "wobble".into(),
                    suggestion: None
                },
            ]
        );
    }

    #[test]
    fn lint_trusts_open_schemas_with_unknown_keys() {
        let mut schema = shape_schema();
        schema["additionalProperties"] = json!(true);
        assert!(lint_params(&schema, &params("anything = 1")).is_empty());
    }
}
