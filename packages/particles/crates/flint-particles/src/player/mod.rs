//! The particles package's player part: what a launcher lists
//! ([`PACKAGE`]). The run's [`ParticleSystem`](crate::ParticleSystem) is a
//! resource it puts in; its subsystem loads each scene's effects into it
//! and steps it; its script module and F3 panel are registered with it.
//!
//! The frame (#1622), each where the player ran it before the package was
//! one:
//!
//! | when | what |
//! |---|---|
//! | the scene's build | the effects the scene sees, the world's emitters, their textures |
//! | `bones_posed` (the host) | the motion-hold clock, through the kernel's point |
//! | `Stage::Animation` | one step, while the game state runs `particles` |
//! | before `prepare` (the host) | packed from the camera and drawn, through the kernel's point |
//! | `scene_unloading` | every instance cleared |
//!
//! A capture (`flint render --time`, `--particle-time`) steps its particles
//! on its own fixed step through the same point, and its subsystem only
//! takes the scripts' commands.

mod module;
mod panel;
mod subsystem;

use crate::{ParticleSystem, PACKAGE_NAME, POLICY};
use flint_core::components as comp;
use flint_package::{Package, Registry, Resources, SceneCtx};

pub use module::{Particle, ParticleModule, PARTICLE_MODULE};
pub use panel::{EmitterRow, ParticlePanelAction, ParticlesDebugPanel, PARTICLES_DEBUG_PANEL};
pub use subsystem::Particles;

/// The `particles` package (`packages/particles/package.toml`).
pub static PACKAGE: ParticlesPackage = ParticlesPackage;

pub struct ParticlesPackage;

impl Package for ParticlesPackage {
    fn name(&self) -> &'static str {
        PACKAGE_NAME
    }

    /// The run's system, its builder (every scene has one), its policy
    /// name, its line for a project that leaves it out, its script module,
    /// the kernel's particles point and its panel at the kernel's point for
    /// a scene's panels (made only where the player has the debug HUD).
    fn register(&self, registry: &mut Registry) {
        registry
            .run_resources(put)
            .scene_subsystem(subsystem::for_scene)
            .policy(POLICY)
            .when_left_out(left_out)
            .script_modules(&[&PARTICLE_MODULE])
            .insert_point(crate::point());
        if let Some(panels) = registry.point_mut::<flint_package::panels::ScenePanels>() {
            panels.0.push((PACKAGE_NAME, panel::for_scene));
        }
    }
}

/// The run's system, made once a run as the player's core made it.
fn put(resources: &mut Resources) {
    resources.insert(ParticleSystem::new());
}

/// What a scene with emitters hears when the project's `[packages]` leaves
/// `particles` out.
fn left_out(ctx: &SceneCtx) -> Option<String> {
    let in_use = ctx.packages?;
    let uses: Vec<&str> = [comp::PARTICLE_EMITTER, comp::PARTICLE_EFFECT]
        .into_iter()
        .filter(|c| !ctx.world.entities_with_component(c).is_empty())
        .collect();
    flint_package::left_out_line(in_use, PACKAGE_NAME, &uses, "emit nothing")
}
