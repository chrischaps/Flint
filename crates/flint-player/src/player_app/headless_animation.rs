//! Animation with no window (#307): what `flint render --anim-time` runs, so a
//! headless capture shows a skinned character in the pose the player shows it
//! in, not its bind pose.
//!
//! It loads what the player loads when a scene comes up (skeletons and clips
//! from the scene's models, `.anim.toml` property tweens, sprite clips and
//! sequences, each found by the project's one asset search), and a step is the
//! animation share of the player's frame: every tier's `update`, the
//! `bone_probe` joints published, then two-bone IK (ADR 0070) over the posed
//! world. The caller keeps the clock, a fixed step at a time, so the same time
//! gives the same pose run to run.
//!
//! It is not the player. Sequence cues and sprite-end events have no script to
//! hear them here; they are counted (see [`HeadlessAnimation::unheard`]) and
//! dropped.

use flint_animation::AnimationSystem;
use flint_core::components as comp;
use flint_core::project::Project;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_render::model_loader::ModelLoadResult;
use flint_render::SceneRenderer;
use flint_runtime::RuntimeSystem;

use super::scene_loading::{
    load_animations_saying, load_sequences_saying, load_sprite_animations_saying,
    register_node_animation_data_saying, register_skeletal_data_saying,
};

/// A scene's animation, run on a clock someone else keeps.
pub struct HeadlessAnimation {
    system: AnimationSystem,
    /// Skinned entities and their asset, in id order, for the bone upload.
    skinned: Vec<(EntityId, String)>,
    /// Seconds stepped so far.
    time: f64,
    /// What the load registered, a line each, as the player prints it.
    loaded: Vec<String>,
    /// Sequence cues and sprite-end events fired with no script to hear them.
    unheard: usize,
}

impl HeadlessAnimation {
    /// The animation of the scene at `scene_path`, whose models `models` are
    /// (the model loader's result), found in `project` as the player finds it.
    /// Nothing has moved yet: [`Self::step`] moves it.
    pub fn load(models: &ModelLoadResult, project: &Project, scene_path: &str) -> Self {
        let mut system = AnimationSystem::new();
        let mut loaded = Vec::new();
        let mut say = |line: String| loaded.push(line);
        register_skeletal_data_saying(models, &mut system, &mut say);
        register_node_animation_data_saying(models, &mut system, &mut say);
        load_animations_saying(project, scene_path, &mut system, &mut say);
        load_sprite_animations_saying(project, scene_path, &mut system, &mut say);
        load_sequences_saying(project, scene_path, &mut system, &mut say);
        let mut skinned: Vec<(EntityId, String)> = models
            .skinned_entities
            .iter()
            .map(|(id, asset)| (*id, asset.clone()))
            .collect();
        skinned.sort();
        Self {
            system,
            skinned,
            time: 0.0,
            loaded,
            unheard: 0,
        }
    }

    /// One frame of animation, `dt` seconds on: every tier, the bone probes,
    /// then IK, in the player's order.
    pub fn step(&mut self, world: &mut FlintWorld, dt: f64) {
        if let Err(e) = self.system.update(world, dt) {
            tracing::warn!("Animation update failed: {e:?}");
        }
        self.unheard += self.system.drain_sprite_events().len();
        self.unheard += self.system.drain_sequence_cues().len();
        publish_bone_probes(&self.system, world);
        self.system.run_ik(world);
        self.time += dt;
    }

    /// The pose as it stands: no time passes, IK is solved once. What a
    /// capture at time 0 draws.
    pub fn settle(&mut self, world: &mut FlintWorld) {
        self.step(world, 0.0);
    }

    /// Give the renderer every skinned entity's bones as they stand.
    pub fn upload(&self, renderer: &mut SceneRenderer, device: &wgpu::Device, queue: &wgpu::Queue) {
        for (id, asset) in &self.skinned {
            if let Some(matrices) = self.system.bone_matrices(id) {
                renderer.update_bone_matrices(device, queue, *id, asset, matrices);
            }
        }
    }

    /// Seconds stepped so far.
    pub fn time(&self) -> f64 {
        self.time
    }

    /// What the load registered, a line each (clips, sequences, sprite clips).
    pub fn loaded(&self) -> &[String] {
        &self.loaded
    }

    /// How many skinned entities are posed.
    pub fn skinned_count(&self) -> usize {
        self.skinned.len()
    }

    /// Sequence cues and sprite-end events that fired with no script to hear
    /// them.
    pub fn unheard(&self) -> usize {
        self.unheard
    }

    /// Skinned entities with an `animator` and no `skeleton` component, by
    /// name: their clip plays nowhere, in the player either, since skeletal
    /// playback is for entities with both.
    pub fn unposed(&self, world: &FlintWorld) -> Vec<String> {
        let mut names: Vec<String> = self
            .skinned
            .iter()
            .filter(|(id, _)| {
                world.get_components(*id).is_some_and(|c| {
                    c.get(comp::ANIMATOR).is_some() && c.get(comp::SKELETON).is_none()
                })
            })
            .filter_map(|(id, _)| world.get_name(*id).map(String::from))
            .collect();
        names.sort();
        names
    }

    /// The names of the entities that animate: an `animator` or a skinned model.
    pub fn animated(&self, world: &FlintWorld) -> Vec<String> {
        let mut ids: Vec<EntityId> = world
            .entities_with_component(comp::ANIMATOR)
            .iter()
            .copied()
            .chain(self.skinned.iter().map(|(id, _)| *id))
            .collect();
        ids.sort();
        ids.dedup();
        let mut names: Vec<String> = ids
            .into_iter()
            .filter_map(|id| world.get_name(id).map(String::from))
            .collect();
        names.sort();
        names
    }
}

/// Write each `bone_probe`'s joint, in model space, into its component, right
/// after the pose is computed, so scripts read this frame's pose (a seat camera
/// following an eye, say).
pub(super) fn publish_bone_probes(animation: &AnimationSystem, world: &mut FlintWorld) {
    let probe_ids: Vec<EntityId> = world
        .entities_with_component(comp::BONE_PROBE)
        .iter()
        .copied()
        .collect();
    for id in probe_ids {
        let joint = world
            .get_components(id)
            .and_then(|c| c.get_field(comp::BONE_PROBE, "joint"))
            .and_then(|v| v.as_str())
            .map(String::from);
        let Some(joint) = joint else {
            continue;
        };
        if let Some(pos) = animation.joint_position(&id, &joint) {
            for (field, value) in [("x", pos[0]), ("y", pos[1]), ("z", pos[2])] {
                let _ = world.set_field(
                    id,
                    comp::BONE_PROBE,
                    field,
                    toml::Value::Float(value as f64),
                );
            }
        }
    }
}
