//! The `wooze` module: the scene's wooze field (#213), which every living
//! prop and shallow-wooze mesh reads: its global knobs and its named zones
//! (ADR 0116). A subsystem's module: #698 gates it behind the field.
//!
//! Each takes an optional last argument of seconds to get there over (an
//! int or a float); without it the change is immediate. The commands take
//! effect when the frame's commands run and reach a living prop on the next
//! cycle of each of its parts, or at once for step, which bends the clock.
//! What's wrong with a table is logged by the player, naming the zone; a
//! scene with no `[wooze]` table has no field to change, and says so.

use super::builder::kind::{Map, Num, Str};
use super::builder::{Call, ScriptApi};
use super::convert::map_to_toml;
use crate::context::ScriptCommand;

fn set_wooze(call: &mut Call<'_>, knobs: &rhai::Map, seconds: f64) {
    call.ctx().commands.push(ScriptCommand::SetWooze {
        knobs: map_to_toml(knobs),
        seconds,
    });
}

fn set_zone(call: &mut Call<'_>, name: &str, fields: &rhai::Map, seconds: f64) {
    call.ctx().commands.push(ScriptCommand::SetWoozeZone {
        name: name.to_string(),
        fields: map_to_toml(fields),
        seconds,
    });
}

fn remove_zone(call: &mut Call<'_>, name: &str, seconds: f64) {
    call.ctx().commands.push(ScriptCommand::RemoveWoozeZone {
        name: name.to_string(),
        seconds,
    });
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("wooze");

    m.func(
        "set_wooze",
        "Change the field's global knobs at once: any of amplitude, step, glide, jitter, preset.",
    )
    .arg("knobs", Map)
    .run(|call, (knobs,)| set_wooze(call, &knobs, 0.0));

    m.func(
        "set_wooze",
        "Change the field's global knobs over some seconds.",
    )
    .arg("knobs", Map)
    .arg("seconds", Num)
    .run(|call, (knobs, seconds)| set_wooze(call, &knobs, seconds));

    m.func(
        "set_wooze_zone",
        "Change a named zone at once, keeping what fields leaves out, or add it (shape, center, radius or size, a knob).",
    )
    .arg("name", Str)
    .arg("fields", Map)
    .run(|call, (name, fields)| set_zone(call, &name, &fields, 0.0));

    m.func(
        "set_wooze_zone",
        "Change or add a named zone, its knobs easing and a new zone fading in over some seconds.",
    )
    .arg("name", Str)
    .arg("fields", Map)
    .arg("seconds", Num)
    .run(|call, (name, fields, seconds)| set_zone(call, &name, &fields, seconds));

    m.func("remove_wooze_zone", "Drop a named zone at once.")
        .arg("name", Str)
        .run(|call, (name,)| remove_zone(call, &name, 0.0));

    m.func(
        "remove_wooze_zone",
        "Fade a named zone out over some seconds, then drop it.",
    )
    .arg("name", Str)
    .arg("seconds", Num)
    .run(|call, (name, seconds)| remove_zone(call, &name, seconds));
}
