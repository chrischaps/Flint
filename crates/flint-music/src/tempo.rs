//! Tempo-map time conversion: sample position <-> musical time (beats, bars).
//!
//! This is the arithmetic core the Conductor will be built on: an ordered list
//! of `(sample, bpm, time_signature)` anchors, where each anchor starts a new
//! bar. Meter changes are a first-class design feature, so everything works
//! per-segment rather than assuming one global BPM.

use crate::manifest::TempoAnchor;

pub struct TempoMap {
    anchors: Vec<TempoAnchor>,
    sample_rate: u32,
}

impl TempoMap {
    /// Anchors must be validated (origin at 0, strictly increasing) before
    /// conversion results mean anything; construction itself is permissive.
    pub fn new(anchors: Vec<TempoAnchor>, sample_rate: u32) -> Self {
        Self {
            anchors,
            sample_rate,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Samples per beat under a given anchor.
    fn samples_per_beat(&self, anchor: &TempoAnchor) -> f64 {
        self.sample_rate as f64 * 60.0 / anchor.bpm
    }

    /// The anchor governing a sample position (the last anchor at or before it).
    pub fn anchor_at(&self, sample: i64) -> Option<&TempoAnchor> {
        self.anchors
            .iter()
            .take_while(|a| a.sample <= sample)
            .last()
    }

    /// Beats elapsed since the governing anchor at `sample`.
    pub fn beats_since_anchor(&self, sample: i64) -> Option<f64> {
        let anchor = self.anchor_at(sample)?;
        Some((sample - anchor.sample) as f64 / self.samples_per_beat(anchor))
    }

    /// Beats from suite start (sample 0) to `sample`, accumulating across
    /// anchors. Returns None if the map is empty or has no origin anchor.
    pub fn beats_at_sample(&self, sample: i64) -> Option<f64> {
        if self.anchors.first().map(|a| a.sample)? != 0 {
            return None;
        }
        let mut beats = 0.0;
        for (i, a) in self.anchors.iter().enumerate() {
            let seg_end = self
                .anchors
                .get(i + 1)
                .map(|n| n.sample.min(sample))
                .unwrap_or(sample);
            if seg_end <= a.sample {
                break;
            }
            beats += (seg_end - a.sample) as f64 / self.samples_per_beat(a);
            if seg_end == sample {
                break;
            }
        }
        Some(beats)
    }

    /// Whether `sample` lands on a bar line, within half a sample of tolerance.
    /// Each anchor is defined to start a new bar.
    pub fn is_bar_line(&self, sample: i64) -> bool {
        let Some(anchor) = self.anchor_at(sample) else {
            return false;
        };
        if anchor.beats_per_bar <= 0 {
            return false;
        }
        let spb = self.samples_per_beat(anchor);
        let beats = (sample - anchor.sample) as f64 / spb;
        let bars = beats / anchor.beats_per_bar as f64;
        // tolerance: half a sample, expressed in bars
        let tol = 0.5 / (spb * anchor.beats_per_bar as f64);
        (bars - bars.round()).abs() <= tol
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> TempoMap {
        // 48 kHz: 120 BPM 4/4 until sample 960_000 (= 40 beats = 10 bars),
        // then 90 BPM 3/4.
        TempoMap::new(
            vec![
                TempoAnchor {
                    sample: 0,
                    bpm: 120.0,
                    beats_per_bar: 4,
                    beat_unit: 4,
                },
                TempoAnchor {
                    sample: 960_000,
                    bpm: 90.0,
                    beats_per_bar: 3,
                    beat_unit: 4,
                },
            ],
            48_000,
        )
    }

    #[test]
    fn beats_accumulate_across_meter_change() {
        let m = map();
        assert_eq!(m.beats_at_sample(0), Some(0.0));
        // 120 BPM @ 48kHz -> 24_000 samples per beat
        assert_eq!(m.beats_at_sample(24_000), Some(1.0));
        assert_eq!(m.beats_at_sample(960_000), Some(40.0));
        // 90 BPM -> 32_000 samples per beat
        assert_eq!(m.beats_at_sample(960_000 + 32_000), Some(41.0));
    }

    #[test]
    fn bar_lines_respect_each_meter() {
        let m = map();
        assert!(m.is_bar_line(0));
        assert!(m.is_bar_line(96_000)); // bar 2 in 4/4
        assert!(!m.is_bar_line(24_000)); // beat 2 is not a bar line
        assert!(m.is_bar_line(960_000)); // anchors start bars
        assert!(m.is_bar_line(960_000 + 3 * 32_000)); // one 3/4 bar later
        assert!(!m.is_bar_line(960_000 + 4 * 32_000)); // 4 beats is mid-bar in 3/4
    }
}
