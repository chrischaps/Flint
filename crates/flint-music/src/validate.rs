//! Structural and asset validation for suite manifests.
//!
//! Validation reports findings as [`Issue`]s with stable string codes; a
//! game's fixtures may pin them (`# expect-error:` headers), so renaming a
//! code is a fixture change. This is the general layer's contract: a tempo
//! map, sections on bar lines, and any number of buses, each named by the
//! manifest. A rhythm game's own rules (the fixed six buses, `[reintegration]`,
//! pulse windows, charts) are `flint_rhythm::validate_manifest` and
//! `flint_rhythm::validate_chart`, which run this pass first.

use crate::manifest::SuiteManifest;
use crate::probe::probe_audio;
use crate::{TempoMap, SCHEMA_VERSION};
use flint_core::vfs;
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
    /// A finding with its stable code. Public so a layer over this one (the
    /// rhythm layer's contract) reports in the same shape.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
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
            format!(
                "first tempo anchor is at sample {}, must be 0",
                first.sample
            ),
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
        // A judgment window is the rhythm layer's (which requires one); a
        // section that gives one anyway must give a usable one.
        if s.pulse_window_ms.is_some_and(|w| w <= 0.0) {
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
    // Any names, as many as the suite has: the manifest's own set is the mix.
    if m.buses.is_empty() {
        issues.push(Issue::new(
            "NoBuses",
            "the manifest declares no buses (`[buses.<name>]` with a `file`)",
        ));
    }
    for (bus, decl) in &m.buses {
        if decl.file.is_some() == decl.silent {
            issues.push(Issue::new(
                "BusFileConflict",
                format!("bus '{bus}' must declare exactly one of `file` or `silent = true`"),
            ));
        }
    }
    if !m.buses.is_empty() && m.buses.values().all(|b| b.file.is_none()) {
        issues.push(Issue::new(
            "NoPlayableBus",
            "every bus is silent: at least one must have a `file`",
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
                format!(
                    "degraded alternate references missing or silent bus '{}'",
                    d.bus
                ),
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

/// Asset validation: stem files exist, agree on sample rate, and all playable
/// buses have equal duration. `base_dir` anchors the manifest's relative paths
/// (typically the game repo root).
pub fn validate_manifest_assets(m: &SuiteManifest, base_dir: &Path) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut durations: Vec<(String, u64)> = Vec::new();

    for (bus, decl) in &m.buses {
        let Some(file) = &decl.file else { continue };
        let path = base_dir.join(file);
        if !vfs::exists(&path) {
            issues.push(Issue::new(
                "MissingStemFile",
                format!(
                    "bus '{bus}': file '{file}' not found under {}",
                    base_dir.display()
                ),
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
