//! An input script's `expect` and `wait` lines (#714): what the run should
//! show at a game time, asked in flint-query's conditions.
//!
//! ```text
//! 2.0  expect player transform.position.z > 1      # one entity, one condition
//! 2.0  expect entities where door.open == true     # at least one matches
//! 2.0  expect no entities where enemy.alive == true
//! 2.0  expect 3 entities where coin.taken == true  # exactly three
//! 2.0  expect persist coins >= 3                   # the persist_* store
//! 2.0  expect log greeted == true                  # a script's test_log
//! 2.0  expect event door_opened                    # a fire_event so far
//! 1.0  wait player transform.position.x > 4 within 6
//! 1.0  wait event level_loaded                     # within 10 s, the default
//! ```
//!
//! An `expect` is asked once, at the first frame at or after its time. A
//! `wait` is asked every frame from its time until it holds, holding back
//! every line after it, which then play that much later: `wait` is how a
//! script says "once the door has opened, walk through". One that doesn't
//! hold `within` its seconds (10 by default) fails, and the lines after it
//! go on from there. An `event` line counts every firing since the run
//! began for an `expect`, and since the check before it settled for a
//! `wait`. Every outcome is kept ([`InputScript::outcomes`]), and
//! a host fails the run on any that failed: a playtest's report lists them,
//! and `flint play --input-script` exits 1.
//!
//! The host answers through a [`Probe`]: the player reads its world, its
//! persistent store, the scripts' test record and the events it has seen.
//!
//! [`InputScript::outcomes`]: super::input_script::InputScript::outcomes

use flint_query::{Condition, QueryValue};

/// How long a `wait` waits with no `within`.
pub const DEFAULT_WAIT: f64 = 10.0;

/// How many entities an `entities where` line asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Count {
    /// `entities where ...`: at least one.
    Some,
    /// `no entities where ...`.
    None,
    /// `<n> entities where ...`.
    Exactly(usize),
}

/// What a line asks about.
#[derive(Debug, Clone)]
pub enum Subject {
    /// `<entity> <condition>`: the named entity meets the condition.
    Entity { name: String, condition: Condition },
    /// `[no|<n>] entities where <condition>`.
    Entities { count: Count, condition: Condition },
    /// `persist <key> <op> <value>`: a key of the persistent store.
    Persist(Condition),
    /// `log <key> <op> <value>`: a value a script gave `test_log`.
    Logged(Condition),
    /// `event <name>`: a script's or the engine's named event has fired
    /// (since the run began, for `expect`; for `wait`, since the check
    /// before it settled, so an event a `tap` just before it caused counts).
    Event(String),
}

/// One `expect` or `wait` line.
#[derive(Debug, Clone)]
pub struct Check {
    /// The line in the input script, 1-based.
    pub line: usize,
    /// What it asks, as written after the verb.
    pub text: String,
    pub subject: Subject,
    /// `Some(seconds)` for a `wait`, `None` for an `expect`.
    pub wait: Option<f64>,
}

impl Check {
    pub fn verb(&self) -> &'static str {
        if self.wait.is_some() {
            "wait"
        } else {
            "expect"
        }
    }
}

/// What the host found: whether the line holds, and what it saw.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub holds: bool,
    /// The value found (`2.5`, `1 entity: door1`, `missing`), for a report.
    pub found: String,
}

/// The host's answer to a line.
pub trait Probe {
    /// Read `subject` now. An `event` counts the ones fired at or after
    /// game time `since`.
    fn read(&self, subject: &Subject, since: f64) -> Reading;
}

/// A probe with nothing to look at (a host that never said): every line
/// fails, saying so.
pub struct NoProbe;

impl Probe for NoProbe {
    fn read(&self, _: &Subject, _: f64) -> Reading {
        Reading {
            holds: false,
            found: "nothing to check against (this host has no world)".into(),
        }
    }
}

/// How a line came out.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CheckOutcome {
    /// The input script's line, 1-based.
    pub line: usize,
    /// `expect` or `wait`.
    pub verb: &'static str,
    /// What it asked, as written.
    pub text: String,
    /// The game time it was due (after any earlier `wait`).
    pub due: f64,
    pub passed: bool,
    /// When it was settled: the frame an `expect` was asked or a `wait`
    /// held, timed out or the run ended.
    pub at: f64,
    /// What was found the last time it was asked.
    pub found: String,
    /// Why it failed, in words (`timed out after 6 s`, `not reached`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

fn condition(text: &str, what: &str) -> anyhow::Result<Condition> {
    flint_query::parse_condition(text)
        .map_err(|e| anyhow::anyhow!("{what}: `{text}` is not a condition ({e})"))
}

/// Parse what follows `expect` or `wait` (a `wait`'s `within S` already
/// taken off).
pub fn parse_subject(text: &str) -> anyhow::Result<Subject> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let rest = |n: usize| -> String { words.get(n..).unwrap_or_default().join(" ") };
    let first = *words
        .first()
        .ok_or_else(|| anyhow::anyhow!("nothing to check"))?;
    // `[no|<n>] entities where <condition>`
    let (count, at) = match first {
        "entities" => (Some(Count::Some), 0),
        "no" if words.get(1) == Some(&"entities") => (Some(Count::None), 1),
        n if words.get(1) == Some(&"entities") => match n.parse::<usize>() {
            Ok(n) => (Some(Count::Exactly(n)), 1),
            Err(_) => (None, 0),
        },
        _ => (None, 0),
    };
    if let Some(count) = count {
        if words.get(at + 1) != Some(&"where") {
            anyhow::bail!("`entities` needs `where <condition>`");
        }
        return Ok(Subject::Entities {
            count,
            condition: condition(&rest(at + 2), "entities where")?,
        });
    }
    Ok(match first {
        "persist" => Subject::Persist(condition(&rest(1), "persist")?),
        "log" => Subject::Logged(condition(&rest(1), "log")?),
        "event" => {
            if words.len() != 2 {
                anyhow::bail!("`event` takes one name");
            }
            Subject::Event(words[1].to_string())
        }
        name => {
            if words.len() < 2 {
                anyhow::bail!("`{name}` needs a condition after it (`{name} door.open == true`)");
            }
            Subject::Entity {
                name: name.to_string(),
                condition: condition(&rest(1), name)?,
            }
        }
    })
}

/// Parse an `expect` (`wait` false) or `wait` line's text after its verb.
pub fn parse(line: usize, text: &str, wait: bool) -> anyhow::Result<Check> {
    let mut text = text.trim().to_string();
    let mut seconds = None;
    if wait {
        // A trailing `within <seconds>`
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.len() >= 2 && words[words.len() - 2] == "within" {
            let s: f64 = words[words.len() - 1]
                .parse()
                .map_err(|_| anyhow::anyhow!("bad `within` seconds"))?;
            if s.is_nan() || s <= 0.0 {
                anyhow::bail!("`within` takes seconds over 0");
            }
            seconds = Some(s);
            text = words[..words.len() - 2].join(" ");
        }
    }
    let subject = parse_subject(&text)?;
    Ok(Check {
        line,
        text,
        subject,
        wait: wait.then(|| seconds.unwrap_or(DEFAULT_WAIT)),
    })
}

/// A value from a TOML table or a script, as a condition compares it.
pub fn toml_value(v: &toml::Value) -> Option<QueryValue> {
    Some(match v {
        toml::Value::String(s) => QueryValue::String(s.clone()),
        toml::Value::Integer(i) => QueryValue::Number(*i as f64),
        toml::Value::Float(f) => QueryValue::Number(*f),
        toml::Value::Boolean(b) => QueryValue::Boolean(*b),
        _ => return None,
    })
}

/// A JSON value (a `test_log`'s), as a condition compares it.
pub fn json_value(v: &serde_json::Value) -> Option<QueryValue> {
    Some(match v {
        serde_json::Value::String(s) => QueryValue::String(s.clone()),
        serde_json::Value::Number(n) => QueryValue::Number(n.as_f64()?),
        serde_json::Value::Bool(b) => QueryValue::Boolean(*b),
        _ => return None,
    })
}

/// A `key <op> value` line against what a store holds under the key.
pub fn compare_keyed(
    condition: &Condition,
    found: Option<QueryValue>,
    shown: impl FnOnce() -> String,
) -> Reading {
    match found {
        Some(v) => Reading {
            holds: flint_query::compare(&v, &condition.operator, &condition.value),
            found: v.to_string(),
        },
        None => Reading {
            holds: false,
            found: shown(),
        },
    }
}

/// Whether `n` entities matching meets `count`.
pub fn count_holds(count: Count, n: usize) -> bool {
    match count {
        Count::Some => n > 0,
        Count::None => n == 0,
        Count::Exactly(want) => n == want,
    }
}

/// One outcome as a line: `line 12: expect door door.open == true: did not
/// hold (found door.open = false)`.
pub fn describe(o: &CheckOutcome) -> String {
    let mut s = format!("line {}: {} {}", o.line, o.verb, o.text);
    if let Some(why) = &o.why {
        s.push_str(&format!(": {why}"));
    }
    if !o.found.is_empty() {
        s.push_str(&format!(" (found {})", o.found));
    }
    s
}

/// The exit report's lines: a count, then each failure.
pub fn summary(outcomes: &[CheckOutcome]) -> Vec<String> {
    let failed: Vec<&CheckOutcome> = outcomes.iter().filter(|o| !o.passed).collect();
    let mut lines = vec![format!(
        "[input] {} of {} checks held",
        outcomes.len() - failed.len(),
        outcomes.len()
    )];
    lines.extend(failed.iter().map(|o| format!("[input]   {}", describe(o))));
    lines
}

/// Why the run fails on its checks, or `None` when every one held.
pub fn failure(outcomes: &[CheckOutcome]) -> Option<String> {
    let failed: Vec<&CheckOutcome> = outcomes.iter().filter(|o| !o.passed).collect();
    let first = failed.first()?;
    Some(format!(
        "{} input script check{} failed; the first: {}",
        failed.len(),
        if failed.len() == 1 { "" } else { "s" },
        describe(first)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_subject_parses() {
        let c = parse(3, "player transform.position.z > 1.5", false).unwrap();
        assert!(c.wait.is_none());
        match c.subject {
            Subject::Entity { name, condition } => {
                assert_eq!(name, "player");
                assert_eq!(condition.to_string(), "transform.position.z > 1.5");
            }
            other => panic!("{other:?}"),
        }
        let count = |s: &str| match parse(1, s, false).unwrap().subject {
            Subject::Entities { count, .. } => count,
            other => panic!("{other:?}"),
        };
        assert_eq!(count("entities where door.open == true"), Count::Some);
        assert_eq!(count("no entities where door.open == true"), Count::None);
        assert_eq!(
            count("3 entities where coin.taken == true"),
            Count::Exactly(3)
        );
        assert!(matches!(
            parse(1, "persist coins >= 3", false).unwrap().subject,
            Subject::Persist(_)
        ));
        assert!(matches!(
            parse(1, "log greeted == true", false).unwrap().subject,
            Subject::Logged(_)
        ));
        assert!(
            matches!(parse(1, "event door_opened", false).unwrap().subject, Subject::Event(ref n) if n == "door_opened")
        );
    }

    #[test]
    fn a_wait_takes_its_within_and_defaults_to_ten_seconds() {
        let w = parse(1, "player transform.position.x > 4 within 6", true).unwrap();
        assert_eq!(w.wait, Some(6.0));
        assert_eq!(w.text, "player transform.position.x > 4");
        assert_eq!(
            parse(1, "event loaded", true).unwrap().wait,
            Some(DEFAULT_WAIT)
        );
        assert_eq!(w.verb(), "wait");
    }

    #[test]
    fn mistakes_say_what_is_wrong() {
        for (line, want) in [
            ("player", "needs a condition"),
            ("entities door.open == true", "needs `where"),
            ("player door.open = true", "not a condition"),
            ("event", "one name"),
        ] {
            let e = parse(1, line, false).unwrap_err().to_string();
            assert!(e.contains(want), "{line}: {e}");
        }
        assert!(parse(1, "event x within 0", true).is_err());
    }
}
