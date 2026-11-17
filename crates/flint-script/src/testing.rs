//! What scripts said about the run they are in (#714): `test_assert`,
//! `test_fail`, `test_pass` and `test_log`.
//!
//! Each call writes into the [`TestRecord`] the script context holds, which
//! lasts the whole run (scene changes keep it). A host that runs a scene as
//! a test (`flint playtest`) reads it at the end: failed assertions fail
//! the run, `test_pass()` or `test_fail()` ends it with that verdict, and
//! what `test_log` set is reported as it was last set. A host that plays
//! the game (`flint play`) only logs a failure, once per place.
//!
//! An assertion is counted per place, its file, line and message, so one
//! in `on_update` is one entry however many frames it ran in: how often it
//! held, how often it failed and the game time of its first failure.

use std::collections::BTreeMap;

/// One `test_assert` place: a file, line and message.
#[derive(Debug, Clone, PartialEq)]
pub struct Assertion {
    /// The script's file, as found.
    pub file: String,
    /// The entity the script is on, by name.
    pub entity: String,
    /// 1-based, when Rhai knows it.
    pub line: Option<usize>,
    pub message: String,
    /// How many times it held.
    pub passed: u64,
    /// How many times it failed.
    pub failed: u64,
    /// The game time of its first failure.
    pub first_failed_at: Option<f64>,
}

/// How a script ended the run.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    /// True for `test_pass()`, false for `test_fail(msg)`.
    pub passed: bool,
    /// `test_fail`'s message; empty for a pass.
    pub message: String,
    pub file: String,
    pub entity: String,
    pub line: Option<usize>,
    /// The game time it was called at.
    pub at: f64,
}

/// Everything the scripts said about the run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TestRecord {
    /// Every `test_assert` place, in the order each first ran.
    pub assertions: Vec<Assertion>,
    /// The first `test_pass()` or `test_fail()`: later ones don't change it.
    pub verdict: Option<Verdict>,
    /// `test_log(key, value)`: each key's last value, as JSON.
    pub logged: BTreeMap<String, serde_json::Value>,
}

/// The place a test function was called from.
pub struct Place<'a> {
    pub file: &'a str,
    pub entity: &'a str,
    pub line: Option<usize>,
    pub at: f64,
}

impl TestRecord {
    /// Count one `test_assert(cond, message)`. Returns true the first time
    /// this place fails, so the caller can say so once.
    pub fn assert(&mut self, cond: bool, message: &str, place: &Place<'_>) -> bool {
        let at = self
            .assertions
            .iter()
            .position(|a| a.file == place.file && a.line == place.line && a.message == message);
        let a = match at {
            Some(i) => &mut self.assertions[i],
            None => {
                self.assertions.push(Assertion {
                    file: place.file.to_string(),
                    entity: place.entity.to_string(),
                    line: place.line,
                    message: message.to_string(),
                    passed: 0,
                    failed: 0,
                    first_failed_at: None,
                });
                self.assertions.last_mut().expect("just pushed")
            }
        };
        if cond {
            a.passed += 1;
            false
        } else {
            a.failed += 1;
            let first = a.first_failed_at.is_none();
            a.first_failed_at.get_or_insert(place.at);
            first
        }
    }

    /// `test_pass()` (`passed`) or `test_fail(message)`. Returns false when
    /// a verdict was already given, which this one doesn't change.
    pub fn decide(&mut self, passed: bool, message: &str, place: &Place<'_>) -> bool {
        if self.verdict.is_some() {
            return false;
        }
        self.verdict = Some(Verdict {
            passed,
            message: message.to_string(),
            file: place.file.to_string(),
            entity: place.entity.to_string(),
            line: place.line,
            at: place.at,
        });
        true
    }

    /// `test_log(key, value)`.
    pub fn log(&mut self, key: &str, value: serde_json::Value) {
        self.logged.insert(key.to_string(), value);
    }

    /// The assertion places that failed at least once.
    pub fn failed_assertions(&self) -> impl Iterator<Item = &Assertion> {
        self.assertions.iter().filter(|a| a.failed > 0)
    }
}

/// A script value as JSON: numbers, strings, booleans, arrays and maps
/// (keys sorted), `()` as null. Anything else (a function pointer, a
/// custom type) as its printed form.
pub fn dynamic_to_json(value: &rhai::Dynamic) -> serde_json::Value {
    use serde_json::Value;
    if value.is_unit() {
        return Value::Null;
    }
    if let Ok(b) = value.as_bool() {
        return Value::Bool(b);
    }
    if let Ok(i) = value.as_int() {
        return Value::from(i);
    }
    if let Ok(f) = value.as_float() {
        return serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null);
    }
    if let Some(s) = value.read_lock::<rhai::ImmutableString>() {
        return Value::String(s.to_string());
    }
    if let Some(array) = value.read_lock::<rhai::Array>() {
        return Value::Array(array.iter().map(dynamic_to_json).collect());
    }
    if let Some(map) = value.read_lock::<rhai::Map>() {
        let sorted: BTreeMap<String, Value> = map
            .iter()
            .map(|(k, v)| (k.to_string(), dynamic_to_json(v)))
            .collect();
        return Value::Object(sorted.into_iter().collect());
    }
    Value::String(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn here(line: usize, at: f64) -> Place<'static> {
        Place {
            file: "scripts/a.rhai",
            entity: "a",
            line: Some(line),
            at,
        }
    }

    #[test]
    fn an_assertion_is_one_entry_a_place_however_often_it_runs() {
        let mut r = TestRecord::default();
        assert!(!r.assert(true, "hp > 0", &here(3, 0.0)));
        assert!(
            r.assert(false, "hp > 0", &here(3, 0.5)),
            "the first failure"
        );
        assert!(!r.assert(false, "hp > 0", &here(3, 0.6)), "said once");
        r.assert(true, "other", &here(9, 0.6));
        assert_eq!(r.assertions.len(), 2);
        let a = &r.assertions[0];
        assert_eq!((a.passed, a.failed, a.first_failed_at), (1, 2, Some(0.5)));
        assert_eq!(r.failed_assertions().count(), 1);
    }

    #[test]
    fn the_first_verdict_stands() {
        let mut r = TestRecord::default();
        assert!(r.decide(false, "fell off", &here(4, 1.0)));
        assert!(!r.decide(true, "", &here(5, 2.0)));
        let v = r.verdict.unwrap();
        assert!(!v.passed);
        assert_eq!(
            (v.message.as_str(), v.line, v.at),
            ("fell off", Some(4), 1.0)
        );
    }

    #[test]
    fn values_become_json_with_sorted_keys() {
        let engine = rhai::Engine::new();
        let v: rhai::Dynamic = engine
            .eval(r#"#{ z: 1, a: [true, 2.5, "s", ()] }"#)
            .unwrap();
        assert_eq!(
            dynamic_to_json(&v).to_string(),
            r#"{"a":[true,2.5,"s",null],"z":1}"#
        );
    }
}
