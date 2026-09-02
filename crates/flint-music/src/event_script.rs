//! Event scripts: a small TOML format describing a scripted session — bus
//! changes and markers at musical times — for the offline render harness and
//! automated Milestone tests.
//!
//! This is a tool/test format, not part of the pinned suite schema, but it is
//! versioned all the same. Times use the console readout's vocabulary:
//! `bar:N` and `bar:N:B` are 1-based (bar 1 beat 1 = suite start), while
//! `beat:F` (beats from start) and `sample:N` are absolute 0-based axes.
//!
//! ```toml
//! schema_version = 0
//!
//! [[events]]
//! at = "bar:9"
//! bus = "home_theme"
//! action = "set_gain"   # set_gain | set_lpf | set_detune | marker
//! db = -60.0            # set_gain: db; set_lpf: hz; set_detune: semitones;
//! ramp_ms = 10.0        # marker: label
//! ```

use crate::config_toml::{self, VersionPolicy};
use crate::scheduler::{BusAction, EventTime, ScheduledEvent};
use flint_core::toml_util::toml_f64;
use flint_core::{FlintError, Result};
use std::path::Path;

pub const EVENT_SCHEMA_VERSION: i64 = 0;

#[derive(Debug, Clone)]
pub struct EventScript {
    pub schema_version: i64,
    pub events: Vec<ScheduledEvent>,
}

impl EventScript {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let (root, schema_version) =
            config_toml::parse_versioned(NAME, text, VersionPolicy::Required)?;
        if schema_version != EVENT_SCHEMA_VERSION {
            return Err(FlintError::ParseError(format!(
                "event script: schema_version {schema_version} not supported (want {EVENT_SCHEMA_VERSION})"
            )));
        }

        let mut events = Vec::new();
        if let Some(arr) = root.get("events").and_then(|v| v.as_array()) {
            for (i, v) in arr.iter().enumerate() {
                let t = v.as_table().ok_or_else(|| bad(&format!("events[{i}]")))?;
                events.push(parse_event(t, i)?);
            }
        }
        Ok(Self {
            schema_version,
            events,
        })
    }
}

fn parse_event(t: &toml::value::Table, i: usize) -> Result<ScheduledEvent> {
    let at_str = t
        .get("at")
        .and_then(|v| v.as_str())
        .ok_or_else(|| bad(&format!("events[{i}].at")))?;
    let at = parse_time(at_str).ok_or_else(|| bad(&format!("events[{i}].at ('{at_str}')")))?;

    let action_str = t
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or_else(|| bad(&format!("events[{i}].action")))?;
    let ramp_ms = t.get("ramp_ms").and_then(toml_f64).unwrap_or(0.0);
    let num = |key: &str| {
        t.get(key)
            .and_then(toml_f64)
            .ok_or_else(|| bad(&format!("events[{i}].{key}")))
    };
    let action = match action_str {
        "set_gain" => BusAction::SetGain {
            db: num("db")? as f32,
            ramp_ms,
        },
        "set_lpf" => BusAction::SetLpf {
            hz: num("hz")?,
            ramp_ms,
        },
        "set_detune" => BusAction::SetDetune {
            semitones: num("semitones")?,
            ramp_ms,
        },
        "marker" => BusAction::Marker(
            t.get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        ),
        other => {
            return Err(FlintError::ParseError(format!(
                "event script: events[{i}].action '{other}' unknown"
            )))
        }
    };

    let bus = t.get("bus").and_then(|v| v.as_str()).map(String::from);
    if bus.is_none() && !matches!(action, BusAction::Marker(_)) {
        return Err(bad(&format!("events[{i}].bus (required for {action_str})")));
    }
    Ok(ScheduledEvent { at, bus, action })
}

/// `bar:N` / `bar:N:B` (1-based, matching the console readout) or
/// `beat:F` / `sample:N` (absolute, 0-based).
fn parse_time(s: &str) -> Option<EventTime> {
    let mut parts = s.split(':');
    match parts.next()? {
        "bar" => {
            let bar: i64 = parts.next()?.parse().ok()?;
            let beat: f64 = match parts.next() {
                Some(b) => b.parse().ok()?,
                None => 1.0,
            };
            if bar < 1 || beat < 1.0 || parts.next().is_some() {
                return None;
            }
            Some(EventTime::BarBeat {
                bar: bar - 1,
                beat: beat - 1.0,
            })
        }
        "beat" => {
            let b: f64 = parts.next()?.parse().ok()?;
            parts.next().is_none().then_some(EventTime::Beat(b))
        }
        "sample" => {
            let n: i64 = parts.next()?.parse().ok()?;
            parts.next().is_none().then_some(EventTime::Sample(n))
        }
        _ => None,
    }
}

const NAME: &str = "event script";

fn bad(what: &str) -> FlintError {
    config_toml::bad(NAME, what)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_forms() {
        let s = EventScript::parse(
            r#"
schema_version = 0

[[events]]
at = "bar:9"
bus = "home_theme"
action = "set_gain"
db = -60.0
ramp_ms = 10.0

[[events]]
at = "bar:13:2.5"
bus = "home_theme"
action = "set_gain"
db = 0

[[events]]
at = "beat:16.5"
bus = "texture"
action = "set_lpf"
hz = 400.0

[[events]]
at = "sample:48000"
action = "marker"
label = "checkpoint"
"#,
        )
        .unwrap();
        assert_eq!(s.events.len(), 4);
        assert_eq!(s.events[0].at, EventTime::BarBeat { bar: 8, beat: 0.0 });
        assert_eq!(s.events[1].at, EventTime::BarBeat { bar: 12, beat: 1.5 });
        // Integer TOML value for a float field must coerce (toml_util rule).
        assert!(matches!(
            s.events[1].action,
            BusAction::SetGain { db, .. } if db == 0.0
        ));
        assert_eq!(s.events[2].at, EventTime::Beat(16.5));
        assert_eq!(s.events[3].at, EventTime::Sample(48_000));
        assert!(matches!(&s.events[3].action, BusAction::Marker(l) if l == "checkpoint"));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(EventScript::parse("schema_version = 1").is_err());
        let bad_bus = r#"
schema_version = 0
[[events]]
at = "bar:1"
action = "set_gain"
db = 0.0
"#;
        assert!(EventScript::parse(bad_bus).is_err(), "bus required");
        let bad_time = r#"
schema_version = 0
[[events]]
at = "bar:0"
bus = "texture"
action = "set_lpf"
hz = 400.0
"#;
        assert!(EventScript::parse(bad_time).is_err(), "bars are 1-based");
    }
}
