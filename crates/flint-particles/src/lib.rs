//! Flint Particles - GPU-instanced particle system
//!
//! Provides pooled per-emitter particle simulation with:
//! - CPU-side position/velocity/lifetime integration
//! - Swap-remove particle pool for O(1) kill
//! - GPU instance packing for instanced draw calls
//! - Configurable emission shapes, gravity, damping, size/color over lifetime

pub mod curves;
pub mod emitter;
pub mod particle;
pub mod rand;
pub mod sync;

use flint_core::Result;
use flint_ecs::FlintWorld;
use flint_runtime::RuntimeSystem;

pub use emitter::{EmitterConfig, EmissionShape, ParticleBlendMode};
pub use particle::{ParticleInstance, ParticlePool};
pub use sync::{ParticleDrawData, ParticleSync};

/// The particle system — implements RuntimeSystem for integration with the game loop.
pub struct ParticleSystem {
    pub sync: ParticleSync,
    rng: rand::ParticleRng,
}

impl ParticleSystem {
    pub fn new() -> Self {
        // Seed from a simple hash of the timestamp-ish value
        let seed = 0xDEAD_BEEF_u32;
        Self {
            sync: ParticleSync::new(),
            rng: rand::ParticleRng::new(seed),
        }
    }
}

impl Default for ParticleSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeSystem for ParticleSystem {
    fn initialize(&mut self, world: &mut FlintWorld) -> Result<()> {
        self.sync.sync_from_world(world);
        let count = self.sync.emitter_count();
        if count > 0 {
            println!("[particles] Discovered {count} emitter(s)");
        }
        Ok(())
    }

    fn fixed_update(&mut self, _world: &mut FlintWorld, _dt: f64) -> Result<()> {
        // Particles are purely visual — no fixed-step needed
        Ok(())
    }

    fn update(&mut self, world: &mut FlintWorld, dt: f64) -> Result<()> {
        self.sync.sync_from_world(world);
        self.sync.update(&mut self.rng, dt as f32);
        self.sync.pack_instances();
        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }

    fn name(&self) -> &str {
        "particles"
    }
}
