//! The `flipbook` script module (ADR 0139 section 7): the 2D names for the
//! animation module's clip calls, under the names they had in the kernel's
//! `sprite` module, each with its old spelling for one more release (#702).
//! `play_sprite` is `play_clip`, `stop_sprite` is `stop_clip`,
//! `set_sprite_speed` is `set_anim_speed`, and `is_sprite_playing` reads
//! the animator's `playing`: they write through `animator::write`, so an
//! entity that still writes the retired `sprite_animator` is written there.

use flint_animation::animator::{self, Animator};
use flint_script::api::builder::kind::{Bool, Entity, Num, Str};
use flint_script::api::Module;
use flint_script::ScriptModule;

/// The module's name.
pub const MODULE: &str = "flipbook";

/// The `flipbook` module, declared by the package and enabled in every
/// scene.
pub struct FlipbookModule;

pub static FLIPBOOK_MODULE: FlipbookModule = FlipbookModule;

impl ScriptModule for FlipbookModule {
    fn name(&self) -> &'static str {
        MODULE
    }

    fn enabled_when(&self) -> &'static str {
        "the flipbook package is in the build (every scene)"
    }

    fn declare(&self, m: &mut Module<'_>) {
        // Verb first, like play_clip; each under its name and, for one
        // release, its old one (#702)
        for (name, old) in [
            ("play_sprite", None),
            ("sprite_play", Some("use play_sprite")),
        ] {
            m.func(
                name,
                "Play one of the entity's flipbook clips: a new clip cuts to its start, a paused one resumes, a finished one-shot replays.",
            )
            .arg("id", Entity)
            .arg("clip", Str)
            .deprecated(old)
            .run(|call, (id, clip)| {
                if let Some(mut comps) = call.world_mut()?.get_components_mut(id) {
                    animator::write(&mut comps)
                        .clip(&clip)
                        .playing(true)
                        .no_blend();
                }
                Ok(())
            });
        }

        for (name, old) in [
            ("stop_sprite", None),
            ("sprite_stop", Some("use stop_sprite")),
        ] {
            m.func(
                name,
                "Pause the entity's flipbook: it holds its frame, and play_sprite resumes it.",
            )
            .arg("id", Entity)
            .deprecated(old)
            .run(|call, (id,)| {
                if let Some(mut comps) = call.world_mut()?.get_components_mut(id) {
                    animator::write(&mut comps).playing(false);
                }
                Ok(())
            });
        }

        for (name, old) in [
            ("set_sprite_speed", None),
            ("sprite_set_speed", Some("use set_sprite_speed")),
        ] {
            m.func(
                name,
                "Set the entity's flipbook playback speed (1 is the clip's own; negative plays it backwards).",
            )
            .arg("id", Entity)
            .arg("speed", Num)
            .deprecated(old)
            .run(|call, (id, speed)| {
                if let Some(mut comps) = call.world_mut()?.get_components_mut(id) {
                    animator::write(&mut comps).speed(speed);
                }
                Ok(())
            });
        }

        for (name, old) in [
            ("is_sprite_playing", None),
            ("sprite_is_playing", Some("use is_sprite_playing")),
        ] {
            m.func(name, "Whether the entity's flipbook is playing.")
                .arg("id", Entity)
                .returns(Bool, "false without an animator")
                .deprecated(old)
                .run(|call, (id,)| Ok(Animator::of(call.world()?, id).playing));
        }
    }
}
