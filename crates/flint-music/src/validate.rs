//! Structural and asset validation for suite manifests and beatmap charts.
//!
//! Validation reports findings as [`Issue`]s with stable string codes; the
//! golden fixtures in the game repo pin these codes via `# expect-error:`
//! headers, so renaming a code is a fixture change (propose-and-escalate).

use crate::manifest::SuiteManifest;
use crate::probe::probe_audio;
use crate::{Chart, TempoMap, BUSES, CHANNELS, INTERPS, MOTIF_BUSES, PULSE_KINDS, SCHEMA_VERSION};
use std::collections::HashMap;
use std::fmt;
use std::path::Path;

/// A single validation finding: a stable code (pinned by fixtures) plus a
/// human-readable message.
#[derive(Debug, Clone)]
pub struct Issue {
    pub code: &'static str,
    pub message: String,
}

impl Issue {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

/// Structural manifest validation (everything checkable from the TOML alone).
pub fn validate_manifest(m: &SuiteManifest) -> Vec<Issue> {
    let mut issues = Vec::new();

    if m.schema_version != SCHEMA_VERSION {
        issues.push(Issue::new(
            "UnknownSchemaVersion",
            format!(
                "schema_version {} is not understood (this build knows {SCHEMA_VERSION})",
                m.schema_version
            ),
        ));
    }

    // --- tempo map -----------------------------------------------------------
    match m.tempo.first() {
        None => issues.push(Issue::new("TempoMapMissingOrigin", "tempo map is empty")),
        Some(first) if first.sample != 0 => issues.push(Issue::new(
            "TempoMapMissingOrigin",
            format!("first tempo anchor is at sample {}, must be 0", first.sample),
        )),
        _ => {}
    }
    for pair in m.tempo.windows(2) {
        if pair[1].sample <= pair[0].sample {
            issues.push(Issue::new(
                "TempoAnchorsNotIncreasing",
                format!(
                    "tempo anchor at sample {} does not come after {}",
                    pair[1].sample, pair[0].sample
                ),
            ));
        }
    }
    for a in &m.tempo {
        if a.bpm <= 0.0 || a.beats_per_bar <= 0 || a.beat_unit <= 0 {
            issues.push(Issue::new(
                "TempoAnchorInvalid",
                format!(
                    "tempo anchor at sample {} has non-positive bpm or time signature",
                    a.sample
                ),
            ));
        }
    }

    // --- sections ------------------------------------------------------------
    let tempo_map = TempoMap::new(m.tempo.clone(), m.sample_rate);
    if m.sections.first().map(|s| s.start_sample) != Some(0) {
        issues.push(Issue::new(
            "FirstSectionNotAtZero",
            "the first section must start at sample 0",
        ));
    }
    for pair in m.sections.windows(2) {
        if pair[1].start_sample <= pair[0].start_sample {
            issues.push(Issue::new(
                "SectionsNotOrdered",
                format!(
                    "section '{}' does not start after '{}'",
                    pair[1].name, pair[0].name
                ),
            ));
        }
    }
    let mut seen = HashMap::new();
    for s in &m.sections {
        if seen.insert(s.name.clone(), ()).is_some() {
            issues.push(Issue::new(
                "DuplicateSection",
                format!("section name '{}' appears more than once", s.name),
            ));
        }
        if s.pulse_window_ms <= 0.0 {
            issues.push(Issue::new(
                "PulseWindowInvalid",
                format!("section '{}' has non-positive pulse_window_ms", s.name),
            ));
        }
        if !tempo_map.is_bar_line(s.start_sample) {
            issues.push(Issue::new(
                "SectionOffBarLine",
                format!(
                    "section '{}' starts at sample {}, which is not on a bar line",
                    s.name, s.start_sample
                ),
            ));
        }
    }

    // --- buses ---------------------------------------------------------------
    for bus in BUSES {
        match m.buses.get(bus) {
            None => issues.push(Issue::new(
                "MissingBus",
                format!("bus '{bus}' is not declared (all six buses are required)"),
            )),
            Some(decl) => {
                if decl.file.is_some() == decl.silent {
                    issues.push(Issue::new(
                        "BusFileConflict",
                        format!("bus '{bus}' must declare exactly one of `file` or `silent = true`"),
                    ));
                }
            }
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
    if m.reintegration.re_entry_sections.is_empty() {
        issues.push(Issue::new(
            "NoReEntrySections",
            "at least one re-entry section is required",
        ));
    }
    for name in &m.reintegration.re_entry_sections {
        if !m.sections.iter().any(|s| &s.name == name) {
            issues.push(Issue::new(
                "UnknownReEntrySection",
                format!("re-entry section '{name}' does not name a declared section"),
            ));
        }
    }
    if !MOTIF_BUSES.contains(&m.reintegration.lead_bus.as_str()) {
        issues.push(Issue::new(
            "InvalidLeadBus",
            format!(
                "lead_bus '{}' must be one of {MOTIF_BUSES:?}",
                m.reintegration.lead_bus
            ),
        ));
    }
    if !(1..=2).contains(&m.reintegration.reassembly_bars) {
        issues.push(Issue::new(
            "ReassemblyBarsOutOfRange",
            format!(
                "reassembly_bars {} must be 1 or 2",
                m.reintegration.reassembly_bars
            ),
        ));
    }

    // --- degraded alternates -------------------------------------------------
    for d in &m.degraded_alternates {
        let ok_bus = m
            .buses
            .get(&d.bus)
            .map(|b| b.file.is_some())
            .unwrap_or(false);
        if !ok_bus {
            issues.push(Issue::new(
                "DegradedAlternateInvalid",
                format!("degraded alternate references missing or silent bus '{}'", d.bus),
            ));
        }
        if d.from_sample < 0 || d.to_sample <= d.from_sample {
            issues.push(Issue::new(
                "DegradedAlternateInvalid",
                format!(
                    "degraded alternate on '{}' has an invalid span {}..{}",
                    d.bus, d.from_sample, d.to_sample
                ),
            ));
        }
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
            format!("chart is for suite '{}' but manifest is '{}'", c.suite, m.id),
        ));
    }

    // --- curves --------------------------------------------------------------
    let arity: HashMap<&str, usize> = CHANNELS.iter().copied().collect();
    let mut last_beat: HashMap<&str, f64> = HashMap::new();
    for k in &c.curves {
        match arity.get(k.channel.as_str()) {
            None => issues.push(Issue::new(
                "UnknownChannel",
                format!("channel '{}' is not in the v0 vocabulary", k.channel),
            )),
            Some(&n) => {
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
                // borrow the 'static channel name from the vocabulary table
                let name = CHANNELS
                    .iter()
                    .find(|(n, _)| *n == k.channel.as_str())
                    .map(|(n, _)| *n)
                    .unwrap();
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
                    format!("press at beat {} has strength {s}, must be in (0, 1]", p.beat),
                )),
                _ => {}
            },
            "flick" => match &p.direction {
                None => issues.push(Issue::new(
                    "FlickMissingDirection",
                    format!("flick at beat {} has no direction", p.beat),
                )),
                Some(d) if d.len() != 2 || d.iter().all(|v| *v == 0.0) => {
                    issues.push(Issue::new(
                        "FlickDirectionInvalid",
                        format!("flick at beat {} needs a non-zero vec2 direction", p.beat),
                    ))
                }
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
                format!("cue '{}' at beat {} is before the suite start", cue.cue, cue.beat),
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

/// Asset validation: stem files exist, agree on sample rate, and all playable
/// buses have equal duration. `base_dir` anchors the manifest's relative paths
/// (typically the game repo root).
pub fn validate_manifest_assets(m: &SuiteManifest, base_dir: &Path) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut durations: Vec<(String, u64)> = Vec::new();

    for (bus, decl) in &m.buses {
        let Some(file) = &decl.file else { continue };
        let path = base_dir.join(file);
        if !path.exists() {
            issues.push(Issue::new(
                "MissingStemFile",
                format!("bus '{bus}': file '{file}' not found under {}", base_dir.display()),
            ));
            continue;
        }
        match probe_audio(&path) {
            Err(e) => issues.push(Issue::new(
                "UnreadableStem",
                format!("bus '{bus}': {file}: {e}"),
            )),
            Ok(info) => {
                if info.priming > 0 {
                    issues.push(Issue::new(
                        "StemPrimingUnsafe",
                        format!(
                            "bus '{bus}': {file} carries {} frames of encoder priming, which the \
                             playback stack does not trim — re-export lossless (FLAC/WAV) for \
                             sample-locked stems",
                            info.priming
                        ),
                    ));
                }
                if info.sample_rate != m.sample_rate {
                    issues.push(Issue::new(
                        "SampleRateMismatch",
                        format!(
                            "bus '{bus}': {file} is {} Hz, manifest declares {} Hz",
                            info.sample_rate, m.sample_rate
                        ),
                    ));
                }
                durations.push((bus.clone(), info.frames));
            }
        }
    }

    if let Some((first_bus, first)) = durations.first() {
        for (bus, frames) in &durations[1..] {
            if frames != first {
                issues.push(Issue::new(
                    "StemDurationMismatch",
                    format!(
                        "bus '{bus}' has {frames} frames but '{first_bus}' has {first} — stems must be sample-aligned equal length"
                    ),
                ));
            }
        }
        // degraded alternate spans must fit inside the stems
        for d in &m.degraded_alternates {
            if d.to_sample as u64 > *first {
                issues.push(Issue::new(
                    "DegradedAlternateInvalid",
                    format!(
                        "degraded alternate on '{}' ends at sample {} beyond stem length {}",
                        d.bus, d.to_sample, first
                    ),
                ));
            }
        }
    }

    issues
}
