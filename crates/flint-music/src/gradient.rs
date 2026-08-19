//! The error-driven audio gradient (ADR 0024): a continuous, hill-climbable
//! mapping from lean error and stick-neutrality onto the mixer — the audio
//! twin of the prologue scene's fog-haze gradient, computed from the same two
//! numbers so the world comes apart as one thing.
//!
//! Two devices, both config-driven (`config/gradient.toml`):
//!
//! - **Tune**: lean error drives the *depth* of a zero-mean pitch LFO on a
//!   designated bus — off the lean, the voice wavers; on it, the voice
//!   settles steady and in tune. Depth, not a flat offset: detune is
//!   playback rate (kira has no per-track pitch), so a sustained one-sided
//!   offset would accrue real timeline drift with no seam to re-lock it,
//!   while a zero-mean LFO's drift stays bounded (the ladder-warble
//!   precedent). Optionally the same error thins the bus a little.
//! - **Sink**: at stick-neutral the mix thins — per-bus gain trims scaled by
//!   a smoothed neutral amount. The world is less alive, not punishing.
//!
//! The driver is a *pure evaluator*: it owns slew state and returns
//! [`GradientOffsets`] that the caller feeds into the single mixer writer
//! ([`crate::ladder::LadderDriver::apply`]) — one shadow state, no sibling
//! fights. Never through Rhai: scripts are read-only toward audio by design.

use crate::config_toml::{self, VersionPolicy};
use crate::ladder::DEGRADABLE_BUSES;
use flint_core::toml_util::toml_f64;
use flint_core::{FlintError, Result};
use std::collections::BTreeMap;
use std::path::Path;

/// The tuning device: err → wobble depth (+ optional gain dulling) on one bus.
#[derive(Debug, Clone, PartialEq)]
pub struct TuneConfig {
    /// Designated bus. Must be degradable — motif buses and the foundation
    /// stay in tune no matter what (the purity anchor).
    pub bus: String,
    /// Wobble depth (semitones, peak) at `err >= err_full`.
    pub max_depth_semitones: f64,
    /// Wobble LFO rate in Hz — slow waver, distinct from the ladder's warble.
    pub rate_hz: f64,
    /// Lean error (Euclidean, lean space) that maps to full depth.
    pub err_full: f64,
    /// Gain trim (dB) on the bus at full error; 0 disables.
    pub gain_trim_db: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GradientConfig {
    pub tune: TuneConfig,
    /// |lean| below this counts as fully neutral (sink target 1).
    pub sink_threshold: f64,
    /// Per-bus gain trim (dB) at full neutral. Degradable buses only.
    pub sink_db: BTreeMap<String, f32>,
    /// Slew toward degraded (error rising / going neutral), ms.
    pub attack_ms: f64,
    /// Slew toward clean (error clearing / stick re-engaging), ms —
    /// recovery a touch slower than onset keeps the gradient hill-climbable.
    pub release_ms: f64,
}

impl Default for GradientConfig {
    /// INERT: zero depth, zero trims. The built-in default (no config file)
    /// must leave every render byte-identical to a gradient-free build —
    /// `config/gradient.toml` is where the real values live.
    fn default() -> Self {
        Self {
            tune: TuneConfig {
                bus: "world_voice".into(),
                max_depth_semitones: 0.0,
                rate_hz: 0.8,
                err_full: 0.9,
                gain_trim_db: 0.0,
            },
            sink_threshold: 0.15,
            sink_db: BTreeMap::new(),
            attack_ms: 120.0,
            release_ms: 450.0,
        }
    }
}

impl GradientConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let (root, _) =
            config_toml::parse_versioned("gradient config", text, VersionPolicy::DefaultZeroStrict)?;
        let d = Self::default();
        let tune_t = root.get("tune");
        let tf = |key: &str, default: f64| {
            tune_t.and_then(|t| t.get(key)).and_then(toml_f64).unwrap_or(default)
        };
        let tune = TuneConfig {
            bus: tune_t
                .and_then(|t| t.get("bus"))
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| d.tune.bus.clone()),
            max_depth_semitones: tf("max_depth_semitones", d.tune.max_depth_semitones),
            rate_hz: tf("rate_hz", d.tune.rate_hz),
            err_full: tf("err_full", d.tune.err_full),
            gain_trim_db: tf("gain_trim_db", d.tune.gain_trim_db as f64) as f32,
        };
        let sink_t = root.get("sink");
        let sink_threshold = sink_t
            .and_then(|t| t.get("threshold"))
            .and_then(toml_f64)
            .unwrap_or(d.sink_threshold);
        let mut sink_db = BTreeMap::new();
        if let Some(tbl) = sink_t.and_then(|t| t.get("db")).and_then(|v| v.as_table()) {
            for (bus, v) in tbl {
                let db = toml_f64(v).ok_or_else(|| {
                    FlintError::ValidationError(format!(
                        "gradient config: sink.db.{bus} not a number"
                    ))
                })?;
                sink_db.insert(bus.clone(), db as f32);
            }
        }
        let smooth_t = root.get("smoothing");
        let sf = |key: &str, default: f64| {
            smooth_t.and_then(|t| t.get(key)).and_then(toml_f64).unwrap_or(default)
        };
        let cfg = Self {
            tune,
            sink_threshold,
            sink_db,
            attack_ms: sf("attack_ms", d.attack_ms),
            release_ms: sf("release_ms", d.release_ms),
        };
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        let err =
            |msg: String| Err(FlintError::ValidationError(format!("gradient config: {msg}")));
        if !DEGRADABLE_BUSES.contains(&self.tune.bus.as_str()) {
            return err(format!(
                "tune.bus '{}' is protected (degradable: {DEGRADABLE_BUSES:?})",
                self.tune.bus
            ));
        }
        if !(0.0..=1.0).contains(&self.tune.max_depth_semitones) {
            return err(format!(
                "tune.max_depth_semitones {} out of range (0..1)",
                self.tune.max_depth_semitones
            ));
        }
        if !(0.0..=8.0).contains(&self.tune.rate_hz) {
            return err(format!("tune.rate_hz {} out of range (0..8)", self.tune.rate_hz));
        }
        if !self.tune.err_full.is_finite() || self.tune.err_full <= 0.0 {
            return err(format!("tune.err_full {} must be > 0", self.tune.err_full));
        }
        if !(-24.0..=0.0).contains(&self.tune.gain_trim_db) {
            return err(format!(
                "tune.gain_trim_db {} out of range (-24..0)",
                self.tune.gain_trim_db
            ));
        }
        if !(0.0..=1.0).contains(&self.sink_threshold) {
            return err(format!(
                "sink.threshold {} out of range (0..1)",
                self.sink_threshold
            ));
        }
        for (bus, db) in &self.sink_db {
            if !DEGRADABLE_BUSES.contains(&bus.as_str()) {
                return err(format!(
                    "sink.db touches protected bus '{bus}' (degradable: {DEGRADABLE_BUSES:?})"
                ));
            }
            if !(-24.0..=0.0).contains(db) {
                return err(format!("sink.db.{bus} {db} out of range (-24..0)"));
            }
        }
        for (name, ms) in [("attack_ms", self.attack_ms), ("release_ms", self.release_ms)] {
            if !ms.is_finite() || ms < 0.0 {
                return err(format!("smoothing.{name} {ms} out of range"));
            }
        }
        Ok(())
    }

    /// Whether the config can ever move the mixer. The inert built-in returns
    /// false, which is what keeps gradient-free runs byte-identical.
    pub fn is_active(&self) -> bool {
        self.tune.max_depth_semitones > 0.0
            || self.tune.gain_trim_db != 0.0
            || self.sink_db.values().any(|db| *db != 0.0)
    }

    /// Snapshot for log headers (session reproducibility).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "tune": {
                "bus": self.tune.bus,
                "max_depth_semitones": self.tune.max_depth_semitones,
                "rate_hz": self.tune.rate_hz,
                "err_full": self.tune.err_full,
                "gain_trim_db": self.tune.gain_trim_db,
            },
            "sink": {
                "threshold": self.sink_threshold,
                "db": self.sink_db,
            },
            "smoothing": {
                "attack_ms": self.attack_ms,
                "release_ms": self.release_ms,
            },
        })
    }
}

/// Per-tick additive offsets, summed into the [`crate::ladder::LadderDriver`]
/// targets before its epsilon-diff — one writer, one shadow state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GradientOffsets {
    /// Additive gain trim (dB) per bus.
    pub gain_db: BTreeMap<String, f32>,
    /// Additive detune (semitones) per bus.
    pub detune: BTreeMap<String, f64>,
}

/// Pure evaluator with slew state: feed it the frame's lean error and lean
/// magnitude (both suite-time deterministic), get offsets back. It never
/// touches the mixer itself.
pub struct GradientDriver {
    cfg: GradientConfig,
    /// Smoothed normalized error (0..1).
    err_s: f64,
    /// Smoothed neutral amount (0..1).
    sink_s: f64,
    last_seconds: Option<f64>,
}

impl GradientDriver {
    pub fn new(cfg: GradientConfig) -> Self {
        Self {
            cfg,
            err_s: 0.0,
            sink_s: 0.0,
            last_seconds: None,
        }
    }

    pub fn config(&self) -> &GradientConfig {
        &self.cfg
    }

    /// Swap the config mid-session; slew state carries over so a hot reload
    /// never steps the audio.
    pub fn reconfigure(&mut self, cfg: GradientConfig) {
        self.cfg = cfg;
    }

    /// Zero the slew state. Call at a seam: the world re-enters clean, and
    /// the gradient eases back in from silence via its own attack.
    pub fn reset(&mut self) {
        self.err_s = 0.0;
        self.sink_s = 0.0;
        self.last_seconds = None;
    }

    /// One tick: slew toward the frame's targets by suite-time dt, evaluate
    /// the wobble LFO at `now_seconds`, return the additive offsets.
    /// Deterministic — same (err, lean_mag, seconds) sequence in, same
    /// offsets out.
    pub fn evaluate(&mut self, err: f64, lean_mag: f64, now_seconds: f64) -> GradientOffsets {
        let dt = match self.last_seconds {
            Some(t) => (now_seconds - t).max(0.0),
            None => 0.0,
        };
        self.last_seconds = Some(now_seconds);

        let err_target = (err / self.cfg.tune.err_full).clamp(0.0, 1.0);
        let sink_target = if lean_mag < self.cfg.sink_threshold {
            1.0
        } else {
            0.0
        };
        self.err_s = slew(self.err_s, err_target, dt, self.cfg.attack_ms, self.cfg.release_ms);
        self.sink_s = slew(self.sink_s, sink_target, dt, self.cfg.attack_ms, self.cfg.release_ms);

        let mut out = GradientOffsets::default();
        if !self.cfg.is_active() {
            return out;
        }
        let t = &self.cfg.tune;
        let depth = t.max_depth_semitones * self.err_s;
        if depth > 0.0 && t.rate_hz > 0.0 {
            let wobble = depth * (std::f64::consts::TAU * t.rate_hz * now_seconds).sin();
            out.detune.insert(t.bus.clone(), wobble);
        }
        if t.gain_trim_db != 0.0 && self.err_s > 0.0 {
            out.gain_db
                .insert(t.bus.clone(), t.gain_trim_db * self.err_s as f32);
        }
        if self.sink_s > 0.0 {
            for (bus, db) in &self.cfg.sink_db {
                *out.gain_db.entry(bus.clone()).or_insert(0.0) += db * self.sink_s as f32;
            }
        }
        out
    }
}

/// Asymmetric exponential slew: attack toward degraded (rising), release
/// toward clean (falling). Sub-millisecond taus snap.
fn slew(current: f64, target: f64, dt: f64, attack_ms: f64, release_ms: f64) -> f64 {
    let tau_ms = if target > current { attack_ms } else { release_ms };
    if tau_ms < 1.0 {
        return target;
    }
    let a = 1.0 - (-dt * 1000.0 / tau_ms).exp();
    current + (target - current) * a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active_cfg() -> GradientConfig {
        GradientConfig::parse(
            r#"
schema_version = 0
[tune]
bus = "world_voice"
max_depth_semitones = 0.25
rate_hz = 0.8
err_full = 0.9
gain_trim_db = -3.0
[sink]
threshold = 0.15
db = { texture = -6.0, harmony = -2.5 }
[smoothing]
attack_ms = 120.0
release_ms = 450.0
"#,
        )
        .unwrap()
    }

    #[test]
    fn builtin_default_is_inert() {
        let cfg = GradientConfig::default();
        assert!(!cfg.is_active());
        let mut d = GradientDriver::new(cfg);
        // Way off-lean, dead stick, time passing: still nothing.
        for i in 0..200 {
            let out = d.evaluate(2.0, 0.0, i as f64 * 0.01);
            assert_eq!(out, GradientOffsets::default());
        }
    }

    #[test]
    fn parse_roundtrip_and_validation() {
        let cfg = active_cfg();
        assert!(cfg.is_active());
        assert_eq!(cfg.tune.bus, "world_voice");
        assert_eq!(cfg.sink_db["texture"], -6.0);
        assert!(cfg.to_json()["tune"]["max_depth_semitones"].as_f64().unwrap() > 0.2);

        // Protected tune bus.
        assert!(GradientConfig::parse("[tune]\nbus = \"home_theme\"\n").is_err());
        assert!(GradientConfig::parse("[tune]\nbus = \"foundation\"\n").is_err());
        // Protected sink bus.
        assert!(GradientConfig::parse("[sink]\ndb = { child_motif = -3.0 }\n").is_err());
        // Unknown schema version.
        assert!(GradientConfig::parse("schema_version = 9\n").is_err());
        // Empty config falls back to the inert defaults and validates.
        assert!(GradientConfig::parse("").is_ok());
        assert!(!GradientConfig::parse("").unwrap().is_active());
        // Out-of-range depth.
        assert!(GradientConfig::parse("[tune]\nmax_depth_semitones = 3.0\n").is_err());
    }

    #[test]
    fn error_raises_wobble_and_on_lean_settles() {
        let mut d = GradientDriver::new(active_cfg());
        // Sustained full error: depth converges toward max, wobble swings
        // both ways, the tune bus gets its gain dulling.
        let mut max_det: f64 = 0.0;
        let mut min_det: f64 = 0.0;
        let mut out = GradientOffsets::default();
        for i in 0..2000 {
            out = d.evaluate(2.0, 1.0, i as f64 * 0.002);
            if let Some(det) = out.detune.get("world_voice") {
                max_det = max_det.max(*det);
                min_det = min_det.min(*det);
            }
        }
        assert!(max_det > 0.2 && min_det < -0.2, "LFO must swing both ways near full depth");
        assert!(out.gain_db["world_voice"] < -2.5, "tune bus dulled at full error");
        // Then the lean lands: depth releases toward zero.
        let mut last = f64::MAX;
        for i in 0..3000 {
            let out = d.evaluate(0.0, 1.0, 4.0 + i as f64 * 0.002);
            let mag = out.detune.get("world_voice").map(|d| d.abs()).unwrap_or(0.0);
            last = mag;
        }
        assert!(last < 0.01, "wobble settles as the lean lands: {last}");
    }

    #[test]
    fn neutral_sink_thins_and_reengaging_restores() {
        let mut d = GradientDriver::new(active_cfg());
        let mut out = GradientOffsets::default();
        for i in 0..2000 {
            out = d.evaluate(0.0, 0.0, i as f64 * 0.002); // dead stick
        }
        assert!(out.gain_db["texture"] < -5.5, "texture thins at neutral");
        assert!(out.gain_db["harmony"] < -2.0, "harmony thins at neutral");
        for i in 0..3000 {
            out = d.evaluate(0.0, 0.8, 4.0 + i as f64 * 0.002); // re-engage
        }
        let residual = out.gain_db.get("texture").copied().unwrap_or(0.0);
        assert!(residual > -0.1, "sink releases on re-engage: {residual}");
    }

    #[test]
    fn slew_is_deterministic_and_reset_zeroes() {
        let run = || {
            let mut d = GradientDriver::new(active_cfg());
            let mut trace = Vec::new();
            for i in 0..500 {
                let out = d.evaluate(1.0, 0.05, i as f64 * 0.002);
                trace.push((
                    out.detune.get("world_voice").copied().unwrap_or(0.0).to_bits(),
                    out.gain_db.get("texture").copied().unwrap_or(0.0).to_bits(),
                ));
            }
            trace
        };
        assert_eq!(run(), run(), "bit-identical evaluation");
        let mut d = GradientDriver::new(active_cfg());
        for i in 0..500 {
            d.evaluate(2.0, 0.0, i as f64 * 0.002);
        }
        d.reset();
        // First post-reset tick has dt 0 → offsets start from silence again.
        let out = d.evaluate(2.0, 0.0, 10.0);
        assert!(out.detune.get("world_voice").map(|d| d.abs()).unwrap_or(0.0) < 1e-9);
    }
}
