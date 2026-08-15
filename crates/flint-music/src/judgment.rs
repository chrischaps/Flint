//! Judgment: raw error signals from the input stream against the chart.
//!
//! Two signals, per the TDD, and only two — no grades, no combos, no
//! per-note pass/fail anywhere:
//! - **tracking error**: at each step of a fixed musical grid (default 1/8
//!   beat), the Euclidean distance between the actual lean (zero-order-hold
//!   latch of the newest sample at or before the grid step) and the chart's
//!   target lean. Target and actual are both logged raw so any alternative
//!   model can be evaluated offline from the logs alone.
//! - **pulse timing error**: signed milliseconds from the matched window's
//!   center (negative = early). A pulse matches the nearest-center
//!   unconsumed window of its kind containing it; each window is consumable
//!   once. A window that closes unconsumed is a **miss**; a pulse landing in
//!   no open window is **spurious** (recorded, never graded).
//!
//! Events and grid steps are merged in sample order internally: `ingest`
//! first advances judgment to the event's sample, so a lean update never
//! retroactively affects an earlier grid step. Everything here is pure and
//! deterministic — same stream in, same records out, bit for bit.

use crate::chart_eval::{ChannelValue, ChartEval, PulseWindow};
use crate::conductor::Conductor;
use crate::input_stream::InputEvent;

/// The channel the continuous tracking signal judges. The prototype's base
/// verb; per-channel expansion is config work, not schema work.
pub const TRACKED_CHANNEL: &str = "lean";

#[derive(Debug, Clone, Copy)]
pub struct JudgmentConfig {
    /// Tracking grid step in beats (default 1/8 beat).
    pub grid_beats: f64,
}

impl Default for JudgmentConfig {
    fn default() -> Self {
        Self { grid_beats: 0.125 }
    }
}

/// One raw judgment fact. Serialization order and shapes are the JSONL log
/// contract (`logs/judgment/`).
#[derive(Debug, Clone, PartialEq)]
pub enum JudgmentRecord {
    Track {
        sample: i64,
        beat: f64,
        target: [f64; 2],
        actual: [f64; 2],
        err: f64,
    },
    Pulse {
        sample: i64,
        beat: f64,
        /// Chart pulse index of the consumed window.
        window: usize,
        kind: String,
        err_ms: f64,
    },
    Miss {
        /// The sample at which the window was declared dead (its close).
        sample: i64,
        window: usize,
        kind: String,
        center_beat: f64,
    },
    Spurious {
        sample: i64,
        beat: f64,
        kind: String,
    },
}

impl JudgmentRecord {
    /// One JSON object, one line, stable field order.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            JudgmentRecord::Track {
                sample,
                beat,
                target,
                actual,
                err,
            } => serde_json::json!({
                "t": "track", "sample": sample, "beat": beat,
                "target": target, "actual": actual, "err": err,
            }),
            JudgmentRecord::Pulse {
                sample,
                beat,
                window,
                kind,
                err_ms,
            } => serde_json::json!({
                "t": "pulse", "sample": sample, "beat": beat,
                "window": window, "kind": kind, "err_ms": err_ms,
            }),
            JudgmentRecord::Miss {
                sample,
                window,
                kind,
                center_beat,
            } => serde_json::json!({
                "t": "miss", "sample": sample, "window": window,
                "kind": kind, "center_beat": center_beat,
            }),
            JudgmentRecord::Spurious { sample, beat, kind } => serde_json::json!({
                "t": "spurious", "sample": sample, "beat": beat, "kind": kind,
            }),
        }
    }
}

/// Per-run judgment state. Owns its evaluator and conductor so live and
/// replay paths construct it identically.
pub struct Judge {
    eval: ChartEval,
    conductor: Conductor,
    cfg: JudgmentConfig,
    consumed: Vec<bool>,
    /// Next tracking-grid step, in beats.
    next_grid_beat: f64,
    /// Grid steps stop after the chart's authored content ends.
    end_sample: i64,
    /// ZOH latch of the newest lean at or before the advance point.
    lean: [f64; 2],
    /// High-water mark of judged time.
    advanced_to: i64,
}

impl Judge {
    pub fn new(eval: ChartEval, conductor: Conductor, cfg: JudgmentConfig) -> Self {
        let consumed = vec![false; eval.pulse_windows().len()];
        let last_window_close = eval.pulse_windows().iter().map(|w| w.close()).max();
        let last_curve_sample = eval
            .last_key_beat(TRACKED_CHANNEL)
            .map(|b| conductor.sample_at_beat(b));
        let end_sample = last_window_close
            .into_iter()
            .chain(last_curve_sample)
            .max()
            .unwrap_or(0);
        Self {
            eval,
            conductor,
            cfg,
            consumed,
            next_grid_beat: 0.0,
            end_sample,
            lean: [0.0, 0.0],
            advanced_to: i64::MIN,
        }
    }

    /// Feed one input event. Events must arrive in nondecreasing sample
    /// order (the capture thread and the replay reader both guarantee it).
    pub fn ingest(&mut self, ev: &InputEvent, out: &mut Vec<JudgmentRecord>) {
        self.advance_internal(ev.sample(), out);
        match ev {
            InputEvent::Lean(l) => self.lean = [l.x, l.y],
            InputEvent::Pulse(p) => {
                let s = p.sample;
                let beat = self.conductor.position_at_sample(s).beat;
                // Nearest-center unconsumed window of the same kind
                // containing the event.
                let best = self
                    .eval
                    .pulse_windows()
                    .iter()
                    .filter(|w| !self.consumed[w.index] && w.kind == p.kind && w.contains(s))
                    .min_by_key(|w| (w.center_sample - s).abs());
                match best {
                    Some(w) => {
                        self.consumed[w.index] = true;
                        let err_ms = (s - w.center_sample) as f64
                            / self.conductor.tempo().sample_rate() as f64
                            * 1000.0;
                        out.push(JudgmentRecord::Pulse {
                            sample: s,
                            beat,
                            window: w.index,
                            kind: p.kind.clone(),
                            err_ms,
                        });
                    }
                    None => out.push(JudgmentRecord::Spurious {
                        sample: s,
                        beat,
                        kind: p.kind.clone(),
                    }),
                }
            }
        }
    }

    /// Advance judged time to `sample` (typically the session's compensated
    /// `now()`), emitting due grid records and expiring dead windows.
    pub fn advance_to(&mut self, sample: i64, out: &mut Vec<JudgmentRecord>) {
        self.advance_internal(sample, out);
    }

    /// Expire every remaining window as missed and emit any final grid steps.
    pub fn finish(&mut self, out: &mut Vec<JudgmentRecord>) {
        self.advance_internal(self.end_sample + 1, out);
    }

    fn advance_internal(&mut self, sample: i64, out: &mut Vec<JudgmentRecord>) {
        if sample <= self.advanced_to {
            return;
        }

        // Grid steps due at or before `sample`, stopping at content end.
        loop {
            let grid_sample = self.conductor.sample_at_beat(self.next_grid_beat);
            if grid_sample > sample || grid_sample > self.end_sample {
                break;
            }
            if let Some(ChannelValue::Vec2(target)) =
                self.eval.sample_channel(TRACKED_CHANNEL, self.next_grid_beat)
            {
                let dx = self.lean[0] - target[0];
                let dy = self.lean[1] - target[1];
                out.push(JudgmentRecord::Track {
                    sample: grid_sample,
                    beat: self.next_grid_beat,
                    target,
                    actual: self.lean,
                    err: (dx * dx + dy * dy).sqrt(),
                });
            }
            self.next_grid_beat += self.cfg.grid_beats;
        }

        // Windows whose close has passed unconsumed are misses.
        for w in self.eval.pulse_windows() {
            if !self.consumed[w.index] && w.close() < sample {
                self.consumed[w.index] = true;
                out.push(JudgmentRecord::Miss {
                    sample: w.close(),
                    window: w.index,
                    kind: w.kind.clone(),
                    center_beat: w.beat,
                });
            }
        }

        self.advanced_to = sample;
    }

    pub fn windows(&self) -> &[PulseWindow] {
        self.eval.pulse_windows()
    }
}

/// Buffered JSONL writer: one header line, then one record per line. The
/// same shape serves judgment logs and (via `write_value`) session files.
pub struct JsonlWriter {
    out: std::io::BufWriter<std::fs::File>,
}

impl JsonlWriter {
    pub fn create(path: &std::path::Path, header: &serde_json::Value) -> flint_core::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut writer = Self {
            out: std::io::BufWriter::new(std::fs::File::create(path)?),
        };
        writer.write_value(header)?;
        Ok(writer)
    }

    pub fn write(&mut self, rec: &JudgmentRecord) -> flint_core::Result<()> {
        self.write_value(&rec.to_json())
    }

    pub fn write_value(&mut self, value: &serde_json::Value) -> flint_core::Result<()> {
        use std::io::Write;
        writeln!(self.out, "{value}")?;
        Ok(())
    }

    pub fn flush(&mut self) -> flint_core::Result<()> {
        use std::io::Write;
        self.out.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chart::Chart;
    use crate::input_stream::{LeanSample, PulseEvent};
    use crate::manifest::SuiteManifest;

    // 120 BPM 4/4 at 48 kHz: beat = 24_000 samples; section window 100 ms
    // half-width = 4_800 samples.
    fn conductor() -> Conductor {
        let manifest: SuiteManifest = SuiteManifest::parse(
            r#"
schema_version = 0
[suite]
id = "t"
title = "T"
[audio]
sample_rate = 48000
[[tempo]]
sample = 0
bpm = 120.0
time_signature = [4, 4]
[[sections]]
name = "a"
start_sample = 0
pulse_window_ms = 100
[reintegration]
re_entry_sections = ["a"]
lead_bus = "home_theme"
reassembly_bars = 1
[buses.foundation]
silent = true
[buses.harmony]
silent = true
[buses.world_voice]
silent = true
[buses.home_theme]
silent = true
[buses.child_motif]
silent = true
[buses.texture]
silent = true
"#,
        )
        .unwrap();
        Conductor::new(&manifest, None)
    }

    fn judge(chart_toml: &str) -> Judge {
        let chart = Chart::parse(chart_toml).unwrap();
        let conductor = conductor();
        let eval = ChartEval::new(&chart, &conductor).unwrap();
        Judge::new(eval, conductor, JudgmentConfig::default())
    }

    const HEADER: &str = "schema_version = 0\nsuite = \"t\"\n";

    fn pulse(sample: i64) -> InputEvent {
        InputEvent::Pulse(PulseEvent {
            sample,
            kind: "pulse".into(),
        })
    }

    #[test]
    fn center_hit_is_zero_late_is_positive() {
        let mut j = judge(&format!("{HEADER}[[pulses]]\nbeat = 2.0\n"));
        let mut out = Vec::new();
        j.ingest(&pulse(48_000), &mut out); // dead center
        j.ingest(&pulse(48_000 + 1_440), &mut out); // +30 ms, now spurious (consumed)
        j.finish(&mut out);
        let hits: Vec<_> = out
            .iter()
            .filter_map(|r| match r {
                JudgmentRecord::Pulse { err_ms, .. } => Some(*err_ms),
                _ => None,
            })
            .collect();
        assert_eq!(hits, vec![0.0]);
        assert!(out
            .iter()
            .any(|r| matches!(r, JudgmentRecord::Spurious { .. })));
        assert!(!out.iter().any(|r| matches!(r, JudgmentRecord::Miss { .. })));
    }

    #[test]
    fn late_within_window_is_signed_ms() {
        let mut j = judge(&format!("{HEADER}[[pulses]]\nbeat = 2.0\n"));
        let mut out = Vec::new();
        j.ingest(&pulse(48_000 + 1_440), &mut out); // +30 ms
        match out
            .iter()
            .find(|r| matches!(r, JudgmentRecord::Pulse { .. }))
        {
            Some(JudgmentRecord::Pulse { err_ms, .. }) => {
                assert!((err_ms - 30.0).abs() < 1e-9)
            }
            other => panic!("{other:?}"),
        }
        // Early pulse on a second window is negative.
        let mut j = judge(&format!("{HEADER}[[pulses]]\nbeat = 2.0\n"));
        let mut out = Vec::new();
        j.ingest(&pulse(48_000 - 2_400), &mut out); // -50 ms
        match &out[..] {
            [JudgmentRecord::Pulse { err_ms, .. }] => assert!((err_ms + 50.0).abs() < 1e-9),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unconsumed_window_expires_as_miss() {
        let mut j = judge(&format!("{HEADER}[[pulses]]\nbeat = 2.0\n"));
        let mut out = Vec::new();
        j.advance_to(48_000 + 4_800, &mut out); // at close: still open
        assert!(!out.iter().any(|r| matches!(r, JudgmentRecord::Miss { .. })));
        j.advance_to(48_000 + 4_801, &mut out); // past close
        match out
            .iter()
            .find(|r| matches!(r, JudgmentRecord::Miss { .. }))
        {
            Some(JudgmentRecord::Miss {
                window,
                center_beat,
                ..
            }) => {
                assert_eq!(*window, 0);
                assert_eq!(*center_beat, 2.0);
            }
            other => panic!("{other:?}"),
        }
        // A pulse after expiry is spurious, not a late hit.
        j.ingest(&pulse(48_000 + 5_000), &mut out);
        assert!(out
            .iter()
            .any(|r| matches!(r, JudgmentRecord::Spurious { .. })));
    }

    #[test]
    fn kind_must_match() {
        let mut j = judge(&format!(
            "{HEADER}[[pulses]]\nbeat = 2.0\nkind = \"press\"\nstrength = 0.5\n"
        ));
        let mut out = Vec::new();
        j.ingest(&pulse(48_000), &mut out); // a plain pulse cannot consume a press window
        assert!(matches!(&out[..], [JudgmentRecord::Spurious { .. }]));
        j.finish(&mut out);
        assert!(out.iter().any(|r| matches!(r, JudgmentRecord::Miss { .. })));
    }

    #[test]
    fn nearest_center_wins_between_overlapping_windows() {
        // Two wide windows overlapping around beats 2 and 2.2.
        let mut j = judge(&format!(
            "{HEADER}[[pulses]]\nbeat = 2.0\nwindow_ms = 200\n[[pulses]]\nbeat = 2.2\nwindow_ms = 200\n"
        ));
        let mut out = Vec::new();
        // Beat 2.2 center = 52_800; event at 51_000 is nearer to 52_800
        // (1_800) than 48_000 (3_000).
        j.ingest(&pulse(51_000), &mut out);
        match &out[..] {
            [JudgmentRecord::Pulse { window, .. }] => assert_eq!(*window, 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn tracking_grid_uses_zero_order_hold() {
        let mut j = judge(&format!(
            r#"{HEADER}
[[curves]]
channel = "lean"
beat = 0.0
value = [1.0, 0.0]
interp = "hold"
[[curves]]
channel = "lean"
beat = 8.0
value = [1.0, 0.0]
interp = "hold"
"#
        ));
        let mut out = Vec::new();
        // Lean hits the target at sample 10_000 (between grid steps at
        // 0.125-beat = 3_000-sample spacing).
        j.ingest(
            &InputEvent::Lean(LeanSample {
                sample: 10_000,
                x: 1.0,
                y: 0.0,
            }),
            &mut out,
        );
        j.advance_to(24_000, &mut out);
        let errs: Vec<(f64, f64)> = out
            .iter()
            .filter_map(|r| match r {
                JudgmentRecord::Track { beat, err, .. } => Some((*beat, *err)),
                _ => None,
            })
            .collect();
        // Grid steps before the lean event see the neutral stick (err 1.0);
        // steps after see err 0. The event lands between beat 0.375
        // (sample 9_000) and beat 0.5 (sample 12_000).
        for (beat, err) in errs {
            if beat <= 0.375 {
                assert!((err - 1.0).abs() < 1e-12, "beat {beat}: {err}");
            } else {
                assert!(err.abs() < 1e-12, "beat {beat}: {err}");
            }
        }
    }

    #[test]
    fn deterministic_bit_for_bit() {
        let chart = format!(
            "{HEADER}[[curves]]\nchannel = \"lean\"\nbeat = 0.0\nvalue = [0.5, 0.5]\ninterp = \"linear\"\n[[curves]]\nchannel = \"lean\"\nbeat = 8.0\nvalue = [-0.5, 0.0]\ninterp = \"smooth\"\n[[pulses]]\nbeat = 2.0\n[[pulses]]\nbeat = 4.0\n"
        );
        let events = vec![
            InputEvent::Lean(LeanSample {
                sample: 5_000,
                x: 0.3,
                y: 0.4,
            }),
            pulse(48_500),
            InputEvent::Lean(LeanSample {
                sample: 80_000,
                x: -0.2,
                y: 0.1,
            }),
            pulse(120_000),
        ];
        let run = || {
            let mut j = judge(&chart);
            let mut out = Vec::new();
            for ev in &events {
                j.ingest(ev, &mut out);
            }
            j.finish(&mut out);
            out.iter()
                .map(|r| r.to_json().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(run(), run());
    }
}
