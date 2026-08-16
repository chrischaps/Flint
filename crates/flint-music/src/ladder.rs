//! The disintegration ladder: ordered rungs with coherence thresholds and
//! per-rung parameters, mirrored across audio and visuals (ADR 0015).
//!
//! The ladder is data, not code: `config/ladder.toml` in the game repo,
//! hot-reloadable mid-session like the coherence config. Each tick the
//! harness feeds the coherence scalar to [`Ladder::observe`]; the resolved
//! [`LadderParams`] for the current rung is the *single source of truth* —
//! the audio driver applies its audio half as mixer tweens and the visual
//! frame carries its visual half, which is what keeps the world coming
//! apart (and reassembling) as one thing.
//!
//! Rung parameters are absolute, not cumulative: each rung states the full
//! degraded state, so a deeper rung's numbers include the shallower rungs'
//! degradation. Hysteresis lives in the thresholds (`enter_below` <
//! `exit_above`), so a noisy coherence value at a boundary never flickers
//! the world.
//!
//! Protected buses: the ladder never touches `foundation` (the world's
//! anchor — "last to go" means it never goes in the prototype) or the motif
//! buses (`home_theme`/`child_motif` — the narrative lines reintegration
//! leads with) via gain or dropout. The low-pass applies to every non-motif
//! bus, foundation included — filtering the whole world is the woozy intent;
//! silencing its pulse is not.

use crate::mixer::{BusMixer, LPF_OPEN_HZ};
use crate::MOTIF_BUSES;
use flint_core::toml_util::toml_f64;
use flint_core::{FlintError, Result};
use kira::Tween;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

/// Gain trim used for a dropped bus (effectively silence).
pub const DROP_DB: f32 = -80.0;

/// Buses the ladder may thin or drop (everything else is protected).
pub const DEGRADABLE_BUSES: [&str; 3] = ["texture", "world_voice", "harmony"];

#[derive(Debug, Clone, PartialEq)]
pub struct RungAudio {
    /// Low-pass cutoff applied to every non-motif bus.
    pub lpf_hz: f64,
    /// Per-bus gain trims in dB (degradable buses only).
    pub thin_db: BTreeMap<String, f32>,
    /// Buses silenced outright at this rung (degradable buses only).
    pub drop: Vec<String>,
    /// Detune warble LFO depth (semitones, peak) on non-motif buses.
    pub warble_depth_semitones: f64,
    /// Warble LFO rate in Hz.
    pub warble_rate_hz: f64,
}

impl Default for RungAudio {
    fn default() -> Self {
        Self {
            lpf_hz: LPF_OPEN_HZ,
            thin_db: BTreeMap::new(),
            drop: Vec::new(),
            warble_depth_semitones: 0.0,
            warble_rate_hz: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RungVisual {
    /// 0 = full color, 1 = fully grey.
    pub desaturate: f32,
    /// 0 = sharp, 1 = maximally defocused.
    pub blur: f32,
    /// 0 = none, 1 = maximal chromatic offset.
    pub chromatic: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rung {
    pub name: String,
    /// Coherence below this enters the rung...
    pub enter_below: f64,
    /// ...and must rise above this to leave it (hysteresis: > enter_below).
    pub exit_above: f64,
    /// Tween length for this rung's audio moves.
    pub ramp_ms: f64,
    pub audio: RungAudio,
    pub visual: RungVisual,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FullFail {
    /// Coherence below this starts (or continues) the full-fail hold. Must
    /// sit below the deepest rung's `enter_below`; per ADR 0010 only
    /// sustained pulse misses reach values down there. Under sustained
    /// neglect the value saw-tooths (miss impulses down, the tracking
    /// integrator back up), so the hold has its own hysteresis:
    pub enter_below: f64,
    /// The hold cancels only when coherence rises above this (> enter_below)
    /// — a sawtooth peak poking over `enter_below` does not save the world.
    pub exit_above: f64,
    /// Debounce: how long the hold must run before reintegration fires.
    pub hold_ms: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LadderConfig {
    pub rungs: Vec<Rung>,
    pub full_fail: FullFail,
    /// Authored seam envelope (ms): the fade that carries the world out of
    /// the old timeline at reintegration. An envelope, not a cut.
    pub seam_fade_ms: f64,
    /// Rewind gesture length in beats (at the tempo where the fail happens):
    /// the whole world spins down like a record played backwards (a
    /// playback-rate ramp on every stem, mirrored visually) for exactly this
    /// long, ending at the seam — which still lands on a bar line, so the
    /// gesture starts on a beat. 0 = no spin-down.
    pub seam_rewind_beats: f64,
    /// Where the spin-down lands, in semitones (negative; −30 ≈ 18% speed —
    /// audibly "stopped" by the time the seam fade takes the tail).
    pub seam_rewind_drop_st: f64,
    /// Pickup spin-up: in the last beats before the seam, the lead bus plays
    /// the re-entry material as a cue winding up from the spin-down's rate
    /// to full speed, arriving exactly on the re-entry downbeat — a musical
    /// pickup that telegraphs when to come back in. Beats are measured at
    /// the re-entry tempo. 0 disables.
    pub seam_pickup_beats: f64,
    /// The ladder arms when coherence first reaches this (sessions open at a
    /// neutral value inside rung territory; the world can only come apart
    /// after it has first cohered). Measured on real engaged play: coherence
    /// reaches 0.80 within a learning session's first half-minute.
    pub arm_above: f64,
}

impl Default for LadderConfig {
    /// The prototype's three rungs (TDD): haze (low-pass + thin /
    /// desaturate), warble (detune + thin / blur), dropout (texture gone /
    /// chromatic). Values are starting points; `config/ladder.toml` is the
    /// tuning surface.
    fn default() -> Self {
        let rung = |name: &str,
                    enter: f64,
                    exit: f64,
                    audio: RungAudio,
                    visual: RungVisual| Rung {
            name: name.into(),
            enter_below: enter,
            exit_above: exit,
            ramp_ms: 350.0,
            audio,
            visual,
        };
        Self {
            rungs: vec![
                rung(
                    "haze",
                    0.82,
                    0.88,
                    RungAudio {
                        lpf_hz: 2_400.0,
                        thin_db: BTreeMap::from([("texture".into(), -6.0)]),
                        ..Default::default()
                    },
                    RungVisual {
                        desaturate: 0.35,
                        blur: 0.0,
                        chromatic: 0.0,
                    },
                ),
                rung(
                    "warble",
                    0.72,
                    0.79,
                    RungAudio {
                        lpf_hz: 1_400.0,
                        thin_db: BTreeMap::from([
                            ("texture".into(), -12.0),
                            ("world_voice".into(), -4.0),
                        ]),
                        warble_depth_semitones: 0.30,
                        warble_rate_hz: 0.8,
                        ..Default::default()
                    },
                    RungVisual {
                        desaturate: 0.60,
                        blur: 0.5,
                        chromatic: 0.15,
                    },
                ),
                rung(
                    "dropout",
                    0.68,
                    0.74,
                    RungAudio {
                        lpf_hz: 900.0,
                        thin_db: BTreeMap::from([
                            ("world_voice".into(), -9.0),
                            ("harmony".into(), -7.0),
                        ]),
                        drop: vec!["texture".into()],
                        warble_depth_semitones: 0.45,
                        warble_rate_hz: 0.7,
                    },
                    RungVisual {
                        desaturate: 0.85,
                        blur: 0.8,
                        chromatic: 0.35,
                    },
                ),
            ],
            // Thresholds live where the signal lives. Measured on the
            // Milestone-2-pinned coherence config over prototype-density
            // charts: perfect play ≈ 1.0; sustained neglect equilibrates
            // near 0.65 with miss-sawtooth dips to ≈ 0.60 (the gentle lean
            // curves keep tracking error, and thus the fit floor, mild).
            // The whole band therefore sits high; the TOML is the tuning
            // surface if the chart's difficulty changes the anatomy.
            // exit_above sits high on purpose: between misses the tracking
            // integrator can climb well past 0.70 (especially in slower
            // meters), and those sawtooth peaks must not cancel the hold —
            // once the thread frays to enter_below, only a firm climb back
            // (real re-engagement) saves the world.
            full_fail: FullFail {
                enter_below: 0.66,
                exit_above: 0.80,
                hold_ms: 1_500.0,
            },
            seam_fade_ms: 30.0,
            seam_rewind_beats: 2.0,
            seam_rewind_drop_st: -30.0,
            seam_pickup_beats: 2.0,
            arm_above: 0.80,
        }
    }
}

impl LadderConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self> {
        let root: toml::Value = text
            .parse()
            .map_err(|e| FlintError::TomlParseError(format!("ladder config: {e}")))?;
        let version = root
            .get("schema_version")
            .and_then(|v| v.as_integer())
            .unwrap_or(0);
        if version != 0 {
            return Err(FlintError::ValidationError(format!(
                "ladder config: unknown schema_version {version}"
            )));
        }
        let d = Self::default().full_fail;
        let ff = root.get("full_fail");
        let ff_f = |key: &str, default: f64| {
            ff.and_then(|t| t.get(key)).and_then(toml_f64).unwrap_or(default)
        };
        let enter_below = ff_f("enter_below", d.enter_below);
        let full_fail = FullFail {
            enter_below,
            exit_above: ff_f("exit_above", (enter_below + 0.10).min(1.0)),
            hold_ms: ff_f("hold_ms", d.hold_ms),
        };

        let mut rungs = Vec::new();
        if let Some(arr) = root.get("rungs").and_then(|v| v.as_array()) {
            for (i, r) in arr.iter().enumerate() {
                let field = |key: &str| r.get(key).and_then(toml_f64);
                let req = |key: &str| {
                    field(key).ok_or_else(|| {
                        FlintError::ValidationError(format!(
                            "ladder config: rung {i} missing `{key}`"
                        ))
                    })
                };
                let audio_t = r.get("audio");
                let af = |key: &str, default: f64| {
                    audio_t
                        .and_then(|t| t.get(key))
                        .and_then(toml_f64)
                        .unwrap_or(default)
                };
                let mut thin_db = BTreeMap::new();
                if let Some(tbl) = audio_t
                    .and_then(|t| t.get("thin_db"))
                    .and_then(|v| v.as_table())
                {
                    for (bus, v) in tbl {
                        let db = toml_f64(v).ok_or_else(|| {
                            FlintError::ValidationError(format!(
                                "ladder config: rung {i} thin_db.{bus} not a number"
                            ))
                        })?;
                        thin_db.insert(bus.clone(), db as f32);
                    }
                }
                let drop: Vec<String> = audio_t
                    .and_then(|t| t.get("drop"))
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let visual_t = r.get("visual");
                let vf = |key: &str| {
                    visual_t
                        .and_then(|t| t.get(key))
                        .and_then(toml_f64)
                        .unwrap_or(0.0) as f32
                };
                rungs.push(Rung {
                    name: r
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .unwrap_or_else(|| format!("rung{}", i + 1)),
                    enter_below: req("enter_below")?,
                    exit_above: req("exit_above")?,
                    ramp_ms: field("ramp_ms").unwrap_or(350.0),
                    audio: RungAudio {
                        lpf_hz: af("lpf_hz", LPF_OPEN_HZ),
                        thin_db,
                        drop,
                        warble_depth_semitones: af("warble_depth_semitones", 0.0),
                        warble_rate_hz: af("warble_rate_hz", 0.0),
                    },
                    visual: RungVisual {
                        desaturate: vf("desaturate"),
                        blur: vf("blur"),
                        chromatic: vf("chromatic"),
                    },
                });
            }
        }
        if rungs.is_empty() {
            rungs = Self::default().rungs;
        }
        let d_all = Self::default();
        let seam_t = root.get("seam");
        let seam_fade_ms = seam_t
            .and_then(|t| t.get("fade_ms"))
            .and_then(toml_f64)
            .unwrap_or(d_all.seam_fade_ms);
        let seam_rewind_beats = seam_t
            .and_then(|t| t.get("rewind_beats"))
            .and_then(toml_f64)
            .unwrap_or(d_all.seam_rewind_beats);
        let seam_rewind_drop_st = seam_t
            .and_then(|t| t.get("rewind_drop_semitones"))
            .and_then(toml_f64)
            .unwrap_or(d_all.seam_rewind_drop_st);
        let seam_pickup_beats = seam_t
            .and_then(|t| t.get("pickup_beats"))
            .and_then(toml_f64)
            .unwrap_or(d_all.seam_pickup_beats);
        let arm_above = root
            .get("arm_above")
            .and_then(toml_f64)
            .unwrap_or_else(|| Self::default().arm_above);
        let cfg = Self {
            rungs,
            full_fail,
            seam_fade_ms,
            seam_rewind_beats,
            seam_rewind_drop_st,
            seam_pickup_beats,
            arm_above,
        };
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        let err = |msg: String| Err(FlintError::ValidationError(format!("ladder config: {msg}")));
        let mut prev_enter = f64::INFINITY;
        for (i, r) in self.rungs.iter().enumerate() {
            if !(0.0..=1.0).contains(&r.enter_below) || !(0.0..=1.0).contains(&r.exit_above) {
                return err(format!("rung {i} thresholds must be in 0..1"));
            }
            if r.exit_above <= r.enter_below {
                return err(format!(
                    "rung {i} needs hysteresis: exit_above {} <= enter_below {}",
                    r.exit_above, r.enter_below
                ));
            }
            if r.enter_below >= prev_enter {
                return err(format!("rung {i} enter_below must strictly decrease"));
            }
            prev_enter = r.enter_below;
            if !r.ramp_ms.is_finite() || r.ramp_ms < 0.0 {
                return err(format!("rung {i} ramp_ms {} out of range", r.ramp_ms));
            }
            if !(1.0..=LPF_OPEN_HZ).contains(&r.audio.lpf_hz) {
                return err(format!("rung {i} lpf_hz {} out of range", r.audio.lpf_hz));
            }
            for bus in r.audio.thin_db.keys().chain(r.audio.drop.iter()) {
                if !DEGRADABLE_BUSES.contains(&bus.as_str()) {
                    return err(format!(
                        "rung {i} touches protected bus '{bus}' (degradable: {DEGRADABLE_BUSES:?})"
                    ));
                }
            }
            if r.audio.warble_depth_semitones < 0.0 || r.audio.warble_depth_semitones > 1.0 {
                return err(format!("rung {i} warble depth out of range (0..1 semitones)"));
            }
            if r.audio.warble_rate_hz < 0.0 || r.audio.warble_rate_hz > 8.0 {
                return err(format!("rung {i} warble rate out of range (0..8 Hz)"));
            }
        }
        let deepest = self.rungs.last().map(|r| r.enter_below).unwrap_or(0.0);
        if self.full_fail.enter_below >= deepest {
            return err(format!(
                "full_fail.enter_below {} must sit below the deepest rung's enter_below {deepest}",
                self.full_fail.enter_below
            ));
        }
        if self.full_fail.exit_above <= self.full_fail.enter_below {
            return err(format!(
                "full_fail needs hysteresis: exit_above {} <= enter_below {}",
                self.full_fail.exit_above, self.full_fail.enter_below
            ));
        }
        if !self.full_fail.hold_ms.is_finite() || self.full_fail.hold_ms < 0.0 {
            return err(format!("full_fail.hold_ms {}", self.full_fail.hold_ms));
        }
        if !(5.0..=200.0).contains(&self.seam_fade_ms) {
            return err(format!(
                "seam.fade_ms {} out of range (5..200)",
                self.seam_fade_ms
            ));
        }
        if !(0.0..=16.0).contains(&self.seam_rewind_beats) {
            return err(format!(
                "seam.rewind_beats {} out of range (0..16)",
                self.seam_rewind_beats
            ));
        }
        if !(-60.0..=0.0).contains(&self.seam_rewind_drop_st) {
            return err(format!(
                "seam.rewind_drop_semitones {} out of range (-60..0)",
                self.seam_rewind_drop_st
            ));
        }
        if !(0.0..=8.0).contains(&self.seam_pickup_beats) {
            return err(format!(
                "seam.pickup_beats {} out of range (0..8)",
                self.seam_pickup_beats
            ));
        }
        if !(0.0..=1.0).contains(&self.arm_above)
            || self.arm_above <= self.full_fail.enter_below
        {
            return err(format!(
                "arm_above {} must be in 0..1 and above full_fail.enter_below {}",
                self.arm_above, self.full_fail.enter_below
            ));
        }
        Ok(())
    }

    /// Snapshot for log headers — session reproducibility depends on knowing
    /// exactly which ladder was live.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "full_fail": {
                "enter_below": self.full_fail.enter_below,
                "exit_above": self.full_fail.exit_above,
                "hold_ms": self.full_fail.hold_ms,
            },
            "seam": {
                "fade_ms": self.seam_fade_ms,
                "rewind_beats": self.seam_rewind_beats,
                "rewind_drop_semitones": self.seam_rewind_drop_st,
                "pickup_beats": self.seam_pickup_beats,
            },
            "arm_above": self.arm_above,
            "rungs": self.rungs.iter().map(|r| serde_json::json!({
                "name": r.name,
                "enter_below": r.enter_below,
                "exit_above": r.exit_above,
                "ramp_ms": r.ramp_ms,
                "audio": {
                    "lpf_hz": r.audio.lpf_hz,
                    "thin_db": r.audio.thin_db,
                    "drop": r.audio.drop,
                    "warble_depth_semitones": r.audio.warble_depth_semitones,
                    "warble_rate_hz": r.audio.warble_rate_hz,
                },
                "visual": {
                    "desaturate": r.visual.desaturate,
                    "blur": r.visual.blur,
                    "chromatic": r.visual.chromatic,
                },
            })).collect::<Vec<_>>(),
        })
    }
}

/// The resolved parameter set for the current ladder level — the one struct
/// both the audio driver and the visual frame read.
#[derive(Debug, Clone, PartialEq)]
pub struct LadderParams {
    /// 0 = clean; 1..=rungs.len() = that rung (1-based).
    pub level: usize,
    pub lpf_hz: f64,
    /// Per-bus gain trim in dB; dropped buses appear as [`DROP_DB`].
    pub gain_trim_db: BTreeMap<String, f32>,
    pub warble_depth_semitones: f64,
    pub warble_rate_hz: f64,
    pub ramp_ms: f64,
    pub visual: RungVisual,
}

impl LadderParams {
    pub fn clean() -> Self {
        Self {
            level: 0,
            lpf_hz: LPF_OPEN_HZ,
            gain_trim_db: BTreeMap::new(),
            warble_depth_semitones: 0.0,
            warble_rate_hz: 0.0,
            ramp_ms: 350.0,
            visual: RungVisual::default(),
        }
    }

    fn from_rung(level: usize, r: &Rung) -> Self {
        let mut gain_trim_db = r.audio.thin_db.clone();
        for bus in &r.audio.drop {
            gain_trim_db.insert(bus.clone(), DROP_DB);
        }
        Self {
            level,
            lpf_hz: r.audio.lpf_hz,
            gain_trim_db,
            warble_depth_semitones: r.audio.warble_depth_semitones,
            warble_rate_hz: r.audio.warble_rate_hz,
            ramp_ms: r.ramp_ms,
            visual: r.visual,
        }
    }

    /// Interpolate between two param sets (reassembly runs the ladder in
    /// reverse by lerping from the full-fail state back to the target).
    /// `t` = 0 → `self`, 1 → `other`. LPF interpolates in log-Hz (perceptual);
    /// gains and visuals linearly; warble rate holds while depth fades.
    pub fn lerp(&self, other: &Self, t: f64) -> Self {
        let t = t.clamp(0.0, 1.0);
        let tf = t as f32;
        let lerp_f = |a: f64, b: f64| a + (b - a) * t;
        let mut gain_trim_db = BTreeMap::new();
        for bus in self.gain_trim_db.keys().chain(other.gain_trim_db.keys()) {
            let a = *self.gain_trim_db.get(bus).unwrap_or(&0.0);
            let b = *other.gain_trim_db.get(bus).unwrap_or(&0.0);
            gain_trim_db.insert(bus.clone(), a + (b - a) * tf);
        }
        Self {
            level: if t < 1.0 { self.level } else { other.level },
            lpf_hz: (lerp_f(self.lpf_hz.ln(), other.lpf_hz.ln())).exp(),
            gain_trim_db,
            warble_depth_semitones: lerp_f(
                self.warble_depth_semitones,
                other.warble_depth_semitones,
            ),
            warble_rate_hz: if t < 0.5 {
                self.warble_rate_hz
            } else {
                other.warble_rate_hz
            },
            ramp_ms: lerp_f(self.ramp_ms, other.ramp_ms),
            visual: RungVisual {
                desaturate: self.visual.desaturate + (other.visual.desaturate - self.visual.desaturate) * tf,
                blur: self.visual.blur + (other.visual.blur - self.visual.blur) * tf,
                chromatic: self.visual.chromatic + (other.visual.chromatic - self.visual.chromatic) * tf,
            },
        }
    }
}

/// The rung state machine: feed it coherence every tick, read the resolved
/// params. Hysteresis means the level only moves when coherence crosses
/// `enter_below` going down or `exit_above` going up.
pub struct Ladder {
    cfg: LadderConfig,
    level: usize,
    armed: bool,
}

impl Ladder {
    pub fn new(cfg: LadderConfig) -> Self {
        Self {
            cfg,
            level: 0,
            armed: false,
        }
    }

    pub fn config(&self) -> &LadderConfig {
        &self.cfg
    }

    /// Swap the config mid-session; the current level carries over (clamped),
    /// so tuning never resets the world's state.
    pub fn reconfigure(&mut self, cfg: LadderConfig) {
        self.level = self.level.min(cfg.rungs.len());
        self.cfg = cfg;
    }

    /// 0 = clean, 1..=n = rung index (1-based).
    pub fn level(&self) -> usize {
        self.level
    }

    pub fn rung(&self) -> Option<&Rung> {
        (self.level > 0).then(|| &self.cfg.rungs[self.level - 1])
    }

    /// Whether the ladder has armed: sessions open at a neutral coherence
    /// (0.5 by default, inside rung territory), so the world would boot
    /// mid-disintegration without this. The ladder stays clean until
    /// coherence first clears the top rung's `exit_above` — the world can
    /// only come apart after it has first cohered.
    pub fn armed(&self) -> bool {
        self.armed
    }

    /// Update the level from the current coherence value. Returns the level.
    pub fn observe(&mut self, coherence: f64) -> usize {
        if !self.armed {
            if coherence >= self.cfg.arm_above {
                self.armed = true;
            } else {
                return 0;
            }
        }
        while self.level < self.cfg.rungs.len()
            && coherence < self.cfg.rungs[self.level].enter_below
        {
            self.level += 1;
        }
        while self.level > 0 && coherence > self.cfg.rungs[self.level - 1].exit_above {
            self.level -= 1;
        }
        self.level
    }

    /// Resolved params for the current level.
    pub fn params(&self) -> LadderParams {
        match self.rung() {
            None => LadderParams::clean(),
            Some(r) => LadderParams::from_rung(self.level, r),
        }
    }

    /// Params at the deepest rung — the state reassembly lerps back from.
    pub fn full_fail_params(&self) -> LadderParams {
        match self.cfg.rungs.last() {
            None => LadderParams::clean(),
            Some(r) => LadderParams::from_rung(self.cfg.rungs.len(), r),
        }
    }
}

/// Applies [`LadderParams`] to the mixer as tweens, idempotently: a command
/// is issued only when a bus's target actually changes, so calling `apply`
/// every tick is free while the world holds still. The warble is a slow
/// symmetric LFO evaluated from suite time (deterministic — same time in,
/// same detune out) and smoothed by short tweens.
///
/// The driver owns *track* gain (ladder trims). Reintegration entry
/// envelopes live on *per-sound* volume (`StemBus::set_sound_volume`), so
/// the two never fight.
pub struct LadderDriver {
    last_lpf: BTreeMap<String, f64>,
    last_gain: BTreeMap<String, f32>,
    last_detune: BTreeMap<String, f64>,
}

/// Smoothing tween for per-tick warble updates (the LFO is the shape; this
/// just bridges between tick evaluations without zipper noise).
const WARBLE_SMOOTH_MS: f64 = 50.0;
/// Gain/detune changes smaller than this don't re-issue commands.
const GAIN_EPS: f32 = 0.01;
const DETUNE_EPS: f64 = 0.001;

impl Default for LadderDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl LadderDriver {
    pub fn new() -> Self {
        Self {
            last_lpf: BTreeMap::new(),
            last_gain: BTreeMap::new(),
            last_detune: BTreeMap::new(),
        }
    }

    /// Forget everything previously issued, so the next `apply` restates the
    /// full target state. Call after a seam: re-played stems reset detune,
    /// and entry gains change out from under the shadow bookkeeping.
    pub fn reset(&mut self) {
        self.last_lpf.clear();
        self.last_gain.clear();
        self.last_detune.clear();
    }

    /// Drive the mixer toward `params`. `now_seconds` is suite time (for the
    /// warble LFO phase); `ramp_ms` is the tween length for gain/LPF moves —
    /// normally `params.ramp_ms`, but reassembly passes tick-scale ramps
    /// while it lerps params itself.
    pub fn apply(
        &mut self,
        params: &LadderParams,
        now_seconds: f64,
        ramp_ms: f64,
        mixer: &mut BusMixer,
    ) {
        let tween = |ms: f64| Tween {
            duration: Duration::from_secs_f64(ms.max(0.0) / 1000.0),
            ..Default::default()
        };
        let warble = if params.warble_depth_semitones > 0.0 && params.warble_rate_hz > 0.0 {
            params.warble_depth_semitones
                * (std::f64::consts::TAU * params.warble_rate_hz * now_seconds).sin()
        } else {
            0.0
        };
        for name in crate::BUSES {
            let is_motif = MOTIF_BUSES.contains(&name);
            let is_degradable = DEGRADABLE_BUSES.contains(&name);
            let Some(bus) = mixer.bus_mut(name) else {
                continue;
            };

            // LPF: every non-motif bus follows the rung cutoff.
            if !is_motif {
                let target = params.lpf_hz;
                if self.last_lpf.get(name) != Some(&target) {
                    bus.set_lpf(target, tween(ramp_ms));
                    self.last_lpf.insert(name.into(), target);
                }
            }

            // Track gain = ladder trim (0 on protected buses by
            // construction — the validator enforces it).
            let target = params.gain_trim_db.get(name).copied().unwrap_or(0.0);
            if (self.last_gain.get(name).copied().unwrap_or(0.0) - target).abs() > GAIN_EPS
                || !self.last_gain.contains_key(name)
            {
                bus.set_gain(target, tween(ramp_ms));
                self.last_gain.insert(name.into(), target);
            }

            // Warble: degradable buses only. Detuning the foundation would
            // un-anchor the pulse, and motif lines stay in tune — the world
            // warbles around them. (Nonzero detune breaks sample-lock while
            // active — mixer docs; the seam re-play re-locks.)
            if is_degradable && bus.state.playing {
                let target = if warble != 0.0 { warble } else { 0.0 };
                let last = self.last_detune.get(name).copied().unwrap_or(0.0);
                if (last - target).abs() > DETUNE_EPS {
                    bus.set_detune(target, tween(WARBLE_SMOOTH_MS.min(ramp_ms.max(1.0))));
                    self.last_detune.insert(name.into(), target);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_validates() {
        LadderConfig::default().validate().expect("default valid");
    }

    #[test]
    fn unarmed_ladder_stays_clean_at_session_start() {
        let mut l = Ladder::new(LadderConfig::default());
        // Sessions open at neutral coherence — no boot-into-broken-world.
        assert_eq!(l.observe(0.5), 0);
        assert!(!l.armed());
        assert_eq!(l.observe(0.2), 0, "still unarmed, still clean");
        assert_eq!(l.observe(0.79), 0, "just under arm_above stays unarmed");
        assert!(!l.armed());
        l.observe(0.81); // clears arm_above (0.80)
        assert!(l.armed());
        assert_eq!(l.observe(0.5), 3, "armed: now the ladder engages");
    }

    #[test]
    fn descend_and_ascend_with_hysteresis() {
        let mut l = Ladder::new(LadderConfig::default());
        assert_eq!(l.observe(0.95), 0);
        assert_eq!(l.observe(0.80), 1, "below rung 1 enter (0.82)");
        // Noise around the boundary must not flicker: above enter, below exit.
        assert_eq!(l.observe(0.85), 1, "inside hysteresis band stays");
        assert_eq!(l.observe(0.89), 0, "above exit (0.88) leaves");
        // A single big drop crosses several rungs at once.
        assert_eq!(l.observe(0.30), 3);
        // 0.75 clears dropout's exit (0.74) but not warble's (0.79).
        assert_eq!(l.observe(0.75), 2);
        assert_eq!(l.observe(0.95), 0, "full recovery walks all the way up");
    }

    #[test]
    fn params_resolve_drop_and_clean() {
        let mut l = Ladder::new(LadderConfig::default());
        assert_eq!(l.params(), LadderParams::clean());
        l.observe(0.95); // arm
        l.observe(0.10);
        let p = l.params();
        assert_eq!(p.level, 3);
        assert_eq!(p.gain_trim_db.get("texture"), Some(&DROP_DB));
        assert!(p.lpf_hz < 1_000.0);
        assert!(p.visual.desaturate > 0.8);
    }

    #[test]
    fn reconfigure_carries_level() {
        let mut l = Ladder::new(LadderConfig::default());
        l.observe(0.95); // arm
        l.observe(0.10);
        assert_eq!(l.level(), 3);
        let mut cfg = LadderConfig::default();
        cfg.rungs.truncate(2);
        cfg.full_fail.enter_below = 0.18; // still below rung 2's enter
        l.reconfigure(cfg);
        assert_eq!(l.level(), 2, "level clamps to the new rung count");
    }

    #[test]
    fn lerp_runs_the_ladder_in_reverse() {
        let l = Ladder::new(LadderConfig::default());
        let fail = l.full_fail_params();
        let clean = LadderParams::clean();
        let mid = fail.lerp(&clean, 0.5);
        assert!(mid.lpf_hz > fail.lpf_hz && mid.lpf_hz < clean.lpf_hz);
        assert!(mid.visual.desaturate < fail.visual.desaturate);
        let trim = mid.gain_trim_db.get("texture").copied().unwrap();
        assert!(trim > DROP_DB && trim < 0.0);
        assert_eq!(fail.lerp(&clean, 1.0).gain_trim_db.get("texture"), Some(&0.0));
    }

    #[test]
    fn parse_validation_rejects_bad_ladders() {
        // Protected bus.
        let bad = r#"
[[rungs]]
enter_below = 0.7
exit_above = 0.8
[rungs.audio]
drop = ["foundation"]
"#;
        assert!(LadderConfig::parse(bad).is_err());
        // No hysteresis.
        let bad = "[[rungs]]\nenter_below = 0.7\nexit_above = 0.7\n";
        assert!(LadderConfig::parse(bad).is_err());
        // Full-fail above deepest rung.
        let bad = r#"
[full_fail]
enter_below = 0.9
[[rungs]]
enter_below = 0.7
exit_above = 0.8
"#;
        assert!(LadderConfig::parse(bad).is_err());
        // Unknown schema version.
        assert!(LadderConfig::parse("schema_version = 9\n").is_err());
        // Empty config falls back to defaults and validates.
        assert!(LadderConfig::parse("").is_ok());
    }

    #[test]
    fn parse_roundtrip_of_a_real_config() {
        let cfg = LadderConfig::parse(
            r#"
schema_version = 0
[full_fail]
enter_below = 0.15
hold_ms = 2000.0

[[rungs]]
name = "haze"
enter_below = 0.75
exit_above = 0.82
ramp_ms = 400.0
[rungs.audio]
lpf_hz = 2000.0
thin_db = { texture = -5.0 }
[rungs.visual]
desaturate = 0.4

[[rungs]]
name = "gone"
enter_below = 0.4
exit_above = 0.5
[rungs.audio]
drop = ["texture"]
warble_depth_semitones = 0.3
warble_rate_hz = 1.0
"#,
        )
        .unwrap();
        assert_eq!(cfg.rungs.len(), 2);
        assert_eq!(cfg.full_fail.hold_ms, 2000.0);
        assert_eq!(cfg.rungs[0].audio.thin_db["texture"], -5.0);
        assert_eq!(cfg.rungs[1].audio.drop, vec!["texture".to_string()]);
        assert_eq!(cfg.rungs[0].visual.desaturate, 0.4);
        assert!(cfg.to_json()["rungs"].as_array().unwrap().len() == 2);
    }
}

