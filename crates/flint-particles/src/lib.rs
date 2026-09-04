//! Flint Particles - GPU-instanced particle system
//!
//! Provides pooled per-emitter particle simulation with:
//! - CPU-side integration with gravity, exponential damping and forces
//!   (wind, drag, noise turbulence, vortex, attractor)
//! - Multi-key curves for size, colour, alpha and speed over lifetime
//! - Burst timelines, emission over distance, velocity inheritance
//! - Sub-emitters on birth and death
//! - Reusable `*.particles.toml` effect assets (`particle_effect` component)
//!   alongside the inline `particle_emitter` component
//! - Deterministic simulation: per-emitter seeded RNG, ordered instances,
//!   fixed-step [`ParticleSystem::simulate_to`] for headless snapshots
//! - GPU instance packing (one shared 64-byte [`ParticleInstance`] type)
//!
//! Design record: ADR 0068.

pub mod curves;
pub mod effect;
pub mod emitter;
pub mod loader;
pub mod noise;
pub mod particle;
pub mod rand;
pub mod sim;
pub mod sync;

use flint_core::Result;
use flint_ecs::FlintWorld;
use flint_runtime::RuntimeSystem;

pub use curves::{Curve, Interp};
pub use effect::{
    BurstDef, CountDef, CurveDef, EmitterDef, ForceDef, Key, ParticleEffect, RangeDef,
    ResolveContext, ShapeDef, SubEmitterDef,
};
pub use emitter::{
    Burst, EmissionShape, EmitterConfig, EmitterState, Force, ParticleBlendMode, SortMode,
    SubEmitter,
};
pub use loader::{
    load_effect_from_file, load_effect_from_str, load_effects_from_dir,
    load_particle_effects_from_world, resolve_particles_dir, texture_search_dirs, EFFECT_SUFFIX,
};
pub use particle::{Particle, ParticleInstance, ParticlePool};
pub use sim::EmitterFrame;
pub use sync::{EffectInstance, EffectSource, InstanceKey, ParticleDrawData, ParticleSync};

/// Default RNG seed for systems created with [`ParticleSystem::new`].
pub const DEFAULT_SEED: u32 = 0xDEAD_BEEF;

/// The particle system — implements RuntimeSystem for integration with the
/// game loop, and exposes explicit stepping for editors and headless renders.
pub struct ParticleSystem {
    pub sync: ParticleSync,
    seed: u32,
}

impl ParticleSystem {
    pub fn new() -> Self {
        Self::with_seed(DEFAULT_SEED)
    }

    /// Create a system whose instances derive their RNG streams from `seed`.
    pub fn with_seed(seed: u32) -> Self {
        Self {
            sync: ParticleSync::new(),
            seed,
        }
    }

    pub fn seed(&self) -> u32 {
        self.seed
    }

    /// Clear all particle state for a scene transition.
    pub fn clear(&mut self) {
        self.sync.clear();
    }

    /// One simulation step: pick up component changes, then integrate.
    /// Does not pack instances — call [`pack`](Self::pack) once per frame
    /// with the camera position after all stepping is done.
    pub fn step(&mut self, world: &FlintWorld, dt: f32) {
        self.sync.sync_from_world(world);
        self.sync.update(dt);
    }

    /// Step detached / already-synced instances without re-reading the
    /// world (editors driving instances directly).
    pub fn step_detached(&mut self, dt: f32) {
        self.sync.update(dt);
    }

    /// Deterministically advance to `seconds` from the current state in
    /// fixed `dt` steps (the count is rounded, so `simulate_to(1.0, 1/60)`
    /// always takes exactly 60 steps).
    pub fn simulate_to(&mut self, world: &FlintWorld, seconds: f32, dt: f32) {
        let dt = dt.max(1e-4);
        let steps = (seconds / dt).round().max(0.0) as u32;
        for _ in 0..steps {
            self.step(world, dt);
        }
    }

    /// Pack alive particles for upload. `camera_pos` enables
    /// back-to-front sorting and per-emitter distance keys.
    pub fn pack(&mut self, camera_pos: Option<[f32; 3]>) {
        self.sync.pack_instances(camera_pos);
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
            println!(
                "[particles] Discovered {count} emitter(s) across {} effect instance(s)",
                self.sync.instance_count()
            );
        }
        Ok(())
    }

    fn fixed_update(&mut self, _world: &mut FlintWorld, _dt: f64) -> Result<()> {
        // Particles are purely visual — no fixed-step needed
        Ok(())
    }

    fn update(&mut self, world: &mut FlintWorld, dt: f64) -> Result<()> {
        self.step(world, dt as f32);
        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }

    fn name(&self) -> &str {
        "particles"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::components as comp;

    #[test]
    fn simulate_to_is_deterministic_and_step_counted() {
        let build = || {
            let mut world = FlintWorld::new();
            let id = flint_core::EntityId(77);
            world.spawn_with_id(id, "fx").unwrap();
            let t: toml::Value =
                toml::from_str("emission_rate = 120.0\nlifetime_min = 0.5\nlifetime_max = 1.0")
                    .unwrap();
            world.set_component(id, comp::PARTICLE_EMITTER, t).unwrap();
            world
        };
        let world_a = build();
        let world_b = build();
        let mut a = ParticleSystem::with_seed(1);
        let mut b = ParticleSystem::with_seed(1);
        a.simulate_to(&world_a, 1.0, 1.0 / 60.0);
        b.simulate_to(&world_b, 1.0, 1.0 / 60.0);
        assert!((a.sync.time() - 1.0).abs() < 1e-4);
        a.pack(Some([0.0, 1.0, 5.0]));
        b.pack(Some([0.0, 1.0, 5.0]));
        assert!(!a.sync.instance_data().is_empty());
        assert_eq!(a.sync.instance_data(), b.sync.instance_data());
    }
}
