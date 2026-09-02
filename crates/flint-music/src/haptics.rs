//! Haptic entrainment (ADR 0026): the pure decision layer mapping musical
//! structure onto motor events — pre-beat tick, pulse-landing thump, rewind
//! grind, pickup ticks. **Entrainment, not guidance** (the tips doc,
//! clarified 2026-08-16): Xbox motors are a low/high *weight* axis, not a
//! direction, and the pulse verb is purely temporal — touch is the best
//! timekeeper we can reach.
//!
//! Two guards, structural:
//!
//! - **Event-shaped, never state-shaped**: the driver never sees the lean
//!   error, coherence, or any continuous wrongness signal — a buzz-when-
//!   wrong reads as punishment and is instantly nameable. Its only inputs
//!   are events (judged pulses, sequencer transitions) and the grid.
//! - **No hardware here**: the driver emits [`HapticEvent`]s; the actual
//!   motor writes live in flint-input-capture's rumble engine (direct
//!   XInput, ADR 0025). flint-music never gains a gilrs dependency. Replay
//!   and offline renders simply never attach a sink — no-op by construction.
//!
//! Timing: bursts are addressed in **suite samples**; the caller re-maps to
//! raw clock samples (`+ timeline_offset`) on the way to the capture thread,
//! which fires them early by the feel-tuned `lead_ms` (rumble transport and
//! actuator spin-up are unmeasurable in software — ADR 0025).

use crate::conductor::{Conductor, Grid};
use crate::config_toml::{self, VersionPolicy};
use crate::reintegration::{ReintegrationEvent, SeqPhase};
use crate::tempo::ms_to_samples;
use flint_core::{FlintError, Result};
use std::path::Path;

/// Which grid the pre-beat tick rides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickGrid {
    Beat,
    Bar,
    Off,
}

impl TickGrid {
    pub fn as_str(&self) -> &'static str {
        match self {
            TickGrid::Beat => "beat",
            TickGrid::Bar => "bar",
            TickGrid::Off => "off",
        }
    }
}

/// One burst shape: motor magnitudes in [0, 1] and a duration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BurstCfg {
    pub strong: f64,
    pub weak: f64,
    pub duration_ms: f64,
}

impl BurstCfg {
    fn silent(duration_ms: f64) -> Self {
        Self {
            strong: 0.0,
            weak: 0.0,
            duration_ms,
        }
    }
    fn is_active(&self) -> bool {
        self.strong > 0.0 || self.weak > 0.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HapticsConfig {
    /// Fire early by this much: audio output latency plus motor spin-up,
    /// tuned by feel (ADR 0025 — seeded from measured latency minus ~30 ms).
    pub lead_ms: f64,
    /// A burst later than this past its fire point is dropped, not played —
    /// a late tick fights the ear and is worse than none.
    pub late_drop_ms: f64,
    /// Master gain [0, 1]; 0 silences everything without touching shapes.
    pub gain: f64,
    /// The metronome-in-the-hands: a light burst on every grid point.
    pub tick_grid: TickGrid,
    pub tick: BurstCfg,
    /// Landing weight on judged pulse *hits* only — misses and spurious
    /// presses stay silent (punishment-shaped otherwise).
    pub thump: BurstCfg,
    /// Sustained low weight through the rewind gesture, scaled by the
    /// spin-down progress (duration_ms unused).
    pub grind: BurstCfg,
    /// "and-a—": discrete ticks on the last beats before the seam, the ONE
    /// being the ordinary post-seam tick/thump.
    pub pickup: BurstCfg,
    pub pickup_count: u32,
}

impl Default for HapticsConfig {
    /// INERT: every magnitude zero. The built-in default (no config file)
    /// emits nothing ever — `config/haptics.toml` is where the real values
    /// live, mirroring the gradient contract.
    fn default() -> Self {
        Self {
            lead_ms: 90.0,
            late_drop_ms: 30.0,
            gain: 1.0,
            tick_grid: TickGrid::Beat,
            tick: BurstCfg::silent(25.0),
            thump: BurstCfg::silent(60.0),
            grind: BurstCfg::silent(0.0),
            pickup: BurstCfg::silent(30.0),
            pickup_count: 2,
        }
    }
}

impl HapticsConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let (root, _) =
            config_toml::parse_versioned("haptics config", text, VersionPolicy::DefaultZeroStrict)?;
        let d = Self::default();
        let f = |table: &str, key: &str, default: f64| {
            config_toml::section_f64(&root, table, key, default)
        };
        let burst = |table: &str, default: BurstCfg| BurstCfg {
            strong: f(table, "strong", default.strong),
            weak: f(table, "weak", default.weak),
            duration_ms: f(table, "duration_ms", default.duration_ms),
        };
        let tick_grid = match root
            .get("tick")
            .and_then(|t| t.get("grid"))
            .and_then(|v| v.as_str())
        {
            None => d.tick_grid,
            Some("beat") => TickGrid::Beat,
            Some("bar") => TickGrid::Bar,
            Some("off") => TickGrid::Off,
            Some(other) => {
                return Err(FlintError::ValidationError(format!(
                    "haptics config: tick.grid '{other}' (expected beat|bar|off)"
                )))
            }
        };
        let cfg = Self {
            lead_ms: f("timing", "lead_ms", d.lead_ms),
            late_drop_ms: f("timing", "late_drop_ms", d.late_drop_ms),
            gain: f("timing", "gain", d.gain),
            tick_grid,
            tick: burst("tick", d.tick),
            thump: burst("thump", d.thump),
            grind: burst("grind", d.grind),
            pickup: burst("pickup", d.pickup),
            pickup_count: root
                .get("pickup")
                .and_then(|t| t.get("count"))
                .and_then(|v| v.as_integer())
                .unwrap_or(d.pickup_count as i64) as u32,
        };
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        let err = |msg: String| {
            Err(FlintError::ValidationError(format!(
                "haptics config: {msg}"
            )))
        };
        for (name, v) in [
            ("timing.lead_ms", self.lead_ms),
            ("timing.late_drop_ms", self.late_drop_ms),
        ] {
            if !(0.0..=250.0).contains(&v) {
                return err(format!("{name} {v} out of range (0..250)"));
            }
        }
        if !(0.0..=1.0).contains(&self.gain) {
            return err(format!("timing.gain {} out of range (0..1)", self.gain));
        }
        for (name, b) in [
            ("tick", &self.tick),
            ("thump", &self.thump),
            ("grind", &self.grind),
            ("pickup", &self.pickup),
        ] {
            for (field, v) in [("strong", b.strong), ("weak", b.weak)] {
                if !(0.0..=1.0).contains(&v) {
                    return err(format!("{name}.{field} {v} out of range (0..1)"));
                }
            }
            if !(0.0..=500.0).contains(&b.duration_ms) {
                return err(format!(
                    "{name}.duration_ms {} out of range (0..500)",
                    b.duration_ms
                ));
            }
        }
        if self.pickup_count > 8 {
            return err(format!(
                "pickup.count {} out of range (0..8)",
                self.pickup_count
            ));
        }
        Ok(())
    }

    /// Whether the config can ever move a motor. The inert built-in returns
    /// false — haptics-free sessions emit no events at all.
    pub fn is_active(&self) -> bool {
        self.gain > 0.0
            && (self.tick.is_active()
                || self.thump.is_active()
                || self.grind.is_active()
                || self.pickup.is_active())
    }

    /// Snapshot for log headers (session reproducibility).
    pub fn to_json(&self) -> serde_json::Value {
        let burst = |b: &BurstCfg| {
            serde_json::json!({
                "strong": b.strong, "weak": b.weak, "duration_ms": b.duration_ms,
            })
        };
        serde_json::json!({
            "timing": {
                "lead_ms": self.lead_ms,
                "late_drop_ms": self.late_drop_ms,
                "gain": self.gain,
            },
            "tick": { "grid": self.tick_grid.as_str(), "shape": burst(&self.tick) },
            "thump": burst(&self.thump),
            "grind": burst(&self.grind),
            "pickup": { "count": self.pickup_count, "shape": burst(&self.pickup) },
        })
    }
}

/// One motor event, suite-sample addressed. The caller (a live front end)
/// re-maps `at_suite_sample` to raw clock samples and forwards to the
/// capture thread's rumble engine; with no sink attached the events vanish.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HapticEvent {
    /// A one-shot burst firing when playback reaches the sample (minus the
    /// engine's lead).
    Burst {
        at_suite_sample: i64,
        strong: f64,
        weak: f64,
        duration_samples: i64,
    },
    /// A one-shot burst played as fast as the motors can — no lead, no
    /// late-drop. For felt confirmations of things that already happened
    /// (the pulse-hit thump): "now" in ear time is already ~latency in the
    /// raw clock's past, so a scheduled burst would always read as late.
    Immediate {
        strong: f64,
        weak: f64,
        duration_samples: i64,
    },
    /// Sustained motor state, applied immediately; `(0, 0)` releases it.
    Continuous { strong: f64, weak: f64 },
    /// Drop everything scheduled and stop the continuous state (seams:
    /// bursts resolved against the old timeline are wrong after the jump).
    Flush,
    /// Engine settings, emitted at session start and on config reload.
    Config {
        lead_samples: i64,
        late_drop_samples: i64,
        gain: f64,
    },
}

/// Pure evaluator: musical structure in, [`HapticEvent`]s out. Owns only
/// grid bookkeeping — no clocks, no hardware, deterministic.
pub struct HapticsDriver {
    cfg: HapticsConfig,
    /// Next unscheduled grid tick (suite sample); `None` = re-seed.
    next_tick: Option<i64>,
    /// Last emitted continuous grind level, delta-gated to avoid spamming
    /// the command channel every tick.
    grind_level: f64,
}

/// How far ahead grid ticks are handed to the engine. Comfortably beyond
/// any front end's tick cadence plus the largest lead; the engine holds
/// them to the sample.
const LOOKAHEAD_S: f64 = 0.5;

impl HapticsDriver {
    pub fn new(cfg: HapticsConfig) -> Self {
        Self {
            cfg,
            next_tick: None,
            grind_level: 0.0,
        }
    }

    pub fn config(&self) -> &HapticsConfig {
        &self.cfg
    }

    /// Swap the config mid-session; grid bookkeeping carries over.
    pub fn reconfigure(&mut self, cfg: HapticsConfig) {
        self.cfg = cfg;
    }

    /// The engine-settings event for the current config (session start and
    /// after every reload — lead changes must reach already-scheduled ticks).
    pub fn config_event(&self, sample_rate: u32) -> HapticEvent {
        let to_samples = |ms: f64| ms_to_samples(ms, sample_rate as f64);
        HapticEvent::Config {
            lead_samples: to_samples(self.cfg.lead_ms),
            late_drop_samples: to_samples(self.cfg.late_drop_ms),
            gain: self.cfg.gain,
        }
    }

    /// One tick, past preroll. Inputs are deliberately event-shaped: the
    /// frame's judged pulses, the sequencer's transitions, the phase, and
    /// the rewind progress scalar — never lean error or coherence.
    #[allow(clippy::too_many_arguments)]
    pub fn evaluate(
        &mut self,
        conductor: &Conductor,
        now_sample: i64,
        phase: SeqPhase,
        rewind: f64,
        pickup_beats: f64,
        pulses: &[(i64, f64, crate::chart_session::PulseKind)],
        seq_events: &[ReintegrationEvent],
        out: &mut Vec<HapticEvent>,
    ) {
        if !self.cfg.is_active() {
            return;
        }
        let sample_rate = conductor.tempo().sample_rate() as f64;
        let to_samples = |ms: f64| ms_to_samples(ms, sample_rate);

        // Sequencer transitions first: a seam flushes everything scheduled
        // (old-timeline bursts are wrong after the jump) and re-seeds the
        // grid from the re-entered position.
        for ev in seq_events {
            match ev {
                ReintegrationEvent::FullFail {
                    seam_suite_sample,
                    re_entry_sample,
                    ..
                } => {
                    // "and-a—": pickup ticks on the last beats before the
                    // seam, spaced at the re-entry tempo like the audio cue
                    // (reintegration.rs), the ONE being the ordinary
                    // post-seam grid tick.
                    if self.cfg.pickup.is_active() && self.cfg.pickup_count > 0 {
                        let beat_samples = samples_per_beat(conductor, *re_entry_sample);
                        let n = self
                            .cfg
                            .pickup_count
                            .min(pickup_beats.floor().max(0.0) as u32);
                        for k in 1..=i64::from(n) {
                            out.push(HapticEvent::Burst {
                                at_suite_sample: seam_suite_sample - k * beat_samples,
                                strong: self.cfg.pickup.strong,
                                weak: self.cfg.pickup.weak,
                                duration_samples: to_samples(self.cfg.pickup.duration_ms),
                            });
                        }
                    }
                }
                ReintegrationEvent::Seam { .. } => {
                    out.push(HapticEvent::Flush);
                    self.grind_level = 0.0;
                    self.next_tick = None;
                }
                ReintegrationEvent::ReassemblyComplete { .. } => {}
            }
        }

        // Rewind grind: sustained low weight growing with the spin-down.
        // Delta-gated; the seam's Flush releases it.
        if self.cfg.grind.is_active() {
            let target = if phase == SeqPhase::Failing {
                rewind
            } else {
                0.0
            };
            if (target - self.grind_level).abs() > 1.0 / 64.0
                || (target == 0.0 && self.grind_level != 0.0)
            {
                self.grind_level = target;
                out.push(HapticEvent::Continuous {
                    strong: self.cfg.grind.strong * target,
                    weak: self.cfg.grind.weak * target,
                });
            }
        }

        // Landing thump: judged hits only. Immediate — the press is already
        // in the past; lead and late-drop must not apply to a felt
        // confirmation (a scheduled "now" is ~latency late in raw-clock
        // terms and would be dropped, never played).
        if self.cfg.thump.is_active() {
            for (_, _, kind) in pulses {
                if *kind == crate::chart_session::PulseKind::Hit {
                    out.push(HapticEvent::Immediate {
                        strong: self.cfg.thump.strong,
                        weak: self.cfg.thump.weak,
                        duration_samples: to_samples(self.cfg.thump.duration_ms),
                    });
                }
            }
        }

        // Grid tick: schedule everything inside the lookahead window.
        // Suppressed outside Playing — the rewind/pickup own the seam span.
        if self.cfg.tick.is_active() && self.cfg.tick_grid != TickGrid::Off {
            if phase == SeqPhase::Playing {
                let grid = match self.cfg.tick_grid {
                    TickGrid::Beat => Grid::Beat,
                    TickGrid::Bar => Grid::Bar,
                    TickGrid::Off => unreachable!(),
                };
                let horizon = now_sample + (LOOKAHEAD_S * sample_rate) as i64;
                let mut next = match self.next_tick {
                    // Strictly-after semantics make re-seeding idempotent.
                    Some(n) if n > now_sample => n,
                    _ => conductor.next_grid_sample(now_sample, grid),
                };
                while next <= horizon {
                    out.push(HapticEvent::Burst {
                        at_suite_sample: next,
                        strong: self.cfg.tick.strong,
                        weak: self.cfg.tick.weak,
                        duration_samples: to_samples(self.cfg.tick.duration_ms),
                    });
                    let after = conductor.next_grid_sample(next, grid);
                    if after <= next {
                        // Grid exhausted or degenerate (the conductor's
                        // documented fallback returns `from`). Never spin.
                        tracing::warn!(
                            "haptic tick grid stalled at sample {next}; suspending ticks"
                        );
                        break;
                    }
                    next = after;
                }
                self.next_tick = Some(next);
            } else {
                self.next_tick = None;
            }
        }
    }
}

/// Beat length in samples at `sample`'s tempo (the reintegrator's own
/// approximation for the pickup span).
fn samples_per_beat(conductor: &Conductor, sample: i64) -> i64 {
    let beat = conductor.position_at_sample(sample.max(0)).beat.floor();
    (conductor.sample_at_beat(beat + 1.0) - conductor.sample_at_beat(beat)).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_cfg() -> HapticsConfig {
        HapticsConfig::parse(
            r#"
schema_version = 0
[timing]
lead_ms = 90.0
late_drop_ms = 30.0
[tick]
grid = "beat"
weak = 0.35
duration_ms = 25.0
[thump]
strong = 0.55
weak = 0.25
duration_ms = 60.0
[grind]
strong = 0.45
[pickup]
weak = 0.4
strong = 0.15
duration_ms = 30.0
count = 2
"#,
        )
        .unwrap()
    }

    #[test]
    fn builtin_default_is_inert() {
        let cfg = HapticsConfig::default();
        assert!(!cfg.is_active());
    }

    #[test]
    fn parse_roundtrip_and_validation() {
        let cfg = active_cfg();
        assert!(cfg.is_active());
        assert_eq!(cfg.tick_grid, TickGrid::Beat);
        assert_eq!(cfg.thump.strong, 0.55);
        assert_eq!(cfg.pickup_count, 2);
        assert!(cfg.to_json()["tick"]["shape"]["weak"].as_f64().unwrap() > 0.3);

        assert!(HapticsConfig::parse("schema_version = 9\n").is_err());
        assert!(HapticsConfig::parse("[tick]\ngrid = \"eighth\"\n").is_err());
        assert!(HapticsConfig::parse("[thump]\nstrong = 1.5\n").is_err());
        assert!(HapticsConfig::parse("[timing]\nlead_ms = 400.0\n").is_err());
        assert!(HapticsConfig::parse("").is_ok());
        assert!(!HapticsConfig::parse("").unwrap().is_active());
    }

    #[test]
    fn gain_zero_makes_everything_inert() {
        let mut cfg = active_cfg();
        cfg.gain = 0.0;
        assert!(!cfg.is_active());
    }
}
