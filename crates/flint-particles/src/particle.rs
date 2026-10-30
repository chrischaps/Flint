//! Particle types: CPU simulation state and GPU instance data

use bytemuck::{Pod, Zeroable};

/// CPU-side particle state (not sent to GPU)
#[derive(Clone, Debug)]
pub struct Particle {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub age: f32,
    pub lifetime: f32,
    /// Current per-axis size (x = width, y = height) after curves.
    pub size: [f32; 2],
    /// Per-particle random size multiplier chosen at birth.
    pub size_scale: f32,
    /// Current rotation in radians.
    pub rotation: f32,
    /// Spin in radians per second.
    pub angular_velocity: f32,
    /// Current RGBA after curves, brightness and alpha.
    pub color: [f32; 4],
    /// Per-particle brightness multiplier chosen at birth.
    pub brightness: f32,
    /// Current sprite-sheet frame.
    pub frame: u32,
    /// Starting frame (random start frame support).
    pub frame_offset: u32,
    /// Stable per-particle random in [0, 1) — jitter, shader hooks.
    pub random: f32,
    pub alive: bool,
}

impl Particle {
    pub fn dead() -> Self {
        Self {
            position: [0.0; 3],
            velocity: [0.0; 3],
            age: 0.0,
            lifetime: 0.0,
            size: [0.0; 2],
            size_scale: 1.0,
            rotation: 0.0,
            angular_velocity: 0.0,
            color: [0.0; 4],
            brightness: 1.0,
            frame: 0,
            frame_offset: 0,
            random: 0.0,
            alive: false,
        }
    }

    /// Normalized age in [0, 1]
    pub fn age_ratio(&self) -> f32 {
        if self.lifetime <= 0.0 {
            1.0
        } else {
            (self.age / self.lifetime).min(1.0)
        }
    }
}

/// GPU instance data — matches WGSL `ParticleInstance` struct.
/// 64 bytes, 16-byte aligned (4 rows of vec4). This is the single
/// sim → GPU contract; `flint-render` re-exports this exact type
/// (ADR 0068), so a future compute path only has to write the same layout.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable, PartialEq)]
pub struct ParticleInstance {
    /// xyz = world position, w = width (size.x)
    pub pos_size: [f32; 4],
    /// rgba tint
    pub color: [f32; 4],
    /// x = rotation, y = frame, z = frames_x, w = frames_y
    pub rotation_frame: [f32; 4],
    /// xyz = velocity pre-scaled by the emitter's stretch factor (all zero
    /// disables velocity-aligned stretching); w = height (size.y).
    pub vel_stretch: [f32; 4],
}

impl ParticleInstance {
    pub fn from_particle(p: &Particle, frames_x: u32, frames_y: u32, stretch: f32) -> Self {
        Self {
            pos_size: [p.position[0], p.position[1], p.position[2], p.size[0]],
            color: p.color,
            rotation_frame: [p.rotation, p.frame as f32, frames_x as f32, frames_y as f32],
            vel_stretch: [
                p.velocity[0] * stretch,
                p.velocity[1] * stretch,
                p.velocity[2] * stretch,
                p.size[1],
            ],
        }
    }
}

/// Swap-remove pool for O(1) particle kill and contiguous alive iteration.
pub struct ParticlePool {
    particles: Vec<Particle>,
    alive_count: usize,
}

impl ParticlePool {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let mut particles = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            particles.push(Particle::dead());
        }
        Self {
            particles,
            alive_count: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.particles.len()
    }

    pub fn alive_count(&self) -> usize {
        self.alive_count
    }

    pub fn is_full(&self) -> bool {
        self.alive_count >= self.particles.len()
    }

    /// Kill every particle.
    pub fn clear(&mut self) {
        for p in &mut self.particles[..self.alive_count] {
            p.alive = false;
        }
        self.alive_count = 0;
    }

    /// Spawn one particle, returning a mutable ref to initialize it.
    /// Returns None if pool is full.
    pub fn spawn(&mut self) -> Option<&mut Particle> {
        if self.alive_count >= self.particles.len() {
            return None;
        }
        let idx = self.alive_count;
        let p = &mut self.particles[idx];
        *p = Particle::dead();
        p.alive = true;
        self.alive_count += 1;
        Some(p)
    }

    /// Iterate alive particles, kill expired ones via swap-remove.
    pub fn update_and_compact(&mut self) {
        let mut i = 0;
        while i < self.alive_count {
            if !self.particles[i].alive || self.particles[i].age >= self.particles[i].lifetime {
                self.particles[i].alive = false;
                self.alive_count -= 1;
                if i < self.alive_count {
                    self.particles.swap(i, self.alive_count);
                }
                // Don't increment i — the swapped-in particle needs checking
            } else {
                i += 1;
            }
        }
    }

    /// Access alive particles slice for reading (first `alive_count` elements)
    pub fn alive_slice(&self) -> &[Particle] {
        &self.particles[..self.alive_count]
    }

    /// Access alive particles mutably
    pub fn alive_slice_mut(&mut self) -> &mut [Particle] {
        &mut self.particles[..self.alive_count]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_spawn_and_kill() {
        let mut pool = ParticlePool::new(4);
        assert_eq!(pool.alive_count(), 0);

        // Spawn 3 particles
        for i in 0..3 {
            let p = pool.spawn().unwrap();
            p.lifetime = 1.0;
            p.age = 0.0;
            p.position[0] = i as f32;
        }
        assert_eq!(pool.alive_count(), 3);

        // Kill the middle one by aging it past lifetime
        pool.alive_slice_mut()[1].age = 2.0;
        pool.update_and_compact();
        assert_eq!(pool.alive_count(), 2);

        // Pool full at capacity 4 — spawn should fail after 4
        pool.spawn().unwrap();
        pool.spawn().unwrap();
        assert!(pool.spawn().is_none());
        assert!(pool.is_full());

        pool.clear();
        assert_eq!(pool.alive_count(), 0);
    }

    #[test]
    fn particle_instance_layout() {
        assert_eq!(std::mem::size_of::<ParticleInstance>(), 64);
        assert_eq!(std::mem::align_of::<ParticleInstance>(), 4);
    }

    #[test]
    fn instance_packs_per_axis_size_and_stretch_flag() {
        let mut p = Particle::dead();
        p.size = [0.5, 2.0];
        p.velocity = [1.0, 0.0, 0.0];
        let a = ParticleInstance::from_particle(&p, 1, 1, 0.0);
        assert_eq!(a.pos_size[3], 0.5);
        assert_eq!(a.vel_stretch, [0.0, 0.0, 0.0, 2.0]);
        let b = ParticleInstance::from_particle(&p, 4, 2, 0.1);
        assert!((b.vel_stretch[0] - 0.1).abs() < 1e-6);
        assert_eq!(b.rotation_frame[2], 4.0);
        assert_eq!(b.rotation_frame[3], 2.0);
    }
}
