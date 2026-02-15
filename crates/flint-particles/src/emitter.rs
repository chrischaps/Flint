//! Emitter configuration (parsed from TOML) and runtime state

use crate::particle::ParticlePool;

/// Blend mode for particle rendering
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParticleBlendMode {
    Alpha,
    Additive,
}

/// Emission shape
#[derive(Debug, Clone, Copy)]
pub enum EmissionShape {
    Point,
    Sphere { radius: f32 },
    Cone { radius: f32, angle: f32 },
    Box { extents: [f32; 3] },
}

/// Configuration parsed from a `particle_emitter` TOML component
#[derive(Debug, Clone)]
pub struct EmitterConfig {
    pub emission_rate: f32,
    pub burst_count: u32,
    pub max_particles: usize,
    pub lifetime_min: f32,
    pub lifetime_max: f32,
    pub speed_min: f32,
    pub speed_max: f32,
    pub direction: [f32; 3],
    pub spread: f32,
    pub gravity: [f32; 3],
    pub damping: f32,
    pub size_start: f32,
    pub size_end: f32,
    pub color_start: [f32; 4],
    pub color_end: [f32; 4],
    pub texture: String,
    pub frames_x: u32,
    pub frames_y: u32,
    pub animate_frames: bool,
    pub blend_mode: ParticleBlendMode,
    pub shape: EmissionShape,
    pub world_space: bool,
    pub duration: f32,
    pub looping: bool,
    pub playing: bool,
    pub autoplay: bool,
}

impl Default for EmitterConfig {
    fn default() -> Self {
        Self {
            emission_rate: 10.0,
            burst_count: 0,
            max_particles: 256,
            lifetime_min: 1.0,
            lifetime_max: 2.0,
            speed_min: 1.0,
            speed_max: 3.0,
            direction: [0.0, 1.0, 0.0],
            spread: 15.0,
            gravity: [0.0, -9.81, 0.0],
            damping: 0.0,
            size_start: 0.1,
            size_end: 0.0,
            color_start: [1.0, 1.0, 1.0, 1.0],
            color_end: [1.0, 1.0, 1.0, 0.0],
            texture: String::new(),
            frames_x: 1,
            frames_y: 1,
            animate_frames: false,
            blend_mode: ParticleBlendMode::Alpha,
            shape: EmissionShape::Point,
            world_space: true,
            duration: 0.0,
            looping: true,
            playing: false,
            autoplay: true,
        }
    }
}

impl EmitterConfig {
    /// Parse an EmitterConfig from a TOML component table
    pub fn from_toml(table: &toml::value::Table) -> Self {
        let mut config = Self::default();

        if let Some(v) = table.get("emission_rate") {
            config.emission_rate = toml_f32(v, config.emission_rate);
        }
        if let Some(v) = table.get("burst_count") {
            config.burst_count = v.as_integer().unwrap_or(0) as u32;
        }
        if let Some(v) = table.get("max_particles") {
            let n = v.as_integer().unwrap_or(256) as usize;
            config.max_particles = n.min(10000);
        }
        if let Some(v) = table.get("lifetime_min") {
            config.lifetime_min = toml_f32(v, config.lifetime_min);
        }
        if let Some(v) = table.get("lifetime_max") {
            config.lifetime_max = toml_f32(v, config.lifetime_max);
        }
        if let Some(v) = table.get("speed_min") {
            config.speed_min = toml_f32(v, config.speed_min);
        }
        if let Some(v) = table.get("speed_max") {
            config.speed_max = toml_f32(v, config.speed_max);
        }
        if let Some(v) = table.get("direction") {
            config.direction = toml_vec3(v, config.direction);
        }
        if let Some(v) = table.get("spread") {
            config.spread = toml_f32(v, config.spread);
        }
        if let Some(v) = table.get("gravity") {
            config.gravity = toml_vec3(v, config.gravity);
        }
        if let Some(v) = table.get("damping") {
            config.damping = toml_f32(v, config.damping);
        }
        if let Some(v) = table.get("size_start") {
            config.size_start = toml_f32(v, config.size_start);
        }
        if let Some(v) = table.get("size_end") {
            config.size_end = toml_f32(v, config.size_end);
        }
        if let Some(v) = table.get("color_start") {
            config.color_start = toml_vec4(v, config.color_start);
        }
        if let Some(v) = table.get("color_end") {
            config.color_end = toml_vec4(v, config.color_end);
        }
        if let Some(v) = table.get("texture") {
            if let Some(s) = v.as_str() {
                config.texture = s.to_string();
            }
        }
        if let Some(v) = table.get("frames_x") {
            config.frames_x = v.as_integer().unwrap_or(1).max(1) as u32;
        }
        if let Some(v) = table.get("frames_y") {
            config.frames_y = v.as_integer().unwrap_or(1).max(1) as u32;
        }
        if let Some(v) = table.get("animate_frames") {
            config.animate_frames = v.as_bool().unwrap_or(false);
        }
        if let Some(v) = table.get("blend_mode") {
            config.blend_mode = match v.as_str().unwrap_or("alpha") {
                "additive" => ParticleBlendMode::Additive,
                _ => ParticleBlendMode::Alpha,
            };
        }

        // Emission shape
        let shape_str = table
            .get("shape")
            .and_then(|v| v.as_str())
            .unwrap_or("point");
        let shape_radius = table
            .get("shape_radius")
            .map(|v| toml_f32(v, 0.5))
            .unwrap_or(0.5);
        let shape_angle = table
            .get("shape_angle")
            .map(|v| toml_f32(v, 30.0))
            .unwrap_or(30.0);
        let shape_extents = table
            .get("shape_extents")
            .map(|v| toml_vec3(v, [0.5, 0.5, 0.5]))
            .unwrap_or([0.5, 0.5, 0.5]);

        config.shape = match shape_str {
            "sphere" => EmissionShape::Sphere {
                radius: shape_radius,
            },
            "cone" => EmissionShape::Cone {
                radius: shape_radius,
                angle: shape_angle,
            },
            "box" => EmissionShape::Box {
                extents: shape_extents,
            },
            _ => EmissionShape::Point,
        };

        if let Some(v) = table.get("world_space") {
            config.world_space = v.as_bool().unwrap_or(true);
        }
        if let Some(v) = table.get("duration") {
            config.duration = toml_f32(v, 0.0);
        }
        if let Some(v) = table.get("looping") {
            config.looping = v.as_bool().unwrap_or(true);
        }
        if let Some(v) = table.get("playing") {
            config.playing = v.as_bool().unwrap_or(false);
        }
        if let Some(v) = table.get("autoplay") {
            config.autoplay = v.as_bool().unwrap_or(true);
        }

        config
    }
}

/// Runtime state for one emitter
pub struct EmitterState {
    pub config: EmitterConfig,
    pub pool: ParticlePool,
    /// Fractional particle accumulator for sub-frame emission
    pub accumulator: f32,
    /// How long this emitter has been running
    pub emitter_time: f32,
    /// Burst particles queued from scripts
    pub pending_burst: u32,
    /// Whether the emitter is currently playing
    pub playing: bool,
    /// Emitter world position (from entity transform)
    pub emitter_position: [f32; 3],
}

impl EmitterState {
    pub fn new(config: EmitterConfig) -> Self {
        let playing = config.playing || config.autoplay;
        let pool = ParticlePool::new(config.max_particles);
        Self {
            config,
            pool,
            accumulator: 0.0,
            emitter_time: 0.0,
            pending_burst: 0,
            playing,
            emitter_position: [0.0; 3],
        }
    }
}

// ── TOML helpers (handle integer/float coercion) ──

fn toml_f32(v: &toml::Value, default: f32) -> f32 {
    v.as_float()
        .map(|f| f as f32)
        .or_else(|| v.as_integer().map(|i| i as f32))
        .unwrap_or(default)
}

fn toml_vec3(v: &toml::Value, default: [f32; 3]) -> [f32; 3] {
    if let Some(arr) = v.as_array() {
        if arr.len() >= 3 {
            return [
                toml_f32(&arr[0], default[0]),
                toml_f32(&arr[1], default[1]),
                toml_f32(&arr[2], default[2]),
            ];
        }
    }
    default
}

fn toml_vec4(v: &toml::Value, default: [f32; 4]) -> [f32; 4] {
    if let Some(arr) = v.as_array() {
        if arr.len() >= 4 {
            return [
                toml_f32(&arr[0], default[0]),
                toml_f32(&arr[1], default[1]),
                toml_f32(&arr[2], default[2]),
                toml_f32(&arr[3], default[3]),
            ];
        }
    }
    default
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
        assert!((config.color_start[1] - 0.5).abs() < 0.01);
        if let EmissionShape::Cone { angle, .. } = config.shape {
            assert!((angle - 45.0).abs() < 0.01);
        } else {
            panic!("Expected Cone shape");
        }
    }

    #[test]
    fn toml_integer_float_coercion() {
        // TOML `gravity = [0, -10, 0]` gives integers for 0, float for -10
        let toml_str = "gravity = [0, -10, 0]";
        let table: toml::value::Table = toml::from_str(toml_str).unwrap();
        let config = EmitterConfig::from_toml(&table);
        assert!((config.gravity[0]).abs() < 0.01);
        assert!((config.gravity[1] - (-10.0)).abs() < 0.01);
    }
}
