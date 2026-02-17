//! Bridges ECS `particle_emitter` components to the particle simulation

use crate::curves::{lerp_color, lerp_f32};
use crate::emitter::{EmissionShape, EmitterConfig, EmitterState, ParticleBlendMode};
use crate::particle::ParticleInstance;
use crate::rand::ParticleRng;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::HashMap;

/// Draw data for one emitter, consumed by the renderer
pub struct ParticleDrawData<'a> {
    pub entity_id: EntityId,
    pub instances: &'a [ParticleInstance],
    pub blend_mode: ParticleBlendMode,
    pub texture: &'a str,
    pub frames_x: u32,
    pub frames_y: u32,
}

/// Manages per-emitter state, syncing between ECS components and simulation
pub struct ParticleSync {
    states: HashMap<EntityId, EmitterState>,
    /// Pre-allocated instance buffer for packing alive particles
    instance_buffer: Vec<ParticleInstance>,
    /// Per-emitter instance ranges: (entity_id, start, count, blend_mode, texture, frames_x, frames_y)
    instance_ranges: Vec<(EntityId, usize, usize, ParticleBlendMode, String, u32, u32)>,
}

impl ParticleSync {
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
            instance_buffer: Vec::new(),
            instance_ranges: Vec::new(),
        }
    }

    /// Clear all emitter states and instance buffers for a scene transition.
    pub fn clear(&mut self) {
        self.states.clear();
        self.instance_buffer.clear();
        self.instance_ranges.clear();
    }

    /// Scan the world for entities with `particle_emitter` components.
    /// Creates new EmitterState for newly discovered emitters,
    /// and updates config for existing ones (e.g., playing state changes from scripts).
    pub fn sync_from_world(&mut self, world: &FlintWorld) {
        // Track which entities still exist
        let mut seen = std::collections::HashSet::new();

        for entity in world.all_entities() {
            let Some(components) = world.get_components(entity.id) else {
                continue;
            };
            let Some(emitter_val) = components.get("particle_emitter") else {
                continue;
            };
            let Some(emitter_table) = emitter_val.as_table() else {
                continue;
            };

            seen.insert(entity.id);

            let config = EmitterConfig::from_toml(emitter_table);

            if let Some(state) = self.states.get_mut(&entity.id) {
                // Update mutable fields that scripts may have changed
                let ecs_playing = config.playing || config.autoplay;
                if ecs_playing && !state.playing {
                    state.playing = true;
                    state.emitter_time = 0.0;
                }
                if !ecs_playing && config.playing != state.config.playing {
                    state.playing = false;
                }

                // Sync rate changes
                state.config.emission_rate = config.emission_rate;
                state.config.gravity = config.gravity;
                state.config.damping = config.damping;
                state.config.size_start = config.size_start;
                state.config.size_end = config.size_end;
                state.config.color_start = config.color_start;
                state.config.color_end = config.color_end;
                state.config.blend_mode = config.blend_mode;
                state.config.texture = config.texture;

                // Update emitter position from transform
                if let Some(transform) = components.get("transform").and_then(|v| v.as_table()) {
                    state.emitter_position = read_position(transform);
                }
            } else {
                // New emitter
                let mut state = EmitterState::new(config);
                if let Some(transform) = components.get("transform").and_then(|v| v.as_table()) {
                    state.emitter_position = read_position(transform);
                }
                self.states.insert(entity.id, state);
            }
        }

        // Remove states for despawned entities
        self.states.retain(|id, _| seen.contains(id));
    }

    /// Run particle simulation for all emitters
    pub fn update(&mut self, rng: &mut ParticleRng, dt: f32) {
        for state in self.states.values_mut() {
            if !state.playing && state.pool.alive_count() == 0 {
                continue;
            }

            // Advance emitter timer
            if state.playing {
                state.emitter_time += dt;

                // Check duration limit
                if state.config.duration > 0.0 && state.emitter_time >= state.config.duration {
                    if state.config.looping {
                        state.emitter_time = 0.0;
                    } else {
                        state.playing = false;
                    }
                }
            }

            // Spawn new particles from emission rate
            if state.playing && state.config.emission_rate > 0.0 {
                state.accumulator += state.config.emission_rate * dt;
                let spawn_count = state.accumulator as u32;
                state.accumulator -= spawn_count as f32;

                for _ in 0..spawn_count {
                    spawn_particle(state, rng);
                }
            }

            // Spawn burst particles (from scripts or initial burst)
            let burst = state.pending_burst;
            state.pending_burst = 0;
            for _ in 0..burst {
                spawn_particle(state, rng);
            }

            // On first frame with autoplay + burst_count, fire initial burst
            if state.emitter_time <= dt
                && state.config.burst_count > 0
                && (state.config.playing || state.config.autoplay)
            {
                for _ in 0..state.config.burst_count {
                    spawn_particle(state, rng);
                }
            }

            // Integrate alive particles
            for p in state.pool.alive_slice_mut() {
                p.age += dt;
                // Apply gravity
                p.velocity[0] += state.config.gravity[0] * dt;
                p.velocity[1] += state.config.gravity[1] * dt;
                p.velocity[2] += state.config.gravity[2] * dt;
                // Apply damping
                if state.config.damping > 0.0 {
                    let factor = (1.0 - state.config.damping * dt).max(0.0);
                    p.velocity[0] *= factor;
                    p.velocity[1] *= factor;
                    p.velocity[2] *= factor;
                }
                // Integrate position
                p.position[0] += p.velocity[0] * dt;
                p.position[1] += p.velocity[1] * dt;
                p.position[2] += p.velocity[2] * dt;
                // Interpolate size and color over lifetime
                let t = p.age_ratio();
                p.size = lerp_f32(state.config.size_start, state.config.size_end, t);
                p.color = lerp_color(state.config.color_start, state.config.color_end, t);
                // Animate sprite frames
                if state.config.animate_frames {
                    let total_frames = state.config.frames_x * state.config.frames_y;
                    p.frame = (t * total_frames as f32).min(total_frames as f32 - 1.0) as u32;
                }
            }

            // Kill expired particles
            state.pool.update_and_compact();
        }
    }

    /// Pack alive particles into instance buffer for GPU upload.
    /// Call this after `update()`.
    pub fn pack_instances(&mut self) {
        self.instance_buffer.clear();
        self.instance_ranges.clear();

        for (&entity_id, state) in &self.states {
            let count = state.pool.alive_count();
            if count == 0 {
                continue;
            }
            let start = self.instance_buffer.len();
            for p in state.pool.alive_slice() {
                self.instance_buffer.push(ParticleInstance::from_particle(
                    p,
                    state.config.frames_x,
                    state.config.frames_y,
                ));
            }
            self.instance_ranges.push((
                entity_id,
                start,
                count,
                state.config.blend_mode,
                state.config.texture.clone(),
                state.config.frames_x,
                state.config.frames_y,
            ));
        }
    }

    /// Get the packed instance data
    pub fn instance_data(&self) -> &[ParticleInstance] {
        &self.instance_buffer
    }

    /// Iterate draw data for each emitter that has alive particles
    pub fn draw_data(&self) -> Vec<ParticleDrawData<'_>> {
        self.instance_ranges
            .iter()
            .map(
                |(entity_id, start, count, blend_mode, texture, frames_x, frames_y)| {
                    ParticleDrawData {
                        entity_id: *entity_id,
                        instances: &self.instance_buffer[*start..*start + *count],
                        blend_mode: *blend_mode,
                        texture,
                        frames_x: *frames_x,
                        frames_y: *frames_y,
                    }
                },
            )
            .collect()
    }

    /// Queue a burst of particles on a specific emitter (from script command)
    pub fn queue_burst(&mut self, entity_id: EntityId, count: u32) {
        if let Some(state) = self.states.get_mut(&entity_id) {
            state.pending_burst += count;
        }
    }

    /// Number of tracked emitters
    pub fn emitter_count(&self) -> usize {
        self.states.len()
    }

    /// Total alive particles across all emitters
    pub fn total_alive(&self) -> usize {
        self.states.values().map(|s| s.pool.alive_count()).sum()
    }
}

impl Default for ParticleSync {
    fn default() -> Self {
        Self::new()
    }
}

fn spawn_particle(state: &mut EmitterState, rng: &mut ParticleRng) {
    let Some(p) = state.pool.spawn() else {
        return;
    };

    // Initial position based on emission shape
    let (offset, shape_dir) = match state.config.shape {
        EmissionShape::Point => ([0.0f32; 3], None),
        EmissionShape::Sphere { radius } => {
            let dir = rng.random_direction();
            let r = rng.range(0.0, radius);
            ([dir[0] * r, dir[1] * r, dir[2] * r], None)
        }
        EmissionShape::Cone { radius: _, angle } => {
            let dir = rng.cone_direction(state.config.direction, angle);
            ([0.0, 0.0, 0.0], Some(dir))
        }
        EmissionShape::Box { extents } => {
            let x = rng.range(-extents[0], extents[0]);
            let y = rng.range(-extents[1], extents[1]);
            let z = rng.range(-extents[2], extents[2]);
            ([x, y, z], None)
        }
    };

    // Position in world space
    if state.config.world_space {
        p.position[0] = state.emitter_position[0] + offset[0];
        p.position[1] = state.emitter_position[1] + offset[1];
        p.position[2] = state.emitter_position[2] + offset[2];
    } else {
        p.position = offset;
    }

    // Velocity
    let speed = rng.range(state.config.speed_min, state.config.speed_max);
    let dir = if let Some(d) = shape_dir {
        d
    } else {
        rng.cone_direction(state.config.direction, state.config.spread)
    };
    p.velocity = [dir[0] * speed, dir[1] * speed, dir[2] * speed];

    // Lifetime and initial values
    p.lifetime = rng.range(state.config.lifetime_min, state.config.lifetime_max);
    p.age = 0.0;
    p.size = state.config.size_start;
    p.color = state.config.color_start;
    p.rotation = rng.range(0.0, std::f32::consts::TAU);
    p.frame = 0;
    p.alive = true;
}

fn read_position(transform: &toml::value::Table) -> [f32; 3] {
    if let Some(pos) = transform.get("position") {
        if let Some(arr) = pos.as_array() {
            if arr.len() >= 3 {
                return [
                    toml_f32(&arr[0], 0.0),
                    toml_f32(&arr[1], 0.0),
                    toml_f32(&arr[2], 0.0),
                ];
            }
        }
    }
    [0.0; 3]
}

fn toml_f32(v: &toml::Value, default: f32) -> f32 {
    v.as_float()
        .map(|f| f as f32)
        .or_else(|| v.as_integer().map(|i| i as f32))
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_basic_lifecycle() {
        let config = EmitterConfig {
            emission_rate: 100.0,
            max_particles: 50,
            autoplay: true,
            ..Default::default()
        };
        let mut state = EmitterState::new(config);
        assert!(state.playing);

        let mut rng = ParticleRng::new(42);

        // Simulate one frame at 60fps
        state.playing = true;
        state.accumulator += 100.0 * (1.0 / 60.0); // ~1.67
        let count = state.accumulator as u32;
        state.accumulator -= count as f32;

        for _ in 0..count {
            spawn_particle(&mut state, &mut rng);
        }
        assert!(state.pool.alive_count() > 0);
    }

    #[test]
    fn pack_instances_produces_correct_count() {
        let mut sync = ParticleSync::new();
        let config = EmitterConfig {
            emission_rate: 0.0,
            burst_count: 5,
            max_particles: 10,
            autoplay: true,
            playing: true,
            ..Default::default()
        };
        let state = EmitterState::new(config);
        sync.states.insert(EntityId(1), state);

        let mut rng = ParticleRng::new(42);
        sync.update(&mut rng, 1.0 / 60.0);
        sync.pack_instances();

        assert_eq!(sync.instance_data().len(), 5);
        assert_eq!(sync.draw_data().len(), 1);
    }
}
