//! Beatmap chart parsing. Same shape-tolerant posture as the manifest parser:
//! unknown channels/kinds parse fine and are reported by the validator.

use flint_core::toml_util::toml_f64;
use flint_core::{FlintError, Result};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct CurveKey {
    pub channel: String,
    pub beat: f64,
    pub value: Vec<f64>,
    pub interp: String,
}

#[derive(Debug, Clone)]
pub struct Pulse {
    pub beat: f64,
    pub kind: String,
    pub window_ms: Option<f64>,
    pub strength: Option<f64>,
    pub direction: Option<Vec<f64>>,
}

#[derive(Debug, Clone)]
pub struct Cue {
    pub beat: f64,
    pub cue: String,
    pub params: Option<toml::value::Table>,
}

#[derive(Debug, Clone)]
pub struct IntensityKey {
    pub beat: f64,
    pub value: f64,
}

#[derive(Debug, Clone)]
pub struct Chart {
    pub schema_version: i64,
    pub suite: String,
    pub curves: Vec<CurveKey>,
    pub pulses: Vec<Pulse>,
    pub cues: Vec<Cue>,
    pub intensity: Vec<IntensityKey>,
}

impl Chart {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let root: toml::Value = text
            .parse()
            .map_err(|e| FlintError::TomlParseError(format!("chart: {e}")))?;

        let schema_version = root
            .get("schema_version")
            .and_then(|v| v.as_integer())
            .ok_or_else(|| bad("schema_version"))?;
        let suite = root
            .get("suite")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| bad("suite"))?;

        let mut curves = Vec::new();
        for (i, t) in tables(&root, "curves").iter().enumerate() {
            curves.push(CurveKey {
                channel: string(t, &format!("curves[{i}].channel"), "channel")?,
                beat: float(t, &format!("curves[{i}].beat"), "beat")?,
                value: floats(t.get("value"), &format!("curves[{i}].value"))?,
                interp: string(t, &format!("curves[{i}].interp"), "interp")?,
            });
        }

        let mut pulses = Vec::new();
        for (i, t) in tables(&root, "pulses").iter().enumerate() {
            pulses.push(Pulse {
                beat: float(t, &format!("pulses[{i}].beat"), "beat")?,
                kind: t
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .unwrap_or("pulse")
                    .to_string(),
                window_ms: t.get("window_ms").and_then(toml_f64),
                strength: t.get("strength").and_then(toml_f64),
                direction: match t.get("direction") {
                    Some(v) => Some(floats(Some(v), &format!("pulses[{i}].direction"))?),
                    None => None,
                },
            });
        }

        let mut cues = Vec::new();
        for (i, t) in tables(&root, "cues").iter().enumerate() {
            cues.push(Cue {
                beat: float(t, &format!("cues[{i}].beat"), "beat")?,
                cue: string(t, &format!("cues[{i}].cue"), "cue")?,
                params: t.get("params").and_then(|v| v.as_table()).cloned(),
            });
        }

        let mut intensity = Vec::new();
        for (i, t) in tables(&root, "intensity").iter().enumerate() {
            intensity.push(IntensityKey {
                beat: float(t, &format!("intensity[{i}].beat"), "beat")?,
                value: float(t, &format!("intensity[{i}].value"), "value")?,
            });
        }

        Ok(Chart {
            schema_version,
            suite,
            curves,
            pulses,
            cues,
            intensity,
        })
    }
}

fn bad(what: &str) -> FlintError {
    FlintError::ParseError(format!("chart: missing or malformed `{what}`"))
}

fn tables<'a>(root: &'a toml::Value, key: &str) -> Vec<&'a toml::value::Table> {
    root.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_table()).collect())
        .unwrap_or_default()
}

fn string(t: &toml::value::Table, ctx: &str, key: &str) -> Result<String> {
    t.get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| bad(ctx))
}

fn float(t: &toml::value::Table, ctx: &str, key: &str) -> Result<f64> {
    t.get(key).and_then(toml_f64).ok_or_else(|| bad(ctx))
}

fn floats(v: Option<&toml::Value>, ctx: &str) -> Result<Vec<f64>> {
    v.and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(|e| toml_f64(e).ok_or_else(|| bad(ctx))).collect())
        .unwrap_or_else(|| Err(bad(ctx)))
}
