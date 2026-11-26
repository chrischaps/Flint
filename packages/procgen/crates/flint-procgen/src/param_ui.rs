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

/// A table's keys grouped by condition, as the references print them: the
/// keys every spec may set first (the empty heading), then one group per
/// `x-when`; `BTreeMap` keeps the groups and their keys in order.
fn condition_groups(
    props: &serde_json::Map<String, Value>,
    markdown: bool,
) -> std::collections::BTreeMap<String, Vec<(&String, &Value)>> {
    let mut groups: std::collections::BTreeMap<String, Vec<(&String, &Value)>> = Default::default();
    for (key, prop) in props {
        let heading = match parse_condition(prop) {
            None => String::new(),
            Some(c) if markdown => format!(
                "Only when `{}` is {}",
                c.key,
                c.any_of
                    .iter()
                    .map(|v| format!("`{v}`"))
                    .collect::<Vec<_>>()
                    .join(" or ")
            ),
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
    groups
}

/// A key whose value is a table the schema names the keys of (#1173),
/// found the way [`crate::validate_params`] walks: the key's own
/// `properties`, the `items` of an array of tables, or the
/// `additionalProperties` of a table of named tables. A tagged array
/// (`items` a `oneOf`, a pipeline's `ops`) isn't one: each branch has its
/// own reference (`op_schema`).
enum Nested<'a> {
    /// `[params.key]`: one table
    Table(&'a Value),
    /// `[[params.key]]`: an array of tables
    Entries(&'a Value),
    /// `[params.key.<name>]`: a table of named tables
    Named(&'a Value),
}

fn nested(prop: &Value) -> Option<Nested<'_>> {
    let has_keys = |s: &Value| {
        s.get("properties")
            .and_then(|p| p.as_object())
            .is_some_and(|p| !p.is_empty())
    };
    if has_keys(prop) {
        Some(Nested::Table(prop))
    } else if let Some(items) = prop.get("items").filter(|i| has_keys(i)) {
        Some(Nested::Entries(items))
    } else {
        prop.get("additionalProperties")
            .filter(|e| has_keys(e))
            .map(Nested::Named)
    }
}

/// Whether `schema`'s `required` names `key`.
fn is_required(schema: &Value, key: &str) -> bool {
    schema
        .get("required")
        .and_then(|r| r.as_array())
        .is_some_and(|r| r.iter().any(|k| k.as_str() == Some(key)))
}

/// A Markdown reference for one generator's `params`, built from its
/// schema: a table per condition (keys every spec may set first, then one
/// table per `x-when`), each row the key, its type and range, its default
/// (or `required`) and its `description`. A key holding tables whose keys
/// the schema names has those under its group, in a table of their own
/// headed with the TOML that writes one (`Each [[params.bones]] entry:`),
/// and so on down. The crate README's catalog is this, for every built-in
/// generator, and a test keeps the two equal.
pub fn param_reference_markdown(schema: &Value) -> String {
    param_reference_markdown_in(schema, "params")
}

/// [`param_reference_markdown`] for a table that sits at `table` in a spec
/// (`params.ops` for a pipeline op's keys): the dotted path its nested
/// tables' headings start from.
pub fn param_reference_markdown_in(schema: &Value, table: &str) -> String {
    let Some(props) = schema.get("properties").and_then(|p| p.as_object()) else {
        return String::new();
    };
    let mut out = String::new();
    for (heading, rows) in &condition_groups(props, true) {
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
            let default = match prop.get("default") {
                Some(d) => default_cell(d),
                None if is_required(schema, key) => "required".into(),
                None => String::new(),
            };
            out.push_str(&format!(
                "| `{key}` | {} | {default} | {} |\n",
                type_cell(prop),
                text.replace('|', "\\|")
            ));
        }
        for (key, prop) in rows {
            let (path, title, inner) = match nested(prop) {
                None => continue,
                Some(Nested::Table(s)) => {
                    let path = format!("{table}.{key}");
                    let title = format!("The `[{path}]` table");
                    (path, title, s)
                }
                Some(Nested::Entries(s)) => {
                    let path = format!("{table}.{key}");
                    let title = format!("Each `[[{path}]]` entry");
                    (path, title, s)
                }
                Some(Nested::Named(s)) => {
                    let path = format!("{table}.{key}.<name>");
                    let title = format!("Each `[{path}]` table");
                    (path, title, s)
                }
            };
            out.push_str(&format!("\n{title}:\n\n"));
            out.push_str(&param_reference_markdown_in(inner, &path));
        }
    }
    out
}

/// [`param_reference_markdown`] as plain text, for a terminal (`flint gen
/// --describe`): the same groups in the same order, a line a key with its
/// type and default, its description indented under it. A nested table's
/// keys follow its key, indented, each named by its path from the top
/// (`limb_chains[].segments[].length`, `materials.<name>.roughness`).
pub fn param_reference_text(schema: &Value) -> String {
    let mut out = String::new();
    reference_text(schema, "", 2, &mut out);
    out
}

fn reference_text(schema: &Value, prefix: &str, indent: usize, out: &mut String) {
    let Some(props) = schema.get("properties").and_then(|p| p.as_object()) else {
        return;
    };
    let plain = |s: String| s.replace('`', "").replace("\\|", "|");
    let width = props
        .keys()
        .map(|k| prefix.chars().count() + k.chars().count())
        .max()
        .unwrap_or(0);
    let pad = "";
    for (heading, rows) in &condition_groups(props, false) {
        if !heading.is_empty() {
            out.push_str(&format!("\n{pad:outer$}{heading}:\n", outer = indent - 2));
        }
        for (key, prop) in rows {
            let name = format!("{prefix}{key}");
            let mut line = format!("{pad:indent$}{name:<width$}  {}", plain(type_cell(prop)));
            if let Some(d) = prop.get("default") {
                line.push_str(&format!(", default {}", plain(default_cell(d))));
            } else if is_required(schema, key) {
                line.push_str(", required");
            }
            if prop.get("deprecated") == Some(&Value::Bool(true)) {
                line.push_str(" (deprecated)");
            }
            out.push_str(line.trim_end());
            out.push('\n');
            if let Some(text) = prop.get("description").and_then(|d| d.as_str()) {
                out.push_str(&format!("{pad:indent$}{pad:width$}    {text}\n"));
            }
            let (inner_prefix, inner) = match nested(prop) {
                None => continue,
                Some(Nested::Table(s)) => (format!("{name}."), s),
                Some(Nested::Entries(s)) => (format!("{name}[]."), s),
                Some(Nested::Named(s)) => (format!("{name}.<name>."), s),
            };
            reference_text(inner, &inner_prefix, indent + 2, out);
        }
    }
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

    /// A creature-shaped schema: an array of tables holding an array of
    /// tables, and a table of named tables (#1173).
    fn creature_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "limb_chains": {
                    "type": "array",
                    "description": "Chains",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "Names its bones" },
                            "segments": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": { "length": { "type": "number", "minimum": 0.0 } },
                                    "required": ["length"]
                                }
                            }
                        },
                        "required": ["name", "segments"]
                    }
                },
                "materials": {
                    "type": "object",
                    "additionalProperties": {
                        "type": "object",
                        "properties": { "roughness": { "type": "number", "default": 0.5 } }
                    }
                }
            }
        })
    }

    #[test]
    fn the_text_reference_names_nested_keys_by_their_path() {
        let text = param_reference_text(&creature_schema());
        let lines: Vec<&str> = text.lines().collect();
        let at = |needle: &str| {
            lines
                .iter()
                .position(|l| l.trim_start().starts_with(needle))
                .unwrap_or_else(|| panic!("no line for {needle} in\n{text}"))
        };
        // A line's indent, and its words with the column padding gone
        let row = |name: &str| {
            let line = lines[at(name)];
            let indent = line.len() - line.trim_start().len();
            (
                indent,
                line.split_whitespace().collect::<Vec<_>>().join(" "),
            )
        };
        // Each under its parent, indented a step deeper, with its type
        assert!(at("limb_chains ") < at("limb_chains[].name"));
        assert!(at("limb_chains[].segments ") < at("limb_chains[].segments[].length"));
        assert_eq!(
            row("limb_chains[].name"),
            (4, "limb_chains[].name string, required".into())
        );
        assert_eq!(
            row("limb_chains[].segments[].length"),
            (
                6,
                "limb_chains[].segments[].length number ≥ 0, required".into()
            )
        );
        assert_eq!(
            row("materials.<name>.roughness"),
            (4, "materials.<name>.roughness number, default 0.5".into())
        );
        // A nested key's description sits under it, past its name
        assert_eq!(
            lines[at("limb_chains[].name") + 1].trim_start(),
            "Names its bones"
        );
    }

    #[test]
    fn the_markdown_reference_gives_a_nested_table_its_own() {
        let md = param_reference_markdown(&creature_schema());
        for heading in [
            "\nEach `[[params.limb_chains]]` entry:\n\n| Key |",
            "\nEach `[[params.limb_chains.segments]]` entry:\n\n| Key |",
            "\nEach `[params.materials.<name>]` table:\n\n| Key |",
        ] {
            assert!(md.contains(heading), "no {heading:?} in\n{md}");
        }
        assert!(md.contains("| `segments` | array of tables | required |"));
        assert!(md.contains("| `length` | number ≥ 0 | required |"));
        assert!(md.contains("| `roughness` | number | `0.5` |"));
        // Under a pipeline op's table, its entries are `params.ops.<key>`
        let op = param_reference_markdown_in(&creature_schema(), "params.ops");
        assert!(op.contains("Each `[[params.ops.limb_chains.segments]]` entry:"));
    }

    #[test]
    fn a_tagged_array_is_left_to_its_own_reference() {
        // `ops` is a oneOf: its branches are `op_schema`'s, not repeated here
        let md = param_reference_markdown(&nested_schema());
        assert!(!md.contains("[[params.ops]]"), "{md}");
        assert!(md.contains("Each `[params.materials.<name>]` table:"));
    }
}
