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
