//! Suite manifest parsing.
//!
//! Parsing is shape-tolerant on purpose: it accepts any bus names, any section
//! ordering, any version number, and reports *semantic* problems as coded
//! [`Issue`](crate::validate::Issue)s from the validator rather than parse
//! errors. Only structurally unreadable input (missing tables, wrong types)
//! fails the parse.

use flint_core::toml_util::toml_f64;
use flint_core::{FlintError, Result};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct TempoAnchor {
    pub sample: i64,
    pub bpm: f64,
    pub beats_per_bar: i64,
    pub beat_unit: i64,
}

#[derive(Debug, Clone)]
pub struct Section {
    pub name: String,
    pub start_sample: i64,
    pub pulse_window_ms: f64,
}

#[derive(Debug, Clone)]
pub struct Reintegration {
    pub re_entry_sections: Vec<String>,
    pub lead_bus: String,
    pub reassembly_bars: i64,
}

#[derive(Debug, Clone, Default)]
pub struct BusDecl {
    pub file: Option<String>,
    pub silent: bool,
}

#[derive(Debug, Clone)]
pub struct DegradedAlternate {
    pub bus: String,
    pub file: String,
    pub from_sample: i64,
    pub to_sample: i64,
}

#[derive(Debug, Clone)]
pub struct SuiteManifest {
    pub schema_version: i64,
    pub id: String,
    pub title: String,
    pub sample_rate: u32,
    pub tempo: Vec<TempoAnchor>,
    pub sections: Vec<Section>,
    pub reintegration: Reintegration,
    pub buses: BTreeMap<String, BusDecl>,
    pub degraded_alternates: Vec<DegradedAlternate>,
}

impl SuiteManifest {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let root: toml::Value = text
            .parse()
            .map_err(|e| FlintError::TomlParseError(format!("suite manifest: {e}")))?;

        let schema_version = int(&root, "schema_version")?;

        let suite = table(&root, "suite")?;
        let id = string(suite, "id")?;
        let title = string(suite, "title")?;

        let audio = table(&root, "audio")?;
        let sample_rate = int_in(audio, "sample_rate")? as u32;

        let mut tempo = Vec::new();
        for (i, v) in array(&root, "tempo")?.iter().enumerate() {
            let t = v.as_table().ok_or_else(|| bad(&format!("tempo[{i}]")))?;
            let sig = t
                .get("time_signature")
                .and_then(|v| v.as_array())
                .ok_or_else(|| bad(&format!("tempo[{i}].time_signature")))?;
            if sig.len() != 2 {
                return Err(bad(&format!(
                    "tempo[{i}].time_signature (need [beats, unit])"
                )));
            }
            tempo.push(TempoAnchor {
                sample: int_in(t, "sample")?,
                bpm: float_in(t, "bpm")?,
                beats_per_bar: sig[0]
                    .as_integer()
                    .ok_or_else(|| bad(&format!("tempo[{i}].time_signature[0]")))?,
                beat_unit: sig[1]
                    .as_integer()
                    .ok_or_else(|| bad(&format!("tempo[{i}].time_signature[1]")))?,
            });
        }

        let mut sections = Vec::new();
        for (i, v) in array(&root, "sections")?.iter().enumerate() {
            let t = v.as_table().ok_or_else(|| bad(&format!("sections[{i}]")))?;
            sections.push(Section {
                name: string(t, "name")?,
                start_sample: int_in(t, "start_sample")?,
                pulse_window_ms: float_in(t, "pulse_window_ms")?,
            });
        }

        let reint = table(&root, "reintegration")?;
        let re_entry_sections = reint
            .get("re_entry_sections")
            .and_then(|v| v.as_array())
            .ok_or_else(|| bad("reintegration.re_entry_sections"))?
            .iter()
            .map(|v| {
                v.as_str()
                    .map(String::from)
                    .ok_or_else(|| bad("reintegration.re_entry_sections entry"))
            })
            .collect::<Result<Vec<_>>>()?;
        let reintegration = Reintegration {
            re_entry_sections,
            lead_bus: string(reint, "lead_bus")?,
            reassembly_bars: int_in(reint, "reassembly_bars")?,
        };

        let mut buses = BTreeMap::new();
        for (name, v) in table(&root, "buses")?.iter() {
            let t = v.as_table().ok_or_else(|| bad(&format!("buses.{name}")))?;
            buses.insert(
                name.clone(),
                BusDecl {
                    file: t.get("file").and_then(|v| v.as_str()).map(String::from),
                    silent: t.get("silent").and_then(|v| v.as_bool()).unwrap_or(false),
                },
            );
        }

        let mut degraded_alternates = Vec::new();
        if let Some(arr) = root.get("degraded_alternates").and_then(|v| v.as_array()) {
            for (i, v) in arr.iter().enumerate() {
                let t = v
                    .as_table()
                    .ok_or_else(|| bad(&format!("degraded_alternates[{i}]")))?;
                degraded_alternates.push(DegradedAlternate {
                    bus: string(t, "bus")?,
                    file: string(t, "file")?,
                    from_sample: int_in(t, "from_sample")?,
                    to_sample: int_in(t, "to_sample")?,
                });
            }
        }

        Ok(SuiteManifest {
            schema_version,
            id,
            title,
            sample_rate,
            tempo,
            sections,
            reintegration,
            buses,
            degraded_alternates,
        })
    }
}

// -- small extraction helpers (FlintError::ParseError on shape problems) ------

fn bad(what: &str) -> FlintError {
    FlintError::ParseError(format!("suite manifest: missing or malformed `{what}`"))
}

fn table<'a>(v: &'a toml::Value, key: &str) -> Result<&'a toml::value::Table> {
    v.get(key)
        .and_then(|v| v.as_table())
        .ok_or_else(|| bad(key))
}

fn array<'a>(v: &'a toml::Value, key: &str) -> Result<&'a Vec<toml::Value>> {
    v.get(key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| bad(key))
}

fn int(v: &toml::Value, key: &str) -> Result<i64> {
    v.get(key)
        .and_then(|v| v.as_integer())
        .ok_or_else(|| bad(key))
}

fn int_in(t: &toml::value::Table, key: &str) -> Result<i64> {
    t.get(key)
        .and_then(|v| v.as_integer())
        .ok_or_else(|| bad(key))
}

fn float_in(t: &toml::value::Table, key: &str) -> Result<f64> {
    t.get(key).and_then(toml_f64).ok_or_else(|| bad(key))
}

fn string(t: &toml::value::Table, key: &str) -> Result<String> {
    t.get(key)
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| bad(key))
}
