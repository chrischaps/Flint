//! Animation with no window (#307): what `flint render --anim-time` runs, so a
//! headless capture shows a skinned character in the pose the player shows it
//! in, not its bind pose.
//!
//! It loads what the player loads when a scene comes up (the animation
//! subsystem's load: each registered kind's clips (the rig's skeletons and
//! clips from the scene's models, flipbooks' `.sprite.toml`), `.anim.toml`
//! property clips and sequences, each found by the project's one asset
//! search), and a step is the animation share of the player's frame: every
//! kind's `update`, then each kind's pose (the rig's `bone_probe` joints
//! published, then two-bone IK, ADR 0070) over the posed world. The caller
//! keeps the clock, a fixed step at a time, so the same time gives the same
//! pose run to run. The animation package's since #1620: the capture's game
//! steps it through [`flint_package::CapturePose`], where the player runs
//! `Stage::Animate` and `Stage::Pose`, and `flint render` makes it through
//! the [`flint_package::CapturePoses`] point (#1621).
//!
//! It is not the player. Sequence cues and one-shot ends have no script to
//! hear them here; they are counted (see [`HeadlessAnimation::unheard`]) and
//! dropped.

use super::ClipKinds;
use crate::AnimationSystem;
use flint_core::components as comp;
use flint_core::project::Project;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_package::PoseReport;
use flint_render::model_loader::ModelLoadResult;
use flint_render::SceneRenderer;
use flint_runtime::RuntimeSystem;

/// A scene's animation, run on a clock someone else keeps.
pub struct HeadlessAnimation {
    system: AnimationSystem,
    /// Seconds stepped so far.
    time: f64,
    /// What the load registered, a line each, as the player prints it.
    loaded: Vec<String>,
    /// Sequence cues and sprite-end events fired with no script to hear them.
    unheard: usize,
}

impl HeadlessAnimation {
    /// The animation of the scene at `scene_path`, whose models `models` are
    /// (the model loader's result), found in `project` as the player finds it,
    /// with the kinds the run's packages registered (`kinds`: the flipbook
    /// package's). Nothing has moved yet: [`Self::step`] moves it.
    pub fn load(
        models: &ModelLoadResult,
        project: &Project,
        scene_path: &str,
        kinds: Option<&ClipKinds>,
    ) -> Self {
        let mut loaded = Vec::new();
        let system =
            super::subsystem::load_system(Some(models), kinds, project, scene_path, &mut |_, l| {
                loaded.push(l)
            });
        Self {
            system,
            time: 0.0,
            loaded,
            unheard: 0,
        }
    }

    /// One frame of animation, `dt` seconds on: every kind, then each
    /// kind's pose (the rig's bone probes, then IK), in the player's order.
    pub fn step(&mut self, world: &mut FlintWorld, dt: f64) {
        if let Err(e) = self.system.update(world, dt) {
            tracing::warn!("Animation update failed: {e:?}");
        }
        self.unheard += self.system.drain_clip_ends().len();
        self.unheard += self.system.drain_sequence_cues().len();
        self.system.pose(world);
        self.time += dt;
    }

    /// The pose as it stands: no time passes, IK is solved once. What a
    /// capture at time 0 draws.
    pub fn settle(&mut self, world: &mut FlintWorld) {
        self.step(world, 0.0);
    }

    /// Give the renderer every skinned entity's bones as they stand.
    pub fn upload(&self, renderer: &mut SceneRenderer, device: &wgpu::Device, queue: &wgpu::Queue) {
        super::subsystem::upload(&self.system, renderer, device, queue);
    }

    /// The clips and their playback.
    pub fn system(&self) -> &AnimationSystem {
        &self.system
    }

    /// Skinned entities and their asset, in id order, as the kinds pose them.
    fn skinned(&self) -> Vec<(EntityId, String)> {
        let mut skinned = Vec::new();
        self.system
            .skins(&mut |id, asset, _| skinned.push((id, asset.to_string())));
        skinned.sort();
        skinned
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
        self.skinned().len()
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
            .skinned()
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
            .chain(self.skinned().iter().map(|(id, _)| *id))
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

impl flint_package::CapturePose for HeadlessAnimation {
    fn step(&mut self, world: &mut FlintWorld, dt: f64) {
        HeadlessAnimation::step(self, world, dt);
    }

    fn settle(&mut self, world: &mut FlintWorld) {
        HeadlessAnimation::settle(self, world);
    }

    fn upload(&self, renderer: &mut SceneRenderer, device: &wgpu::Device, queue: &wgpu::Queue) {
        HeadlessAnimation::upload(self, renderer, device, queue);
    }

    fn time(&self) -> f64 {
        self.time
    }

    fn report(&self, world: &FlintWorld) -> PoseReport {
        PoseReport {
            animated: self.animated(world),
            skinned: self.skinned_count(),
            unposed: self.unposed(world),
            loaded: self.loaded.clone(),
            unheard: self.unheard,
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
