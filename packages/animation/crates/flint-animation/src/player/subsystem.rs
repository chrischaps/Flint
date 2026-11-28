//! The animation subsystem: a scene's clips, sequences, kinds, tweens and
//! poses, at the frame's stages where the player ran them before #1620.

use super::loading::{
    load_animations_saying, load_sequences_saying, loaded, models_of,
    register_node_animation_data_saying, register_skeletal_data_saying,
};
use super::{module, tween_module, ClipKinds, TweenField};
use crate::tween::FieldTweens;
use crate::AnimationSystem;
use flint_core::frame_notes::{self, NoteKind};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_package::{SceneCtx, Stage, StageCtx, Subsystem};
use flint_render::model_loader::ModelLoadResult;
use flint_runtime::state::SCRIPTS;
use flint_runtime::RuntimeSystem;
use flint_script::context::ScriptCommand;

/// The subsystem's name, which is the game state's policy name too.
pub const NAME: &str = crate::POLICY;

/// Every scene has one: its `tween` and `animation` modules are any scene's.
pub fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    let _note = frame_notes::begin(NoteKind::AnimationLoad, String::new);
    let kinds = ctx.registry.point::<ClipKinds>();
    let mut say = |kind: &'static str, line: String| loaded(kind)(line);
    let animation = Animation::load(
        ctx.models,
        kinds,
        ctx.project,
        ctx.scene_path,
        ctx.world,
        &mut say,
    );
    Some(Box::new(animation))
}

/// A scene's animation: the clips and their playback, the skinned entities
/// whose bones go to the GPU, and the scripts' field tweens.
pub struct Animation {
    system: AnimationSystem,
    /// Skinned entities and their asset, for the bone upload.
    skinned: Vec<(EntityId, String)>,
    tweens: FieldTweens,
}

impl Animation {
    /// The scene's animation, loaded as the player always loaded it: the
    /// models' skeletons and clips, `.anim.toml`, each registered kind's
    /// clips (`kinds`), `.sequence.toml`, then the first sync. `say` hears
    /// each line with what it is (`skeletal_clip`, `animation`...).
    pub fn load(
        models: Option<&ModelLoadResult>,
        kinds: Option<&ClipKinds>,
        project: &flint_core::project::Project,
        scene_path: &str,
        world: &mut FlintWorld,
        say: &mut dyn FnMut(&'static str, String),
    ) -> Self {
        let (system, skinned) = load_system(models, kinds, project, scene_path, say);
        let mut animation = Self {
            system,
            skinned,
            tweens: FieldTweens::default(),
        };
        animation
            .system
            .initialize(world)
            .unwrap_or_else(|e| tracing::warn!("Animation init failed: {:?}", e));
        animation
    }

    /// The clips and their playback.
    pub fn system(&self) -> &AnimationSystem {
        &self.system
    }

    /// The clock: every clip, sequence and kind `dt` on, and what ended or
    /// was cued said to the scripts.
    fn animate(&mut self, ctx: &mut StageCtx) {
        flint_core::scope!("animate");
        if !ctx.runs(NAME) {
            return;
        }
        self.system.update(ctx.world, ctx.clock.delta_time).ok();

        // A one-shot's end, of any kind: `on_animation_end(clip)`
        let ends: Vec<(EntityId, String)> = self
            .system
            .drain_clip_ends()
            .into_iter()
            .map(|e| (e.entity_id, e.clip_name))
            .collect();
        if !ends.is_empty() {
            ctx.script.call_animation_ends(ctx.world, &ends);
        }

        // Sequence cues to the owning entity's script
        let cues = self.system.drain_sequence_cues();
        if !cues.is_empty() {
            for cue in &cues {
                // Each cue is the script's to say (`on_sequence_cue`);
                // `RUST_LOG=script=debug` lists them (#716)
                tracing::debug!(
                    target: "script",
                    "[sequence] {:?} '{}' cue '{}' at {:.2}s",
                    cue.entity_id, cue.sequence, cue.cue, cue.time
                );
            }
            let cues: Vec<(EntityId, String, String)> = cues
                .into_iter()
                .map(|c| (c.entity_id, c.sequence, c.cue))
                .collect();
            ctx.script.call_sequence_cues(ctx.world, &cues);
        }
    }

    /// The bones to the GPU, the bone probes, then IK (ADR 0070): the
    /// last writer of transforms the renderer sees.
    fn pose(&mut self, ctx: &mut StageCtx) {
        if let (Some(renderer), Some((device, queue))) = (ctx.renderer.as_deref_mut(), ctx.gpu) {
            flint_core::scope!("upload_bones", self.skinned.len());
            for (id, asset) in &self.skinned {
                if let Some(matrices) = self.system.bone_matrices(id) {
                    renderer.update_bone_matrices(device, queue, *id, asset, matrices);
                }
            }
        }
        {
            flint_core::scope!("publish_bone_probes");
            super::capture::publish_bone_probes(&self.system, ctx.world);
        }
        self.system.run_ik(ctx.world);
    }
}

/// The clips a scene's animation plays, before the first sync, and its
/// skinned entities in id order: the player's load and a capture's.
pub(super) fn load_system(
    models: Option<&ModelLoadResult>,
    kinds: Option<&ClipKinds>,
    project: &flint_core::project::Project,
    scene_path: &str,
    say: &mut dyn FnMut(&'static str, String),
) -> (AnimationSystem, Vec<(EntityId, String)>) {
    let mut system = AnimationSystem::new();
    let mut skinned = Vec::new();
    if let Some(models) = models {
        let list = models_of(models);
        register_skeletal_data_saying(&list, &mut system, &mut |l| say("skeletal_clip", l));
        register_node_animation_data_saying(&list, &mut system, &mut |l| say("node_clip", l));
        skinned.extend(
            models
                .skinned_entities
                .iter()
                .map(|(id, a)| (*id, a.clone())),
        );
        skinned.sort();
    }
    load_animations_saying(project, scene_path, &mut system, &mut |l| {
        say("animation", l)
    });
    if let Some(kinds) = kinds {
        for kind in kinds.make(project, scene_path, &mut |l| say("sprite_clip", l)) {
            system.add_kind(kind);
        }
    }
    load_sequences_saying(project, scene_path, &mut system, &mut |l| {
        say("sequence", l)
    });
    (system, skinned)
}

impl Subsystem for Animation {
    fn name(&self) -> &'static str {
        NAME
    }

    fn script_modules(&self) -> &[&'static str] {
        &[module::MODULE, tween_module::MODULE]
    }

    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        match stage {
            // The field tweens, while scripts run (#734)
            Stage::Tweens if ctx.runs(SCRIPTS) && !self.tweens.is_empty() => {
                for event in self.tweens.step(ctx.world, ctx.clock.delta_time) {
                    ctx.bus.push(event);
                }
            }
            Stage::Animate => self.animate(ctx),
            Stage::Pose => self.pose(ctx),
            _ => {}
        }
    }

    fn script_command(&mut self, command: &ScriptCommand, ctx: &mut StageCtx) -> bool {
        let ScriptCommand::Module(command) = command else {
            return false;
        };
        let Some(t) = command.payload::<TweenField>() else {
            return false;
        };
        let started = self
            .tweens
            .start(ctx.world, t.entity, &t.path, &t.target, t.seconds, t.easing);
        match started {
            Ok(Some(done)) => ctx.bus.push(done),
            Ok(None) => {}
            // The world changed between the call, which checked, and now
            Err(message) => tracing::warn!(target: "script", "{message}"),
        }
        true
    }

    /// A spawned entity's model (#1107): a skinned mesh already cached gives
    /// its new entity no import to build a skeleton from, so it takes a copy
    /// of one an entity drawing the same asset has, in that entity's pose
    /// until it animates.
    fn late_models(&mut self, _world: &mut FlintWorld, models: &ModelLoadResult) {
        let mut rest = Vec::new();
        for model in &models.models {
            if model.is_skinned && model.import_result.is_none() {
                let donor = self
                    .skinned
                    .iter()
                    .filter(|(_, asset)| *asset == model.asset_name)
                    .find_map(|(id, _)| self.system.skeleton(id).cloned());
                if let Some(skeleton) = donor {
                    self.system.add_skeleton(model.entity_id, skeleton);
                    continue;
                }
            }
            rest.push(model);
        }
        register_skeletal_data_saying(&rest, &mut self.system, &mut loaded("skeletal_clip"));
        register_node_animation_data_saying(&rest, &mut self.system, &mut loaded("node_clip"));
        self.skinned.extend(
            models
                .skinned_entities
                .iter()
                .map(|(id, a)| (*id, a.clone())),
        );
    }
}
