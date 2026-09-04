//! Per-emitter simulation: spawning, forces, curves, bursts, sub-emitters.
//!
//! Everything here operates on one [`EmitterState`] plus a read-only
//! [`EmitterFrame`] describing where the owning effect is this step. That
//! narrow signature is the seam for a future GPU path: a compute shader
//! would replace [`step_emitter`] while [`crate::ParticleInstance`] stays
//! the only output contract (ADR 0068).

use crate::emitter::{EmissionShape, EmitterState, Force};
use crate::noise::noise3_vec;
use crate::rand::{cross, normalize, perpendicular_basis};

/// Where the owning effect instance is during this step.
#[derive(Clone, Copy, Debug)]
pub struct EmitterFrame {
    /// World position now.
    pub position: [f32; 3],
    /// World position at the previous step (for sub-frame interpolation and
    /// emission over distance).
    pub prev_position: [f32; 3],
    /// Emitter velocity in world units per second.
    pub velocity: [f32; 3],
    /// Unit rotation basis (columns); `world = basis · local`.
    pub basis: [[f32; 3]; 3],
    /// Uniform scale from the entity transform.
    pub transform_scale: f32,
    /// Scale from the `particle_effect` component / script parameter.
    pub effect_scale: f32,
    /// Emission-rate multiplier from the component / script parameter.
    pub emission_scale: f32,
    /// Global simulation time (drives animated noise).
    pub time: f32,
}

impl Default for EmitterFrame {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            prev_position: [0.0; 3],
            velocity: [0.0; 3],
            basis: IDENTITY,
            transform_scale: 1.0,
            effect_scale: 1.0,
            emission_scale: 1.0,
            time: 0.0,
        }
    }
}

pub const IDENTITY: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// A deferred spawn into a sibling emitter, produced by `on_birth` /
/// `on_death`. Positions are in the *source* emitter's space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpawnRequest {
    pub target: usize,
    pub position: [f32; 3],
    /// Already multiplied by the sub-emitter's `inherit_velocity`.
    pub velocity: [f32; 3],
    pub count: u32,
    /// Was the source emitter simulating in world space?
    pub source_world_space: bool,
}

/// Where a new particle comes from.
#[derive(Clone, Copy, Debug)]
pub enum SpawnOrigin {
    /// From the emitter's own shape; `frac` ∈ [0, 1] is the sub-frame
    /// position along `prev_position → position`.
    Emitter { frac: f32 },
    /// At an explicit point (sub-emitter), in this emitter's space.
    At {
        position: [f32; 3],
        inherited_velocity: [f32; 3],
    },
}

#[inline]
fn mul_basis(b: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        b[0][0] * v[0] + b[1][0] * v[1] + b[2][0] * v[2],
        b[0][1] * v[0] + b[1][1] * v[1] + b[2][1] * v[2],
        b[0][2] * v[0] + b[1][2] * v[1] + b[2][2] * v[2],
    ]
}

#[inline]
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn scale(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[inline]
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

#[inline]
fn length(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// Orthonormal (u, v, u×v) from two authored axes; `None` when either is
/// degenerate (zero or parallel), which selects the legacy axis-aligned box.
pub fn oriented_basis(u: [f32; 3], v: [f32; 3]) -> Option<([f32; 3], [f32; 3], [f32; 3])> {
    fn norm(a: [f32; 3]) -> Option<[f32; 3]> {
        let l = length(a);
        (l > 1e-6).then(|| [a[0] / l, a[1] / l, a[2] / l])
    }
    let u = norm(u)?;
    let w = norm(cross(u, v))?;
    let v = cross(w, u); // re-orthogonalised, unit by construction
    Some((u, v, w))
}

/// Spawn one particle. Returns `false` when the pool or budget is full.
pub fn spawn_particle(
    state: &mut EmitterState,
    emitter_index: usize,
    frame: &EmitterFrame,
    origin: SpawnOrigin,
    dt: f32,
    budget_left: &mut usize,
    requests: &mut Vec<SpawnRequest>,
) -> bool {
    if *budget_left == 0 || state.pool.is_full() {
        return false;
    }
    let cfg = &state.config;
    let rng = &mut state.rng;

    let dist_scale = if cfg.local_axes {
        frame.transform_scale * frame.effect_scale
    } else {
        frame.effect_scale
    };
    let rotate = |v: [f32; 3]| {
        if cfg.local_axes {
            mul_basis(&frame.basis, v)
        } else {
            v
        }
    };
    let base_dir = normalize(rotate(cfg.direction));

    let (position, velocity, age) = match origin {
        SpawnOrigin::Emitter { frac } => {
            // Offset from the shape, in emitter-local units.
            let (local_offset, shape_dir) = match cfg.shape {
                EmissionShape::Point => ([0.0f32; 3], None),
                EmissionShape::Sphere { radius } => {
                    let dir = rng.random_direction();
                    let r = rng.range(0.0, radius);
                    (scale(dir, r), None)
                }
                EmissionShape::Cone { radius, angle } => {
                    let dir = rng.cone_direction(base_dir, angle);
                    let offset = if radius > 0.0 {
                        let (right, up, _) = perpendicular_basis(base_dir);
                        let [dx, dy] = rng.unit_disc();
                        add(scale(right, dx * radius), scale(up, dy * radius))
                    } else {
                        [0.0; 3]
                    };
                    // The cone offset is already in world orientation.
                    (offset, Some(dir))
                }
                EmissionShape::Box { extents } => {
                    let x = rng.range(-extents[0], extents[0]);
                    let y = rng.range(-extents[1], extents[1]);
                    let z = rng.range(-extents[2], extents[2]);
                    match oriented_basis(cfg.shape_axis_u, cfg.shape_axis_v) {
                        Some((u, v, w)) => (add(add(scale(u, x), scale(v, y)), scale(w, z)), None),
                        None => ([x, y, z], None),
                    }
                }
            };
            // Cone offsets were built in world orientation (see above); the
            // other shapes are local and get rotated here.
            let offset = match cfg.shape {
                EmissionShape::Cone { .. } => scale(local_offset, dist_scale),
                _ => scale(rotate(local_offset), dist_scale),
            };
            let offset = add(offset, scale(rotate(cfg.shape_offset), dist_scale));

            let emitter_pos = lerp3(frame.prev_position, frame.position, frac.clamp(0.0, 1.0));
            let position = if cfg.world_space {
                add(emitter_pos, offset)
            } else {
                offset
            };

            let speed = rng.range(cfg.speed_min, cfg.speed_max) * frame.effect_scale;
            let dir = match shape_dir {
                Some(d) => d,
                None => rng.cone_direction(base_dir, cfg.spread),
            };
            let mut velocity = scale(dir, speed);
            if cfg.inherit_velocity != 0.0 && cfg.world_space {
                velocity = add(velocity, scale(frame.velocity, cfg.inherit_velocity));
            }
            // Sub-frame: a particle born part-way through the step has
            // already lived the remainder of it.
            let age = ((1.0 - frac) * dt).clamp(0.0, dt.max(0.0));
            (position, velocity, age)
        }
        SpawnOrigin::At {
            position,
            inherited_velocity,
        } => {
            let speed = rng.range(cfg.speed_min, cfg.speed_max) * frame.effect_scale;
            let dir = rng.cone_direction(base_dir, cfg.spread);
            (position, add(scale(dir, speed), inherited_velocity), 0.0)
        }
    };

    let lifetime = rng.range(cfg.lifetime_min, cfg.lifetime_max).max(1e-4);
    let size_scale = rng.range(cfg.size_scale_min, cfg.size_scale_max);
    let brightness = rng.range(cfg.brightness_min, cfg.brightness_max);
    let rotation = rng.range(cfg.rotation_min, cfg.rotation_max);
    let angular_velocity = rng.range(cfg.angular_velocity_min, cfg.angular_velocity_max);
    let total_frames = cfg.frames_x * cfg.frames_y;
    let frame_offset = if cfg.random_start_frame && total_frames > 1 {
        rng.range_u32(0, total_frames - 1)
    } else {
        0
    };
    let random = rng.next_f32();

    // Pre-integrate the sub-frame remainder so streams stay continuous
    // (forces are not applied to the partial step).
    let position = add(position, scale(velocity, age));

    let size0 = cfg.size.first();
    let size_mul = size_scale * frame.effect_scale;
    let mut color0 = cfg.color.first();
    color0[0] *= brightness;
    color0[1] *= brightness;
    color0[2] *= brightness;
    if let Some(a) = &cfg.alpha {
        color0[3] *= a.first();
    }

    let on_birth = cfg.on_birth.clone();
    let on_birth_target = state.on_birth_target;
    let world_space = cfg.world_space;

    let Some(p) = state.pool.spawn() else {
        return false;
    };
    p.position = position;
    p.velocity = velocity;
    p.age = age;
    p.lifetime = lifetime;
    p.size = [size0[0] * size_mul, size0[1] * size_mul];
    p.size_scale = size_scale;
    p.rotation = rotation;
    p.angular_velocity = angular_velocity;
    p.color = color0;
    p.brightness = brightness;
    p.frame = frame_offset;
    p.frame_offset = frame_offset;
    p.random = random;
    p.alive = true;
    *budget_left -= 1;

    if let (Some(sub), Some(target)) = (on_birth, on_birth_target) {
        if target != emitter_index {
            let count = state.rng.range_u32(sub.count_min, sub.count_max);
            if count > 0 {
                requests.push(SpawnRequest {
                    target,
                    position,
                    velocity: scale(velocity, sub.inherit_velocity),
                    count,
                    source_world_space: world_space,
                });
            }
        }
    }
    true
}

/// Advance one emitter by `dt`: timeline, bursts, emission, integration,
/// deaths. Sub-emitter spawns are pushed to `requests` for the owner to
/// resolve after every sibling has stepped.
pub fn step_emitter(
    state: &mut EmitterState,
    emitter_index: usize,
    frame: &EmitterFrame,
    dt: f32,
    budget_left: &mut usize,
    requests: &mut Vec<SpawnRequest>,
) {
    if !state.playing && state.pool.alive_count() == 0 && state.pending_burst == 0 {
        return;
    }

    // --- Timeline ---
    if state.playing {
        state.emitter_time += dt;
        let cfg = &state.config;
        if cfg.duration > 0.0 && state.emitter_time >= cfg.start_delay + cfg.duration {
            if cfg.looping {
                let over = state.emitter_time - (cfg.start_delay + cfg.duration);
                state.emitter_time = cfg.start_delay + over.rem_euclid(cfg.duration.max(1e-6));
                state.reset_bursts();
            } else {
                state.playing = false;
            }
        }
    }
    let emitting = state.playing && state.emitter_time >= state.config.start_delay;

    // --- Bursts ---
    if emitting {
        let local_t = state.emitter_time - state.config.start_delay;
        let n_bursts = state.bursts.len().min(state.config.bursts.len());
        for bi in 0..n_bursts {
            loop {
                let b = state.config.bursts[bi];
                let rt = state.bursts[bi];
                if (b.cycles != 0 && rt.fired >= b.cycles) || local_t < rt.next_time {
                    break;
                }
                let fire = state.rng.chance(b.probability);
                let count = state.rng.range_u32(b.count_min, b.count_max);
                if fire {
                    for _ in 0..count {
                        if !spawn_particle(
                            state,
                            emitter_index,
                            frame,
                            SpawnOrigin::Emitter { frac: 1.0 },
                            dt,
                            budget_left,
                            requests,
                        ) {
                            break;
                        }
                    }
                }
                let rt = &mut state.bursts[bi];
                rt.fired += 1;
                if b.interval > 0.0 {
                    rt.next_time += b.interval;
                } else {
                    rt.next_time = f32::INFINITY;
                }
            }
        }
    }

    // --- Rate emission (sub-frame interpolated) ---
    if emitting && state.config.emission_rate > 0.0 {
        state.accumulator += state.config.emission_rate * frame.emission_scale * dt;
        let n = state.accumulator.floor() as u32;
        state.accumulator -= n as f32;
        for i in 0..n {
            let frac = (i as f32 + 0.5) / n as f32;
            if !spawn_particle(
                state,
                emitter_index,
                frame,
                SpawnOrigin::Emitter { frac },
                dt,
                budget_left,
                requests,
            ) {
                break;
            }
        }
    }

    // --- Emission over distance ---
    if emitting && state.config.emission_per_meter > 0.0 {
        let moved = length(sub(frame.position, frame.prev_position));
        state.distance_accum += moved * state.config.emission_per_meter * frame.emission_scale;
        let n = state.distance_accum.floor() as u32;
        state.distance_accum -= n as f32;
        for i in 0..n {
            let frac = (i as f32 + 0.5) / n as f32;
            if !spawn_particle(
                state,
                emitter_index,
                frame,
                SpawnOrigin::Emitter { frac },
                dt,
                budget_left,
                requests,
            ) {
                break;
            }
        }
    }

    // --- Script bursts ---
    let pending = std::mem::take(&mut state.pending_burst);
    for _ in 0..pending {
        if !spawn_particle(
            state,
            emitter_index,
            frame,
            SpawnOrigin::Emitter { frac: 1.0 },
            dt,
            budget_left,
            requests,
        ) {
            break;
        }
    }

    // --- Integrate ---
    integrate(state, frame, dt);

    // --- Deaths → sub-emitters ---
    if let (Some(sub), Some(target)) = (&state.config.on_death, state.on_death_target) {
        if target != emitter_index {
            let world_space = state.config.world_space;
            let (cmin, cmax, inherit) = (sub.count_min, sub.count_max, sub.inherit_velocity);
            let mut dying: Vec<([f32; 3], [f32; 3])> = Vec::new();
            for p in state.pool.alive_slice() {
                if p.age >= p.lifetime {
                    dying.push((p.position, p.velocity));
                }
            }
            for (position, velocity) in dying {
                let count = state.rng.range_u32(cmin, cmax);
                if count > 0 {
                    requests.push(SpawnRequest {
                        target,
                        position,
                        velocity: scale(velocity, inherit),
                        count,
                        source_world_space: world_space,
                    });
                }
            }
        }
    }

    state.pool.update_and_compact();
}

/// Apply forces and curves to every alive particle.
fn integrate(state: &mut EmitterState, frame: &EmitterFrame, dt: f32) {
    let cfg = &state.config;
    let damping_k = if cfg.damping > 0.0 {
        (-cfg.damping * dt).exp()
    } else {
        1.0
    };
    let origin = if cfg.world_space {
        frame.position
    } else {
        [0.0; 3]
    };
    let size_mul = frame.effect_scale;
    let total_frames = (cfg.frames_x * cfg.frames_y).max(1);
    let noise_seed = 0x5EED ^ (cfg.name.len() as u32);

    for p in state.pool.alive_slice_mut() {
        p.age += dt;
        let t = p.age_ratio();

        // Gravity + exponential damping (frame-rate independent).
        p.velocity = add(p.velocity, scale(cfg.gravity, dt));
        if damping_k != 1.0 {
            p.velocity = scale(p.velocity, damping_k);
        }

        for f in &cfg.forces {
            match *f {
                Force::Wind { velocity, strength } => {
                    let k = 1.0 - (-strength * dt).exp();
                    p.velocity = add(p.velocity, scale(sub(velocity, p.velocity), k));
                }
                Force::Drag { coefficient } => {
                    let v = length(p.velocity);
                    let k = 1.0 / (1.0 + coefficient * v * dt);
                    p.velocity = scale(p.velocity, k);
                }
                Force::Noise {
                    strength,
                    frequency,
                    speed,
                    octaves,
                } => {
                    let q = [
                        p.position[0] * frequency + frame.time * speed,
                        p.position[1] * frequency - frame.time * speed * 0.7,
                        p.position[2] * frequency + frame.time * speed * 0.3,
                    ];
                    let n = noise3_vec(q, noise_seed, octaves);
                    p.velocity = add(p.velocity, scale(n, strength * dt));
                }
                Force::Vortex {
                    center,
                    axis,
                    strength,
                    falloff,
                } => {
                    let axis = normalize(axis);
                    let r = sub(p.position, add(origin, center));
                    let along = r[0] * axis[0] + r[1] * axis[1] + r[2] * axis[2];
                    let radial = sub(r, scale(axis, along));
                    let dist = length(radial);
                    if dist > 1e-4 {
                        let tangent = cross(axis, scale(radial, 1.0 / dist));
                        let mag = strength / (1.0 + falloff * dist);
                        p.velocity = add(p.velocity, scale(tangent, mag * dt));
                    }
                }
                Force::Attractor {
                    position,
                    strength,
                    radius,
                } => {
                    let d = sub(add(origin, position), p.position);
                    let dist = length(d);
                    if dist > 1e-4 && (radius <= 0.0 || dist <= radius) {
                        p.velocity = add(p.velocity, scale(d, strength * dt / dist));
                    }
                }
            }
        }

        let speed_mul = cfg.speed_curve.as_ref().map_or(1.0, |c| c.sample(t));
        p.position = add(p.position, scale(p.velocity, speed_mul * dt));
        p.rotation += p.angular_velocity * dt;

        let s = cfg.size.sample(t);
        let m = p.size_scale * size_mul;
        p.size = [s[0] * m, s[1] * m];

        let mut c = cfg.color.sample(t);
        c[0] *= p.brightness;
        c[1] *= p.brightness;
        c[2] *= p.brightness;
        if let Some(a) = &cfg.alpha {
            c[3] *= a.sample(t);
        }
        p.color = c;

        p.frame = if cfg.frame_rate > 0.0 {
            (p.frame_offset + (p.age * cfg.frame_rate) as u32) % total_frames
        } else if cfg.animate_frames {
            (p.frame_offset + ((t * total_frames as f32) as u32).min(total_frames - 1))
                % total_frames
        } else {
            p.frame_offset
        };
    }
}

/// Resolve a batch of sub-emitter requests into their target emitters.
pub fn apply_spawn_requests(
    emitters: &mut [EmitterState],
    requests: &[SpawnRequest],
    frame: &EmitterFrame,
    dt: f32,
    budget_left: &mut usize,
    next_requests: &mut Vec<SpawnRequest>,
) {
    for req in requests {
        let Some(target) = emitters.get_mut(req.target) else {
            continue;
        };
        // Convert between world and emitter-local spaces when the two
        // emitters disagree (rotation is ignored; translation is enough
        // for puffs and sparks).
        let position = match (req.source_world_space, target.config.world_space) {
            (true, false) => sub(req.position, frame.position),
            (false, true) => add(req.position, frame.position),
            _ => req.position,
        };
        for _ in 0..req.count {
            if !spawn_particle(
                target,
                req.target,
                frame,
                SpawnOrigin::At {
                    position,
                    inherited_velocity: req.velocity,
                },
                dt,
                budget_left,
                next_requests,
            ) {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curves::Curve;
    use crate::emitter::{Burst, EmitterConfig, SubEmitter};

    fn quiet(cfg: EmitterConfig) -> EmitterState {
        EmitterState::with_seed(cfg, 42)
    }

    fn run(
        state: &mut EmitterState,
        frame: &EmitterFrame,
        dt: f32,
        steps: usize,
    ) -> Vec<SpawnRequest> {
        let mut budget = usize::MAX / 2;
        let mut reqs = Vec::new();
        for _ in 0..steps {
            step_emitter(state, 0, frame, dt, &mut budget, &mut reqs);
        }
        reqs
    }

    #[test]
    fn damping_is_frame_rate_independent() {
        let cfg = EmitterConfig {
            emission_rate: 0.0,
            gravity: [0.0; 3],
            damping: 2.0,
            ..Default::default()
        };
        let mut a = quiet(cfg.clone());
        let mut b = quiet(cfg);
        for s in [&mut a, &mut b] {
            let p = s.pool.spawn().unwrap();
            p.velocity = [10.0, 0.0, 0.0];
            p.lifetime = 10.0;
        }
        let frame = EmitterFrame::default();
        run(&mut a, &frame, 1.0 / 30.0, 1);
        run(&mut b, &frame, 1.0 / 60.0, 2);
        let va = a.pool.alive_slice()[0].velocity[0];
        let vb = b.pool.alive_slice()[0].velocity[0];
        assert!((va - vb).abs() < 1e-4, "{va} vs {vb}");
        assert!((va - 10.0 * (-2.0f32 / 30.0).exp()).abs() < 1e-4);
    }

    #[test]
    fn initial_burst_fires_once_per_play() {
        let cfg = EmitterConfig {
            emission_rate: 0.0,
            lifetime_min: 100.0,
            lifetime_max: 100.0,
            bursts: vec![Burst {
                time: 0.0,
                count_min: 5,
                count_max: 5,
                cycles: 1,
                interval: 0.0,
                probability: 1.0,
            }],
            ..Default::default()
        };
        let mut s = quiet(cfg);
        let frame = EmitterFrame::default();
        run(&mut s, &frame, 1.0 / 60.0, 10);
        assert_eq!(s.pool.alive_count(), 5);
        // Restarting the emitter fires it again.
        s.restart();
        run(&mut s, &frame, 1.0 / 60.0, 3);
        assert_eq!(s.pool.alive_count(), 10);
    }

    #[test]
    fn bursts_fire_on_schedule_and_respect_cycles() {
        let cfg = EmitterConfig {
            emission_rate: 0.0,
            lifetime_min: 100.0,
            lifetime_max: 100.0,
            bursts: vec![Burst {
                time: 0.5,
                count_min: 2,
                count_max: 2,
                cycles: 3,
                interval: 1.0,
                probability: 1.0,
            }],
            ..Default::default()
        };
        let mut s = quiet(cfg);
        let frame = EmitterFrame::default();
        let dt = 0.1;
        run(&mut s, &frame, dt, 4); // t = 0.4
        assert_eq!(s.pool.alive_count(), 0);
        run(&mut s, &frame, dt, 1); // t = 0.5
        assert_eq!(s.pool.alive_count(), 2);
        run(&mut s, &frame, dt, 10); // t = 1.5
        assert_eq!(s.pool.alive_count(), 4);
        run(&mut s, &frame, dt, 30); // t = 4.5 — third cycle at 2.5, none after
        assert_eq!(s.pool.alive_count(), 6);
    }

    #[test]
    fn looping_duration_resets_bursts() {
        let cfg = EmitterConfig {
            emission_rate: 0.0,
            lifetime_min: 100.0,
            lifetime_max: 100.0,
            duration: 1.0,
            looping: true,
            bursts: vec![Burst {
                time: 0.0,
                count_min: 1,
                count_max: 1,
                cycles: 1,
                interval: 0.0,
                probability: 1.0,
            }],
            ..Default::default()
        };
        let mut s = quiet(cfg);
        let frame = EmitterFrame::default();
        run(&mut s, &frame, 0.25, 9); // t = 2.25 → loops at 1.0 and 2.0
        assert_eq!(s.pool.alive_count(), 3);
    }

    #[test]
    fn non_looping_duration_stops() {
        let cfg = EmitterConfig {
            emission_rate: 100.0,
            duration: 0.5,
            looping: false,
            lifetime_min: 0.1,
            lifetime_max: 0.1,
            ..Default::default()
        };
        let mut s = quiet(cfg);
        let frame = EmitterFrame::default();
        run(&mut s, &frame, 0.1, 20);
        assert!(!s.playing);
        assert_eq!(s.pool.alive_count(), 0);
    }

    #[test]
    fn cone_radius_offsets_spawn() {
        let cfg = EmitterConfig {
            emission_rate: 0.0,
            speed_min: 0.0,
            speed_max: 0.0,
            shape: EmissionShape::Cone {
                radius: 1.0,
                angle: 0.0,
            },
            direction: [0.0, 1.0, 0.0],
            ..Default::default()
        };
        let mut s = quiet(cfg);
        s.pending_burst = 50;
        let frame = EmitterFrame::default();
        run(&mut s, &frame, 0.0, 1);
        let mut spread = 0.0f32;
        for p in s.pool.alive_slice() {
            assert!(p.position[1].abs() < 1e-5, "disc is perpendicular to +Y");
            let r = (p.position[0].powi(2) + p.position[2].powi(2)).sqrt();
            assert!(r <= 1.0 + 1e-5);
            spread = spread.max(r);
        }
        assert!(spread > 0.5);
    }

    #[test]
    fn sub_frame_spawn_interpolates_between_prev_and_cur() {
        let cfg = EmitterConfig {
            emission_rate: 600.0, // 10 per 1/60 s
            speed_min: 0.0,
            speed_max: 0.0,
            gravity: [0.0; 3],
            lifetime_min: 10.0,
            lifetime_max: 10.0,
            ..Default::default()
        };
        let mut s = quiet(cfg);
        let frame = EmitterFrame {
            position: [10.0, 0.0, 0.0],
            prev_position: [0.0, 0.0, 0.0],
            ..Default::default()
        };
        run(&mut s, &frame, 1.0 / 60.0, 1);
        assert_eq!(s.pool.alive_count(), 10);
        let xs: Vec<f32> = s.pool.alive_slice().iter().map(|p| p.position[0]).collect();
        assert!(xs.iter().all(|x| (0.0..=10.0).contains(x)));
        let min = xs.iter().cloned().fold(f32::MAX, f32::min);
        let max = xs.iter().cloned().fold(f32::MIN, f32::max);
        assert!(
            min < 2.0 && max > 8.0,
            "spawns spread along the path: {xs:?}"
        );
    }

    #[test]
    fn emission_per_meter_spawns_along_path() {
        let cfg = EmitterConfig {
            emission_rate: 0.0,
            emission_per_meter: 2.0,
            speed_min: 0.0,
            speed_max: 0.0,
            gravity: [0.0; 3],
            lifetime_min: 10.0,
            lifetime_max: 10.0,
            ..Default::default()
        };
        let mut s = quiet(cfg);
        let frame = EmitterFrame {
            position: [5.0, 0.0, 0.0],
            prev_position: [0.0, 0.0, 0.0],
            ..Default::default()
        };
        run(&mut s, &frame, 1.0 / 60.0, 1);
        assert_eq!(s.pool.alive_count(), 10);
        let still = EmitterFrame {
            position: [5.0, 0.0, 0.0],
            prev_position: [5.0, 0.0, 0.0],
            ..Default::default()
        };
        run(&mut s, &still, 1.0 / 60.0, 5);
        assert_eq!(s.pool.alive_count(), 10, "no motion → no spawns");
    }

    #[test]
    fn inherit_velocity_adds_emitter_motion() {
        let cfg = EmitterConfig {
            emission_rate: 0.0,
            speed_min: 0.0,
            speed_max: 0.0,
            gravity: [0.0; 3],
            inherit_velocity: 0.5,
            ..Default::default()
        };
        let mut s = quiet(cfg);
        s.pending_burst = 1;
        let frame = EmitterFrame {
            velocity: [4.0, 0.0, 0.0],
            ..Default::default()
        };
        run(&mut s, &frame, 1.0 / 60.0, 1);
        let v = s.pool.alive_slice()[0].velocity;
        assert!((v[0] - 2.0).abs() < 1e-4, "{v:?}");
    }

    #[test]
    fn local_axes_rotate_direction() {
        // Basis rotating +Y onto +X.
        let basis = [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let cfg = EmitterConfig {
            emission_rate: 0.0,
            speed_min: 1.0,
            speed_max: 1.0,
            spread: 0.0,
            gravity: [0.0; 3],
            local_axes: true,
            ..Default::default()
        };
        let mut s = quiet(cfg);
        s.pending_burst = 1;
        let frame = EmitterFrame {
            basis,
            ..Default::default()
        };
        run(&mut s, &frame, 0.0, 1);
        let v = s.pool.alive_slice()[0].velocity;
        assert!((v[0] - 1.0).abs() < 1e-4 && v[1].abs() < 1e-4, "{v:?}");
    }

    #[test]
    fn on_death_requests_and_apply_spawn_into_target() {
        let parent = EmitterConfig {
            name: "parent".into(),
            emission_rate: 0.0,
            lifetime_min: 0.05,
            lifetime_max: 0.05,
            on_death: Some(SubEmitter {
                emitter: "child".into(),
                count_min: 3,
                count_max: 3,
                inherit_velocity: 1.0,
            }),
            ..Default::default()
        };
        let child = EmitterConfig {
            name: "child".into(),
            emission_rate: 0.0,
            lifetime_min: 10.0,
            lifetime_max: 10.0,
            ..Default::default()
        };
        let mut emitters = vec![quiet(parent), quiet(child)];
        emitters[0].on_death_target = Some(1);
        emitters[0].pending_burst = 2;
        let frame = EmitterFrame::default();
        let mut budget = 1000;
        let mut reqs = Vec::new();
        step_emitter(&mut emitters[0], 0, &frame, 0.1, &mut budget, &mut reqs);
        assert_eq!(reqs.len(), 2);
        assert_eq!(emitters[0].pool.alive_count(), 0);
        let mut next = Vec::new();
        apply_spawn_requests(&mut emitters, &reqs, &frame, 0.1, &mut budget, &mut next);
        assert_eq!(emitters[1].pool.alive_count(), 6);
        assert!(next.is_empty());
    }

    #[test]
    fn budget_caps_spawns() {
        let cfg = EmitterConfig {
            emission_rate: 0.0,
            ..Default::default()
        };
        let mut s = quiet(cfg);
        s.pending_burst = 100;
        let frame = EmitterFrame::default();
        let mut budget = 7;
        let mut reqs = Vec::new();
        step_emitter(&mut s, 0, &frame, 1.0 / 60.0, &mut budget, &mut reqs);
        assert_eq!(s.pool.alive_count(), 7);
        assert_eq!(budget, 0);
    }

    #[test]
    fn curves_drive_size_color_and_frames() {
        let cfg = EmitterConfig {
            emission_rate: 0.0,
            gravity: [0.0; 3],
            lifetime_min: 1.0,
            lifetime_max: 1.0,
            size: Curve::start_end([1.0, 2.0], [3.0, 4.0]),
            color: Curve::start_end([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 0.0]),
            alpha: Some(Curve::constant(0.5)),
            frames_x: 4,
            frames_y: 1,
            frame_rate: 10.0,
            ..Default::default()
        };
        let mut s = quiet(cfg);
        s.pending_burst = 1;
        let frame = EmitterFrame::default();
        run(&mut s, &frame, 0.0, 1);
        run(&mut s, &frame, 0.5, 1);
        let p = &s.pool.alive_slice()[0];
        assert!((p.size[0] - 2.0).abs() < 1e-4 && (p.size[1] - 3.0).abs() < 1e-4);
        assert!((p.color[0] - 0.5).abs() < 1e-4 && (p.color[3] - 0.25).abs() < 1e-4);
        assert_eq!(p.frame, 1); // 0.5 s × 10 fps = frame 5 → mod 4 = 1
    }

    #[test]
    fn vortex_keeps_particles_circling() {
        let cfg = EmitterConfig {
            emission_rate: 0.0,
            gravity: [0.0; 3],
            speed_min: 0.0,
            speed_max: 0.0,
            lifetime_min: 100.0,
            lifetime_max: 100.0,
            forces: vec![Force::Vortex {
                center: [0.0; 3],
                axis: [0.0, 1.0, 0.0],
                strength: 1.0,
                falloff: 0.0,
            }],
            ..Default::default()
        };
        let mut s = quiet(cfg);
        {
            let p = s.pool.spawn().unwrap();
            p.position = [1.0, 0.0, 0.0];
            p.lifetime = 100.0;
        }
        let frame = EmitterFrame::default();
        run(&mut s, &frame, 1.0 / 120.0, 60);
        let p = &s.pool.alive_slice()[0];
        assert!(
            p.position[2].abs() > 0.05,
            "moved tangentially: {:?}",
            p.position
        );
        assert!(p.position[1].abs() < 1e-4, "stays in the plane");
    }
}
