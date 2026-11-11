//! The `animation` module: clips, blends, layers and sequences, written
//! straight onto the entity's `animator` component, which the animation
//! system follows next frame (ADR 0116).

use super::builder::kind::{Entity, Int, Num, Str};
use super::builder::{CallResult, ScriptApi};
use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("animation");

    // ── Clips ──
    m.func(
        "play_clip",
        "Start playing a named clip on the entity's animator.",
    )
    .arg("id", Entity)
    .arg("clip", Str)
    .run(|call, (id, clip)| {
        if let Some(comps) = call.world_mut()?.get_components_mut(id) {
            comps.set_field(
                comp::ANIMATOR,
                "clip",
                toml::Value::String(clip.to_string()),
            );
            comps.set_field(comp::ANIMATOR, "playing", toml::Value::Boolean(true));
        }
        Ok(())
    });

    m.func("stop_clip", "Stop the entity's animator.")
        .arg("id", Entity)
        .run(|call, (id,)| {
            if let Some(comps) = call.world_mut()?.get_components_mut(id) {
                comps.set_field(comp::ANIMATOR, "playing", toml::Value::Boolean(false));
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
        if let Some(comps) = call.world_mut()?.get_components_mut(id) {
            comps.set_field(
                comp::ANIMATOR,
                "blend_target",
                toml::Value::String(clip.to_string()),
            );
            comps.set_field(
                comp::ANIMATOR,
                "blend_duration",
                toml::Value::Float(duration),
            );
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
        if let Some(comps) = call.world_mut()?.get_components_mut(id) {
            comps.set_field(comp::ANIMATOR, "speed", toml::Value::Float(speed));
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
        edit_anim_layer(call.world_mut()?, id, index, |t| {
            t.insert("clip".into(), toml::Value::String(clip.to_string()));
            t.insert("mode".into(), toml::Value::String("additive".into()));
            t.insert("mask".into(), toml::Value::String(String::new()));
            flint_animation::layer_edit::set_weight(t, weight);
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
    .run(|call, (id, index, clip, weight, mode, mask)| {
        edit_anim_layer(call.world_mut()?, id, index, |t| {
            t.insert("clip".into(), toml::Value::String(clip.to_string()));
            t.insert("mode".into(), toml::Value::String(mode.to_string()));
            t.insert("mask".into(), toml::Value::String(mask.to_string()));
            flint_animation::layer_edit::set_weight(t, weight);
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
        edit_anim_layer(call.world_mut()?, id, index, |t| {
            flint_animation::layer_edit::set_weight(t, weight);
        })
    });

    m.func(
        "clear_anim_layer",
        "Deactivate a layer, keeping its slot so the indices stay stable.",
    )
    .arg("id", Entity)
    .arg("index", Int)
    .run(|call, (id, index)| {
        edit_anim_layer(call.world_mut()?, id, index, |t| {
            t.insert("clip".into(), toml::Value::String(String::new()));
            t.insert("fade_duration".into(), toml::Value::Float(0.0));
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
        edit_anim_layer(call.world_mut()?, id, index, |t| {
            flint_animation::layer_edit::fade_weight(t, weight, duration);
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
        if let Some(comps) = call.world_mut()?.get_components_mut(id) {
            comps.set_field(
                comp::ANIMATOR,
                "sequence",
                toml::Value::String(name.to_string()),
            );
            comps.set_field(comp::ANIMATOR, "playing", toml::Value::Boolean(true));
        }
        Ok(())
    });

    m.func("stop_sequence", "Stop the animator's sequence.")
        .arg("id", Entity)
        .run(|call, (id,)| {
            if let Some(comps) = call.world_mut()?.get_components_mut(id) {
                comps.set_field(
                    comp::ANIMATOR,
                    "sequence",
                    toml::Value::String(String::new()),
                );
            }
            Ok(())
        });
}

/// Read-modify-write one entry of `animator.layers` via the shared
/// `flint_animation::layer_edit` helper (legacy-field migration + slot
/// padding live there so the sequence runner behaves identically). A
/// negative index does nothing.
fn edit_anim_layer(
    world: &mut FlintWorld,
    id: EntityId,
    index: i64,
    edit: impl FnOnce(&mut toml::map::Map<String, toml::Value>),
) -> CallResult<()> {
    if index < 0 {
        return Ok(());
    }
    if let Some(comps) = world.get_components_mut(id) {
        flint_animation::layer_edit::edit_layer_table(comps, index as usize, edit);
    }
    Ok(())
}
