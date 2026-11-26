//! Checking a spec's `params` against its generator's parameter schema: the
//! one validator for specs written by hand and specs written by an agent
//! (#718).
//!
//! Every [`crate::Generator`] publishes `param_schema()`, a small subset of
//! JSON Schema: `type`, `minimum` / `exclusiveMinimum`, `maximum` /
//! `exclusiveMaximum`, `enum`, `const`, a hex colour `pattern`, `required`,
//! `additionalProperties` (`false` closes a table, a schema describes a table
//! of named tables), `items` (with a `oneOf` + `discriminator` for a tagged
//! array, such as a texture pipeline's `ops`), `deprecated`, and Flint's own
//! `x-when` (a key read only for some values of another).
//!
//! [`validate_params`] reads all of it, at every depth, and returns every
//! problem as a [`ParamIssue`] whose key is a path (`ops[2].columns`,
//! `materials.shell.roughness`). Two kinds of problem come back:
//!
//! - a key the generator will **ignore** (unknown, for another mode, or
//!   deprecated): the spec still generates, without it;
//! - a value the generator **can't use** as written (wrong type, out of
//!   range, not one of the choices, not a colour, a required key missing):
//!   [`ParamIssue::is_error`].
//!
//! `flint gen` checks a spec with it before generating, `flint validate` and
//! `flint check` check the specs a scene names, the runtime resolver warns,
//! and the AI layer repairs what it reports (`flint_procgen_ai::repair_params`).

use std::fmt;

use flint_core::toml_util::edit_distance;
use serde_json::Value;

use crate::param_ui::{effective_string, parse_condition};

// ─── Ranges ────────────────────────────────────────────────────────────────

/// A numeric range from a schema property, each end inclusive or exclusive.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NumBounds {
    /// Lower bound and whether it is exclusive.
    pub min: Option<(f64, bool)>,
    /// Upper bound and whether it is exclusive.
    pub max: Option<(f64, bool)>,
}

impl NumBounds {
    /// Read the bounds of one schema property.
    pub fn of(prop: &Value) -> Self {
        let get = |k: &str| prop.get(k).and_then(Value::as_f64);
        Self {
            min: get("minimum")
                .map(|v| (v, false))
                .or_else(|| get("exclusiveMinimum").map(|v| (v, true))),
            max: get("maximum")
                .map(|v| (v, false))
                .or_else(|| get("exclusiveMaximum").map(|v| (v, true))),
        }
    }

    /// Whether the property has a bound at all.
    pub fn is_bounded(&self) -> bool {
        self.min.is_some() || self.max.is_some()
    }

    /// Whether `v` lies inside the range.
    pub fn contains(&self, v: f64) -> bool {
        let lo = match self.min {
            Some((m, true)) => v > m,
            Some((m, false)) => v >= m,
            None => true,
        };
        let hi = match self.max {
            Some((m, true)) => v < m,
            Some((m, false)) => v <= m,
            None => true,
        };
        lo && hi
    }

    /// The nearest value inside the range. An exclusive end is stepped
    /// inside by 1% of its magnitude (at least 0.001).
    pub fn clamp(&self, v: f64) -> f64 {
        let nudge = |m: f64| (m.abs() * 0.01).max(0.001);
        let mut out = v;
        if let Some((m, excl)) = self.min {
            if out < m || (excl && out <= m) {
                out = if excl { m + nudge(m) } else { m };
            }
        }
        if let Some((m, excl)) = self.max {
            if out > m || (excl && out >= m) {
                out = if excl { m - nudge(m) } else { m };
            }
        }
        out
    }
}

/// Interval notation: `[0.5, 30]`, `(0, 1]`, `[1, inf)`.
impl fmt::Display for NumBounds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.min {
            Some((m, true)) => write!(f, "({}", number(m))?,
            Some((m, false)) => write!(f, "[{}", number(m))?,
            None => write!(f, "(-inf")?,
        }
        match self.max {
            Some((m, true)) => write!(f, ", {})", number(m)),
            Some((m, false)) => write!(f, ", {}]", number(m)),
            None => write!(f, ", inf)"),
        }
    }
}

/// A number as a person writes it: `3`, not `3.0`; three decimals at most.
fn number(v: f64) -> String {
    let v = (v * 1000.0).round() / 1000.0;
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

// ─── Issues ────────────────────────────────────────────────────────────────

/// One problem in a spec's `[params]`. Every variant's `key` is the
/// parameter's path: `trunk_height`, `ops[2].columns`,
/// `materials.shell.roughness`.
#[derive(Debug, Clone, PartialEq)]
pub enum ParamIssue {
    /// The key is not in the schema at all (usually a typo), so the
    /// generator ignores it. `suggestion` is the closest schema key, when
    /// one is close. `within` names the table the key sits in (its schema's
    /// `title`, e.g. "brick_grid op") when that isn't the generator's own
    /// params.
    Unknown {
        key: String,
        suggestion: Option<String>,
        within: Option<String>,
    },
    /// The key exists but only applies when `controller` is one of `needs`;
    /// the spec has `controller = actual`, so the value is ignored.
    Inapplicable {
        key: String,
        controller: String,
        actual: String,
        needs: Vec<String>,
    },
    /// The schema marks the key `"deprecated": true`: it is read (or once
    /// was) but no longer does anything. `reason` is the key's description.
    Deprecated { key: String, reason: Option<String> },
    /// The value has the wrong TOML type. `expected` is the schema's type
    /// (`number`, `integer`, `string`, ...), `found` the value's, and
    /// `bounds` the range a number must be in, when the schema gives one.
    WrongType {
        key: String,
        expected: String,
        found: String,
        bounds: Option<NumBounds>,
    },
    /// A number outside the schema's range.
    OutOfRange {
        key: String,
        value: f64,
        bounds: NumBounds,
    },
    /// A value that isn't one of the schema's choices (its `enum`, its
    /// `const`, or for a tagged array the tags there are). `suggestion` is
    /// the closest choice, when one is close.
    NotInEnum {
        key: String,
        value: String,
        allowed: Vec<String>,
        suggestion: Option<String>,
    },
    /// A string that should be a `#RRGGBB` / `#RRGGBBAA` colour.
    BadColor { key: String, value: String },
    /// A key the schema requires is absent. `within` names the table, as
    /// for [`ParamIssue::Unknown`].
    MissingRequired { key: String, within: Option<String> },
}

impl ParamIssue {
    /// The params key the issue is about: a path such as `ops[2].colums`
    /// when it sits inside a table or an array of them.
    pub fn key(&self) -> &str {
        match self {
            Self::Unknown { key, .. }
            | Self::Inapplicable { key, .. }
            | Self::Deprecated { key, .. }
            | Self::WrongType { key, .. }
            | Self::OutOfRange { key, .. }
            | Self::NotInEnum { key, .. }
            | Self::BadColor { key, .. }
            | Self::MissingRequired { key, .. } => key,
        }
    }

    /// Whether the value can't be used as written. The other issues are
    /// keys the generator ignores: the spec generates without them.
    pub fn is_error(&self) -> bool {
        !self.is_ignored_key()
    }

    /// Whether the issue is a key the generator won't read (unknown, for
    /// another mode, or deprecated).
    pub fn is_ignored_key(&self) -> bool {
        matches!(
            self,
            Self::Unknown { .. } | Self::Inapplicable { .. } | Self::Deprecated { .. }
        )
    }

    /// A short kebab-case name for the kind of issue, for JSON.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unknown { .. } => "unknown-param",
            Self::Inapplicable { .. } => "inapplicable-param",
            Self::Deprecated { .. } => "deprecated-param",
            Self::WrongType { .. } => "wrong-type",
            Self::OutOfRange { .. } => "out-of-range",
            Self::NotInEnum { .. } => "not-in-enum",
            Self::BadColor { .. } => "bad-color",
            Self::MissingRequired { .. } => "missing-required",
        }
    }
}

impl fmt::Display for ParamIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown {
                key,
                suggestion,
                within,
            } => {
                match within {
                    Some(w) => write!(f, "`{key}` is not a parameter of the {w}")?,
                    None => write!(f, "`{key}` is not a parameter of this generator")?,
                }
                match suggestion {
                    Some(s) => write!(f, " (did you mean `{s}`?)"),
                    None => Ok(()),
                }
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
            Self::Deprecated {
                key,
                reason: Some(r),
            } => write!(f, "`{key}` is deprecated: {r}"),
            Self::Deprecated { key, .. } => write!(f, "`{key}` is deprecated"),
            Self::WrongType {
                key,
                expected,
                found,
                bounds,
            } => {
                write!(f, "`{key}` should be {}", a(expected))?;
                if let Some(b) = bounds {
                    write!(f, " in the range {b}")?;
                }
                write!(f, ", not {}", a(found))
            }
            Self::OutOfRange { key, value, bounds } => write!(
                f,
                "`{key}` = {} is outside the allowed range {bounds}; the nearest allowed value is {}",
                number(*value),
                number(bounds.clamp(*value))
            ),
            Self::NotInEnum {
                key,
                value,
                allowed,
                suggestion,
            } => {
                write!(
                    f,
                    "`{key}` = \"{value}\" is not one of: {}",
                    allowed.join(", ")
                )?;
                match suggestion {
                    Some(s) => write!(f, " (did you mean \"{s}\"?)"),
                    None => Ok(()),
                }
            }
            Self::BadColor { key, value } => {
                write!(f, "`{key}` = \"{value}\" is not a #RRGGBB colour")
            }
            Self::MissingRequired { key, within: None } => write!(f, "`{key}` is required"),
            Self::MissingRequired {
                key,
                within: Some(w),
            } => write!(f, "`{key}` is required in every {w}"),
        }
    }
}

/// "a number", "an integer", "a table".
fn a(noun: &str) -> String {
    let article = if noun.starts_with(['a', 'e', 'i', 'o', 'u']) {
        "an"
    } else {
        "a"
    };
    format!("{article} {noun}")
}

/// What a TOML value is, in the schema's words.
fn type_of(value: &toml::Value) -> &'static str {
    match value {
        toml::Value::String(_) => "string",
        toml::Value::Integer(_) => "integer",
        toml::Value::Float(_) => "number",
        toml::Value::Boolean(_) => "boolean",
        toml::Value::Datetime(_) => "date",
        toml::Value::Array(_) => "array",
        toml::Value::Table(_) => "table",
    }
}

// ─── The walk ──────────────────────────────────────────────────────────────

/// Every problem in `params` against the generator `schema`, at every depth,
/// each table's keys in name order: keys the generator will ignore (missing
/// from a closed schema, for a mode the spec isn't in, deprecated) and
/// values it can't use (wrong type, out of range, not a choice, not a
/// colour, a required key missing). Empty means every key is read and every
/// value fits.
///
/// Tables inside the params are checked against their own schemas, the
/// issue's key then being a path (`materials.shell.rougness`, `ops[2].colums`):
/// - a property with `"properties"` is checked like the params themselves;
/// - a table of named tables (`"additionalProperties": { ... }`) checks
///   every entry against that schema;
/// - an array checks each element against `"items"`, and when `items` is a
///   `"oneOf"` with a `"discriminator": { "propertyName": "type" }`, against
///   the branch whose `type` property has the element's value as `const`
///   (a tag no branch has is a [`ParamIssue::NotInEnum`]).
///
/// Nothing inside a key the generator ignores, or a value of the wrong
/// type, is checked: it isn't read.
pub fn validate_params(schema: &Value, params: &toml::Value) -> Vec<ParamIssue> {
    let mut issues = Vec::new();
    walk(schema, params, "", &mut issues);
    issues
}

/// Check `value` against `schema`, prefixing issue keys with `path`.
fn walk(schema: &Value, value: &toml::Value, path: &str, issues: &mut Vec<ParamIssue>) {
    // The params themselves are a table whatever the schema says
    if !path.is_empty() {
        if let Some(issue) = check_value(schema, value, path) {
            let stop = matches!(issue, ParamIssue::WrongType { .. });
            issues.push(issue);
            if stop {
                return;
            }
        }
    }
    match value {
        toml::Value::Table(table) => {
            if schema.get("properties").is_some() {
                check_table(schema, table, path, issues);
            } else if let Some(entry) = schema.get("additionalProperties").filter(|e| e.is_object())
            {
                let mut names: Vec<&String> = table.keys().collect();
                names.sort();
                for name in names {
                    walk(entry, &table[name.as_str()], &join(path, name), issues);
                }
            }
        }
        toml::Value::Array(items) => {
            let Some(item_schema) = schema.get("items") else {
                return;
            };
            for (i, item) in items.iter().enumerate() {
                let at = format!("{path}[{i}]");
                match branch(item_schema, item, &at) {
                    Ok(b) => walk(b, item, &at, issues),
                    Err(Some(issue)) => issues.push(issue),
                    Err(None) => {}
                }
            }
        }
        _ => {}
    }
}

/// The schema an array element is checked against: `items` itself, or for
/// a discriminated `oneOf` the branch matching the element's tag. `Err`
/// when the element has no branch: the issue that says why (no tag, a tag
/// no branch has), or `None` when the schema can't say.
fn branch<'a>(
    items: &'a Value,
    item: &toml::Value,
    at: &str,
) -> Result<&'a Value, Option<ParamIssue>> {
    let Some(branches) = items.get("oneOf").and_then(|b| b.as_array()) else {
        return Ok(items);
    };
    let Some(tag) = items
        .get("discriminator")
        .and_then(|d| d.get("propertyName"))
        .and_then(|t| t.as_str())
    else {
        return Err(None);
    };
    let Some(table) = item.as_table() else {
        return Err(Some(ParamIssue::WrongType {
            key: at.to_string(),
            expected: "table".into(),
            found: type_of(item).into(),
            bounds: None,
        }));
    };
    let tags: Vec<String> = branches
        .iter()
        .filter_map(|b| b.get("properties")?.get(tag)?.get("const")?.as_str())
        .map(str::to_string)
        .collect();
    let key = join(at, tag);
    let wanted = match table.get(tag) {
        None => return Err(Some(ParamIssue::MissingRequired { key, within: None })),
        Some(toml::Value::String(s)) => s,
        Some(other) => {
            return Err(Some(ParamIssue::WrongType {
                key,
                expected: "string".into(),
                found: type_of(other).into(),
                bounds: None,
            }))
        }
    };
    branches
        .iter()
        .find(|b| {
            b.get("properties")
                .and_then(|p| p.get(tag))
                .and_then(|t| t.get("const"))
                .and_then(|c| c.as_str())
                == Some(wanted.as_str())
        })
        .ok_or_else(|| {
            Some(ParamIssue::NotInEnum {
                key,
                value: wanted.clone(),
                suggestion: closest(wanted, tags.iter()),
                allowed: tags,
            })
        })
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

fn check_table(
    schema: &Value,
    table: &toml::map::Map<String, toml::Value>,
    path: &str,
    issues: &mut Vec<ParamIssue>,
) {
    let properties = schema.get("properties").and_then(|p| p.as_object());
    let closed = schema.get("additionalProperties") == Some(&Value::Bool(false));
    let within = if path.is_empty() {
        None
    } else {
        schema
            .get("title")
            .and_then(|t| t.as_str())
            .map(str::to_string)
    };

    let mut keys: Vec<&String> = table.keys().collect();
    keys.sort();
    for key in keys {
        match properties.and_then(|p| p.get(key.as_str())) {
            None if closed => issues.push(ParamIssue::Unknown {
                key: join(path, key),
                suggestion: properties.and_then(|p| closest(key, p.keys())),
                within: within.clone(),
            }),
            None => {}
            Some(prop) => {
                if prop.get("deprecated") == Some(&Value::Bool(true)) {
                    issues.push(ParamIssue::Deprecated {
                        key: join(path, key),
                        reason: prop
                            .get("description")
                            .and_then(|d| d.as_str())
                            .map(str::to_string),
                    });
                    continue;
                }
                if let Some(c) = parse_condition(prop) {
                    let actual = effective_string(schema, table, &c.key).unwrap_or_default();
                    if !c.any_of.contains(&actual) {
                        issues.push(ParamIssue::Inapplicable {
                            key: join(path, key),
                            controller: c.key,
                            actual,
                            needs: c.any_of,
                        });
                        // Nothing inside an ignored key is read either.
                        continue;
                    }
                }
                walk(prop, &table[key.as_str()], &join(path, key), issues);
            }
        }
    }
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        for r in required.iter().filter_map(|r| r.as_str()) {
            if !table.contains_key(r) {
                issues.push(ParamIssue::MissingRequired {
                    key: join(path, r),
                    within: within.clone(),
                });
            }
        }
    }
}

/// The schema `type` of one property (`""` when absent).
pub fn prop_type(prop: &Value) -> &str {
    prop.get("type").and_then(Value::as_str).unwrap_or("")
}

/// The string choices of a property's `enum`, when it has some.
pub fn string_enum(prop: &Value) -> Option<Vec<String>> {
    let values: Vec<String> = prop
        .get("enum")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    (!values.is_empty()).then_some(values)
}

fn is_hex_color(s: &str) -> bool {
    let h = s.strip_prefix('#').unwrap_or(s);
    (h.len() == 6 || h.len() == 8) && h.chars().all(|c| c.is_ascii_hexdigit())
}

fn color_pattern(prop: &Value) -> bool {
    prop.get("pattern")
        .and_then(Value::as_str)
        .is_some_and(|p| p.contains("[0-9a-fA-F]"))
}

/// The one problem with a single value, judged by its own property alone.
fn check_value(prop: &Value, value: &toml::Value, key: &str) -> Option<ParamIssue> {
    let wrong = |expected: &str| {
        let bounds = NumBounds::of(prop);
        Some(ParamIssue::WrongType {
            key: key.to_string(),
            expected: expected.to_string(),
            found: type_of(value).to_string(),
            bounds: bounds.is_bounded().then_some(bounds),
        })
    };
    let range = |v: f64| {
        let bounds = NumBounds::of(prop);
        (!bounds.contains(v)).then(|| ParamIssue::OutOfRange {
            key: key.to_string(),
            value: v,
            bounds,
        })
    };
    if let Some(c) = prop.get("const").and_then(Value::as_str) {
        if value.as_str() != Some(c) {
            return Some(ParamIssue::NotInEnum {
                key: key.to_string(),
                value: value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_string),
                allowed: vec![c.to_string()],
                suggestion: None,
            });
        }
    }
    match prop_type(prop) {
        "number" => match value {
            toml::Value::Float(f) => range(*f),
            toml::Value::Integer(i) => range(*i as f64),
            _ => wrong("number"),
        },
        "integer" => match value {
            toml::Value::Integer(i) => range(*i as f64),
            _ => wrong("integer"),
        },
        "boolean" if !value.is_bool() => wrong("boolean"),
        "string" => {
            let Some(s) = value.as_str() else {
                return wrong("string");
            };
            if let Some(allowed) = string_enum(prop) {
                if !allowed.iter().any(|a| a == s) {
                    return Some(ParamIssue::NotInEnum {
                        key: key.to_string(),
                        value: s.to_string(),
                        suggestion: closest(s, allowed.iter()),
                        allowed,
                    });
                }
            }
            if color_pattern(prop) && !is_hex_color(s) {
                return Some(ParamIssue::BadColor {
                    key: key.to_string(),
                    value: s.to_string(),
                });
            }
            None
        }
        "array" if !value.is_array() => wrong("array"),
        "object" if !value.is_table() => wrong("table"),
        _ => None,
    }
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

// ─── Paths ─────────────────────────────────────────────────────────────────

/// One step of an issue's key path: a table key or an array index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathStep {
    Key(String),
    Index(usize),
}

/// Split an issue's key (`ops[2].stops[0].position`) into its steps.
pub fn parse_path(path: &str) -> Vec<PathStep> {
    let mut steps = Vec::new();
    for part in path.split('.') {
        let (name, mut rest) = match part.find('[') {
            Some(i) => (&part[..i], &part[i..]),
            None => (part, ""),
        };
        if !name.is_empty() {
            steps.push(PathStep::Key(name.to_string()));
        }
        while let Some(end) = rest.find(']') {
            if let Ok(i) = rest[1..end].parse() {
                steps.push(PathStep::Index(i));
            }
            rest = &rest[end + 1..];
        }
    }
    steps
}

/// The property schema that `path` (an issue's key) is checked against in
/// `params`, found the way [`validate_params`] walks: through `properties`,
/// a table of named tables, and array `items` (a tagged array's branch for
/// that element). `None` when the schema doesn't describe it.
pub fn schema_at<'a>(schema: &'a Value, params: &toml::Value, path: &str) -> Option<&'a Value> {
    let mut s = schema;
    let mut v = Some(params);
    for step in parse_path(path) {
        match step {
            PathStep::Key(k) => {
                s = match s.get("properties").and_then(|p| p.get(&k)) {
                    Some(prop) => prop,
                    None => s.get("additionalProperties").filter(|e| e.is_object())?,
                };
                v = v.and_then(|v| v.get(&k));
            }
            PathStep::Index(i) => {
                let items = s.get("items")?;
                let item = v.and_then(|v| v.get(i));
                s = match item {
                    Some(item) => branch(items, item, "").ok()?,
                    None => items,
                };
                v = item;
            }
        }
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(src: &str) -> toml::Value {
        toml::from_str(src).unwrap()
    }

    fn shown(issues: &[ParamIssue]) -> Vec<String> {
        issues.iter().map(ToString::to_string).collect()
    }

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

    fn value_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "height": { "type": "number", "minimum": 0.01, "default": 5.0 },
                "falloff": { "type": "number", "exclusiveMinimum": 0.0, "maximum": 1.0, "default": 0.7 },
                "levels": { "type": "integer", "minimum": 1, "maximum": 8, "default": 3 },
                "shape": { "type": "string", "enum": ["sphere", "box"], "default": "sphere" },
                "color": { "type": "string", "pattern": "^#?[0-9a-fA-F]{6,8}$", "default": "#2D8C0D" },
                "flat": { "type": "boolean" }
            },
            "required": ["shape"],
            "additionalProperties": false
        })
    }

    fn nested_schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "materials": {
                    "type": "object",
                    "additionalProperties": {
                        "type": "object",
                        "title": "material",
                        "properties": { "roughness": { "type": "number", "minimum": 0, "maximum": 1 } },
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
                                "type": "object",
                                "properties": {
                                    "type": { "const": "grid" },
                                    "columns": { "type": "integer", "minimum": 1, "maximum": 64 },
                                    "stops": {
                                        "type": "array",
                                        "items": {
                                            "title": "stop",
                                            "type": "object",
                                            "properties": { "position": { "type": "number" } },
                                            "required": ["position"],
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

    // ── keys the generator ignores ─────────────────────────────────────────

    #[test]
    fn quiet_when_every_key_is_read() {
        let p = params("shape = \"square\"\nside = 2.0\ncolor = \"red\"");
        assert!(validate_params(&shape_schema(), &p).is_empty());
    }

    #[test]
    fn keys_for_another_mode_are_ignored() {
        let issues = validate_params(&shape_schema(), &params("side = 2.0"));
        assert_eq!(
            issues,
            vec![ParamIssue::Inapplicable {
                key: "side".into(),
                controller: "shape".into(),
                actual: "circle".into(),
                needs: vec!["square".into()],
            }]
        );
        assert!(!issues[0].is_error());
        assert_eq!(
            issues[0].to_string(),
            "`side` is ignored: it only applies when shape is \"square\", and this spec has shape = \"circle\""
        );
    }

    #[test]
    fn typos_come_with_a_suggestion() {
        let issues = validate_params(&shape_schema(), &params("radus = 1.0\nwobble = 3"));
        assert_eq!(
            issues,
            vec![
                ParamIssue::Unknown {
                    key: "radus".into(),
                    suggestion: Some("radius".into()),
                    within: None,
                },
                ParamIssue::Unknown {
                    key: "wobble".into(),
                    suggestion: None,
                    within: None,
                },
            ]
        );
    }

    #[test]
    fn open_schemas_are_trusted_with_unknown_keys() {
        let mut schema = shape_schema();
        schema["additionalProperties"] = json!(true);
        assert!(validate_params(&schema, &params("anything = 1")).is_empty());
    }

    // ── values it can't use ────────────────────────────────────────────────

    #[test]
    fn clean_values_have_no_issues() {
        let p = params("height = 3.0\nlevels = 2\nshape = \"box\"\ncolor = \"#112233\"");
        assert!(validate_params(&value_schema(), &p).is_empty());
        // An integer is a number
        let p = params("height = 3\nshape = \"box\"");
        assert!(validate_params(&value_schema(), &p).is_empty());
    }

    #[test]
    fn every_kind_of_value_problem_is_reported() {
        let p = params(
            "height = -1.0\nfalloff = 0.0\nlevels = 2.5\nshape = \"spere\"\ncolor = \"green\"\nflat = 1",
        );
        let issues = validate_params(&value_schema(), &p);
        assert!(issues.iter().all(ParamIssue::is_error), "{issues:#?}");
        assert_eq!(
            shown(&issues),
            [
                "`color` = \"green\" is not a #RRGGBB colour",
                "`falloff` = 0 is outside the allowed range (0, 1]; the nearest allowed value is 0.001",
                "`flat` should be a boolean, not an integer",
                "`height` = -1 is outside the allowed range [0.01, inf); the nearest allowed value is 0.01",
                "`levels` should be an integer in the range [1, 8], not a number",
                "`shape` = \"spere\" is not one of: sphere, box (did you mean \"sphere\"?)",
            ]
        );
    }

    #[test]
    fn a_missing_required_key_is_reported() {
        let issues = validate_params(&value_schema(), &params("height = 2.0"));
        assert_eq!(
            issues,
            vec![ParamIssue::MissingRequired {
                key: "shape".into(),
                within: None
            }]
        );
        assert_eq!(issues[0].to_string(), "`shape` is required");
    }

    // ── at every depth ─────────────────────────────────────────────────────

    #[test]
    fn named_tables_and_tagged_arrays_are_followed() {
        let p = params(
            r##"
[materials.shell]
rougness = 0.4
[materials.leg]
roughness = 1.6
[[ops]]
type = "grid"
colums = 8
[[ops.stops]]
position = 0.0
colour = "#fff"
[[ops.stops]]
[[ops]]
type = "open"
anything = 1
[[ops]]
type = "gird"
columns = 2
[[ops]]
type = "grid"
columns = "x"
[[ops]]
type = "grid"
columns = 100
[[ops]]
columns = 1
"##,
        );
        let issues = validate_params(&nested_schema(), &p);
        assert_eq!(
            shown(&issues),
            [
                "`materials.leg.roughness` = 1.6 is outside the allowed range [0, 1]; the nearest allowed value is 1",
                "`materials.shell.rougness` is not a parameter of the material (did you mean `roughness`?)",
                "`ops[0].colums` is not a parameter of the grid op (did you mean `columns`?)",
                "`ops[0].stops[0].colour` is not a parameter of the stop",
                "`ops[0].stops[1].position` is required in every stop",
                "`ops[2].type` = \"gird\" is not one of: grid, open (did you mean \"grid\"?)",
                "`ops[3].columns` should be an integer in the range [1, 64], not a string",
                "`ops[4].columns` = 100 is outside the allowed range [1, 64]; the nearest allowed value is 64",
                "`ops[5].type` is required",
            ]
        );
        assert_eq!(issues[2].key(), "ops[0].colums");
    }

    #[test]
    fn nothing_inside_an_ignored_key_is_checked() {
        let schema = json!({
            "type": "object",
            "properties": {
                "mode": { "type": "string", "default": "a" },
                "extra": {
                    "type": "object",
                    "x-when": { "mode": ["b"] },
                    "properties": { "n": { "type": "integer" } },
                    "additionalProperties": false
                }
            },
            "additionalProperties": false
        });
        let issues = validate_params(&schema, &params("[extra]\nn = \"x\"\ntypo = 1"));
        assert_eq!(issues.len(), 1, "{issues:#?}");
        assert!(matches!(issues[0], ParamIssue::Inapplicable { .. }));
    }

    #[test]
    fn paths_split_into_keys_and_indices() {
        use PathStep::*;
        assert_eq!(
            parse_path("ops[2].stops[0].position"),
            [
                Key("ops".into()),
                Index(2),
                Key("stops".into()),
                Index(0),
                Key("position".into())
            ]
        );
        assert_eq!(parse_path("height"), [Key("height".into())]);
    }

    #[test]
    fn schema_at_finds_the_property_an_issue_is_about() {
        let schema = nested_schema();
        let p =
            params("[[ops]]\ntype = \"grid\"\ncolumns = 100\n[materials.shell]\nroughness = 2.0");
        let columns = schema_at(&schema, &p, "ops[0].columns").unwrap();
        assert_eq!(columns["maximum"], json!(64));
        let roughness = schema_at(&schema, &p, "materials.shell.roughness").unwrap();
        assert_eq!(roughness["maximum"], json!(1));
        assert!(schema_at(&schema, &p, "ops[0].nope").is_none());
    }

    #[test]
    fn exclusive_bounds_clamp_inside() {
        let b = NumBounds {
            min: Some((0.0, true)),
            max: Some((1.0, true)),
        };
        assert!(b.contains(b.clamp(0.0)));
        assert!(b.contains(b.clamp(-5.0)));
        assert!(b.contains(b.clamp(1.0)));
        assert!(b.contains(b.clamp(9.0)));
        assert_eq!(b.clamp(0.5), 0.5);
        assert_eq!(b.to_string(), "(0, 1)");
    }
}
