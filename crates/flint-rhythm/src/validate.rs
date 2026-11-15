//! The rhythm layer's contract over a suite: the six buses, `[reintegration]`,
//! a judgment window on every section, and the beatmap chart.
//!
//! [`validate_manifest`] runs the general pass
//! ([`flint_music::validate_manifest`]) first and adds this layer's rules;
//! [`validate_chart`] cross-checks a chart against its manifest. Findings are
//! [`Issue`]s with stable codes; a rhythm game's fixtures pin them via
//! `# expect-error:` headers, so renaming a code is a fixture change.

use crate::{Chart, BUSES, CHANNELS, INTERPS, MOTIF_BUSES, PULSE_KINDS};
use flint_music::{Issue, SuiteManifest, SCHEMA_VERSION};
use std::collections::HashMap;

/// A rhythm suite's manifest: the general checks, then the six-bus layout,
/// motif isolation, `[reintegration]` and the sections' judgment windows.
pub fn validate_manifest(m: &SuiteManifest) -> Vec<Issue> {
    let mut issues = flint_music::validate_manifest(m);

    // --- sections: every one is judged ---------------------------------------
    for s in &m.sections {
        if s.pulse_window_ms.is_none() {
            issues.push(Issue::new(
                "PulseWindowMissing",
                format!("section '{}' has no pulse_window_ms", s.name),
            ));
        }
    }

    // --- buses: the fixed six ------------------------------------------------
    for bus in BUSES {
        if !m.buses.contains_key(bus) {
            issues.push(Issue::new(
                "MissingBus",
                format!("bus '{bus}' is not declared (all six buses are required)"),
            ));
        }
    }
    for name in m.buses.keys() {
        if !BUSES.contains(&name.as_str()) {
            issues.push(Issue::new(
                "UnknownBus",
                format!("bus '{name}' is not part of the fixed six-bus layout"),
            ));
        }
    }
    for motif in MOTIF_BUSES {
        let Some(file) = m.buses.get(motif).and_then(|b| b.file.as_deref()) else {
            continue;
        };
        for (other, decl) in &m.buses {
            if other != motif && decl.file.as_deref() == Some(file) {
                issues.push(Issue::new(
                    "MotifBusShared",
                    format!(
                        "motif bus '{motif}' shares file '{file}' with bus '{other}' — motif buses must stay isolated"
                    ),
                ));
            }
        }
    }

    // --- reintegration -------------------------------------------------------
    let Some(reintegration) = &m.reintegration else {
        issues.push(Issue::new(
            "MissingReintegration",
            "a rhythm suite needs a [reintegration] table (re_entry_sections, lead_bus, reassembly_bars)",
        ));
        return issues;
    };
    if reintegration.re_entry_sections.is_empty() {
        issues.push(Issue::new(
            "NoReEntrySections",
            "at least one re-entry section is required",
        ));
    }
    for name in &reintegration.re_entry_sections {
        if !m.sections.iter().any(|s| &s.name == name) {
            issues.push(Issue::new(
                "UnknownReEntrySection",
                format!("re-entry section '{name}' does not name a declared section"),
            ));
        }
    }
    if !MOTIF_BUSES.contains(&reintegration.lead_bus.as_str()) {
        issues.push(Issue::new(
            "InvalidLeadBus",
            format!(
                "lead_bus '{}' must be one of {MOTIF_BUSES:?}",
                reintegration.lead_bus
            ),
        ));
    }
    if !(1..=2).contains(&reintegration.reassembly_bars) {
        issues.push(Issue::new(
            "ReassemblyBarsOutOfRange",
            format!(
                "reassembly_bars {} must be 1 or 2",
                reintegration.reassembly_bars
            ),
        ));
    }

    issues
}

/// Structural chart validation, cross-checked against its manifest.
pub fn validate_chart(c: &Chart, m: &SuiteManifest) -> Vec<Issue> {
    let mut issues = Vec::new();

    if c.schema_version != SCHEMA_VERSION {
        issues.push(Issue::new(
            "UnknownSchemaVersion",
            format!(
                "schema_version {} is not understood (this build knows {SCHEMA_VERSION})",
                c.schema_version
            ),
        ));
    }
    if c.suite != m.id {
        issues.push(Issue::new(
            "SuiteIdMismatch",
            format!(
                "chart is for suite '{}' but manifest is '{}'",
                c.suite, m.id
            ),
        ));
    }

    // --- curves --------------------------------------------------------------
    let arity: HashMap<&str, usize> = CHANNELS.iter().copied().collect();
    let mut last_beat: HashMap<&str, f64> = HashMap::new();
    for k in &c.curves {
        // get_key_value: the map key IS the 'static vocabulary name, so the
        // later `last_beat` insert needs no second lookup (and no unwrap).
        match arity.get_key_value(k.channel.as_str()) {
            None => issues.push(Issue::new(
                "UnknownChannel",
                format!("channel '{}' is not in the v0 vocabulary", k.channel),
            )),
            Some((&name, &n)) => {
                if k.value.len() != n {
                    issues.push(Issue::new(
                        "ChannelArityMismatch",
                        format!(
                            "channel '{}' expects {n} value component(s), got {}",
                            k.channel,
                            k.value.len()
                        ),
                    ));
                } else {
                    let (lo, hi) = if n == 2 { (-1.0, 1.0) } else { (0.0, 1.0) };
                    if k.value.iter().any(|v| *v < lo || *v > hi) {
                        issues.push(Issue::new(
                            "CurveValueOutOfRange",
                            format!(
                                "channel '{}' at beat {} has value outside [{lo}, {hi}]",
                                k.channel, k.beat
                            ),
                        ));
                    }
                }
                if let Some(prev) = last_beat.get(k.channel.as_str()) {
                    if k.beat <= *prev {
                        issues.push(Issue::new(
                            "CurveKeysNotIncreasing",
                            format!(
                                "channel '{}' key at beat {} does not come after beat {}",
                                k.channel, k.beat, prev
                            ),
                        ));
                    }
                }
                last_beat.insert(name, k.beat);
            }
        }
        if !INTERPS.contains(&k.interp.as_str()) {
            issues.push(Issue::new(
                "UnknownInterp",
                format!("interp '{}' is not one of {INTERPS:?}", k.interp),
            ));
        }
    }

    // --- pulses --------------------------------------------------------------
    for p in &c.pulses {
        if p.beat < 0.0 {
            issues.push(Issue::new(
                "PulseOutOfRange",
                format!("pulse at beat {} is before the suite start", p.beat),
            ));
        }
        if !PULSE_KINDS.contains(&p.kind.as_str()) {
            issues.push(Issue::new(
                "UnknownPulseKind",
                format!("pulse kind '{}' is not one of {PULSE_KINDS:?}", p.kind),
            ));
            continue;
        }
        match p.kind.as_str() {
            "press" => match p.strength {
                None => issues.push(Issue::new(
                    "PressMissingStrength",
                    format!("press at beat {} has no strength target", p.beat),
                )),
                Some(s) if !(s > 0.0 && s <= 1.0) => issues.push(Issue::new(
                    "StrengthOutOfRange",
                    format!(
                        "press at beat {} has strength {s}, must be in (0, 1]",
                        p.beat
                    ),
                )),
                _ => {}
            },
            "flick" => match &p.direction {
                None => issues.push(Issue::new(
                    "FlickMissingDirection",
                    format!("flick at beat {} has no direction", p.beat),
                )),
                Some(d) if d.len() != 2 || d.iter().all(|v| *v == 0.0) => issues.push(Issue::new(
                    "FlickDirectionInvalid",
                    format!("flick at beat {} needs a non-zero vec2 direction", p.beat),
                )),
                _ => {}
            },
            _ => {
                if p.strength.is_some() || p.direction.is_some() {
                    issues.push(Issue::new(
                        "PulseFieldMismatch",
                        format!(
                            "pulse at beat {} carries strength/direction, which only press/flick use",
                            p.beat
                        ),
                    ));
                }
            }
        }
        if let Some(w) = p.window_ms {
            let beat_ms = m
                .tempo
                .first()
                .map(|a| 60_000.0 / a.bpm)
                .unwrap_or(f64::MAX);
            if w <= 0.0 || w >= beat_ms {
                issues.push(Issue::new(
                    "WindowOverrideInvalid",
                    format!(
                        "pulse at beat {} has window_ms {w}, must be positive and under one beat",
                        p.beat
                    ),
                ));
            }
        }
    }

    // --- cues + intensity ----------------------------------------------------
    for cue in &c.cues {
        if cue.beat < 0.0 {
            issues.push(Issue::new(
                "CueOutOfRange",
                format!(
                    "cue '{}' at beat {} is before the suite start",
                    cue.cue, cue.beat
                ),
            ));
        }
    }
    for k in &c.intensity {
        if !(0.0..=1.0).contains(&k.value) {
            issues.push(Issue::new(
                "IntensityOutOfRange",
                format!("intensity {} at beat {} must be in [0, 1]", k.value, k.beat),
            ));
        }
    }

    issues
}
