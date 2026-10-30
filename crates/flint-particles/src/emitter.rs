//! Resolved emitter configuration and per-emitter runtime state.
//!
//! [`EmitterConfig`] is the *runtime* form: every range is a min/max pair,
//! every curve is a sampled [`Curve`], every angle is in radians. It is
//! produced from the authored [`crate::effect::EmitterDef`] by `resolve()`
//! and never parsed directly from TOML any more (ADR 0068). The legacy
//! `EmitterConfig::from_toml` shim keeps the inline `particle_emitter`
//! component path working.

use crate::curves::Curve;
use crate::particle::ParticlePool;
use crate::rand::ParticleRng;
use serde::{Deserialize, Serialize};

/// Blend mode for particle rendering
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ParticleBlendMode {
    #[default]
    Alpha,
    Additive,
    /// Texture colour is already multiplied by alpha (`One, OneMinusSrcAlpha`).
    Premultiplied,
    /// Darkens what is behind it (`Dst, OneMinusSrcAlpha`).
    Multiply,
}

impl ParticleBlendMode {
    pub const ALL: [ParticleBlendMode; 4] = [
        ParticleBlendMode::Alpha,
        ParticleBlendMode::Additive,
        ParticleBlendMode::Premultiplied,
        ParticleBlendMode::Multiply,
    ];

    /// Stable index into per-blend pipeline arrays.
    pub fn index(self) -> usize {
        match self {
            ParticleBlendMode::Alpha => 0,
            ParticleBlendMode::Additive => 1,
            ParticleBlendMode::Premultiplied => 2,
            ParticleBlendMode::Multiply => 3,
        }
    }

    /// Additive blending is order-independent and is always drawn last.
    pub fn is_order_independent(self) -> bool {
        matches!(self, ParticleBlendMode::Additive)
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "alpha" => Some(Self::Alpha),
            "additive" => Some(Self::Additive),
            "premultiplied" => Some(Self::Premultiplied),
            "multiply" => Some(Self::Multiply),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ParticleBlendMode::Alpha => "alpha",
            ParticleBlendMode::Additive => "additive",
            ParticleBlendMode::Premultiplied => "premultiplied",
            ParticleBlendMode::Multiply => "multiply",
        }
    }
}

/// Per-emitter draw ordering of alive particles.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SortMode {
    /// Pool order (cheapest; fine for additive).
    #[default]
    None,
    /// Farthest from the camera first — correct alpha blending.
    BackToFront,
    /// Newest particles drawn first so older ones stack on top.
    YoungestFirst,
    /// Oldest particles drawn first so the newest sit on top.
    OldestFirst,
}

/// Emission shape (resolved; distances already scaled by the effect scale
/// at spawn time).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EmissionShape {
    Point,
    Sphere {
        radius: f32,
    },
    /// Spawns on a disc of `radius` perpendicular to `direction`, moving
    /// within `angle` degrees of it.
    Cone {
        radius: f32,
        angle: f32,
    },
    Box {
        extents: [f32; 3],
    },
}

/// A resolved force acting on every particle of an emitter each step.
/// Positions are relative to the emitter origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Force {
    /// Pull velocity toward `velocity` at rate `strength` per second.
    Wind { velocity: [f32; 3], strength: f32 },
    /// Quadratic drag: `v *= 1 / (1 + c·|v|·dt)`.
    Drag { coefficient: f32 },
    /// Turbulence from a deterministic noise field.
    Noise {
        strength: f32,
        frequency: f32,
        speed: f32,
        octaves: u32,
    },
    /// Swirl around `axis` through `center`.
    Vortex {
        center: [f32; 3],
        axis: [f32; 3],
        strength: f32,
        falloff: f32,
    },
    /// Accelerate toward (`strength > 0`) or away from a point.
    Attractor {
        position: [f32; 3],
        strength: f32,
        radius: f32,
    },
}

/// One entry of an emitter's burst timeline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Burst {
    pub time: f32,
    pub count_min: u32,
    pub count_max: u32,
    /// 0 = repeat forever (every `interval`).
    pub cycles: u32,
    pub interval: f32,
    pub probability: f32,
}

/// Spawn into a sibling emitter when a particle is born or dies.
#[derive(Debug, Clone, PartialEq)]
pub struct SubEmitter {
    pub emitter: String,
    pub count_min: u32,
    pub count_max: u32,
    pub inherit_velocity: f32,
}

/// Resolved runtime emitter configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct EmitterConfig {
    pub name: String,

    // Emission
    pub emission_rate: f32,
    pub emission_per_meter: f32,
    pub max_particles: usize,
    pub lifetime_min: f32,
    pub lifetime_max: f32,
    pub duration: f32,
    pub looping: bool,
    pub start_delay: f32,
    pub autoplay: bool,
    pub playing: bool,
    pub bursts: Vec<Burst>,

    // Shape
    pub shape: EmissionShape,
    pub shape_offset: [f32; 3],
    /// Optional orientation for the box shape: extents.x runs along `u`,
    /// extents.y along `v`, extents.z along `u × v`. Both zero keeps the
    /// axis-aligned box (ADR 0061).
    pub shape_axis_u: [f32; 3],
    pub shape_axis_v: [f32; 3],
    /// Rotate `direction`, `shape_offset` and shape axes by the emitter's
    /// world rotation (and scale distances by its scale). Inline components
    /// default to `false` for ADR 0061 compatibility; assets default to `true`.
    pub local_axes: bool,

    // Motion
    pub speed_min: f32,
    pub speed_max: f32,
    pub direction: [f32; 3],
    pub spread: f32,
    pub gravity: [f32; 3],
    /// Exponential velocity decay per second.
    pub damping: f32,
    pub inherit_velocity: f32,
    pub world_space: bool,
    pub forces: Vec<Force>,
    pub speed_curve: Option<Curve<f32>>,

    // Over lifetime
    pub size: Curve<[f32; 2]>,
    pub size_scale_min: f32,
    pub size_scale_max: f32,
    pub color: Curve<[f32; 4]>,
    pub alpha: Option<Curve<f32>>,
    pub brightness_min: f32,
    pub brightness_max: f32,
    /// Initial rotation range in radians.
    pub rotation_min: f32,
    pub rotation_max: f32,
    /// Spin range in radians per second.
    pub angular_velocity_min: f32,
    pub angular_velocity_max: f32,

    // Rendering
    pub texture: String,
    pub frames_x: u32,
    pub frames_y: u32,
    pub animate_frames: bool,
    /// Frames per second (0 = follow `animate_frames`).
    pub frame_rate: f32,
    pub random_start_frame: bool,
    pub blend_mode: ParticleBlendMode,
    pub sort: SortMode,
    /// Depth-fade distance for soft particles (0 = off).
    pub soft_distance: f32,
    pub fade_near: f32,
    pub fade_far: f32,
    pub lighting: f32,
    pub fog: bool,
    /// Velocity-aligned billboard stretch (s): quad half-length grows by
    /// |velocity| * stretch / 2 along the screen-projected velocity.
    pub stretch: f32,

    // Sub-emitters
    pub on_death: Option<SubEmitter>,
    pub on_birth: Option<SubEmitter>,
}

impl Default for EmitterConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            emission_rate: 10.0,
            emission_per_meter: 0.0,
            max_particles: 256,
            lifetime_min: 1.0,
            lifetime_max: 2.0,
            duration: 0.0,
            looping: true,
            start_delay: 0.0,
            autoplay: true,
            playing: false,
            bursts: Vec::new(),
            shape: EmissionShape::Point,
            shape_offset: [0.0; 3],
            shape_axis_u: [0.0; 3],
            shape_axis_v: [0.0; 3],
            local_axes: false,
            speed_min: 1.0,
            speed_max: 3.0,
            direction: [0.0, 1.0, 0.0],
            spread: 15.0,
            gravity: [0.0, -9.81, 0.0],
            damping: 0.0,
            inherit_velocity: 0.0,
            world_space: true,
            forces: Vec::new(),
            speed_curve: None,
            size: Curve::start_end([0.1, 0.1], [0.0, 0.0]),
            size_scale_min: 1.0,
            size_scale_max: 1.0,
            color: Curve::start_end([1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 0.0]),
            alpha: None,
            brightness_min: 1.0,
            brightness_max: 1.0,
            rotation_min: 0.0,
            rotation_max: std::f32::consts::TAU,
            angular_velocity_min: 0.0,
            angular_velocity_max: 0.0,
            texture: String::new(),
            frames_x: 1,
            frames_y: 1,
            animate_frames: false,
            frame_rate: 0.0,
            random_start_frame: false,
            blend_mode: ParticleBlendMode::Alpha,
            sort: SortMode::None,
            soft_distance: 0.0,
            fade_near: 0.0,
            fade_far: 0.0,
            lighting: 0.0,
            fog: false,
            stretch: 0.0,
            on_death: None,
            on_birth: None,
        }
    }
}

impl EmitterConfig {
    /// Parse an inline `particle_emitter` component table (lenient: unknown
    /// keys ignored, malformed values fall back to defaults).
    pub fn from_toml(table: &toml::value::Table) -> Self {
        crate::effect::EmitterDef::from_component(table)
            .and_then(|def| def.resolve(crate::effect::ResolveContext::inline()))
            .unwrap_or_else(|e| {
                tracing::warn!("particle_emitter component failed to parse: {e}");
                Self::default()
            })
    }

    /// Does this emitter start playing on creation?
    pub fn starts_playing(&self) -> bool {
        self.playing || self.autoplay
    }

    /// Longest possible particle life plus the emitter's own timeline —
    /// a sensible default preview length for editors.
    pub fn preview_length(&self) -> f32 {
        let last_burst = self
            .bursts
            .iter()
            .map(|b| {
                b.time
                    + if b.cycles == 0 {
                        0.0
                    } else {
                        b.interval * b.cycles as f32
                    }
            })
            .fold(0.0, f32::max);
        let base = if self.duration > 0.0 {
            self.duration
        } else {
            last_burst
        };
        base + self.start_delay + self.lifetime_max
    }
}

/// Runtime state for a burst timeline entry.
#[derive(Debug, Clone, Copy)]
pub struct BurstRuntime {
    pub next_time: f32,
    pub fired: u32,
}

/// Runtime state for one emitter
pub struct EmitterState {
    pub config: EmitterConfig,
    pub pool: ParticlePool,
    /// Fractional particle accumulator for rate emission
    pub accumulator: f32,
    /// Fractional accumulator for emission-over-distance
    pub distance_accum: f32,
    /// How long this emitter has been running
    pub emitter_time: f32,
    /// Burst particles queued from scripts
    pub pending_burst: u32,
    /// Whether the emitter is currently playing
    pub playing: bool,
    /// Burst timeline progress (parallel to `config.bursts`)
    pub bursts: Vec<BurstRuntime>,
    /// Per-emitter RNG (see [`ParticleRng`] docs)
    pub rng: ParticleRng,
    /// Sibling emitter indices resolved from `on_death` / `on_birth`
    pub on_death_target: Option<usize>,
    pub on_birth_target: Option<usize>,
}

impl EmitterState {
    pub fn new(config: EmitterConfig) -> Self {
        Self::with_seed(config, 0xDEAD_BEEF)
    }

    pub fn with_seed(config: EmitterConfig, seed: u32) -> Self {
        let playing = config.starts_playing();
        let pool = ParticlePool::new(config.max_particles);
        let bursts = config
            .bursts
            .iter()
            .map(|b| BurstRuntime {
                next_time: b.time,
                fired: 0,
            })
            .collect();
        Self {
            config,
            pool,
            accumulator: 0.0,
            distance_accum: 0.0,
            emitter_time: 0.0,
            pending_burst: 0,
            playing,
            bursts,
            rng: ParticleRng::new(seed),
            on_death_target: None,
            on_birth_target: None,
        }
    }

    /// Restart the emitter timeline (bursts included) without killing
    /// particles already in flight.
    pub fn restart(&mut self) {
        self.emitter_time = 0.0;
        self.accumulator = 0.0;
        self.distance_accum = 0.0;
        self.reset_bursts();
        self.playing = true;
    }

    pub fn reset_bursts(&mut self) {
        for (rt, b) in self.bursts.iter_mut().zip(self.config.bursts.iter()) {
            rt.next_time = b.time;
            rt.fired = 0;
        }
    }

    /// Replace the configuration, keeping the pool when its capacity is
    /// unchanged so live edits don't blink particles out of existence.
    pub fn replace_config(&mut self, config: EmitterConfig) {
        if config.max_particles != self.pool.capacity() {
            self.pool = ParticlePool::new(config.max_particles);
        }
        let bursts_changed = config.bursts != self.config.bursts;
        self.config = config;
        if bursts_changed {
            self.bursts = self
                .config
                .bursts
                .iter()
                .map(|b| BurstRuntime {
                    next_time: b.time,
                    fired: 0,
                })
                .collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_sane() {
        let config = EmitterConfig::default();
        assert!(config.emission_rate > 0.0);
        assert!(config.lifetime_max >= config.lifetime_min);
        assert!(config.max_particles > 0);
        assert!(config.starts_playing());
    }

    #[test]
    fn parse_from_toml() {
        let toml_str = r#"
emission_rate = 50.0
max_particles = 500
blend_mode = "additive"
gravity = [0, 0, 0]
color_start = [1.0, 0.5, 0.0, 1.0]
color_end = [1.0, 0.0, 0.0, 0.0]
shape = "cone"
shape_angle = 45.0
"#;
        let table: toml::value::Table = toml::from_str(toml_str).unwrap();
        let config = EmitterConfig::from_toml(&table);
        assert!((config.emission_rate - 50.0).abs() < 0.01);
        assert_eq!(config.max_particles, 500);
        assert_eq!(config.blend_mode, ParticleBlendMode::Additive);
        assert!((config.gravity[1]).abs() < 0.01);
        assert!((config.color.first()[1] - 0.5).abs() < 0.01);
        assert!((config.color.last()[0] - 1.0).abs() < 0.01);
        if let EmissionShape::Cone { angle, .. } = config.shape {
            assert!((angle - 45.0).abs() < 0.01);
        } else {
            panic!("Expected Cone shape");
        }
        // Inline components keep world-space axes (ADR 0061).
        assert!(!config.local_axes);
    }

    #[test]
    fn toml_integer_float_coercion() {
        // TOML `gravity = [0, -10, 0]` gives integers for 0, float for -10
        let toml_str = "gravity = [0, -10, 0]\nsize_start = 1\nlifetime_min = 2";
        let table: toml::value::Table = toml::from_str(toml_str).unwrap();
        let config = EmitterConfig::from_toml(&table);
        assert!((config.gravity[0]).abs() < 0.01);
        assert!((config.gravity[1] - (-10.0)).abs() < 0.01);
        assert_eq!(config.size.first(), [1.0, 1.0]);
        assert_eq!(config.lifetime_min, 2.0);
    }

    #[test]
    fn replace_config_keeps_pool_when_capacity_unchanged() {
        let mut state = EmitterState::new(EmitterConfig::default());
        state.pool.spawn().unwrap().lifetime = 5.0;
        let mut cfg = state.config.clone();
        cfg.emission_rate = 99.0;
        state.replace_config(cfg);
        assert_eq!(state.pool.alive_count(), 1);
        let mut cfg = state.config.clone();
        cfg.max_particles = 8;
        state.replace_config(cfg);
        assert_eq!(state.pool.alive_count(), 0);
        assert_eq!(state.pool.capacity(), 8);
    }

    #[test]
    fn blend_mode_round_trip() {
        for m in ParticleBlendMode::ALL {
            assert_eq!(ParticleBlendMode::parse(m.as_str()), Some(m));
            assert_eq!(ParticleBlendMode::ALL[m.index()], m);
        }
    }
}
