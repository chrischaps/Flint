//! The animation subsystem: a scene's clips, sequences, kinds, tweens and
//! poses, at the frame's stages where the player ran them before #1620.

use super::loading::{load_animations_saying, load_sequences_saying, loaded};
use super::{module, tween_module, ClipKinds, KindCtx, TweenField};
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
    // The kinds of the packages the project keeps
    let kinds = ctx
        .registry
        .point::<ClipKinds>()
        .map(|k| k.of(|package| ctx.leaves_out(package).is_none()));
    let kinds = kinds.as_ref();
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

/// A scene's animation: the clips and their playback, and the scripts'
/// field tweens.
pub struct Animation {
    system: AnimationSystem,
    tweens: FieldTweens,
    /// What the registered kinds do with a model spawned later.
    late: Vec<super::LateModels>,
}

impl Animation {
    /// The scene's animation, loaded as the player always loaded it: each
    /// registered kind's clips (`kinds`: the rig's from the models, the
    /// flipbook's `.sprite.toml`), `.anim.toml`, `.sequence.toml`, then the
    /// first sync. `say` hears each line with what it is (`skeletal_clip`,
    /// `animation`...).
    pub fn load(
        models: Option<&ModelLoadResult>,
        kinds: Option<&ClipKinds>,
        project: &flint_core::project::Project,
        scene_path: &str,
        world: &mut FlintWorld,
        say: &mut dyn FnMut(&'static str, String),
    ) -> Self {
        let system = load_system(models, kinds, project, scene_path, say);
        let mut animation = Self {
            system,
            tweens: FieldTweens::default(),
            late: kinds
                .map(|k| k.0.iter().filter_map(|k| k.late).collect())
                .unwrap_or_default(),
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

    /// The kinds' skins to the GPU, then each kind's pose (the rig's bone
    /// probes and two-bone IK, ADR 0070): the last writer of transforms the
    /// renderer sees.
    fn pose(&mut self, ctx: &mut StageCtx) {
        if let (Some(renderer), Some((device, queue))) = (ctx.renderer.as_deref_mut(), ctx.gpu) {
            flint_core::scope!("upload_bones");
            upload(&self.system, renderer, device, queue);
        }
        self.system.pose(ctx.world);
    }
}

/// Every skinned entity's bones, as the kinds hold them, to the renderer.
pub(super) fn upload(
    system: &AnimationSystem,
    renderer: &mut flint_render::SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) {
    system.skins(&mut |id, asset, matrices| {
        if let Some(matrices) = matrices {
            renderer.update_bone_matrices(device, queue, id, asset, matrices);
        }
    });
}

/// The clips a scene's animation plays, before the first sync: the
/// player's load and a capture's. The kinds load first, so the rig's
/// skeletal and node clips are said first, as they were before #1621.
pub(super) fn load_system(
    models: Option<&ModelLoadResult>,
    kinds: Option<&ClipKinds>,
    project: &flint_core::project::Project,
    scene_path: &str,
    say: &mut dyn FnMut(&'static str, String),
) -> AnimationSystem {
    let mut system = AnimationSystem::new();
    if let Some(kinds) = kinds {
        let ctx = KindCtx {
            project,
            scene_path,
            models,
        };
        for kind in kinds.make(&ctx, say) {
            system.add_kind(kind);
        }
    }
    load_animations_saying(project, scene_path, &mut system, &mut |l| {
        say("animation", l)
    });
    load_sequences_saying(project, scene_path, &mut system, &mut |l| {
        say("sequence", l)
    });
    system
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

    /// A spawned entity's model (#1107), to each kind that takes one: the
    /// rig's skeleton and clips.
    fn late_models(&mut self, _world: &mut FlintWorld, models: &ModelLoadResult) {
        let mut say = |kind: &'static str, line: String| loaded(kind)(line);
        for late in &self.late {
            late(&mut self.system, models, &mut say);
        }
    }
}
