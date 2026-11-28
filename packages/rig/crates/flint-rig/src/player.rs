//! The rig package's player part: what a launcher lists ([`PACKAGE`]).
//!
//! It owns no subsystem of its own. Its two kinds go on the animation
//! package's `ClipKinds` point (animation registers first: rig requires it),
//! so each scene's animation, and each capture's, makes them from the
//! scene's models and plays them at their slots (skeletal, then node, before
//! the flipbook's), and runs the skeletal kind's pose (bone probes, then
//! two-bone IK) at `Stage::Pose`, after its bones go to the GPU. A model
//! spawned later reaches them through the point's `late` hook. A still that
//! steps no animation solves IK once through the `StillPose` point.

use crate::kind::{NodeKind, Rig, SkeletalKind, NODE, SKELETAL};
use crate::node_clip::NodeClip;
use crate::skeletal_clip::SkeletalClip;
use crate::skeleton::Skeleton;
use crate::{IkPass, PACKAGE_NAME};
use flint_animation::player::{ClipKinds, KindCtx, KindEntry, Say};
use flint_animation::{AnimationSystem, ClipKind};
use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_package::{Package, Registry, SceneCtx, StillPose};
use flint_render::model_loader::{LoadedModel, ModelLoadResult};
use std::collections::HashMap;

/// The `rig` package (`packages/rig/package.toml`).
pub static PACKAGE: RigPackage = RigPackage;

pub struct RigPackage;

impl Package for RigPackage {
    fn name(&self) -> &'static str {
        PACKAGE_NAME
    }

    /// Its two kinds on animation's point, the still's IK, and its line
    /// for a project that leaves it out.
    fn register(&self, registry: &mut Registry) {
        match registry.point_mut::<ClipKinds>() {
            Some(kinds) => {
                kinds.0.push(KindEntry {
                    package: PACKAGE_NAME,
                    name: SKELETAL,
                    make: make_skeletal,
                    late: Some(late_models),
                });
                kinds.0.push(KindEntry {
                    package: PACKAGE_NAME,
                    name: NODE,
                    make: make_nodes,
                    late: None,
                });
            }
            None => tracing::warn!(
                "rig: the animation package isn't in this run, so skeletons and nodes play no clips"
            ),
        }
        registry
            .insert_point(StillPose(solve_ik))
            .when_left_out(left_out);
    }
}

/// The scene's skeletal kind: a skeleton for each skinned model, and every
/// skeletal clip the models carry.
fn make_skeletal(ctx: &KindCtx, say: &mut Say) -> Box<dyn ClipKind> {
    let mut system = AnimationSystem::new();
    if let Some(models) = ctx.models {
        let list: Vec<&LoadedModel> = models.models.iter().collect();
        register_skeletal_data(&list, &mut system, &mut |l| say("skeletal_clip", l));
        system.skeletal_mut().add_skinned(
            models
                .skinned_entities
                .iter()
                .map(|(id, a)| (*id, a.clone())),
        );
    }
    take::<SkeletalKind>(system)
}

/// The scene's node kind: every node clip the models carry, and each
/// model's node map.
fn make_nodes(ctx: &KindCtx, say: &mut Say) -> Box<dyn ClipKind> {
    let mut system = AnimationSystem::new();
    system.nodes_mut();
    if let Some(models) = ctx.models {
        let list: Vec<&LoadedModel> = models.models.iter().collect();
        register_node_animation_data(&list, &mut system, &mut |l| say("node_clip", l));
    }
    take::<NodeKind>(system)
}

/// The one kind of type `K` a scratch system holds, made if none.
fn take<K: ClipKind + Default>(system: AnimationSystem) -> Box<dyn ClipKind> {
    let mut system = system;
    match system.kind_of_mut::<K>() {
        Some(kind) => Box::new(std::mem::take(kind)),
        None => Box::new(K::default()),
    }
}

/// A spawned entity's model (#1107): a skinned mesh already cached gives
/// its new entity no import to build a skeleton from, so it takes a copy of
/// one an entity drawing the same asset has, in that entity's pose until it
/// animates. The rest register as a scene's models do.
fn late_models(system: &mut AnimationSystem, models: &ModelLoadResult, say: &mut Say) {
    let mut rest = Vec::new();
    for model in &models.models {
        if model.is_skinned && model.import_result.is_none() {
            let donor = system.skeletal().and_then(|k| {
                k.skinned()
                    .iter()
                    .filter(|(_, asset)| *asset == model.asset_name)
                    .find_map(|(id, _)| k.sync().skeleton(id).cloned())
            });
            if let Some(skeleton) = donor {
                system.add_skeleton(model.entity_id, skeleton);
                continue;
            }
        }
        rest.push(model);
    }
    register_skeletal_data(&rest, system, &mut |l| say("skeletal_clip", l));
    register_node_animation_data(&rest, system, &mut |l| say("node_clip", l));
    system.skeletal_mut().add_skinned(
        models
            .skinned_entities
            .iter()
            .map(|(id, a)| (*id, a.clone())),
    );
}

/// Register skeletal data from loaded models into the animation system,
/// each clip told to `say`.
pub fn register_skeletal_data(
    models: &[&LoadedModel],
    animation: &mut AnimationSystem,
    say: &mut dyn FnMut(String),
) {
    // Skeletons built from the first import of each asset, so entities that hit
    // the mesh cache (`import_result: None` — every 2nd+ entity sharing a
    // skinned asset) still get their own skeleton and animate independently.
    let mut skeletons_by_asset: HashMap<String, Vec<Skeleton>> = HashMap::new();
    let mut deferred: Vec<(EntityId, String)> = Vec::new();

    for loaded in models {
        if !loaded.is_skinned {
            continue;
        }
        let Some(ref import_result) = loaded.import_result else {
            deferred.push((loaded.entity_id, loaded.asset_name.clone()));
            continue;
        };
        let skeletons: Vec<Skeleton> = import_result
            .skeletons
            .iter()
            .map(Skeleton::from_imported)
            .collect();
        for skeleton in &skeletons {
            animation.add_skeleton(loaded.entity_id, skeleton.clone());
        }
        skeletons_by_asset
            .entry(loaded.asset_name.clone())
            .or_insert(skeletons);
        for imported_clip in &import_result.skeletal_clips {
            let clip = SkeletalClip::from_imported(imported_clip);
            say(format!(
                "  Skeletal clip: {} ({:.1}s, {} tracks)",
                clip.name,
                clip.duration,
                clip.joint_tracks.len()
            ));
            animation.add_skeletal_clip(clip);
        }
    }

    for (entity_id, asset_name) in deferred {
        match skeletons_by_asset.get(&asset_name) {
            Some(skeletons) => {
                for skeleton in skeletons {
                    animation.add_skeleton(entity_id, skeleton.clone());
                }
            }
            None => tracing::warn!(
                "skinned entity {:?} uses `{}` but no import of that asset produced a skeleton this load; it will render in rest pose",
                entity_id,
                asset_name
            ),
        }
    }
}

/// Register node-level animation data from loaded models into the animation
/// system, each clip told to `say`.
pub fn register_node_animation_data(
    models: &[&LoadedModel],
    animation: &mut AnimationSystem,
    say: &mut dyn FnMut(String),
) {
    for loaded in models {
        if let Some(ref import_result) = loaded.import_result {
            for imported_clip in &import_result.node_clips {
                let clip = NodeClip::from_imported(imported_clip);
                say(format!(
                    "  Node clip: {} ({:.1}s, {} tracks)",
                    clip.name,
                    clip.duration,
                    clip.node_tracks.len()
                ));
                animation.add_node_clip(clip);
            }
        }
        if let Some(ref node_map) = loaded.node_map {
            animation.register_node_entity(loaded.entity_id, node_map.clone());
        }
    }
}

/// Two-bone IK (ADR 0070) solved once over the authored chains: what a
/// still that steps no animation draws.
fn solve_ik(world: &mut FlintWorld) {
    IkPass::new().run(world);
}

/// What a scene with skeletons, IK chains or bone probes hears when the
/// project's `[packages]` leaves `rig` out.
fn left_out(ctx: &SceneCtx) -> Option<String> {
    let in_use = ctx.packages?;
    let uses: Vec<&str> = [comp::SKELETON, comp::IK_TWO_BONE, comp::BONE_PROBE]
        .into_iter()
        .filter(|c| !ctx.world.entities_with_component(c).is_empty())
        .collect();
    flint_package::left_out_line(in_use, PACKAGE_NAME, &uses, "stand in their rest pose")
}
