//! The Conductor: the single query API for musical time.
//!
//! Everything that needs to know "where are we in the music" asks the
//! Conductor and nothing else. It owns the tempo map and section list from the
//! manifest and converts between sample time, seconds, and bar/beat/tick,
//! including across tempo and meter changes. It also applies latency
//! compensation: `now` answers for what the ear hears, not what the DAC was
//! just handed.
//!
//! The Conductor holds no audio handles and never mutates — the playback
//! clock counts samples (see `session`), and all musical interpretation is
//! pure arithmetic here. One source of truth, no drift.

use crate::manifest::{Section, SuiteManifest};
use crate::TempoMap;

/// Pulses per quarter note for the `tick` readout field.
pub const TICKS_PER_BEAT: i64 = 960;

/// A grid to schedule or align against.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Grid {
    Beat,
    Bar,
    /// Every `n` beats since the governing anchor (e.g. 0.5 for eighths).
    Beats(f64),
}

/// A fully resolved moment in musical time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MusicalPosition {
    /// Suite sample position (negative during pre-roll).
    pub sample: i64,
    /// Suite seconds (sample / sample_rate).
    pub seconds: f64,
    /// Beats from suite sample 0, accumulated across tempo changes.
    pub beat: f64,
    /// Bar index from 0; anchors always start a new bar.
    pub bar: i64,
    /// Beat within the current bar, from 0.
    pub beat_in_bar: f64,
    /// Ticks (at [`TICKS_PER_BEAT`]) within the current beat.
    pub tick: i64,
}

pub struct Conductor {
    tempo: TempoMap,
    sections: Vec<Section>,
    latency_samples: i64,
}

impl Conductor {
    /// Build from a validated manifest. `latency_ms` is the measured output
    /// latency (from `logs/latency/`); `None` means zero compensation, which
    /// is exact for offline rendering.
    pub fn new(manifest: &SuiteManifest, latency_ms: Option<f64>) -> Self {
        let latency_samples = latency_ms
            .map(|ms| (ms / 1000.0 * manifest.sample_rate as f64).round() as i64)
            .unwrap_or(0);
        Self {
            tempo: TempoMap::new(manifest.tempo.clone(), manifest.sample_rate),
            sections: manifest.sections.clone(),
            latency_samples,
        }
    }

    pub fn tempo(&self) -> &TempoMap {
        &self.tempo
    }

    pub fn latency_samples(&self) -> i64 {
        self.latency_samples
    }

    /// The musical position at an absolute suite sample.
    pub fn position_at_sample(&self, sample: i64) -> MusicalPosition {
        let beat = self.tempo.beats_at_sample(sample).unwrap_or(0.0);
        let (bar, beat_in_bar) = self.tempo.bar_beat_at_sample(sample).unwrap_or((0, 0.0));
        let tick =
            (beat_in_bar.rem_euclid(1.0) * TICKS_PER_BEAT as f64).round() as i64 % TICKS_PER_BEAT;
        MusicalPosition {
            sample,
            seconds: sample as f64 / self.tempo.sample_rate() as f64,
            beat,
            bar,
            beat_in_bar,
            tick,
        }
    }

    /// What the ear hears now, given the playback clock's sample count:
    /// the clock position pulled back by the measured output latency.
    pub fn now(&self, clock_sample: i64) -> MusicalPosition {
        self.position_at_sample(clock_sample - self.latency_samples)
    }

    /// Nearest whole sample of a beats-from-start position.
    pub fn sample_at_beat(&self, beat: f64) -> i64 {
        self.tempo.sample_at_beat(beat).unwrap_or(0.0).round() as i64
    }

    /// Nearest whole sample of `beat_in_bar` beats into bar `bar`.
    pub fn sample_at_bar(&self, bar: i64, beat_in_bar: f64) -> i64 {
        self.tempo
            .sample_at_bar(bar, beat_in_bar)
            .unwrap_or(0.0)
            .round() as i64
    }

    /// The first grid sample strictly after `from`. Anchors count as grid
    /// points for every grid (they start a new bar).
    ///
    /// Strictly-after is load-bearing (callers step the grid by feeding each
    /// result back in — a non-advancing return would loop them forever), and
    /// fractional grids can violate it naively: at e.g. 84 BPM / 48 kHz a
    /// beat is 34285.71 samples, and when the ideal next-grid point rounds
    /// *down* onto `from`, `bar_beat_at_sample(from)` reads fractionally
    /// below the grid index (a half-sample beats `next_in_bar`'s 1e-9 beat
    /// epsilon) and re-derives the same point. One re-probe from `from + 1`
    /// lands cleanly past it. Degenerate maps (no tempo data) still return
    /// `from` — callers advancing a cursor must treat `<= from` as "grid
    /// exhausted", never as a point.
    pub fn next_grid_sample(&self, from: i64, grid: Grid) -> i64 {
        let mut probe = from;
        for _ in 0..2 {
            let (bar, beat_in_bar) = self.tempo.bar_beat_at_sample(probe).unwrap_or((0, 0.0));
            let next = match grid {
                Grid::Bar => self.tempo.sample_at_bar(bar + 1, 0.0),
                Grid::Beat => self.next_in_bar(bar, beat_in_bar, 1.0),
                Grid::Beats(n) if n > 0.0 => self.next_in_bar(bar, beat_in_bar, n),
                Grid::Beats(_) => None,
            };
            match next.map(|s| s.round() as i64) {
                Some(s) if s > from => return s,
                Some(_) => probe = from + 1,
                None => return from,
            }
        }
        from
    }

    /// Next multiple of `step` beats within the bar, or the next bar line if
    /// the bar runs out first (which also covers anchors cutting bars short).
    fn next_in_bar(&self, bar: i64, beat_in_bar: f64, step: f64) -> Option<f64> {
        let next_beat = ((beat_in_bar / step + 1e-9).floor() + 1.0) * step;
        let candidate = self.tempo.sample_at_bar(bar, next_beat)?;
        let bar_end = self.tempo.sample_at_bar(bar + 1, 0.0)?;
        Some(candidate.min(bar_end))
    }

    /// The section containing `sample` (the last one starting at or before it).
    pub fn section_at_sample(&self, sample: i64) -> Option<&Section> {
        self.sections
            .iter()
            .take_while(|s| s.start_sample <= sample)
            .last()
    }

    pub fn section_by_name(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    /// The reintegration checkpoint for a full-fail at `sample`: the latest
    /// manifest re-entry section starting at or before it, falling back to
    /// the earliest re-entry section when the fail lands before any (the
    /// validator guarantees `re_entry_sections` is non-empty and resolvable).
    pub fn previous_re_entry(
        &self,
        sample: i64,
        reintegration: &crate::manifest::Reintegration,
    ) -> Option<&Section> {
        let mut entries: Vec<&Section> = reintegration
            .re_entry_sections
            .iter()
            .filter_map(|name| self.section_by_name(name))
            .collect();
        entries.sort_by_key(|s| s.start_sample);
        entries
            .iter()
            .rev()
            .find(|s| s.start_sample <= sample)
            .or(entries.first())
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::TempoAnchor;

    fn manifest() -> SuiteManifest {
        // Mirrors the tempo.rs test map: 120 BPM 4/4, then 90 BPM 3/4 at
        // sample 960_000 (= bar 10), with two sections.
        SuiteManifest {
            schema_version: 0,
            id: "test".into(),
            title: "Test".into(),
            sample_rate: 48_000,
            tempo: vec![
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
            sections: vec![
                Section {
                    name: "one".into(),
                    start_sample: 0,
                    pulse_window_ms: 120.0,
                },
                Section {
                    name: "two".into(),
                    start_sample: 960_000,
                    pulse_window_ms: 100.0,
                },
            ],
            reintegration: crate::manifest::Reintegration {
                re_entry_sections: vec!["one".into()],
                lead_bus: "home_theme".into(),
                reassembly_bars: 2,
            },
            buses: Default::default(),
            degraded_alternates: vec![],
        }
    }

    #[test]
    fn position_readout_across_tempo_change() {
        let c = Conductor::new(&manifest(), None);
        let p = c.position_at_sample(960_000 + 32_000);
        assert_eq!(p.beat, 41.0);
        assert_eq!(p.bar, 10);
        assert!((p.beat_in_bar - 1.0).abs() < 1e-9);
        assert_eq!(p.tick, 0);
        assert!((p.seconds - (960_000.0 + 32_000.0) / 48_000.0).abs() < 1e-12);
    }

    #[test]
    fn latency_compensation_shifts_now() {
        let c = Conductor::new(&manifest(), Some(500.0)); // 24_000 samples
        assert_eq!(c.latency_samples(), 24_000);
        let p = c.now(24_000);
        assert_eq!(p.sample, 0);
        // Early in pre-roll the ear-position is negative.
        assert!(c.now(0).sample < 0 && c.now(0).beat < 0.0);
    }

    #[test]
    fn next_grid_monotone_and_lands_on_grid() {
        let c = Conductor::new(&manifest(), None);
        let mut s = 1; // just after the downbeat
        for _ in 0..50 {
            let n = c.next_grid_sample(s, Grid::Beat);
            assert!(n > s, "grid must advance: {s} -> {n}");
            let bib = c.position_at_sample(n).beat_in_bar;
            assert!((bib - bib.round()).abs() < 1e-6, "off-beat landing: {bib}");
            s = n;
        }
        // Beat grid crosses the tempo change cleanly.
        assert!(s > 960_000);
        // Bar grid: from mid-bar 9 the next bar is the anchor itself.
        let from = c.sample_at_bar(9, 2.0);
        assert_eq!(c.next_grid_sample(from, Grid::Bar), 960_000);
    }

    #[test]
    fn next_grid_advances_on_fractional_beat_lengths() {
        // 84 BPM at 48 kHz: 34285.714… samples per beat — the prototype
        // track's real tempo. Rounding can land the ideal grid point back on
        // `from` (a half-sample beats next_in_bar's 1e-9 beat epsilon),
        // which must never stall the strictly-after contract: haptics (ADR
        // 0026) steps this grid by feeding results back in, and a stall
        // there was an unbounded allocation before the re-probe fix.
        let mut m = manifest();
        m.tempo = vec![TempoAnchor {
            sample: 0,
            bpm: 84.0,
            beats_per_bar: 4,
            beat_unit: 4,
        }];
        let c = Conductor::new(&m, None);
        let mut s = 0;
        for _ in 0..2000 {
            let n = c.next_grid_sample(s, Grid::Beat);
            assert!(n > s, "fractional grid must still advance: {s} -> {n}");
            assert!(
                n - s <= 34_287,
                "advance must stay one beat, not skip: {s} -> {n}"
            );
            s = n;
        }
    }

    #[test]
    fn eighth_grid_subdivides() {
        let c = Conductor::new(&manifest(), None);
        // 120 BPM: an eighth is 12_000 samples.
        assert_eq!(c.next_grid_sample(0, Grid::Beats(0.5)), 12_000);
        assert_eq!(c.next_grid_sample(12_000, Grid::Beats(0.5)), 24_000);
    }

    #[test]
    fn sections_resolve_by_start() {
        let c = Conductor::new(&manifest(), None);
        assert_eq!(c.section_at_sample(0).unwrap().name, "one");
        assert_eq!(c.section_at_sample(959_999).unwrap().name, "one");
        assert_eq!(c.section_at_sample(960_000).unwrap().name, "two");
        assert!(c.section_at_sample(-1).is_none()); // pre-roll has no section
    }
}
