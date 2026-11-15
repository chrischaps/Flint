//! Suite manifest parsing.
//!
//! Parsing is shape-tolerant on purpose: it accepts any bus names, any section
//! ordering, any version number, and reports *semantic* problems as coded
//! [`Issue`](crate::validate::Issue)s from the validator rather than parse
//! errors. Only structurally unreadable input (missing tables, wrong types)
//! fails the parse.
//!
//! The buses are the manifest's own: as many as it declares, named as it
//! likes, mixed and listed in the order it declares them
//! ([`SuiteManifest::bus_order`]). `[reintegration]` and each section's
//! `pulse_window_ms` are the rhythm layer's (flint-rhythm requires them);
//! a suite that only plays music leaves them out.

use crate::config_toml::{self, VersionPolicy};
use flint_core::{vfs, FlintError, Result};
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
    /// The section's judgment window (a half-width), for the rhythm layer.
    pub pulse_window_ms: Option<f64>,
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
    /// The rhythm layer's seam: where a full fail re-enters and which bus
    /// leads it back. `None` for a suite that only plays music.
    pub reintegration: Option<Reintegration>,
    pub buses: BTreeMap<String, BusDecl>,
    /// Every bus name in `buses`, in the order the manifest declares them:
    /// the order they are built, mixed and listed in.
    pub bus_order: Vec<String>,
    pub degraded_alternates: Vec<DegradedAlternate>,
}

impl SuiteManifest {
    pub fn load(path: &Path) -> Result<Self> {
        let text = vfs::read_to_string(path)?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let (root, schema_version) =
            config_toml::parse_versioned(NAME, text, VersionPolicy::Required)?;

        let suite = table(&root, "suite")?;
        let id = string(suite, "id")?;
        let title = string(suite, "title")?;

        let audio = table(&root, "audio")?;
        // Checked, not `as`: a negative or >u32::MAX value would silently
        // wrap into a plausible-looking rate.
        let sample_rate = u32::try_from(int_in(audio, "sample_rate")?)
            .map_err(|_| bad("audio.sample_rate (out of range for u32)"))?;

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
                pulse_window_ms: match t.get("pulse_window_ms") {
                    None => None,
                    Some(_) => Some(float_in(t, "pulse_window_ms")?),
                },
            });
        }

        let reintegration = match root.get("reintegration") {
            None => None,
            Some(_) => {
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
                Some(Reintegration {
                    re_entry_sections,
                    lead_bus: string(reint, "lead_bus")?,
                    reassembly_bars: int_in(reint, "reassembly_bars")?,
                })
            }
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

        let bus_order = declared_order(text, &buses);

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
            bus_order,
            degraded_alternates,
        })
    }

    /// The buses in declared order, each with its declaration: `bus_order`,
    /// then any bus it doesn't name (a manifest built in code), by name.
    pub fn buses_in_order(&self) -> impl Iterator<Item = (&str, &BusDecl)> {
        let listed = self
            .bus_order
            .iter()
            .filter_map(|name| self.buses.get_key_value(name));
        let rest = self
            .buses
            .iter()
            .filter(|(name, _)| !self.bus_order.contains(name));
        listed.chain(rest).map(|(name, decl)| (name.as_str(), decl))
    }
}

/// The bus names in the order `text` declares them. `toml::Value` keeps a
/// table's keys sorted, so the order comes from `toml_edit`, which keeps the
/// document's; a name it somehow misses goes after, alphabetically.
fn declared_order(text: &str, buses: &BTreeMap<String, BusDecl>) -> Vec<String> {
    let mut order: Vec<String> = text
        .parse::<toml_edit::DocumentMut>()
        .ok()
        .and_then(|doc| {
            doc.get("buses")
                .and_then(|b| b.as_table_like())
                .map(|t| t.iter().map(|(k, _)| k.to_string()).collect())
        })
        .unwrap_or_default();
    order.retain(|name| buses.contains_key(name));
    for name in buses.keys() {
        if !order.contains(name) {
            order.push(name.clone());
        }
    }
    order
}

// -- small extraction helpers (FlintError::ParseError on shape problems) ------

const NAME: &str = "suite manifest";

fn bad(what: &str) -> FlintError {
    config_toml::bad(NAME, what)
}

fn table<'a>(v: &'a toml::Value, key: &str) -> Result<&'a toml::value::Table> {
    config_toml::table(NAME, v, key)
}

fn array<'a>(v: &'a toml::Value, key: &str) -> Result<&'a Vec<toml::Value>> {
    config_toml::array(NAME, v, key)
}

fn int_in(t: &toml::value::Table, key: &str) -> Result<i64> {
    config_toml::int_in(NAME, t, key)
}

fn float_in(t: &toml::value::Table, key: &str) -> Result<f64> {
    config_toml::float_in(NAME, t, key, key)
}

fn string(t: &toml::value::Table, key: &str) -> Result<String> {
    config_toml::string_in(NAME, t, key, key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_BUSES: &str = r#"
schema_version = 0
[suite]
id = "two"
title = "Two"
[audio]
sample_rate = 48000
[[tempo]]
sample = 0
bpm = 120.0
time_signature = [4, 4]
[[sections]]
name = "a"
start_sample = 0
[buses.zither]
file = "stems/zither.wav"
[buses.bass]
file = "stems/bass.wav"
"#;

    #[test]
    fn buses_are_the_manifests_own_in_declared_order() {
        let m = SuiteManifest::parse(TWO_BUSES).unwrap();
        assert_eq!(m.bus_order, ["zither", "bass"]);
        let names: Vec<&str> = m.buses_in_order().map(|(n, _)| n).collect();
        assert_eq!(
            names,
            ["zither", "bass"],
            "not the map's alphabetical order"
        );
        assert!(m.reintegration.is_none());
        assert_eq!(m.sections[0].pulse_window_ms, None);
        assert!(
            crate::validate_manifest(&m).is_empty(),
            "{:?}",
            crate::validate_manifest(&m)
        );
    }

    #[test]
    fn a_reintegration_table_and_pulse_windows_still_parse() {
        let text = TWO_BUSES.replace(
            "start_sample = 0\n",
            "start_sample = 0\npulse_window_ms = 90\n",
        ) + "[reintegration]\nre_entry_sections = [\"a\"]\nlead_bus = \"bass\"\nreassembly_bars = 2\n";
        let m = SuiteManifest::parse(&text).unwrap();
        assert_eq!(m.sections[0].pulse_window_ms, Some(90.0));
        let r = m.reintegration.expect("parsed");
        assert_eq!((r.lead_bus.as_str(), r.reassembly_bars), ("bass", 2));
    }

    #[test]
    fn a_manifest_built_in_code_lists_every_bus() {
        let mut m = SuiteManifest::parse(TWO_BUSES).unwrap();
        m.bus_order.clear();
        let names: Vec<&str> = m.buses_in_order().map(|(n, _)| n).collect();
        assert_eq!(names, ["bass", "zither"]);
    }

    #[test]
    fn no_buses_or_only_silent_ones_is_an_issue() {
        let silent = TWO_BUSES
            .replace("file = \"stems/zither.wav\"", "silent = true")
            .replace("file = \"stems/bass.wav\"", "silent = true");
        let codes: Vec<_> = crate::validate_manifest(&SuiteManifest::parse(&silent).unwrap())
            .into_iter()
            .map(|i| i.code)
            .collect();
        assert_eq!(codes, ["NoPlayableBus"]);
    }
}
