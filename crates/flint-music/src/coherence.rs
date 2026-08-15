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

use crate::judgment::JudgmentRecord;
use flint_core::toml_util::toml_f64;
use flint_core::{FlintError, Result};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoherenceConfig {
    /// Blend weight of the continuous tracking signal.
    pub w_lean: f64,
    /// Blend weight of discrete pulse outcomes.
    pub w_pulse: f64,
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
        let root: toml::Value = text
            .parse()
            .map_err(|e| FlintError::TomlParseError(format!("coherence config: {e}")))?;
        let version = root
            .get("schema_version")
            .and_then(|v| v.as_integer())
            .unwrap_or(0);
        if version != 0 {
            return Err(FlintError::ValidationError(format!(
                "coherence config: unknown schema_version {version}"
            )));
        }
        let f = |table: &str, key: &str, default: f64| -> f64 {
            root.get(table)
                .and_then(|t| t.get(key))
                .and_then(toml_f64)
                .unwrap_or(default)
        };
        let d = Self::default();
        let cfg = Self {
            w_lean: f("weights", "lean", d.w_lean),
            w_pulse: f("weights", "pulse", d.w_pulse),
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
            ("tracking.err_full", cfg.track_err_full, f64::MIN_POSITIVE),
            ("pulse.err_full_ms", cfg.pulse_err_full_ms, f64::MIN_POSITIVE),
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
    /// Snapshot for log headers and reload records — reproducibility of a
    /// session depends on knowing exactly which knobs were live.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
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
        })
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
    pub fn step(
        &mut self,
        records: &[JudgmentRecord],
        grid_beats: f64,
        beats_per_bar: f64,
    ) -> f64 {
        let c = &self.cfg;
        for rec in records {
            match rec {
                JudgmentRecord::Track { err, .. } => {
                    let pen = (err / c.track_err_full).clamp(0.0, 1.0).powf(c.track_curve);
                    let fit = 1.0 - c.w_lean * pen;
                    let tau = if fit >= self.value {
                        c.rise_bars
                    } else {
                        c.fall_bars
                    };
                    let step_bars = grid_beats / beats_per_bar.max(f64::MIN_POSITIVE);
                    let alpha = 1.0 - (-step_bars / tau).exp();
                    self.value += (fit - self.value) * alpha;
                }
                JudgmentRecord::Pulse { err_ms, .. } => {
                    let pen = (err_ms.abs() / c.pulse_err_full_ms).clamp(0.0, 1.0);
                    self.value += c.w_pulse * c.impulse_gain * (1.0 - pen);
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
