//! Debug-timeline data (ADR 0037): the static suite map plus this-run
//! history marks that the player's Manifest Map panel draws. Dev surface
//! only, never the player-facing world (the no-gamified-UI rule). Retained
//! histories are written by `ChartCore` and read by
//! [`ChartSession::timeline_frame`](crate::chart_session::ChartSession::timeline_frame)
//! — no deterministic path reads them.

use crate::chart_session::PulseKind;
use crate::conductor::Conductor;
use crate::manifest::SuiteManifest;

/// The suite's static structure for the debug timeline map: everything the
/// manifest pins that a strip-chart overlay wants to draw. Built once at
/// session open ([`TimelineMap::build`]); dev surface only, never the
/// player-facing world (the no-gamified-UI rule).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TimelineMap {
    pub sample_rate: u32,
    /// Suite-axis extent of the map: authored content end (last judgment
    /// window close), rounded up to the next bar line.
    pub end_sample: i64,
    pub sections: Vec<TimelineSection>,
    /// Every bar line within `end_sample`, precomputed so the overlay needs
    /// no tempo math (correct across tempo/meter changes by construction).
    pub bars: Vec<BarMark>,
    /// The manifest's tempo anchors, verbatim (index 0 is the opening
    /// tempo/meter, not a "change").
    pub tempo_marks: Vec<TempoMark>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineSection {
    pub name: String,
    pub start_sample: i64,
    /// The next section's start (the map's `end_sample` for the last one) —
    /// sections carry no authored end.
    pub end_sample: i64,
    /// Listed in the manifest's `reintegration.re_entry_sections`.
    pub re_entry: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarMark {
    /// Bar index, counted from 0 (the tempo map's convention).
    pub bar: i64,
    pub sample: i64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoMark {
    pub sample: i64,
    pub bpm: f64,
    pub beats_per_bar: i64,
    pub beat_unit: i64,
}

impl TimelineMap {
    /// Snapshot the manifest's structure. `judge_end_sample` is the judge's
    /// authored-content end ([`Judge::end_sample`]); the map extends to at
    /// least the last section start, rounded up to a bar line.
    pub fn build(manifest: &SuiteManifest, conductor: &Conductor, judge_end_sample: i64) -> Self {
        let last_section_start = manifest
            .sections
            .iter()
            .map(|s| s.start_sample)
            .max()
            .unwrap_or(0);
        let raw_end = judge_end_sample.max(last_section_start).max(0);
        // Round up to the next bar line so the strip ends on musical ground.
        let end_bar = conductor.position_at_sample(raw_end).bar + 1;
        let end_sample = conductor.sample_at_bar(end_bar, 0.0);

        let mut sections: Vec<TimelineSection> = manifest
            .sections
            .iter()
            .map(|s| TimelineSection {
                name: s.name.clone(),
                start_sample: s.start_sample,
                end_sample, // patched below from the next section's start
                re_entry: manifest.reintegration.re_entry_sections.contains(&s.name),
            })
            .collect();
        for i in 0..sections.len().saturating_sub(1) {
            sections[i].end_sample = sections[i + 1].start_sample;
        }

        let mut bars = Vec::new();
        // Hard cap defends against a degenerate tempo map, not real content.
        for bar in 0..10_000 {
            let sample = conductor.sample_at_bar(bar, 0.0);
            if sample > end_sample {
                break;
            }
            bars.push(BarMark { bar, sample });
        }

        Self {
            sample_rate: manifest.sample_rate,
            end_sample,
            sections,
            bars,
            tempo_marks: manifest
                .tempo
                .iter()
                .map(|a| TempoMark {
                    sample: a.sample,
                    bpm: a.bpm,
                    beats_per_bar: a.beats_per_bar,
                    beat_unit: a.beat_unit,
                })
                .collect(),
        }
    }
}

/// A reintegration-sequencer event retained for the debug timeline (this-run
/// history). Suite-sample addressed; pushed by [`ChartCore::step_seq`] as
/// events fire, read only by [`ChartSession::timeline_frame`] — no
/// deterministic path touches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeamMarkKind {
    /// Full-fail latched: the rewind gesture begins here.
    FullFail,
    /// The seam committed: playback re-entered at `re_entry_sample`.
    Seam,
    /// Reassembly ramp finished.
    ReassemblyComplete,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeamMark {
    pub kind: SeamMarkKind,
    /// Where on the suite axis the event happened (for `Seam` this IS the
    /// re-entry point — post-jump suite position).
    pub suite_sample: i64,
    pub re_entry_sample: i64,
}

/// One judged pulse retained for the debug timeline (this-run history).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PulseMark {
    pub sample: i64,
    /// Timing error in ms (hits only; 0.0 for miss/spurious).
    pub err_ms: f64,
    pub kind: PulseKind,
}

/// Per-frame debug-timeline snapshot: the playhead plus this-run history.
/// Pairs with the static [`TimelineMap`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TimelineFrame {
    /// Latency-compensated suite sample (negative during preroll).
    pub playhead_sample: i64,
    /// The monotonic raw clock (never rewinds).
    pub raw_clock_sample: i64,
    /// `raw − suite`; changes once per committed seam.
    pub timeline_offset: i64,
    pub preroll: bool,
    pub seams: Vec<SeamMark>,
    pub pulses: Vec<PulseMark>,
}
