//! Checking a spec's `params` against its generator's JSON param schema.
//!
//! Every [`flint_procgen::Generator`] publishes `param_schema()`. This module
//! reads the small subset of JSON Schema those schemas use (`type`,
//! `minimum`/`exclusiveMinimum`, `maximum`/`exclusiveMaximum`, `enum`, hex
//! colour `pattern`, `required`, `additionalProperties: false`), reports what
//! a spec gets wrong, and repairs it. Agents use it to tell an honest spec
//! from a plausible-looking one: a param the generator never reads is a
//! silent no-op, and a value out of range is an error at generation time.

use std::fmt;

use serde_json::Value as Json;

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
    pub fn of(prop: &Json) -> Self {
        let get = |k: &str| prop.get(k).and_then(Json::as_f64);
        Self {
            min: get("minimum")
                .map(|v| (v, false))
                .or_else(|| get("exclusiveMinimum").map(|v| (v, true))),
            max: get("maximum")
                .map(|v| (v, false))
                .or_else(|| get("exclusiveMaximum").map(|v| (v, true))),
        }
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

/// What is wrong with one param.
#[derive(Debug, Clone, PartialEq)]
pub enum IssueKind {
    /// The generator's schema has no such param, so the generator ignores it.
    Unknown,
    /// The value has the wrong TOML type.
    WrongType {
        /// The schema's type name (`number`, `integer`, ...).
        expected: String,
    },
    /// A number outside the schema's range.
    OutOfRange {
        /// The offending value.
        value: f64,
        /// The allowed range.
        bounds: NumBounds,
    },
    /// A string not among the schema's `enum` values.
    NotInEnum {
        /// The offending value.
        value: String,
        /// The allowed values.
        allowed: Vec<String>,
    },
    /// A string that should be a `#RRGGBB` / `#RRGGBBAA` colour.
    BadColor {
        /// The offending value.
        value: String,
    },
    /// A param the schema requires is absent.
    MissingRequired,
}

/// One problem found in a spec's params.
#[derive(Debug, Clone, PartialEq)]
pub struct ParamIssue {
    /// The param's name.
    pub param: String,
    /// What is wrong with it.
    pub kind: IssueKind,
}

impl fmt::Display for ParamIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let p = &self.param;
        match &self.kind {
            IssueKind::Unknown => write!(
                f,
                "`{p}` is not a parameter of this generator and will be ignored; remove it"
            ),
            IssueKind::WrongType { expected } => {
                write!(f, "`{p}` should be a {expected}")
            }
            IssueKind::OutOfRange { value, bounds } => {
                write!(f, "`{p}` = {value} is outside {}", describe(bounds))?;
                write!(f, "; use {}", round3(bounds.clamp(*value)))
            }
            IssueKind::NotInEnum { value, allowed } => write!(
                f,
                "`{p}` = \"{value}\" is not one of: {}",
                allowed.join(", ")
            ),
            IssueKind::BadColor { value } => {
                write!(f, "`{p}` = \"{value}\" is not a #RRGGBB colour")
            }
            IssueKind::MissingRequired => write!(f, "`{p}` is required"),
        }
    }
}

fn describe(b: &NumBounds) -> String {
    let lo = match b.min {
        Some((m, true)) => format!("({m}"),
        Some((m, false)) => format!("[{m}"),
        None => "(-inf".to_string(),
    };
    let hi = match b.max {
        Some((m, true)) => format!("{m})"),
        Some((m, false)) => format!("{m}]"),
        None => "inf)".to_string(),
    };
    format!("{lo}, {hi}")
}

/// Round to three decimals, so agent-written specs stay readable.
pub fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn properties(schema: &Json) -> Option<&serde_json::Map<String, Json>> {
    schema.get("properties").and_then(Json::as_object)
}

fn is_hex_color(s: &str) -> bool {
    let h = s.strip_prefix('#').unwrap_or(s);
    (h.len() == 6 || h.len() == 8) && h.chars().all(|c| c.is_ascii_hexdigit())
}

fn color_pattern(prop: &Json) -> bool {
    prop.get("pattern")
        .and_then(Json::as_str)
        .is_some_and(|p| p.contains("[0-9a-fA-F]"))
}

fn string_enum(prop: &Json) -> Option<Vec<String>> {
    let values: Vec<String> = prop
        .get("enum")?
        .as_array()?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    (!values.is_empty()).then_some(values)
}

/// The schema `type` of one property (`""` when absent).
pub fn prop_type(prop: &Json) -> &str {
    prop.get("type").and_then(Json::as_str).unwrap_or("")
}

fn check_one(prop: &Json, value: &toml::Value) -> Option<IssueKind> {
    let wrong = || {
        Some(IssueKind::WrongType {
            expected: prop_type(prop).to_string(),
        })
    };
    match prop_type(prop) {
        "number" => {
            let v = match value {
                toml::Value::Float(f) => *f,
                toml::Value::Integer(i) => *i as f64,
                _ => return wrong(),
            };
            let bounds = NumBounds::of(prop);
            (!bounds.contains(v)).then_some(IssueKind::OutOfRange { value: v, bounds })
        }
        "integer" => {
            let toml::Value::Integer(i) = value else {
                return wrong();
            };
            let bounds = NumBounds::of(prop);
            let v = *i as f64;
            (!bounds.contains(v)).then_some(IssueKind::OutOfRange { value: v, bounds })
        }
        "boolean" if !value.is_bool() => wrong(),
        "string" => {
            let Some(s) = value.as_str() else {
                return wrong();
            };
            if let Some(allowed) = string_enum(prop) {
                if !allowed.iter().any(|a| a == s) {
                    return Some(IssueKind::NotInEnum {
                        value: s.to_string(),
                        allowed,
                    });
                }
            }
            if color_pattern(prop) && !is_hex_color(s) {
                return Some(IssueKind::BadColor {
                    value: s.to_string(),
                });
            }
            None
        }
        "array" if !value.is_array() => wrong(),
        "object" if !value.is_table() => wrong(),
        _ => None,
    }
}

/// Every problem in `params` against `schema`, sorted by param name.
pub fn check_params(schema: &Json, params: &toml::Value) -> Vec<ParamIssue> {
    let mut issues = Vec::new();
    let Some(props) = properties(schema) else {
        return issues;
    };
    let closed = schema.get("additionalProperties") == Some(&Json::Bool(false));
    let empty = toml::map::Map::new();
    let table = params.as_table().unwrap_or(&empty);

    for (name, value) in table {
        match props.get(name) {
            Some(prop) => {
                if let Some(kind) = check_one(prop, value) {
                    issues.push(ParamIssue {
                        param: name.clone(),
                        kind,
                    });
                }
            }
            None if closed => issues.push(ParamIssue {
                param: name.clone(),
                kind: IssueKind::Unknown,
            }),
            None => {}
        }
    }
    if let Some(required) = schema.get("required").and_then(Json::as_array) {
        for r in required.iter().filter_map(Json::as_str) {
            if !table.contains_key(r) {
                issues.push(ParamIssue {
                    param: r.to_string(),
                    kind: IssueKind::MissingRequired,
                });
            }
        }
    }
    issues.sort_by(|a, b| a.param.cmp(&b.param));
    issues
}

/// Convert a JSON default into TOML. Returns `None` for `null`.
pub fn json_to_toml(v: &Json) -> Option<toml::Value> {
    Some(match v {
        Json::Null => return None,
        Json::Bool(b) => toml::Value::Boolean(*b),
        Json::Number(n) => match n.as_i64() {
            Some(i) => toml::Value::Integer(i),
            None => toml::Value::Float(n.as_f64()?),
        },
        Json::String(s) => toml::Value::String(s.clone()),
        Json::Array(a) => toml::Value::Array(a.iter().filter_map(json_to_toml).collect()),
        Json::Object(o) => toml::Value::Table(
            o.iter()
                .filter_map(|(k, v)| Some((k.clone(), json_to_toml(v)?)))
                .collect(),
        ),
    })
}

/// A schema property's default as TOML, typed to match the property
/// (`"number"` defaults always come back as floats).
pub fn default_of(prop: &Json) -> Option<toml::Value> {
    let v = json_to_toml(prop.get("default")?)?;
    Some(match (prop_type(prop), v) {
        ("number", toml::Value::Integer(i)) => toml::Value::Float(i as f64),
        (_, v) => v,
    })
}

/// The value a repair writes for a missing or mistyped param: its default,
/// else the first enum value, else nothing.
fn fallback(prop: &Json) -> Option<toml::Value> {
    default_of(prop).or_else(|| {
        string_enum(prop)
            .and_then(|e| e.into_iter().next())
            .map(toml::Value::String)
    })
}

/// Fix every issue [`check_params`] reports: drop unknown params, clamp
/// numbers into range, and replace bad values with the schema default.
/// Returns the number of params changed.
pub fn repair_params(schema: &Json, params: &mut toml::Value) -> usize {
    let issues = check_params(schema, params);
    let Some(props) = properties(schema) else {
        return 0;
    };
    if !params.is_table() {
        *params = toml::Value::Table(Default::default());
    }
    let table = params.as_table_mut().expect("params is a table");
    let mut changed = 0;
    for issue in &issues {
        let prop = props.get(&issue.param);
        match (&issue.kind, prop) {
            (IssueKind::Unknown, _) => {
                table.remove(&issue.param);
            }
            (IssueKind::OutOfRange { value, bounds }, Some(prop)) => {
                let fixed = bounds.clamp(*value);
                let v = if prop_type(prop) == "integer" {
                    toml::Value::Integer(fixed.round() as i64)
                } else {
                    toml::Value::Float(round3(fixed))
                };
                table.insert(issue.param.clone(), v);
            }
            (_, Some(prop)) => match fallback(prop) {
                Some(v) => {
                    table.insert(issue.param.clone(), v);
                }
                None => {
                    table.remove(&issue.param);
                }
            },
            (_, None) => continue,
        }
        changed += 1;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Json {
        json!({
            "type": "object",
            "properties": {
                "height": { "type": "number", "minimum": 0.01, "default": 5.0 },
                "falloff": { "type": "number", "exclusiveMinimum": 0.0, "maximum": 1.0, "default": 0.7 },
                "levels": { "type": "integer", "minimum": 1, "default": 3 },
                "shape": { "type": "string", "enum": ["sphere", "box"], "default": "sphere" },
                "color": { "type": "string", "pattern": "^#?[0-9a-fA-F]{6,8}$", "default": "#2D8C0D" }
            },
            "required": ["shape"],
            "additionalProperties": false
        })
    }

    fn params(src: &str) -> toml::Value {
        toml::from_str(src).unwrap()
    }

    #[test]
    fn clean_params_have_no_issues() {
        let p = params("height = 3.0\nlevels = 2\nshape = \"box\"\ncolor = \"#112233\"");
        assert!(check_params(&schema(), &p).is_empty());
    }

    #[test]
    fn integers_are_accepted_for_numbers() {
        let p = params("height = 3\nshape = \"box\"");
        assert!(check_params(&schema(), &p).is_empty());
    }

    #[test]
    fn every_kind_of_problem_is_reported() {
        let p = params(
            "height = -1.0\nfalloff = 0.0\nlevels = 2.5\nshape = \"cone\"\ncolor = \"green\"\ntrunk_radius = 0.3",
        );
        let issues = check_params(&schema(), &p);
        let kinds: Vec<_> = issues
            .iter()
            .map(|i| (i.param.as_str(), std::mem::discriminant(&i.kind)))
            .collect();
        assert_eq!(issues.len(), 6, "{issues:#?}");
        assert!(kinds.contains(&("trunk_radius", std::mem::discriminant(&IssueKind::Unknown))));
        assert!(issues
            .iter()
            .any(|i| i.param == "falloff" && matches!(i.kind, IssueKind::OutOfRange { .. })));
        assert!(issues
            .iter()
            .any(|i| i.param == "levels" && matches!(i.kind, IssueKind::WrongType { .. })));
    }

    #[test]
    fn missing_required_is_reported() {
        let issues = check_params(&schema(), &params("height = 2.0"));
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, IssueKind::MissingRequired);
    }

    #[test]
    fn repair_leaves_nothing_to_report() {
        let mut p = params(
            "height = -1.0\nfalloff = 0.0\nlevels = 2.5\nshape = \"cone\"\ncolor = \"green\"\ntrunk_radius = 0.3",
        );
        let changed = repair_params(&schema(), &mut p);
        assert_eq!(changed, 6);
        assert!(check_params(&schema(), &p).is_empty(), "{p:#?}");
        assert!(p.get("trunk_radius").is_none());
        assert_eq!(p["height"].as_float(), Some(0.01));
        assert_eq!(p["shape"].as_str(), Some("sphere"));
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
    }

    #[test]
    fn suggestions_read_as_sentences() {
        let issues = check_params(&schema(), &params("height = -1.0\nshape = \"box\""));
        let text = issues[0].to_string();
        assert!(text.contains("`height` = -1"), "{text}");
        assert!(text.contains("use 0.01"), "{text}");
    }
}
