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

#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "player")]
pub mod player;

use flint_core::Result;
use flint_ecs::FlintWorld;
use flint_runtime::RuntimeSystem;

pub use curves::{Curve, Interp};
pub use effect::{
    BurstDef, CountDef, CurveDef, EmitterDef, EmitterProblem, ForceDef, Key, ParticleEffect,
    RangeDef, ResolveContext, ShapeDef, SubEmitterDef,
};
pub use emitter::{
    Burst, EmissionShape, EmitterConfig, EmitterState, Force, ParticleBlendMode, SortMode,
    SubEmitter,
};
pub use loader::{
    load_effect_from_file, load_effect_from_str, load_effects_from_dir,
    load_particle_effects_from_world, load_visible_effects, EFFECT_SUFFIX,
};
pub use particle::{Particle, ParticleInstance, ParticlePool};
pub use sim::EmitterFrame;
pub use sync::{EffectInstance, EffectSource, InstanceKey, ParticleDraw, ParticleSync};

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

/// The package's name, as its manifest gives it.
pub const PACKAGE_NAME: &str = "particles";

/// The name a game state's policy pauses the simulation by (#727): `particles:
/// "pause"`. Registered with the package (ADR 0138 decision 7), and known to a
/// build without it from the manifest's `[without] policies`.
pub const POLICY: &str = PACKAGE_NAME;

/// What a host steps (#1622): every call is the one it made on the system
/// itself before the package was one, so a frame is the same.
impl flint_render::ParticleSource for ParticleSystem {
    fn load(
        &mut self,
        project: &flint_core::project::Project,
        scene_path: &str,
        world: &FlintWorld,
    ) -> Result<()> {
        load_particle_effects_from_world(project, scene_path, self);
        self.sync.sync_from_world(world);
        let count = self.sync.emitter_count();
        if count > 0 {
            tracing::debug!(target: "load",
                "[particles] Discovered {count} emitter(s) across {} effect instance(s)",
                self.sync.instance_count()
            );
        }
        Ok(())
    }

    fn texture_names(&self) -> Vec<String> {
        self.sync.texture_names()
    }

    fn sync_from_world(&mut self, world: &FlintWorld) {
        self.sync.sync_from_world(world);
    }

    fn step(&mut self, world: &FlintWorld, dt: f32) {
        ParticleSystem::step(self, world, dt);
    }

    fn simulate_to(&mut self, world: &FlintWorld, seconds: f32, dt: f32) {
        ParticleSystem::simulate_to(self, world, seconds, dt);
    }

    fn set_motion(&mut self, seconds: f64, whole_hold: Option<f32>) {
        self.sync.set_motion_time(seconds);
        if let Some(whole) = whole_hold {
            self.sync.set_motion_hold(whole);
        }
    }

    fn pack(&mut self, eye: Option<[f32; 3]>) {
        ParticleSystem::pack(self, eye);
    }

    fn draw_data(&self) -> Vec<ParticleDraw<'_>> {
        self.sync.draw_data()
    }

    fn total_alive(&self) -> usize {
        self.sync.total_alive()
    }

    fn emitter_count(&self) -> usize {
        self.sync.emitter_count()
    }

    fn instance_count(&self) -> usize {
        self.sync.instance_count()
    }

    fn entity_effects(&self) -> Vec<(flint_core::EntityId, String)> {
        self.sync
            .instances()
            .filter_map(|(key, inst)| match (&inst.source, key) {
                (EffectSource::Asset(fx), InstanceKey::Entity(id)) => Some((id, fx.name.clone())),
                _ => None,
            })
            .collect()
    }

    fn restart_all(&mut self) {
        self.sync.restart_all();
    }

    fn clear(&mut self) {
        ParticleSystem::clear(self);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// The run's system, among a host's resources.
#[cfg(any(feature = "player", feature = "cli"))]
pub fn of(resources: &flint_package::Resources) -> Option<&ParticleSystem> {
    resources.get::<ParticleSystem>()
}

/// The run's system, to change.
#[cfg(any(feature = "player", feature = "cli"))]
pub fn of_mut(resources: &mut flint_package::Resources) -> Option<&mut ParticleSystem> {
    resources.get_mut::<ParticleSystem>()
}

/// Whether a capture draws particles (`flint_package::particles::Particles::set_drawn`):
/// a run resource of its own, which a capture's subsystem reads.
#[cfg(any(feature = "player", feature = "cli"))]
#[derive(Debug, Default, Clone, Copy)]
pub struct CaptureDrawn(pub bool);

/// The kernel's point, as this package fills it (on the player's registry
/// and on the command line alike).
#[cfg(any(feature = "player", feature = "cli"))]
pub fn point() -> flint_package::particles::Particles {
    use flint_render::ParticleSource;
    fn new() -> Box<dyn ParticleSource> {
        Box::new(ParticleSystem::new())
    }
    fn of_dyn(resources: &flint_package::Resources) -> Option<&dyn ParticleSource> {
        of(resources).map(|p| p as &dyn ParticleSource)
    }
    fn of_mut_dyn(resources: &mut flint_package::Resources) -> Option<&mut dyn ParticleSource> {
        of_mut(resources).map(|p| p as &mut dyn ParticleSource)
    }
    fn set_drawn(resources: &mut flint_package::Resources, drawn: bool) {
        resources.insert(CaptureDrawn(drawn));
    }
    fn emitter_problems(table: &toml::value::Table) -> Vec<flint_package::particles::EmitterIssue> {
        EmitterDef::problems(table)
            .into_iter()
            .map(|p| flint_package::particles::EmitterIssue {
                key: p.key,
                message: p.message,
                unknown: p.unknown,
                hint: p.hint,
            })
            .collect()
    }
    flint_package::particles::Particles {
        new,
        of: of_dyn,
        of_mut: of_mut_dyn,
        set_drawn,
        emitter_problems,
    }
}

impl RuntimeSystem for ParticleSystem {
    fn initialize(&mut self, world: &mut FlintWorld) -> Result<()> {
        self.sync.sync_from_world(world);
        let count = self.sync.emitter_count();
        if count > 0 {
            tracing::debug!(target: "load",
                "[particles] Discovered {count} emitter(s) across {} effect instance(s)",
                self.sync.instance_count()
            );
        }
        Ok(())
    }

    fn update(&mut self, world: &mut FlintWorld, dt: f64) -> Result<()> {
        self.step(world, dt as f32);
        Ok(())
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
