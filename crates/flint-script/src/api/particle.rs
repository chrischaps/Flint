//! The `particle` module: an entity's particle emitter, and detached
//! one-shot effects from `particles/<name>.particles.toml` (ADR 0068),
//! addressed by the handle `play_effect` returns. ADR 0116.

use super::builder::kind::{Entity, Int, Num, Str};
use super::builder::{CallResult, ScriptApi};
use crate::context::ScriptCommand;
use flint_core::components as comp;
use std::sync::atomic::{AtomicU64, Ordering};

/// Where `play_effect`'s handles come from; 0 means none, so it starts at 1.
static NEXT_EFFECT_HANDLE: AtomicU64 = AtomicU64::new(1);

/// A handle as the host takes it: a negative one is no effect's.
fn handle(h: i64) -> u64 {
    h.max(0) as u64
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("particle");

    m.func(
        "emit_burst",
        "Fire a number of particles from the entity's emitter at once.",
    )
    .arg("id", Entity)
    .arg("count", Int)
    .run(|call, (entity, count)| {
        call.ctx()
            .commands
            .push(ScriptCommand::EmitBurst { entity, count });
    });

    m.func("start_emitter", "Start the entity's particle emitter.")
        .arg("id", Entity)
        .run(|call, (id,)| set_emitter(call, id, "playing", toml::Value::Boolean(true)));

    m.func(
        "stop_emitter",
        "Stop the entity's particle emitter; particles in flight finish.",
    )
    .arg("id", Entity)
    .run(|call, (id,)| set_emitter(call, id, "playing", toml::Value::Boolean(false)));

    m.func(
        "set_emission_rate",
        "Set the entity's emitter's rate, in particles a second.",
    )
    .arg("id", Entity)
    .arg("rate", Num)
    .run(|call, (id, rate)| set_emitter(call, id, "emission_rate", toml::Value::Float(rate)));

    m.func(
        "play_effect",
        "Play a detached one-shot particle effect (particles/<name>.particles.toml) at a world point.",
    )
    .arg("name", Str)
    .arg("x", Num)
    .arg("y", Num)
    .arg("z", Num)
    .returns(Int, "its handle, for stop_effect and set_effect_param")
    .run(|call, (name, x, y, z)| {
        let handle = NEXT_EFFECT_HANDLE.fetch_add(1, Ordering::Relaxed);
        call.ctx().commands.push(ScriptCommand::PlayEffect {
            handle,
            name: name.to_string(),
            position: (x, y, z),
        });
        handle as i64
    });

    m.func(
        "stop_effect",
        "Stop a detached effect's emission; particles in flight finish.",
    )
    .arg("handle", Int)
    .run(|call, (h,)| {
        call.ctx()
            .commands
            .push(ScriptCommand::StopEffect { handle: handle(h) });
    });

    m.func(
        "set_effect_param",
        "Tune a detached effect: emission_scale, scale, playing (above 0.5 is on), x, y or z.",
    )
    .arg("handle", Int)
    .arg("param", Str)
    .arg("value", Num)
    .run(|call, (h, param, value)| {
        call.ctx().commands.push(ScriptCommand::SetEffectParam {
            handle: handle(h),
            param: param.to_string(),
            value,
        });
    });
}

/// Write one field of the entity's `particle_emitter`.
fn set_emitter(
    call: &mut super::builder::Call<'_>,
    id: flint_core::EntityId,
    field: &str,
    value: toml::Value,
) -> CallResult<()> {
    if let Some(comps) = call.world_mut()?.get_components_mut(id) {
        comps.set_field(comp::PARTICLE_EMITTER, field, value);
    }
    Ok(())
}
