//! The `animation` module: clips, blends, layers and sequences, written
//! straight onto the entity's `animator` component, which the animation
//! system follows next frame (ADR 0116), through `flint_animation::animator`
//! (the one file that spells the animator's keys, #733).

use super::builder::kind::{Entity, Int, Map, Num, Str};
use super::builder::{CallResult, ScriptApi};
use super::opts::read_opts;
use flint_animation::animator::{self, LayerEdit};
use flint_core::tween::Easing;
use flint_core::EntityId;
use flint_ecs::FlintWorld;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("animation");

    // ── Clips ──
    m.func(
        "play_clip",
        "Play a named clip on the entity's animator: a new clip cuts to its start (cancelling a crossfade), a stopped one resumes, a finished one-shot replays.",
    )
    .arg("id", Entity)
    .arg("clip", Str)
    .run(|call, (id, clip)| {
        if let Some(mut comps) = call.world_mut()?.get_components_mut(id) {
            // A crossfade still in flight would otherwise finish on its
            // target and override this clip (#732).
            animator::write(&mut comps)
                .clip(&clip)
                .playing(true)
                .no_blend();
        }
        Ok(())
    });

    m.func("stop_clip", "Pause the entity's animator: its clip holds the current frame (layers keep their own clocks).")
        .arg("id", Entity)
        .run(|call, (id,)| {
            if let Some(mut comps) = call.world_mut()?.get_components_mut(id) {
                animator::write(&mut comps).playing(false);
            }
            Ok(())
        });

    m.func(
        "blend_to",
        "Crossfade to another clip over a duration in seconds.",
    )
    .arg("id", Entity)
    .arg("clip", Str)
    .arg("duration", Num)
    .run(|call, (id, clip, duration)| {
        if let Some(mut comps) = call.world_mut()?.get_components_mut(id) {
            animator::write(&mut comps).blend(&clip, duration);
        }
        Ok(())
    });

    m.func(
        "blend_to",
        "Crossfade to another clip over seconds, eased: linear, smooth, ease_in, ease_out or ease_in_out (later crossfades keep it).",
    )
    .arg("id", Entity)
    .arg("clip", Str)
    .arg("duration", Num)
    .arg("easing", Str)
    .run(|call, (id, clip, duration, easing)| {
        let easing =
            Easing::from_name(&easing).ok_or_else(|| call.error(Easing::refusal(&easing)))?;
        if let Some(mut comps) = call.world_mut()?.get_components_mut(id) {
            animator::write(&mut comps).blend_eased(&clip, duration, easing);
        }
        Ok(())
    });

    m.func(
        "set_anim_speed",
        "Set the animator's playback speed (1 normal).",
    )
    .arg("id", Entity)
    .arg("speed", Num)
    .run(|call, (id, speed)| {
        if let Some(mut comps) = call.world_mut()?.get_components_mut(id) {
            animator::write(&mut comps).speed(speed);
        }
        Ok(())
    });

    // ── Layers ──
    // All of these read-modify-write `animator.layers`; slots below `index`
    // are padded with inactive (empty-clip) entries so indices stay stable.
    m.func(
        "set_anim_layer",
        "Play a clip on layer `index`, additive and unmasked, at a weight.",
    )
    .arg("id", Entity)
    .arg("index", Int)
    .arg("clip", Str)
    .arg("weight", Num)
    .run(|call, (id, index, clip, weight)| {
        edit_anim_layer(call.world_mut()?, id, index, |layer| {
            layer.clip(&clip).mode("additive").mask("").weight(weight);
        })
    });

    m.func(
        "set_anim_layer",
        "Play a clip on layer `index` at a weight; opts: mode (\"additive\", the default, or \"override\"), mask (a root joint; \"\" unmasked).",
    )
    .arg("id", Entity)
    .arg("index", Int)
    .arg("clip", Str)
    .arg("weight", Num)
    .arg("opts", Map)
    .run(|call, (id, index, clip, weight, map)| {
        let o = read_opts(call.ctx(), "set_anim_layer", &map, &["mode", "mask"]);
        let mode = o.mode.unwrap_or_else(|| "additive".into());
        let mask = o.mask.unwrap_or_default();
        edit_anim_layer(call.world_mut()?, id, index, |layer| {
            layer.clip(&clip).mode(&mode).mask(&mask).weight(weight);
        })
    });

    m.func(
        "set_anim_layer_ex",
        "Play a clip on a layer with a mode (\"additive\" or \"override\") and a root-joint mask.",
    )
    .arg("id", Entity)
    .arg("index", Int)
    .arg("clip", Str)
    .arg("weight", Num)
    .arg("mode", Str)
    .arg("mask", Str)
    .deprecated("use set_anim_layer(id, index, clip, weight, #{mode, mask})")
    .run(|call, (id, index, clip, weight, mode, mask)| {
        edit_anim_layer(call.world_mut()?, id, index, |layer| {
            layer.clip(&clip).mode(&mode).mask(&mask).weight(weight);
        })
    });

    m.func(
        "set_anim_layer_weight",
        "Set a layer's weight at once, cancelling any fade.",
    )
    .arg("id", Entity)
    .arg("index", Int)
    .arg("weight", Num)
    .run(|call, (id, index, weight)| {
        edit_anim_layer(call.world_mut()?, id, index, |layer| {
            layer.weight(weight);
        })
    });

    m.func(
        "clear_anim_layer",
        "Deactivate a layer, keeping its slot so the indices stay stable.",
    )
    .arg("id", Entity)
    .arg("index", Int)
    .run(|call, (id, index)| {
        edit_anim_layer(call.world_mut()?, id, index, |layer| {
            layer.clip("").no_fade();
        })
    });

    m.func(
        "fade_anim_layer",
        "Ramp a layer's weight over a duration in seconds; set_anim_layer_weight cancels it.",
    )
    .arg("id", Entity)
    .arg("index", Int)
    .arg("weight", Num)
    .arg("duration", Num)
    .run(|call, (id, index, weight, duration)| {
        edit_anim_layer(call.world_mut()?, id, index, |layer| {
            layer.fade(weight, duration);
        })
    });

    m.func(
        "fade_anim_layer",
        "Ramp a layer's weight over seconds, eased: linear, smooth, ease_in, ease_out or ease_in_out.",
    )
    .arg("id", Entity)
    .arg("index", Int)
    .arg("weight", Num)
    .arg("duration", Num)
    .arg("easing", Str)
    .run(|call, (id, index, weight, duration, easing)| {
        let easing =
            Easing::from_name(&easing).ok_or_else(|| call.error(Easing::refusal(&easing)))?;
        edit_anim_layer(call.world_mut()?, id, index, |layer| {
            layer.fade_eased(weight, duration, easing);
        })
    });

    // ── Sequences (*.sequence.toml) ──
    // Both write `animator.sequence`; the animation system follows the edge
    // next frame (and clears the field when a non-looping sequence ends, so
    // replaying the same name works).
    m.func(
        "play_sequence",
        "Play a *.sequence.toml (timed blends, layers, speeds and cues) on the entity's animator.",
    )
    .arg("id", Entity)
    .arg("name", Str)
    .run(|call, (id, name)| {
        if let Some(mut comps) = call.world_mut()?.get_components_mut(id) {
            animator::write(&mut comps).sequence(&name).playing(true);
        }
        Ok(())
    });

    m.func("stop_sequence", "Stop the animator's sequence.")
        .arg("id", Entity)
        .run(|call, (id,)| {
            if let Some(mut comps) = call.world_mut()?.get_components_mut(id) {
                animator::write(&mut comps).sequence("");
            }
            Ok(())
        });
}

/// Read-modify-write one entry of `animator.layers` through
/// `flint_animation::animator::edit_layer` (legacy-field migration and slot
/// padding live there, so the sequence runner behaves identically). A
/// negative index does nothing.
fn edit_anim_layer(
    world: &mut FlintWorld,
    id: EntityId,
    index: i64,
    edit: impl FnOnce(&mut LayerEdit),
) -> CallResult<()> {
    if index < 0 {
        return Ok(());
    }
    if let Some(mut comps) = world.get_components_mut(id) {
        animator::edit_layer(&mut comps, index as usize, edit);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::engine::ScriptEngine;
    use flint_animation::animator::Animator;
    use flint_core::tween::Easing;
    use flint_ecs::FlintWorld;

    fn run(source: &str) -> (ScriptEngine, FlintWorld, flint_core::EntityId) {
        let mut engine = ScriptEngine::new();
        engine.errors.set_quiet(true);
        let mut world = FlintWorld::new();
        let id = world.spawn("rig").unwrap();
        let ast = engine.compile(source).unwrap();
        engine.add_script(id, ast, "rig.rhai".into());
        engine.call_inits(&mut world);
        (engine, world, id)
    }

    /// `blend_to(id, clip, seconds, easing)` (#1248) writes the crossfade
    /// and its `blend_easing`; the three-argument form leaves the easing
    /// as written.
    #[test]
    fn an_eased_blend_to_writes_its_easing_and_a_plain_one_keeps_it() {
        let (engine, world, id) = run(
            "fn on_init() { let me = self_entity(); blend_to(me, \"walk\", 0.5, \"ease_in_out\"); blend_to(me, \"run\", 0.25); }",
        );
        assert!(
            engine.errors().all().is_empty(),
            "{:?}",
            engine.errors().all()
        );
        let animator = Animator::of(&world, id);
        assert_eq!(animator.blend_target, "run");
        assert_eq!(animator.blend_duration, 0.25);
        assert_eq!(animator.blend_easing, Easing::EaseInOut);
    }

    #[test]
    fn an_easing_that_is_none_is_a_script_error() {
        let (engine, world, id) =
            run("fn on_init() { blend_to(self_entity(), \"walk\", 0.5, \"wobbly\"); }");
        let errors = engine.errors().all();
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0]
                .message
                .contains("blend_to: no easing `wobbly`; one of linear"),
            "{}",
            errors[0].message
        );
        assert_eq!(Animator::of(&world, id).blend_target, "");
    }
}
