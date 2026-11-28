//! Physics' subsystem (#1617): built for every scene of a run with the
//! package, it does at the kernel's stages what the player's frame called
//! physics for by name until then, in the same order (the module's table,
//! `super`).

use super::colliders::{self, Colliders};
use super::module::{Velocity2D, PHYSICS, PHYSICS_2D};
use super::nav_module::NAV;
use crate::{of_mut, CollisionLayers, PhysicsSystem, StaticHeightfield};
use flint_core::frame_notes::{self, NoteKind};
use flint_core::EntityId;
use flint_package::surfaces::{MadeTriangles, Surfaces};
use flint_package::{FixedCtx, FrameCtx, SceneCtx, Stage, StageCtx, StepStage, Subsystem};
use flint_render::FrameInputs;
use flint_script::context::ScriptCommand;

/// The scene's physics at the kernel's stages.
#[derive(Default)]
pub struct PhysicsSubsystem {
    /// Built for a capture, which steps no physics: it only enables the
    /// modules
    capture: bool,
    /// Each `ocean_contact` hull's velocity this frame (#749), lent to the
    /// renderer's inputs
    contact_velocities: Vec<(EntityId, [f32; 3])>,
}

/// The subsystem for the scene being entered: always one, since a scene's
/// scripts may cast rays whatever it holds. In the player the scene's
/// bodies are made here; in a capture nothing is (the first-person
/// package says who the player is there, #1618).
pub(crate) fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    if ctx.capture.is_some() {
        return Some(Box::new(PhysicsSubsystem {
            capture: true,
            ..Default::default()
        }));
    }
    enter_scene(ctx);
    Some(Box::new(PhysicsSubsystem::default()))
}

/// The scene's bodies, in the order the player made them before #1617:
/// the host's static surfaces (the splines' swept meshes, then each
/// terrain's heightfield), the project's layers and the mesh colliders'
/// triangles, then every body and joint. Who the player is comes after,
/// from the first-person package's builder (#1618), which the registry
/// orders after physics'.
fn enter_scene(ctx: &mut SceneCtx) {
    let surfaces = ctx.resources.remove::<Surfaces>().unwrap_or_default();
    if let Some(physics) = of_mut(ctx.resources) {
        for mesh in surfaces.meshes {
            physics.register_static_trimesh(
                mesh.entity,
                mesh.vertices,
                mesh.triangles,
                mesh.friction,
                mesh.restitution,
            );
        }
        for grid in surfaces.grids {
            let field = StaticHeightfield {
                columns: grid.columns,
                rows: grid.rows,
                heights: grid.heights,
                width: grid.width,
                depth: grid.depth,
                origin: grid.origin,
            };
            physics.register_static_heightfield(
                grid.entity,
                &field,
                grid.friction,
                grid.restitution,
            );
        }
    }
    // The project's collision layers and the mesh colliders' triangles,
    // before physics makes the scene's bodies (each a frame note, #1092)
    frame_notes::timed(NoteKind::PhysicsBuild, String::new, || prepare(ctx));
    let Some(physics) = of_mut(ctx.resources) else {
        return;
    };
    frame_notes::timed(NoteKind::PhysicsBuild, String::new, || {
        if let Err(e) = physics.initialize(ctx.world) {
            tracing::warn!("Failed to initialize physics: {e:#}");
        }
    });
}

/// The project's layers, then the triangles of the mesh colliders the last
/// `scene_read` found: a made mesh's, else its model file's. A model a
/// maker is still making waits quietly for its triangles.
fn prepare(ctx: &mut SceneCtx) {
    let layers = CollisionLayers::new(&ctx.project.physics_layers).unwrap_or_else(|e| {
        // The manifest's reader refuses these first; a project made in
        // code might not have been
        tracing::warn!("physics layers: {e}; only `default` is used");
        CollisionLayers::default()
    });
    let Some(physics) = of_mut(ctx.resources) else {
        return;
    };
    physics.set_layers(layers);
    // A new scene's (or chunk's) colliders are walked at the next frame,
    // for the runtime meshes the renderer already holds
    let colliders = ctx.resources.get_or_insert_with(Colliders::default);
    colliders.walk_again();
    let requests = std::mem::take(&mut colliders.requests);
    if requests.is_empty() {
        return;
    }
    let Some(config) = ctx.model_config else {
        return;
    };
    // A model still being made (procgen's queue, `ProcGenMode::Deferred`)
    // has no triangles yet: they come when it's made
    let (queued, requests): (Vec<_>, Vec<_>) = {
        let resources: &flint_package::Resources = ctx.resources;
        let makers: Vec<_> = ctx.registry.asset_makers_in(ctx.packages).collect();
        requests
            .into_iter()
            .partition(|(_, asset)| makers.iter().any(|maker| maker.pending(resources, asset)))
    };
    let made = ctx.resources.remove::<MadeTriangles>().unwrap_or_default();
    if let Some(physics) = of_mut(ctx.resources) {
        for (entity, _) in &queued {
            physics.expect_collider_mesh(*entity);
        }
        colliders::hand_over(physics, &requests, config, &made);
    }
    ctx.resources.insert(made);
}

impl PhysicsSubsystem {
    /// A ground that changed while the scene plays (#1697: the terrain
    /// package rebaked its recipe and put the kernel's `Surfaces` in
    /// again): each grid's entity's heightfield made again, the old one
    /// removed first. Nothing to do on nearly every frame, one map lookup.
    fn refresh_surfaces(ctx: &mut StageCtx) {
        let Some(surfaces) = ctx.resources.remove::<Surfaces>() else {
            return;
        };
        let Some(physics) = of_mut(ctx.resources) else {
            return;
        };
        for grid in surfaces.grids {
            physics.remove_entity(grid.entity);
            let field = StaticHeightfield {
                columns: grid.columns,
                rows: grid.rows,
                heights: grid.heights,
                width: grid.width,
                depth: grid.depth,
                origin: grid.origin,
            };
            physics.register_static_heightfield(
                grid.entity,
                &field,
                grid.friction,
                grid.restitution,
            );
        }
    }

    /// The made and runtime meshes' triangles that came since the last
    /// frame (a procgen model the queue finished, a mesh an extension
    /// uploaded), for the bodies whose `mesh` or `convex` collider names
    /// them (#1222). One comparison when nothing moved.
    fn refresh_meshes(ctx: &mut StageCtx) {
        let renderer = ctx.renderer.as_deref();
        let made_at = ctx
            .resources
            .get::<MadeTriangles>()
            .map_or(0, MadeTriangles::generation);
        let due = ctx
            .resources
            .get::<Colliders>()
            .is_some_and(|c| c.due(ctx.world, renderer, made_at));
        if !due {
            return;
        }
        let made = ctx.resources.remove::<MadeTriangles>().unwrap_or_default();
        if let Some((colliders, physics)) = ctx.resources.get_two_mut::<Colliders, PhysicsSystem>()
        {
            colliders.refresh(ctx.world, physics, renderer, &made);
        }
        ctx.resources.insert(made);
    }
}

impl Subsystem for PhysicsSubsystem {
    fn name(&self) -> &'static str {
        super::PACKAGE_NAME
    }

    fn script_modules(&self) -> &[&'static str] {
        &[PHYSICS, PHYSICS_2D, NAV]
    }

    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        match stage {
            // The frame's share before its steps, against the poses and
            // joint targets the scripts just wrote (a controller's look
            // comes after, at the same stage: first-person's, #1618)
            Stage::Scripts => {
                if !self.capture {
                    Self::refresh_meshes(ctx);
                    Self::refresh_surfaces(ctx);
                }
                let ratio = ctx.clock.pending_fixed_ratio();
                let Some(physics) = of_mut(ctx.resources) else {
                    return;
                };
                physics.begin_frame(ratio);
            }
            // A hull on a dynamic body foams with the velocity the solver
            // gave it (#749); the renderer differences the rest's motion
            Stage::MadeAssets if !self.capture => {
                self.contact_velocities.clear();
                if let Some(physics) = crate::of(ctx.resources) {
                    for &id in ctx
                        .world
                        .entities_with_component(flint_geometry::components::OCEAN_CONTACT)
                    {
                        if let Some(v) = physics.dynamic_velocity(id) {
                            self.contact_velocities.push((id, v));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn step(&mut self, stage: StepStage, ctx: &mut FixedCtx) {
        let Some(physics) = of_mut(ctx.resources) else {
            return;
        };
        // A controller walks at `Control`, before the sync-in (first-person's,
        // #1618)
        match stage {
            StepStage::SyncIn => {
                flint_core::scope!("physics::fixed_update");
                physics.sync_in(ctx.world);
            }
            StepStage::Integrate => {
                flint_core::scope!("physics::fixed_update");
                physics.step_and_publish(ctx.world, ctx.dt, ctx.events);
            }
            _ => {}
        }
    }

    /// `set_velocity_2d`, at the drain the core command was taken at.
    fn script_command(&mut self, command: &ScriptCommand, ctx: &mut StageCtx) -> bool {
        let ScriptCommand::Module(command) = command else {
            return false;
        };
        let Some(v) = command.payload::<Velocity2D>() else {
            return false;
        };
        if let Some(physics) = of_mut(ctx.resources) {
            physics.set_velocity_2d(v.entity, v.vx as f32, v.vy as f32);
        }
        true
    }

    fn frame_inputs<'s>(&'s self, _ctx: &FrameCtx, inputs: &mut FrameInputs<'s>) {
        inputs.contact_velocities = &self.contact_velocities;
    }

    /// A chunk's bodies: its mesh colliders' triangles first (#729), then
    /// every new entity.
    fn chunk_loaded(&mut self, spawned: &[EntityId], ctx: &mut SceneCtx) {
        if self.capture {
            return;
        }
        prepare(ctx);
        if let Some(physics) = of_mut(ctx.resources) {
            physics.sync_new_entities(ctx.world, spawned);
        }
    }

    fn chunk_unloading(&mut self, ids: &[EntityId], ctx: &mut SceneCtx) {
        if let Some(physics) = of_mut(ctx.resources) {
            for id in ids {
                physics.remove_entity(*id);
            }
        }
    }

    /// The scene is going: its world and bodies, all at once.
    /// The run's layers and made triangles stay.
    fn scene_unloading(&mut self, ctx: &mut SceneCtx) {
        if let Some(physics) = of_mut(ctx.resources) {
            physics.clear();
        }
    }
}
