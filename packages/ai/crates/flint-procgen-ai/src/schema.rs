//! Repairing a spec's `params` against its generator's JSON param schema.
//!
//! What is wrong with a spec is [`flint_procgen::validate_params`]'s to say:
//! the one validator `flint gen`, `flint check` and the runtime share
//! (#718). This module fixes what it reports, so an agent's spec is one the
//! generator reads as written: a param the generator never reads is a silent
//! no-op, and a value out of range is an error at generation time.

use flint_procgen::{parse_path, schema_at, string_enum, validate_params, ParamIssue, PathStep};
use serde_json::Value as Json;

pub use flint_procgen::{prop_type, NumBounds};

/// Round to three decimals, so agent-written specs stay readable.
pub fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
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

/// The container holding the last step of `steps`, and that step.
fn parent_of<'a>(
    params: &'a mut toml::Value,
    steps: &'a [PathStep],
) -> Option<(&'a mut toml::Value, &'a PathStep)> {
    let (last, above) = steps.split_last()?;
    let mut at = params;
    for step in above {
        at = match step {
            PathStep::Key(k) => at.get_mut(k.as_str())?,
            PathStep::Index(i) => at.get_mut(*i)?,
        };
    }
    Some((at, last))
}

/// Fix every issue [`validate_params`] reports, at any depth: drop keys the
/// generator ignores (unknown, for another mode, deprecated), clamp numbers
/// into range, and replace a bad or missing value with the schema default
/// (or the first choice). Returns the number of params changed. What has no
/// fix (a required key with no default, a pipeline op of an unknown type)
/// is left for the caller, which still sees it in `validate_params`.
pub fn repair_params(schema: &Json, params: &mut toml::Value) -> usize {
    if !params.is_table() {
        *params = toml::Value::Table(Default::default());
    }
    let issues = validate_params(schema, params);
    let mut changed = 0;
    for issue in &issues {
        let key = issue.key();
        // Read against the params as validated: an earlier fix moves no key
        let prop = schema_at(schema, params, key).cloned();
        let steps = parse_path(key);
        let Some((parent, last)) = parent_of(params, &steps) else {
            continue;
        };
        let fixed = match (issue, &prop) {
            (i, _) if i.is_ignored_key() => None,
            (ParamIssue::OutOfRange { value, bounds, .. }, Some(prop)) => {
                let v = bounds.clamp(*value);
                Some(if prop_type(prop) == "integer" {
                    toml::Value::Integer(v.round() as i64)
                } else {
                    toml::Value::Float(round3(v))
                })
            }
            (_, Some(prop)) => match fallback(prop) {
                Some(v) => Some(v),
                // A missing key with nothing to fill it with stays missing
                None if matches!(issue, ParamIssue::MissingRequired { .. }) => continue,
                None => None,
            },
            (_, None) => continue,
        };
        match (last, fixed) {
            (PathStep::Key(k), Some(v)) => {
                let Some(table) = parent.as_table_mut() else {
                    continue;
                };
                table.insert(k.clone(), v);
            }
            (PathStep::Key(k), None) => {
                let Some(table) = parent.as_table_mut() else {
                    continue;
                };
                table.remove(k);
            }
            (PathStep::Index(i), Some(v)) => {
                let Some(slot) = parent.get_mut(*i) else {
                    continue;
                };
                *slot = v;
            }
            // Removing an element would move every index after it
            (PathStep::Index(_), None) => continue,
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
                "color": { "type": "string", "pattern": "^#?[0-9a-fA-F]{6,8}$", "default": "#2D8C0D" },
                "parts": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "title": "part",
                        "properties": {
                            "size": { "type": "number", "minimum": 0, "maximum": 2, "default": 1.0 }
                        },
                        "additionalProperties": false
                    }
                }
            },
            "required": ["shape"],
            "additionalProperties": false
        })
    }

    fn params(src: &str) -> toml::Value {
        toml::from_str(src).unwrap()
    }

    #[test]
    fn repair_leaves_nothing_to_report() {
        let mut p = params(
            "height = -1.0\nfalloff = 0.0\nlevels = 2.5\nshape = \"cone\"\ncolor = \"green\"\ntrunk_radius = 0.3",
        );
        let changed = repair_params(&schema(), &mut p);
        assert_eq!(changed, 6);
        assert!(validate_params(&schema(), &p).is_empty(), "{p:#?}");
        assert!(p.get("trunk_radius").is_none());
        assert_eq!(p["height"].as_float(), Some(0.01));
        assert_eq!(p["shape"].as_str(), Some("sphere"));
    }

    #[test]
    fn repair_reaches_inside_arrays_of_tables() {
        let mut p =
            params("shape = \"box\"\n[[parts]]\nsize = 9.0\nsise = 1.0\n[[parts]]\nsize = \"big\"");
        let changed = repair_params(&schema(), &mut p);
        assert_eq!(changed, 3, "{p:#?}");
        assert!(validate_params(&schema(), &p).is_empty(), "{p:#?}");
        assert_eq!(p["parts"][0]["size"].as_float(), Some(2.0));
        assert!(p["parts"][0].get("sise").is_none());
        assert_eq!(p["parts"][1]["size"].as_float(), Some(1.0));
    }

    #[test]
    fn a_required_key_is_filled_from_its_default() {
        let mut p = params("height = 2.0");
        assert_eq!(repair_params(&schema(), &mut p), 1);
        assert_eq!(p["shape"].as_str(), Some("sphere"));
    }

    #[test]
    fn suggestions_read_as_sentences() {
        let issues = validate_params(&schema(), &params("height = -1.0\nshape = \"box\""));
        let text = issues[0].to_string();
        assert!(text.contains("`height` = -1"), "{text}");
        assert!(text.contains("nearest allowed value is 0.01"), "{text}");
    }
}
