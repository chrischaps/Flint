//! The coherence scalar: the single 0..1 value downstream systems see.
//!
//! Model (ADR 0010): a leaky integrator with asymmetric, bar-denominated
//! time constants. The continuous tracking signal sets a per-step target
//! ("fit"); the value eases toward it with `alpha = 1 - exp(-Δbars/τ)`,
//! rising with `rise_bars` and falling with `fall_bars`. Discrete pulse
//! outcomes enter as bounded impulses: a judged hit nudges up by how clean
//! it was, a miss nudges down by `miss_penalty`, a spurious pulse by
//! `spurious_penalty` (0 by default — flow, not evaluation). Everything is
//! plain f64 arithmetic in a fixed order: same records in, same value out.
//!
//! Every knob lives in `CoherenceConfig`, loaded from TOML and reloadable
//! mid-session (the value survives a reload). Tuning these is a primary
//! goal of the feel prototype; the raw judgment logs exist precisely so
//! alternatives can be evaluated offline.

use crate::config_toml::{self, VersionPolicy};
use crate::judgment::JudgmentRecord;
use flint_core::{FlintError, Result};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoherenceConfig {
    /// Blend weight of the continuous tracking signal.
    pub w_lean: f64,
    /// Blend weight of discrete pulse outcomes.
    pub w_pulse: f64,
    /// Blend weights of the W2 continuous channels. **Default 0.0 = inert**:
    /// a zero-weight non-lean Track record is skipped entirely (no fit, no
    /// time advance), so an unmodified config produces bit-identical values
    /// on any chart.
    pub w_sway: f64,
    pub w_pressure_l: f64,
    pub w_pressure_r: f64,
    /// Press depth error treated as fully wrong (quality factor on the hit
    /// impulse; only press records carry one).
    pub press_depth_err_full: f64,
    /// Flick direction error (degrees) treated as fully wrong.
    pub flick_dir_err_full_deg: f64,
    /// Tracking error treated as fully wrong (Euclidean, lean plane).
    pub track_err_full: f64,
    /// Exponent shaping the tracking penalty curve.
    pub track_curve: f64,
    /// |timing error| treated as fully wrong for a judged pulse.
    pub pulse_err_full_ms: f64,
    /// Impulse magnitude of a miss (in penalty units).
    pub miss_penalty: f64,
    /// Impulse magnitude of a spurious pulse (default 0: free).
    pub spurious_penalty: f64,
    /// Scale of all pulse impulses on the 0..1 value.
    pub impulse_gain: f64,
    /// Starting value of a session.
    pub initial: f64,
    /// Time constant (bars) when the value is rising.
    pub rise_bars: f64,
    /// Time constant (bars) when the value is falling.
    pub fall_bars: f64,
}

impl Default for CoherenceConfig {
    fn default() -> Self {
        Self {
            w_lean: 0.6,
            w_pulse: 0.4,
            w_sway: 0.0,
            w_pressure_l: 0.0,
            w_pressure_r: 0.0,
            press_depth_err_full: 0.5,
            flick_dir_err_full_deg: 90.0,
            track_err_full: 0.8,
            track_curve: 1.0,
            pulse_err_full_ms: 120.0,
            miss_penalty: 1.0,
            spurious_penalty: 0.0,
            impulse_gain: 0.15,
            initial: 0.5,
            rise_bars: 1.0,
            fall_bars: 2.0,
        }
    }
}

impl CoherenceConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let (root, _) = config_toml::parse_versioned(
            "coherence config",
            text,
            VersionPolicy::DefaultZeroStrict,
        )?;
        let f = |table: &str, key: &str, default: f64| -> f64 {
            config_toml::section_f64(&root, table, key, default)
        };
        let d = Self::default();
        let cfg = Self {
            w_lean: f("weights", "lean", d.w_lean),
            w_pulse: f("weights", "pulse", d.w_pulse),
            w_sway: f("weights", "sway", d.w_sway),
            w_pressure_l: f("weights", "pressure_l", d.w_pressure_l),
            w_pressure_r: f("weights", "pressure_r", d.w_pressure_r),
            press_depth_err_full: f("press", "depth_err_full", d.press_depth_err_full),
            flick_dir_err_full_deg: f("flick", "dir_err_full_deg", d.flick_dir_err_full_deg),
            track_err_full: f("tracking", "err_full", d.track_err_full),
            track_curve: f("tracking", "curve", d.track_curve),
            pulse_err_full_ms: f("pulse", "err_full_ms", d.pulse_err_full_ms),
            miss_penalty: f("pulse", "miss_penalty", d.miss_penalty),
            spurious_penalty: f("pulse", "spurious_penalty", d.spurious_penalty),
            impulse_gain: f("pulse", "impulse_gain", d.impulse_gain),
            initial: f("smoothing", "initial", d.initial),
            rise_bars: f("smoothing", "rise_bars", d.rise_bars),
            fall_bars: f("smoothing", "fall_bars", d.fall_bars),
        };
        for (name, v, lo) in [
            ("weights.lean", cfg.w_lean, 0.0),
            ("weights.pulse", cfg.w_pulse, 0.0),
            ("weights.sway", cfg.w_sway, 0.0),
            ("weights.pressure_l", cfg.w_pressure_l, 0.0),
            ("weights.pressure_r", cfg.w_pressure_r, 0.0),
            (
                "press.depth_err_full",
                cfg.press_depth_err_full,
                f64::MIN_POSITIVE,
            ),
            (
                "flick.dir_err_full_deg",
                cfg.flick_dir_err_full_deg,
                f64::MIN_POSITIVE,
            ),
            ("tracking.err_full", cfg.track_err_full, f64::MIN_POSITIVE),
            (
                "pulse.err_full_ms",
                cfg.pulse_err_full_ms,
                f64::MIN_POSITIVE,
            ),
            ("smoothing.rise_bars", cfg.rise_bars, f64::MIN_POSITIVE),
            ("smoothing.fall_bars", cfg.fall_bars, f64::MIN_POSITIVE),
        ] {
            if !v.is_finite() || v < lo {
                return Err(FlintError::ValidationError(format!(
                    "coherence config: `{name}` = {v} out of range"
                )));
            }
        }
        Ok(cfg)
    }
}

impl CoherenceConfig {
    /// Rebuild from a header snapshot (inverse of [`Self::to_json`]);
    /// absent keys keep defaults, so old snapshots stay readable.
    pub fn from_json(v: &serde_json::Value) -> Self {
        let d = Self::default();
        let f = |table: &str, key: &str, default: f64| {
            v.get(table)
                .and_then(|t| t.get(key))
                .and_then(|x| x.as_f64())
                .unwrap_or(default)
        };
        Self {
            w_lean: f("weights", "lean", d.w_lean),
            w_pulse: f("weights", "pulse", d.w_pulse),
            w_sway: f("weights", "sway", d.w_sway),
            w_pressure_l: f("weights", "pressure_l", d.w_pressure_l),
            w_pressure_r: f("weights", "pressure_r", d.w_pressure_r),
            press_depth_err_full: f("press", "depth_err_full", d.press_depth_err_full),
            flick_dir_err_full_deg: f("flick", "dir_err_full_deg", d.flick_dir_err_full_deg),
            track_err_full: f("tracking", "err_full", d.track_err_full),
            track_curve: f("tracking", "curve", d.track_curve),
            pulse_err_full_ms: f("pulse", "err_full_ms", d.pulse_err_full_ms),
            miss_penalty: f("pulse", "miss_penalty", d.miss_penalty),
            spurious_penalty: f("pulse", "spurious_penalty", d.spurious_penalty),
            impulse_gain: f("pulse", "impulse_gain", d.impulse_gain),
            initial: f("smoothing", "initial", d.initial),
            rise_bars: f("smoothing", "rise_bars", d.rise_bars),
            fall_bars: f("smoothing", "fall_bars", d.fall_bars),
        }
    }

    /// Snapshot for log headers and reload records — reproducibility of a
    /// session depends on knowing exactly which knobs were live. The W2
    /// keys are emitted only when they differ from defaults, so pre-W2
    /// sessions and lean-only configs produce byte-identical headers.
    pub fn to_json(&self) -> serde_json::Value {
        let d = Self::default();
        let mut v = serde_json::json!({
            "weights": { "lean": self.w_lean, "pulse": self.w_pulse },
            "tracking": { "err_full": self.track_err_full, "curve": self.track_curve },
            "pulse": {
                "err_full_ms": self.pulse_err_full_ms,
                "miss_penalty": self.miss_penalty,
                "spurious_penalty": self.spurious_penalty,
                "impulse_gain": self.impulse_gain,
            },
            "smoothing": {
                "initial": self.initial,
                "rise_bars": self.rise_bars,
                "fall_bars": self.fall_bars,
            },
        });
        if self.w_sway != d.w_sway {
            v["weights"]["sway"] = serde_json::json!(self.w_sway);
        }
        if self.w_pressure_l != d.w_pressure_l {
            v["weights"]["pressure_l"] = serde_json::json!(self.w_pressure_l);
        }
        if self.w_pressure_r != d.w_pressure_r {
            v["weights"]["pressure_r"] = serde_json::json!(self.w_pressure_r);
        }
        if self.press_depth_err_full != d.press_depth_err_full {
            v["press"] = serde_json::json!({ "depth_err_full": self.press_depth_err_full });
        }
        if self.flick_dir_err_full_deg != d.flick_dir_err_full_deg {
            v["flick"] = serde_json::json!({ "dir_err_full_deg": self.flick_dir_err_full_deg });
        }
        v
    }
}

pub struct Coherence {
    value: f64,
    cfg: CoherenceConfig,
}

impl Coherence {
    pub fn new(cfg: CoherenceConfig) -> Self {
        Self {
            value: cfg.initial.clamp(0.0, 1.0),
            cfg,
        }
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    /// Swap the config mid-session; the current value survives, so tuning
    /// never resets the run.
    pub fn reconfigure(&mut self, cfg: CoherenceConfig) {
        self.cfg = cfg;
    }

    /// Fold one batch of judgment records into the scalar. `beats_per_bar`
    /// is the current meter (bar-denominated time constants); `grid_beats`
    /// is the judgment grid step, so each track record advances time by
    /// `grid_beats / beats_per_bar` bars.
    pub fn step(&mut self, records: &[JudgmentRecord], grid_beats: f64, beats_per_bar: f64) -> f64 {
        let c = &self.cfg;
        for rec in records {
            match rec {
                JudgmentRecord::Track { channel, err, .. } => {
                    // Lean keeps its historical semantics exactly; the W2
                    // channels are skipped at weight 0 (inert by default —
                    // fit=1 at w=0 would push the value *up*).
                    let weight = match channel.as_str() {
                        "sway" => c.w_sway,
                        "pressure_l" => c.w_pressure_l,
                        "pressure_r" => c.w_pressure_r,
                        _ => c.w_lean,
                    };
                    if channel != "lean" && weight == 0.0 {
                        continue;
                    }
                    let pen = (err / c.track_err_full).clamp(0.0, 1.0).powf(c.track_curve);
                    let fit = 1.0 - weight * pen;
                    let tau = if fit >= self.value {
                        c.rise_bars
                    } else {
                        c.fall_bars
                    };
                    let step_bars = grid_beats / beats_per_bar.max(f64::MIN_POSITIVE);
                    let alpha = 1.0 - (-step_bars / tau).exp();
                    self.value += (fit - self.value) * alpha;
                }
                JudgmentRecord::Pulse {
                    err_ms,
                    depth_err,
                    dir_err,
                    ..
                } => {
                    let pen = (err_ms.abs() / c.pulse_err_full_ms).clamp(0.0, 1.0);
                    let mut quality = 1.0 - pen;
                    if let Some(d) = depth_err {
                        quality *= 1.0 - (d / c.press_depth_err_full).clamp(0.0, 1.0);
                    }
                    if let Some(d) = dir_err {
                        quality *= 1.0 - (d / c.flick_dir_err_full_deg).clamp(0.0, 1.0);
                    }
                    self.value += c.w_pulse * c.impulse_gain * quality;
                }
                JudgmentRecord::Miss { .. } => {
                    self.value -= c.w_pulse * c.impulse_gain * c.miss_penalty;
                }
                JudgmentRecord::Spurious { .. } => {
                    self.value -= c.w_pulse * c.impulse_gain * c.spurious_penalty;
                }
            }
            self.value = self.value.clamp(0.0, 1.0);
        }
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(err: f64) -> JudgmentRecord {
        JudgmentRecord::Track {
            sample: 0,
            beat: 0.0,
            channel: "lean".into(),
            target: [0.0, 0.0],
            actual: [0.0, 0.0],
            err,
        }
    }

    fn miss() -> JudgmentRecord {
        JudgmentRecord::Miss {
            sample: 0,
            window: 0,
            kind: "pulse".into(),
            center_beat: 0.0,
        }
    }

    /// 4/4, 1/8-beat grid: 32 track records per bar.
    fn run_bars(c: &mut Coherence, bars: usize, err: f64) {
        for _ in 0..bars * 32 {
            c.step(&[track(err)], 0.125, 4.0);
        }
    }

    #[test]
    fn perfect_play_rises_toward_one() {
        let mut c = Coherence::new(CoherenceConfig::default());
        let mut last = c.value();
        for _ in 0..4 {
            run_bars(&mut c, 1, 0.0);
            assert!(c.value() > last, "must rise monotonically");
            last = c.value();
        }
        assert!(c.value() > 0.9, "got {}", c.value());
    }

    #[test]
    fn neglect_decays_with_the_configured_half_life() {
        let cfg = CoherenceConfig::default();
        let mut c = Coherence::new(cfg);
        run_bars(&mut c, 8, 0.0); // settle near 1.0
        let start = c.value();
        assert!(start > 0.99);

        // Full tracking error: fit floor = 1 - w_lean = 0.4.
        let floor = 1.0 - cfg.w_lean;
        run_bars(&mut c, 2, 10.0); // fall_bars = 2 → one time constant
        let expected = floor + (start - floor) * (-1.0f64).exp();
        assert!(
            (c.value() - expected).abs() < 0.02,
            "after one τ: {} vs expected {expected}",
            c.value()
        );
        // Smooth: no step exceeds one grid-step alpha bound.
        let alpha = 1.0 - (-0.125 / 4.0 / cfg.fall_bars).exp();
        let mut prev = c.value();
        for _ in 0..32 {
            c.step(&[track(10.0)], 0.125, 4.0);
            assert!((prev - c.value()) <= alpha * 1.0 + 1e-12);
            prev = c.value();
        }
    }

    #[test]
    fn single_miss_is_nearly_invisible() {
        let mut c = Coherence::new(CoherenceConfig::default());
        run_bars(&mut c, 8, 0.0);
        let before = c.value();
        c.step(&[miss()], 0.125, 4.0);
        let dip = before - c.value();
        // GDD: single mistakes nearly invisible. One miss at defaults is a
        // 0.06 dip that the next bar of good play erases.
        assert!(dip > 0.0 && dip < 0.1, "dip {dip}");
        // rise_bars = 1: one bar recovers ~63% of the dip, two bars ~86%.
        run_bars(&mut c, 2, 0.0);
        assert!(c.value() > before - 0.01, "recovered to {}", c.value());
    }

    #[test]
    fn clean_hits_lift_sloppy_hits_less() {
        let cfg = CoherenceConfig::default();
        let hit = |ms: f64| JudgmentRecord::Pulse {
            sample: 0,
            beat: 0.0,
            window: 0,
            kind: "pulse".into(),
            err_ms: ms,
            depth_err: None,
            dir_err: None,
        };
        let mut a = Coherence::new(cfg);
        let mut b = Coherence::new(cfg);
        a.step(&[hit(0.0)], 0.125, 4.0);
        b.step(&[hit(100.0)], 0.125, 4.0);
        assert!(a.value() > b.value());
        assert!(b.value() >= cfg.initial, "sloppy hit still never punishes");
    }

    #[test]
    fn config_reload_preserves_value() {
        let mut c = Coherence::new(CoherenceConfig::default());
        run_bars(&mut c, 4, 0.0);
        let v = c.value();
        let mut cfg = CoherenceConfig::default();
        cfg.fall_bars = 4.0;
        c.reconfigure(cfg);
        assert_eq!(c.value(), v);
    }

    #[test]
    fn w2_channels_parse_and_default_inert() {
        // Parsing the W2 tables works…
        let cfg = CoherenceConfig::parse(
            "schema_version = 0\n[weights]\nsway = 0.3\npressure_r = 0.2\n[press]\ndepth_err_full = 0.4\n[flick]\ndir_err_full_deg = 60.0\n",
        )
        .unwrap();
        assert_eq!(cfg.w_sway, 0.3);
        assert_eq!(cfg.w_pressure_r, 0.2);
        assert_eq!(cfg.press_depth_err_full, 0.4);
        assert_eq!(cfg.flick_dir_err_full_deg, 60.0);

        // …and at the default weights a non-lean Track record is fully
        // inert: no fit, no time advance (the inert-by-default contract).
        let mut c = Coherence::new(CoherenceConfig::default());
        let before = c.value();
        c.step(
            &[JudgmentRecord::Track {
                sample: 0,
                beat: 0.0,
                channel: "sway".into(),
                target: [1.0, 0.0],
                actual: [0.0, 0.0],
                err: 1.0,
            }],
            0.125,
            4.0,
        );
        assert_eq!(c.value(), before);

        // Default snapshots carry no W2 keys (header byte-stability).
        let snap = CoherenceConfig::default().to_json();
        assert!(snap["weights"].get("sway").is_none());
        assert!(snap.get("press").is_none());
        assert!(snap.get("flick").is_none());

        // Quality factors shrink the hit impulse.
        let mut cfg = CoherenceConfig::default();
        cfg.w_sway = 0.0;
        let hit = |depth_err: Option<f64>, dir_err: Option<f64>| JudgmentRecord::Pulse {
            sample: 0,
            beat: 0.0,
            window: 0,
            kind: "press".into(),
            err_ms: 0.0,
            depth_err,
            dir_err,
        };
        let mut clean = Coherence::new(cfg);
        let mut shallow = Coherence::new(cfg);
        clean.step(&[hit(Some(0.0), None)], 0.125, 4.0);
        shallow.step(&[hit(Some(0.4), None)], 0.125, 4.0);
        assert!(clean.value() > shallow.value());
    }

    #[test]
    fn parse_roundtrip_and_validation() {
        let cfg = CoherenceConfig::parse(
            r#"
schema_version = 0
[weights]
lean = 0.7
pulse = 0.3
[smoothing]
fall_bars = 3.0
"#,
        )
        .unwrap();
        assert_eq!(cfg.w_lean, 0.7);
        assert_eq!(cfg.fall_bars, 3.0);
        // Unspecified keys keep defaults.
        assert_eq!(cfg.impulse_gain, CoherenceConfig::default().impulse_gain);
        // Invalid values refuse to load.
        assert!(CoherenceConfig::parse("[smoothing]\nrise_bars = 0.0\n").is_err());
        assert!(CoherenceConfig::parse("schema_version = 9\n").is_err());
    }
}
