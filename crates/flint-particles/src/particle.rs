//! Particle types: CPU simulation state and GPU instance data

use bytemuck::{Pod, Zeroable};

/// CPU-side particle state (not sent to GPU)
#[derive(Clone)]
pub struct Particle {
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub age: f32,
    pub lifetime: f32,
    pub size: f32,
    pub rotation: f32,
    pub color: [f32; 4],
    pub frame: u32,
    pub alive: bool,
}

impl Particle {
    pub fn dead() -> Self {
        Self {
            position: [0.0; 3],
            velocity: [0.0; 3],
            age: 0.0,
            lifetime: 0.0,
            size: 0.0,
            rotation: 0.0,
            color: [0.0; 4],
            frame: 0,
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
/// 48 bytes, 16-byte aligned (3 rows of vec4).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ParticleInstance {
    /// World position + size packed into vec4
    pub pos_size: [f32; 4],    // xyz = position, w = size
    /// Color with alpha
    pub color: [f32; 4],       // rgba
    /// Rotation, sprite frame, sprite sheet dimensions
    pub rotation_frame: [f32; 4], // x = rotation, y = frame, z = frames_x, w = frames_y
}

impl ParticleInstance {
    pub fn from_particle(p: &Particle, frames_x: u32, frames_y: u32) -> Self {
        Self {
            pos_size: [p.position[0], p.position[1], p.position[2], p.size],
            color: p.color,
            rotation_frame: [p.rotation, p.frame as f32, frames_x as f32, frames_y as f32],
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

    /// Spawn one particle, returning a mutable ref to initialize it.
    /// Returns None if pool is full.
    pub fn spawn(&mut self) -> Option<&mut Particle> {
        if self.alive_count >= self.particles.len() {
            return None;
        }
        let idx = self.alive_count;
        self.particles[idx].alive = true;
        self.alive_count += 1;
        Some(&mut self.particles[idx])
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
    }

    #[test]
    fn particle_instance_layout() {
        assert_eq!(std::mem::size_of::<ParticleInstance>(), 48);
        assert_eq!(std::mem::align_of::<ParticleInstance>(), 4);
    }
}
