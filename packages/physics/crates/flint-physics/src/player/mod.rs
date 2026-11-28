//! The physics package's player part (#1617, ADR 0138): what a launcher
//! lists ([`PACKAGE`]). Physics' state is the run's resource, a subsystem
//! every scene has does its work at the kernel's stages, and its three
//! script modules read the state through the call's lent resources.
//!
//! The frame, as ADR 0138 decision 2 lays it out:
//!
//! | when | what |
//! |---|---|
//! | `scene_read` (before the models load) | which model each mesh collider names |
//! | the builder (the scene's entry) | the host's surfaces, the project's layers, the mesh colliders' triangles, the bodies (`initialize`), who the player is |
//! | `Stage::Scripts` | the made and runtime meshes' triangles, the frame's span for kinematic bodies (`begin_frame`), the character's look and jump press |
//! | step `Control` | the character's walk |
//! | step `SyncIn` | new bodies and joints, kinematic poses, colliders, joint targets |
//! | step `Integrate` | Rapier's step, the contacts onto the bus, poses back |
//! | `Stage::View` | the player's eye |
//! | `Stage::MadeAssets` | the ocean hulls' velocities, for the frame's foam |
//! | `scene_unloading` | everything cleared |
//!
//! A capture (`flint render --scripts`) steps no physics: its subsystem
//! finds the player by the controller's rule and enables the modules, and
//! nothing else.

mod colliders;
mod module;
#[cfg(test)]
mod module_tests;
mod nav_module;
#[cfg(test)]
mod nav_module_tests;
mod subsystem;

pub use colliders::Colliders;
pub use module::{Velocity2D, PHYSICS, PHYSICS_2D};
pub use nav_module::NAV;
pub use subsystem::PhysicsSubsystem;

use crate::PhysicsSystem;
use flint_core::components as comp;
use flint_package::{Package, Registry, Resources, SceneCtx};

/// The package's name, as its manifest says it.
pub const PACKAGE_NAME: &str = "physics";

/// The `physics` package (`packages/physics/package.toml`).
pub static PACKAGE: PhysicsPackage = PhysicsPackage;

pub struct PhysicsPackage;

impl Package for PhysicsPackage {
    fn name(&self) -> &'static str {
        PACKAGE_NAME
    }

    /// Its state, its look at a scene before the models, its subsystem
    /// and the line for a project that leaves it out, its policy name and
    /// its three modules.
    fn register(&self, registry: &mut Registry) {
        registry
            .run_resources(put)
            .scene_read(colliders::read_requests)
            .scene_subsystem(subsystem::for_scene)
            .when_left_out(left_out)
            .policy(flint_runtime::state::PHYSICS)
            .script_modules(&[
                &module::PHYSICS_MODULE,
                &module::PHYSICS_2D_MODULE,
                &nav_module::NAV_MODULE,
            ]);
    }
}

/// What every run's resources start with: an empty simulation, and the
/// mesh colliders' sources.
fn put(resources: &mut Resources) {
    resources.insert(PhysicsSystem::new());
    resources.insert(Colliders::default());
}

/// What a scene with bodies hears when the project's `[packages]` leaves
/// `physics` out.
fn left_out(ctx: &SceneCtx) -> Option<String> {
    let in_use = ctx.packages?;
    let uses: Vec<&str> = [comp::COLLIDER, comp::RIGIDBODY, comp::JOINT]
        .into_iter()
        .filter(|c| !ctx.world.entities_with_component(c).is_empty())
        .collect();
    flint_package::left_out_line(
        in_use,
        PACKAGE_NAME,
        &uses,
        "stand where they are written: nothing falls, collides or is cast against",
    )
}
