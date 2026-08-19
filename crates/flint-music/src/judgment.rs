//! Judgment: raw error signals from the input stream against the chart.
//!
//! Two signals, per the TDD, and only two — no grades, no combos, no
//! per-note pass/fail anywhere:
//! - **lean error**, in one of two modes ([`LeanMode`]):
//!   - `Track` (continuous): at each step of a fixed musical grid (default
//!     1/8 beat), the Euclidean distance between the actual lean
//!     (zero-order-hold latch of the newest sample at or before the grid
//!     step) and the chart's interpolated target.
//!   - `Arrival` (beat-anchored, the Milestone 2 [H] iteration): each lean
//!     *key* is a target to arrive at; within a window around the key's
//!     beat, the best (minimum) distance observed counts — rolling through
//!     the target on time is as good as parking on it, and motion between
//!     keys is free. Emits one `Track` record per key (same shape, same
//!     logs), sample-stamped at the key's beat.
//!   Target and actual are both logged raw so any alternative model can be
//!   evaluated offline from the logs alone.
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

/// How lean is judged. `Track` is the original continuous model and remains
/// the config default (the Milestone 2 automated evidence pins it);
/// `Arrival` is the beat-anchored model from Chris's first feel check —
/// continuous micro-tracking at small radius was too delicate to sustain
/// flow, so lean became "be at the target on its beat, roll freely between".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeanMode {
    Track,
    Arrival,
}

#[derive(Debug, Clone, Copy)]
pub struct JudgmentConfig {
    /// Tracking grid step in beats (default 1/8 beat). `Track` mode only.
    pub grid_beats: f64,
    pub lean_mode: LeanMode,
    /// Arrival window half-width in beats around each lean key (`Arrival`
    /// mode only). Deliberately wide: arrival is gross motion, not aim.
    pub arrival_half_beats: f64,
}

impl Default for JudgmentConfig {
    fn default() -> Self {
        Self {
            grid_beats: 0.125,
            lean_mode: LeanMode::Track,
            arrival_half_beats: 0.3,
        }
    }
}

/// One raw judgment fact. Serialization order and shapes are the JSONL log
/// contract (`logs/judgment/`). W2 additions are strictly additive: the
/// `channel` key is emitted only for non-lean records and `depth_err`/
/// `dir_err` only when present, so pre-W2 logs and lean-only charts stay
/// byte-identical.
#[derive(Debug, Clone, PartialEq)]
pub enum JudgmentRecord {
    Track {
        sample: i64,
        beat: f64,
        /// Which continuous channel this record judges ("lean", "sway",
        /// "pressure_l", "pressure_r"). Scalar channels put their value in
        /// `target[0]`/`actual[0]` with the second component 0.
        channel: String,
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
        /// Press only: `max(0, strength - peak pressure in window)` —
        /// over-squeezing is free.
        depth_err: Option<f64>,
        /// Flick only: degrees between the gesture direction and the
        /// authored direction (180 when the event carried none).
        dir_err: Option<f64>,
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
                channel,
                target,
                actual,
                err,
            } => {
                let mut v = serde_json::json!({
                    "t": "track", "sample": sample, "beat": beat,
                    "target": target, "actual": actual, "err": err,
                });
                if channel != TRACKED_CHANNEL {
                    v["channel"] = serde_json::json!(channel);
                }
                v
            }
            JudgmentRecord::Pulse {
                sample,
                beat,
                window,
                kind,
                err_ms,
                depth_err,
                dir_err,
            } => {
                let mut v = serde_json::json!({
                    "t": "pulse", "sample": sample, "beat": beat,
                    "window": window, "kind": kind, "err_ms": err_ms,
                });
                if let Some(d) = depth_err {
                    v["depth_err"] = serde_json::json!(d);
                }
                if let Some(d) = dir_err {
                    v["dir_err"] = serde_json::json!(d);
                }
                v
            }
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

/// One beat-anchored continuous-channel target (`Arrival` mode): the best
/// distance seen inside the window counts when the window closes. Scalar
/// channels (pressure) live in the x component with y pinned to 0, so one
/// Euclidean distance serves both arities.
struct Arrival {
    channel: &'static str,
    beat: f64,
    center_sample: i64,
    open: i64,
    close: i64,
    target: [f64; 2],
    best_err: f64,
    best_actual: [f64; 2],
    done: bool,
}

/// Degrees between two directions; 180 when either is (near) zero-length —
/// a flick with no direction is fully wrong, never a divide-by-zero.
fn direction_err_deg(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (ma, mb) = ((a[0] * a[0] + a[1] * a[1]).sqrt(), (b[0] * b[0] + b[1] * b[1]).sqrt());
    if ma < 1e-9 || mb < 1e-9 {
        return 180.0;
    }
    let cos = ((a[0] * b[0] + a[1] * b[1]) / (ma * mb)).clamp(-1.0, 1.0);
    cos.acos().to_degrees()
}

/// Per-run judgment state. Owns its evaluator and conductor so live and
/// replay paths construct it identically.
pub struct Judge {
    eval: ChartEval,
    conductor: Conductor,
    cfg: JudgmentConfig,
    consumed: Vec<bool>,
    /// Next tracking-grid step, in beats (`Track` mode).
    next_grid_beat: f64,
    /// Continuous-channel arrival windows, per channel in `crate::CHANNELS`
    /// order then by beat (`Arrival` mode; empty in `Track` mode, which
    /// stays lean-only — the Milestone 2 evidence pins it).
    arrivals: Vec<Arrival>,
    /// Grid steps stop after the chart's authored content ends.
    end_sample: i64,
    /// ZOH latch of the newest lean at or before the advance point.
    lean: [f64; 2],
    /// ZOH latch of the newest sway.
    sway: [f64; 2],
    /// ZOH latches of trigger depth ([left, right]).
    pressure: [f64; 2],
    /// Peak pressure (either side) observed inside each press window while
    /// open — the press-depth source (D3: depth never rides the event).
    press_peaks: Vec<f64>,
    /// High-water mark of judged time.
    advanced_to: i64,
}

impl Judge {
    pub fn new(eval: ChartEval, conductor: Conductor, cfg: JudgmentConfig) -> Self {
        let consumed = vec![false; eval.pulse_windows().len()];
        let press_peaks = vec![0.0; eval.pulse_windows().len()];
        let arrivals: Vec<Arrival> = match cfg.lean_mode {
            LeanMode::Track => Vec::new(),
            LeanMode::Arrival => crate::CHANNELS
                .iter()
                .flat_map(|(channel, _)| {
                    eval.channel_keys(channel)
                        .into_iter()
                        .map(move |(beat, value)| (*channel, beat, value))
                })
                .map(|(channel, beat, value)| {
                    let target = match value {
                        ChannelValue::Vec2(v) => v,
                        ChannelValue::Scalar(v) => [v, 0.0],
                    };
                    Arrival {
                        channel,
                        beat,
                        center_sample: conductor.sample_at_beat(beat),
                        open: conductor.sample_at_beat(beat - cfg.arrival_half_beats),
                        close: conductor.sample_at_beat(beat + cfg.arrival_half_beats),
                        target,
                        best_err: f64::INFINITY,
                        best_actual: [0.0, 0.0],
                        done: false,
                    }
                })
                .collect(),
        };
        let last_window_close = eval.pulse_windows().iter().map(|w| w.close()).max();
        let last_curve_sample = match cfg.lean_mode {
            LeanMode::Track => eval
                .last_key_beat(TRACKED_CHANNEL)
                .map(|b| conductor.sample_at_beat(b)),
            LeanMode::Arrival => arrivals.iter().map(|a| a.close).max(),
        };
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
            arrivals,
            end_sample,
            lean: [0.0, 0.0],
            sway: [0.0, 0.0],
            pressure: [0.0, 0.0],
            press_peaks,
            advanced_to: i64::MIN,
        }
    }

    /// Fold a continuous observation into the open arrival windows of its
    /// channel (best-in-window judging: extra samples never hurt).
    fn observe_arrivals(&mut self, channel: &str, sample: i64, actual: [f64; 2]) {
        for a in self.arrivals.iter_mut() {
            if a.done || a.channel != channel || sample < a.open || sample > a.close {
                continue;
            }
            let dx = actual[0] - a.target[0];
            let dy = actual[1] - a.target[1];
            let err = (dx * dx + dy * dy).sqrt();
            if err < a.best_err {
                a.best_err = err;
                a.best_actual = actual;
            }
        }
    }

    /// Sample where the chart's authored content ends (last window close /
    /// grid stop) — the suite-extent input to the debug timeline map.
    pub fn end_sample(&self) -> i64 {
        self.end_sample
    }

    /// The musical time one lean record represents, for coherence's
    /// integrator: the grid step in `Track` mode, the mean key spacing in
    /// `Arrival` mode.
    pub fn lean_step_beats(&self) -> f64 {
        match self.cfg.lean_mode {
            LeanMode::Track => self.cfg.grid_beats,
            LeanMode::Arrival => {
                // Mean spacing of the *lean* keys (the base verb paces the
                // integrator; other channels ride the same step).
                let lean: Vec<f64> = self
                    .arrivals
                    .iter()
                    .filter(|a| a.channel == TRACKED_CHANNEL)
                    .map(|a| a.beat)
                    .collect();
                let n = lean.len();
                if n >= 2 {
                    (lean[n - 1] - lean[0]) / (n - 1) as f64
                } else {
                    1.0
                }
            }
        }
    }

    /// Feed one input event. Events must arrive in nondecreasing sample
    /// order (the capture thread and the replay reader both guarantee it).
    pub fn ingest(&mut self, ev: &InputEvent, out: &mut Vec<JudgmentRecord>) {
        self.advance_internal(ev.sample(), out);
        match ev {
            InputEvent::Lean(l) => {
                self.lean = [l.x, l.y];
                self.observe_arrivals(TRACKED_CHANNEL, l.sample, [l.x, l.y]);
            }
            InputEvent::Sway(sw) => {
                self.sway = [sw.x, sw.y];
                self.observe_arrivals("sway", sw.sample, [sw.x, sw.y]);
            }
            InputEvent::Pressure(p) => {
                let slot = match p.side {
                    crate::input_stream::PressureSide::Left => 0,
                    crate::input_stream::PressureSide::Right => 1,
                };
                self.pressure[slot] = p.value;
                self.observe_arrivals(p.side.channel(), p.sample, [p.value, 0.0]);
                // Press depth: peak of either trigger inside an open press
                // window.
                for w in self.eval.pulse_windows() {
                    if !self.consumed[w.index]
                        && w.kind == "press"
                        && w.contains(p.sample)
                        && p.value > self.press_peaks[w.index]
                    {
                        self.press_peaks[w.index] = p.value;
                    }
                }
            }
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
                        // Press: depth against authored strength, sourced
                        // from the pressure stream (window peak, or the ZOH
                        // latch of a trigger held steady through the window).
                        let depth_err = match (w.kind.as_str(), w.strength) {
                            ("press", Some(strength)) => {
                                let peak = self.press_peaks[w.index]
                                    .max(self.pressure[0])
                                    .max(self.pressure[1]);
                                Some((strength - peak).max(0.0))
                            }
                            _ => None,
                        };
                        // Flick: angular error against the authored line.
                        let dir_err = w.direction.map(|wd| match p.direction {
                            Some(pd) => direction_err_deg(pd, wd),
                            None => 180.0,
                        });
                        out.push(JudgmentRecord::Pulse {
                            sample: s,
                            beat,
                            window: w.index,
                            kind: p.kind.clone(),
                            err_ms,
                            depth_err,
                            dir_err,
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

        // Grid steps due at or before `sample`, stopping at content end
        // (`Track` mode; `Arrival` mode has no grid).
        if self.cfg.lean_mode == LeanMode::Track {
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
                        channel: TRACKED_CHANNEL.to_string(),
                        target,
                        actual: self.lean,
                        err: (dx * dx + dy * dy).sqrt(),
                    });
                }
                self.next_grid_beat += self.cfg.grid_beats;
            }
        }

        // Arrival windows whose close has passed settle now. The channel's
        // ZOH latch gets a say too: a player parked on the target the whole
        // window never sends an event inside it, and still errs ~0.
        let latches = [
            self.lean,
            self.sway,
            [self.pressure[0], 0.0],
            [self.pressure[1], 0.0],
        ];
        for a in self.arrivals.iter_mut() {
            if a.done || a.close >= sample {
                continue;
            }
            let latch = match a.channel {
                "sway" => latches[1],
                "pressure_l" => latches[2],
                "pressure_r" => latches[3],
                _ => latches[0],
            };
            let dx = latch[0] - a.target[0];
            let dy = latch[1] - a.target[1];
            let latch_err = (dx * dx + dy * dy).sqrt();
            if latch_err < a.best_err {
                a.best_err = latch_err;
                a.best_actual = latch;
            }
            out.push(JudgmentRecord::Track {
                sample: a.center_sample,
                beat: a.beat,
                channel: a.channel.to_string(),
                target: a.target,
                actual: a.best_actual,
                err: a.best_err,
            });
            a.done = true;
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

    /// Whether the pulse window at `index` (into [`Judge::windows`]) has been
    /// consumed by a hit. Debug-guide surface (ADR 0035): a consumed window
    /// needs no further guidance. Un-consumes across a reintegration rewind
    /// like everything else.
    pub fn window_consumed(&self, index: usize) -> bool {
        self.consumed.get(index).copied().unwrap_or(false)
    }

    /// Reintegration rewind: judged time returns to `sample` (a re-entry
    /// point on the suite timeline). Content at or after it becomes
    /// judgeable again — windows un-consume, arrivals reset — while
    /// everything earlier stays consumed, so no misses fire for the past
    /// the seam skipped. The lean latch survives (the stick is where it
    /// is). Input stays live through reintegration by construction.
    pub fn rewind_to(&mut self, sample: i64) {
        self.advanced_to = sample;
        let eval = &self.eval;
        let consumed = &mut self.consumed;
        for w in eval.pulse_windows() {
            if w.close() >= sample {
                consumed[w.index] = false;
                self.press_peaks[w.index] = 0.0;
            }
        }
        for a in self.arrivals.iter_mut() {
            if a.close >= sample {
                a.done = false;
                a.best_err = f64::INFINITY;
                a.best_actual = [0.0, 0.0];
            }
        }
        if self.cfg.lean_mode == LeanMode::Track {
            let beat = self.conductor.position_at_sample(sample).beat;
            self.next_grid_beat = (beat / self.cfg.grid_beats).ceil() * self.cfg.grid_beats;
        }
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
            direction: None,
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

    fn arrival_judge(chart_toml: &str) -> Judge {
        let chart = Chart::parse(chart_toml).unwrap();
        let conductor = conductor();
        let eval = ChartEval::new(&chart, &conductor).unwrap();
        Judge::new(
            eval,
            conductor,
            JudgmentConfig {
                lean_mode: LeanMode::Arrival,
                ..Default::default()
            },
        )
    }

    /// Two keys, beats 2 and 4. Beat = 24_000 samples; half-window 0.3 beat
    /// = 7_200 samples, so key 0 spans 40_800..=55_200.
    const ARRIVAL_CHART: &str = r#"schema_version = 0
suite = "t"
[[curves]]
channel = "lean"
beat = 2.0
value = [1.0, 0.0]
interp = "smooth"
[[curves]]
channel = "lean"
beat = 4.0
value = [-1.0, 0.0]
interp = "smooth"
"#;

    fn lean(sample: i64, x: f64, y: f64) -> InputEvent {
        InputEvent::Lean(LeanSample { sample, x, y })
    }

    fn arrival_errs(out: &[JudgmentRecord]) -> Vec<(f64, f64)> {
        out.iter()
            .filter_map(|r| match r {
                JudgmentRecord::Track { beat, err, .. } => Some((*beat, *err)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn arrival_rolling_through_on_time_counts() {
        let mut j = arrival_judge(ARRIVAL_CHART);
        let mut out = Vec::new();
        // Roll through the target inside the window, then well past it —
        // the excursion after the pass-through must not hurt.
        j.ingest(&lean(47_000, 1.0, 0.0), &mut out);
        j.ingest(&lean(52_000, 0.2, 0.6), &mut out);
        j.finish(&mut out);
        let errs = arrival_errs(&out);
        assert_eq!(errs.len(), 2, "{errs:?}");
        assert!(errs[0].0 == 2.0 && errs[0].1.abs() < 1e-12, "{errs:?}");
    }

    #[test]
    fn arrival_neutral_stick_errs_target_magnitude_via_latch() {
        let mut j = arrival_judge(ARRIVAL_CHART);
        let mut out = Vec::new();
        j.finish(&mut out); // no input at all
        let errs = arrival_errs(&out);
        assert_eq!(errs.len(), 2, "{errs:?}");
        for (_, err) in errs {
            assert!((err - 1.0).abs() < 1e-12, "{err}");
        }
    }

    #[test]
    fn arrival_emits_no_grid_records() {
        let mut j = arrival_judge(ARRIVAL_CHART);
        let mut out = Vec::new();
        j.advance_to(200_000, &mut out);
        // Exactly one record per key, none from any 1/8-beat grid.
        assert_eq!(arrival_errs(&out).len(), 2);
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
