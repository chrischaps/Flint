//! Tempo-map time conversion: sample position <-> musical time (beats, bars).
//!
//! This is the arithmetic core the Conductor will be built on: an ordered list
//! of `(sample, bpm, time_signature)` anchors, where each anchor starts a new
//! bar. Meter changes are a first-class design feature, so everything works
//! per-segment rather than assuming one global BPM.

use crate::manifest::TempoAnchor;

/// Milliseconds → whole samples, rounded — the one spelling of the crate's
/// most-repeated conversion. Callers pass `sample_rate as f64`; keep the
/// argument order (ms, rate) everywhere.
pub fn ms_to_samples(ms: f64, sample_rate: f64) -> i64 {
    (ms / 1000.0 * sample_rate).round() as i64
}

/// Snap `x` to the nearest integer when within `tol`, else leave it alone.
/// Bar/beat boundaries land on fractional samples, so every place that floors
/// or ceils a bar count must first snap through the half-sample tolerance.
fn snap(x: f64, tol: f64) -> f64 {
    if (x - x.round()).abs() <= tol {
        x.round()
    } else {
        x
    }
}

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
    /// anchors. Negative samples (pre-roll) extrapolate the origin anchor
    /// backwards. Returns None if the map is empty or has no origin anchor.
    pub fn beats_at_sample(&self, sample: i64) -> Option<f64> {
        if self.anchors.first().map(|a| a.sample)? != 0 {
            return None;
        }
        if sample < 0 {
            return Some(sample as f64 / self.samples_per_beat(self.anchors.first()?));
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

    /// Exact inverse of [`beats_at_sample`](Self::beats_at_sample): the
    /// (fractional) sample position of a beats-from-start value. Negative
    /// beats extrapolate the origin anchor backwards.
    pub fn sample_at_beat(&self, beats: f64) -> Option<f64> {
        if self.anchors.first().map(|a| a.sample)? != 0 {
            return None;
        }
        if beats < 0.0 {
            return Some(beats * self.samples_per_beat(self.anchors.first()?));
        }
        let mut acc = 0.0;
        for (i, a) in self.anchors.iter().enumerate() {
            let spb = self.samples_per_beat(a);
            let seg_beats = match self.anchors.get(i + 1) {
                Some(next) => (next.sample - a.sample) as f64 / spb,
                None => f64::INFINITY,
            };
            if beats <= acc + seg_beats {
                return Some(a.sample as f64 + (beats - acc) * spb);
            }
            acc += seg_beats;
        }
        None
    }

    /// Bar index (from 0) and beat-within-bar at `sample`. Each anchor starts
    /// a new bar, so a partial bar cut short by an anchor still counts as one
    /// bar. Bar arithmetic snaps through the half-sample tolerance so bars
    /// landing on fractional samples (e.g. 84 BPM at 48 kHz) resolve cleanly.
    pub fn bar_beat_at_sample(&self, sample: i64) -> Option<(i64, f64)> {
        if self.anchors.first().map(|a| a.sample)? != 0 {
            return None;
        }
        if sample < 0 {
            let a = self.anchors.first()?;
            let beats = sample as f64 / self.samples_per_beat(a);
            let bars = (beats / a.beats_per_bar as f64).floor();
            return Some((bars as i64, beats - bars * a.beats_per_bar as f64));
        }
        let mut bars_before: i64 = 0;
        for (i, a) in self.anchors.iter().enumerate() {
            let spb = self.samples_per_beat(a);
            let bpb = a.beats_per_bar.max(1) as f64;
            let seg_end = self.anchors.get(i + 1).map(|n| n.sample);
            let in_segment = match seg_end {
                Some(end) => sample < end,
                None => true,
            };
            if in_segment {
                let beats = (sample - a.sample) as f64 / spb;
                let bars = snap(beats / bpb, 0.5 / (spb * bpb));
                let bar_in_seg = bars.floor();
                // Snapped onto a bar line: exactly 0. Otherwise subtract
                // rather than un-divide (beats/bpb*bpb is lossy).
                let beat_in_bar = if bars == bar_in_seg {
                    0.0
                } else {
                    (beats - bar_in_seg * bpb).max(0.0)
                };
                return Some((bars_before + bar_in_seg as i64, beat_in_bar));
            }
            let end = seg_end.expect("in_segment is false only with a next anchor");
            let seg_bars = snap((end - a.sample) as f64 / (spb * bpb), 0.5 / (spb * bpb));
            bars_before += seg_bars.ceil() as i64;
        }
        None
    }

    /// The (fractional) sample position of a bar line, `bar` counted from 0
    /// with anchors starting new bars (the inverse of
    /// [`bar_beat_at_sample`](Self::bar_beat_at_sample) at beat 0).
    pub fn sample_at_bar(&self, bar: i64, beat_in_bar: f64) -> Option<f64> {
        if self.anchors.first().map(|a| a.sample)? != 0 {
            return None;
        }
        if bar < 0 {
            let a = self.anchors.first()?;
            let spb = self.samples_per_beat(a);
            return Some((bar as f64 * a.beats_per_bar as f64 + beat_in_bar) * spb);
        }
        let mut bars_before: i64 = 0;
        for (i, a) in self.anchors.iter().enumerate() {
            let spb = self.samples_per_beat(a);
            let bpb = a.beats_per_bar.max(1) as f64;
            let seg_bars = match self.anchors.get(i + 1) {
                Some(next) => snap(
                    (next.sample - a.sample) as f64 / (spb * bpb),
                    0.5 / (spb * bpb),
                )
                .ceil() as i64,
                None => i64::MAX,
            };
            if bar - bars_before < seg_bars {
                return Some(
                    a.sample as f64 + ((bar - bars_before) as f64 * bpb + beat_in_bar) * spb,
                );
            }
            bars_before += seg_bars;
        }
        None
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
    fn sample_at_beat_inverts_beats_at_sample() {
        let m = map();
        for sample in [0i64, 24_000, 500_000, 960_000, 960_000 + 32_000, 2_000_000] {
            let beats = m.beats_at_sample(sample).unwrap();
            let back = m.sample_at_beat(beats).unwrap();
            assert!(
                (back - sample as f64).abs() < 0.5,
                "round-trip {sample} -> {beats} -> {back}"
            );
        }
        // Negative (pre-roll) extrapolation, both directions.
        assert_eq!(m.beats_at_sample(-24_000), Some(-1.0));
        assert_eq!(m.sample_at_beat(-1.0), Some(-24_000.0));
    }

    #[test]
    fn bar_numbering_accumulates_across_meter_change() {
        let m = map();
        assert_eq!(m.bar_beat_at_sample(0), Some((0, 0.0)));
        // 4/4 @ 120 BPM: bar = 96_000 samples.
        assert_eq!(m.bar_beat_at_sample(96_000), Some((1, 0.0)));
        let (bar, beat) = m.bar_beat_at_sample(96_000 + 24_000).unwrap();
        assert_eq!(bar, 1);
        assert!((beat - 1.0).abs() < 1e-9);
        // Anchor at 960_000 = 10 full 4/4 bars, starts bar 10 in 3/4.
        assert_eq!(m.bar_beat_at_sample(960_000), Some((10, 0.0)));
        // One 3/4 bar @ 90 BPM = 96_000 samples.
        assert_eq!(m.bar_beat_at_sample(960_000 + 96_000), Some((11, 0.0)));
    }

    #[test]
    fn partial_bar_before_anchor_counts_as_a_bar() {
        // Anchor lands mid-bar: 120 BPM 4/4, anchor at 2.5 bars (240_000).
        let m = TempoMap::new(
            vec![
                TempoAnchor {
                    sample: 0,
                    bpm: 120.0,
                    beats_per_bar: 4,
                    beat_unit: 4,
                },
                TempoAnchor {
                    sample: 240_000,
                    bpm: 120.0,
                    beats_per_bar: 4,
                    beat_unit: 4,
                },
            ],
            48_000,
        );
        // Sample just before the anchor is inside partial bar 2...
        assert_eq!(m.bar_beat_at_sample(239_999).unwrap().0, 2);
        // ...and the anchor starts bar 3, not bar 2-and-a-half.
        assert_eq!(m.bar_beat_at_sample(240_000), Some((3, 0.0)));
        assert!((m.sample_at_bar(3, 0.0).unwrap() - 240_000.0).abs() < 0.5);
    }

    #[test]
    fn sample_at_bar_inverts_bar_beat() {
        let m = map();
        for bar in [0i64, 1, 9, 10, 11, 20] {
            let s = m.sample_at_bar(bar, 0.0).unwrap();
            assert_eq!(m.bar_beat_at_sample(s.round() as i64), Some((bar, 0.0)));
        }
        // With a beat offset, in the 3/4 region.
        let s = m.sample_at_bar(11, 2.0).unwrap();
        assert!((s - (960_000.0 + 96_000.0 + 2.0 * 32_000.0)).abs() < 0.5);
    }

    #[test]
    fn fractional_bars_snap_through_tolerance() {
        // The prototype tempo: 84 BPM 4/4 @ 48 kHz -> bar = 960000/7 samples,
        // never integral. Section starts are rounded; grid math must agree.
        let m = TempoMap::new(
            vec![TempoAnchor {
                sample: 0,
                bpm: 84.0,
                beats_per_bar: 4,
                beat_unit: 4,
            }],
            48_000,
        );
        let bar_len = 960_000.0 / 7.0;
        for bar in [1i64, 7, 16, 100] {
            let rounded = (bar as f64 * bar_len).round() as i64;
            assert!(m.is_bar_line(rounded), "bar {bar} at rounded {rounded}");
            assert_eq!(m.bar_beat_at_sample(rounded), Some((bar, 0.0)));
            assert!((m.sample_at_bar(bar, 0.0).unwrap() - bar as f64 * bar_len).abs() < 0.5);
        }
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
