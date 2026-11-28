//! The particles subsystem: a scene's effects and emitters loaded into the
//! run's [`ParticleSystem`], stepped once a frame, and the `particle`
//! module's commands carried out, each where the player did it before the
//! package was one (#1622).

use super::module::Particle;
use crate::{CaptureDrawn, ParticleSystem, POLICY};
use flint_core::frame_notes::{self, NoteKind};
use flint_package::{SceneCtx, Stage, StageCtx, Subsystem};
use flint_runtime::RuntimeSystem;
use flint_script::context::ScriptCommand;
use std::path::Path;

/// The subsystem's name, which is the game state's policy name too.
pub const NAME: &str = POLICY;

/// What a capture says once when its scripts emit and it draws no
/// particles (a still with neither `--time` nor `--particle-time`).
const UNDRAWN: &str = "particles: a still with no --time draws none; give it --time or \
                       --particle-time to see what the scripts emit";

/// Every scene has one: an effect needs no emitter in the scene to play.
///
/// In the player it loads the scene's effects, finds the world's emitters
/// and uploads their textures, right before the scene's other subsystems
/// are built (where the player loaded them before #1622). A capture loads
/// and steps its particles on its own clock (`flint render`'s stage, through
/// `flint_package::particles::Particles`), so there it only takes the
/// scripts' commands.
pub fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    let capture = ctx.capture.is_some();
    if !capture {
        let _note = frame_notes::begin(NoteKind::ParticleLoad, String::new);
        let particles = crate::of_mut(ctx.resources)?;
        // `particles/*.particles.toml` registered first, so `particle_effect`
        // components resolve on the first sync
        crate::load_particle_effects_from_world(ctx.project, ctx.scene_path, particles);
        particles
            .initialize(ctx.world)
            .unwrap_or_else(|e| tracing::warn!("Particles init failed: {:?}", e));
        if let (Some(renderer), Some((device, queue))) = (ctx.renderer.as_deref_mut(), ctx.gpu) {
            flint_render::load_scene_particle_textures(
                renderer,
                device,
                queue,
                &particles.sync.texture_names(),
                ctx.project,
                Path::new(ctx.scene_path)
                    .parent()
                    .unwrap_or_else(|| Path::new(".")),
            );
        }
    }
    Some(Box::new(Particles {
        capture,
        said_undrawn: false,
    }))
}

/// A scene's share of the run's particles.
pub struct Particles {
    /// Built for a capture, which steps its particles itself.
    capture: bool,
    /// Whether a capture that draws none has said so.
    said_undrawn: bool,
}

impl Particles {
    /// Carry out one `particle` call on the run's system, as the player's
    /// core did (#1622 moved it here, word for word).
    fn apply(particles: &mut ParticleSystem, what: &Particle) {
        match what {
            Particle::Burst { entity, count } => {
                particles.sync.queue_burst(*entity, (*count).max(0) as u32);
            }
            Particle::Play {
                handle,
                name,
                position,
            } => {
                let pos = [position.0 as f32, position.1 as f32, position.2 as f32];
                if !particles.sync.spawn_effect(*handle, name, pos) {
                    tracing::warn!(
                        target: "script",
                        "play_effect: {}",
                        particles.sync.missing_effect_report(name)
                    );
                }
            }
            Particle::Stop { handle } => particles.sync.stop_effect(*handle),
            Particle::Param {
                handle,
                param,
                value,
            } => {
                if !particles.sync.set_effect_param(*handle, param, *value) {
                    tracing::warn!(
                        target: "script",
                        "set_effect_param: unknown handle {} or param '{}'",
                        handle,
                        param
                    );
                }
            }
        }
    }
}

impl Subsystem for Particles {
    fn name(&self) -> &'static str {
        NAME
    }

    fn script_modules(&self) -> &[&'static str] {
        &[super::module::MODULE]
    }

    /// The step, after the bone probes and IK (ADR 0138's `Animation`),
    /// while the game state runs particles; a capture steps its own.
    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        if stage != Stage::Animation || self.capture || !ctx.runs(NAME) {
            return;
        }
        if let Some(particles) = crate::of_mut(ctx.resources) {
            particles.update(ctx.world, ctx.clock.delta_time).ok();
        }
    }

    fn script_command(&mut self, command: &ScriptCommand, ctx: &mut StageCtx) -> bool {
        let ScriptCommand::Module(command) = command else {
            return false;
        };
        let Some(what) = command.payload::<Particle>() else {
            return false;
        };
        // A capture that draws none drops what its scripts emit, saying so
        let drawn = !self.capture || ctx.resources.get::<CaptureDrawn>().is_some_and(|d| d.0);
        if !drawn {
            if !std::mem::replace(&mut self.said_undrawn, true) {
                tracing::warn!("{UNDRAWN}");
            }
            return true;
        }
        if let Some(particles) = crate::of_mut(ctx.resources) {
            Self::apply(particles, what);
        }
        true
    }

    /// The scene's instances and particles go with it.
    fn scene_unloading(&mut self, ctx: &mut SceneCtx) {
        if let Some(particles) = crate::of_mut(ctx.resources) {
            particles.clear();
        }
    }
}
