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
    /// The schema's `description`: what the parameter does, in a sentence.
    /// The previewer shows it on hover.
    pub description: Option<String>,
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
    /// A table (`"type": "object"`), usually of named tables, such as a
    /// creature's `[params.materials.<name>]`. Shown, not edited.
    ObjectMap,
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
                description: prop
                    .get("description")
                    .and_then(|d| d.as_str())
                    .map(str::to_string),
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
        "object" => ParamFieldType::ObjectMap,
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
/// the schema; the UI hides the field and [`crate::validate_params`] reports it when the
/// condition is false.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamCondition {
    /// The controlling field (e.g. `"pattern"`).
    pub key: String,
    /// The values of `key` under which this field is read.
    pub any_of: Vec<String>,
}

pub(crate) fn parse_condition(prop: &Value) -> Option<ParamCondition> {
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
pub(crate) fn effective_string(
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

// ─── Parameter reference ───────────────────────────────────────────────────

/// A Markdown reference for one generator's `params`, built from its
/// schema: a table per condition (keys every spec may set first, then one
/// table per `x-when`), each row the key, its type and range, its default
/// and its `description`. The crate README's catalog is this, for every
/// built-in generator, and a test keeps the two equal.
pub fn param_reference_markdown(schema: &Value) -> String {
    let Some(props) = schema.get("properties").and_then(|p| p.as_object()) else {
        return String::new();
    };
    // Group by condition; BTreeMap keeps the groups and their keys in order.
    let mut groups: std::collections::BTreeMap<String, Vec<(&String, &Value)>> = Default::default();
    for (key, prop) in props {
        let heading = match parse_condition(prop) {
            None => String::new(),
            Some(c) => format!(
                "Only when `{}` is {}",
                c.key,
                c.any_of
                    .iter()
                    .map(|v| format!("`{v}`"))
                    .collect::<Vec<_>>()
                    .join(" or ")
            ),
        };
        groups.entry(heading).or_default().push((key, prop));
    }

    let mut out = String::new();
    for (heading, rows) in &groups {
        if !heading.is_empty() {
            out.push_str(&format!("\n{heading}:\n\n"));
        }
        out.push_str("| Key | Type | Default | What it does |\n|---|---|---|---|\n");
        for (key, prop) in rows {
            let mut text = prop
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            if prop.get("deprecated") == Some(&Value::Bool(true)) {
                text = format!("**Deprecated.** {text}");
            }
            out.push_str(&format!(
                "| `{key}` | {} | {} | {} |\n",
                type_cell(prop),
                prop.get("default").map(default_cell).unwrap_or_default(),
                text.replace('|', "\\|")
            ));
        }
    }
    out
}

/// [`param_reference_markdown`] as plain text, for a terminal (`flint gen
/// --describe`): the same groups in the same order, a line a key with its
/// type and default, its description indented under it.
pub fn param_reference_text(schema: &Value) -> String {
    let Some(props) = schema.get("properties").and_then(|p| p.as_object()) else {
        return String::new();
    };
    let plain = |s: String| s.replace('`', "").replace("\\|", "|");
    let mut groups: std::collections::BTreeMap<String, Vec<(&String, &Value)>> = Default::default();
    for (key, prop) in props {
        let heading = match parse_condition(prop) {
            None => String::new(),
            Some(c) => format!(
                "Only when {} is {}",
                c.key,
                c.any_of
                    .iter()
                    .map(|v| format!("\"{v}\""))
                    .collect::<Vec<_>>()
                    .join(" or ")
            ),
        };
        groups.entry(heading).or_default().push((key, prop));
    }
    let width = props.keys().map(|k| k.chars().count()).max().unwrap_or(0);
    let mut out = String::new();
    for (heading, rows) in &groups {
        if !heading.is_empty() {
            out.push_str(&format!("\n{heading}:\n"));
        }
        for (key, prop) in rows {
            let mut line = format!("  {key:<width$}  {}", plain(type_cell(prop)));
            if let Some(d) = prop.get("default") {
                line.push_str(&format!(", default {}", plain(default_cell(d))));
            }
            if prop.get("deprecated") == Some(&Value::Bool(true)) {
                line.push_str(" (deprecated)");
            }
            out.push_str(line.trim_end());
            out.push('\n');
            if let Some(text) = prop.get("description").and_then(|d| d.as_str()) {
                out.push_str(&format!("  {:width$}    {text}\n", ""));
            }
        }
    }
    out
}

/// "number ≥ 0", "integer 1–8", "`a` \| `b`", "hex colour" and so on.
fn type_cell(prop: &Value) -> String {
    // A pipeline op's `type`: the one value it may have
    if let Some(c) = prop.get("const") {
        return format!("`{c}`");
    }
    let num = |k: &str| prop.get(k).and_then(|v| v.as_f64());
    let fmt = |v: f64| {
        if v.fract() == 0.0 {
            format!("{v:.0}")
        } else {
            format!("{v}")
        }
    };
    let range = || match (num("minimum"), num("exclusiveMinimum"), num("maximum")) {
        (Some(lo), _, Some(hi)) => format!(" {}–{}", fmt(lo), fmt(hi)),
        (_, Some(lo), Some(hi)) => format!(" >{} to {}", fmt(lo), fmt(hi)),
        (Some(lo), _, None) => format!(" ≥ {}", fmt(lo)),
        (_, Some(lo), None) => format!(" > {}", fmt(lo)),
        (None, None, Some(hi)) => format!(" ≤ {}", fmt(hi)),
        _ => String::new(),
    };
    match classify_field(prop) {
        ParamFieldType::Float { .. } => format!("number{}", range()),
        ParamFieldType::Integer { .. } => format!("integer{}", range()),
        ParamFieldType::Bool => "bool".into(),
        ParamFieldType::HexColor => "hex colour".into(),
        ParamFieldType::Enum { values } => values
            .iter()
            .map(|v| format!("`{v}`"))
            .collect::<Vec<_>>()
            .join(" \\| "),
        ParamFieldType::String => "string".into(),
        ParamFieldType::StringArray {
            item_enum: Some(values),
        } => format!(
            "list of {}",
            values
                .iter()
                .map(|v| format!("`{v}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        // The items' own type, when they say one (#750: a list of numbers,
        // or of [right, up] points, read "list of strings")
        ParamFieldType::StringArray { item_enum: None } => {
            let items = prop.get("items");
            fn ty(v: Option<&Value>) -> Option<&str> {
                v.and_then(|i| i.get("type")).and_then(|t| t.as_str())
            }
            match ty(items) {
                Some("number") | Some("integer") => "list of numbers".into(),
                Some("array") => match ty(items.and_then(|i| i.get("items"))) {
                    Some("number") | Some("integer") => "list of number lists".into(),
                    _ => "list of lists".into(),
                },
                _ => "list of strings".into(),
            }
        }
        ParamFieldType::ObjectArray => "array of tables".into(),
        ParamFieldType::ObjectMap => "table".into(),
    }
}

fn default_cell(v: &Value) -> String {
    match v {
        Value::String(s) => format!("`\"{s}\"`"),
        Value::Array(items) => format!(
            "`[{}]`",
            items
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => format!("`{other}`"),
    }
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

    // ── x-when ──────────────────────────────────────────────────────────────

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

    // ── nested tables and arrays ────────────────────────────────────────────

    fn nested_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "materials": {
                    "type": "object",
                    "additionalProperties": {
                        "type": "object",
                        "title": "material",
                        "properties": { "roughness": { "type": "number" } },
                        "additionalProperties": false
                    }
                },
                "ops": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "discriminator": { "propertyName": "type" },
                        "oneOf": [
                            {
                                "title": "grid op",
                                "properties": {
                                    "type": { "const": "grid" },
                                    "columns": { "type": "integer" },
                                    "stops": {
                                        "type": "array",
                                        "items": {
                                            "title": "stop",
                                            "properties": { "position": { "type": "number" } },
                                            "additionalProperties": false
                                        }
                                    }
                                },
                                "additionalProperties": false
                            },
                            {
                                "title": "open op",
                                "properties": { "type": { "const": "open" } }
                            }
                        ]
                    }
                }
            }
        })
    }

    #[test]
    fn object_properties_are_maps_not_strings() {
        let fields = parse_param_schema(&nested_schema());
        let materials = fields.iter().find(|f| f.name == "materials").unwrap();
        assert!(matches!(materials.field_type, ParamFieldType::ObjectMap));
    }
}
