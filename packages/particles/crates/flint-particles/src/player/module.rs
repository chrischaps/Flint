//! The `particle` module: an entity's particle emitter, and detached
//! one-shot effects from `particles/<name>.particles.toml` (ADR 0068),
//! addressed by the handle `play_effect` returns. ADR 0116.
//!
//! The particles package's since #1622: what changes the simulation is a
//! module command ([`Particle`]) the package's subsystem takes, at the drain
//! the core's `EmitBurst` and the rest were taken at before, so no frame
//! changes. Enabled in every scene the package is in (its subsystem is
//! built for every scene, as an effect needs no emitter in the scene).

use flint_core::components as comp;
use flint_core::EntityId;
use flint_script::api::builder::kind::{Entity, Int, Num, Str};
use flint_script::api::{Call, CallResult, Module};
use flint_script::context::ScriptCommand;
use flint_script::{ModuleCommand, ScriptModule};
use std::sync::atomic::{AtomicU64, Ordering};

/// The module's name.
pub const MODULE: &str = "particle";

/// What a `particle` call asks of the simulation, carried out by the
/// subsystem where the core carried it out before.
#[derive(Debug, Clone, PartialEq)]
pub enum Particle {
    /// `emit_burst(id, count)`: fire `count` from the entity's emitter.
    Burst { entity: EntityId, count: i64 },
    /// `play_effect(name, x, y, z)`: a detached effect at a world point.
    Play {
        handle: u64,
        name: String,
        position: (f64, f64, f64),
    },
    /// `stop_effect(handle)`: its emission ends; particles in flight finish.
    Stop { handle: u64 },
    /// `set_effect_param(handle, param, value)`.
    Param {
        handle: u64,
        param: String,
        value: f64,
    },
}

/// Where `play_effect`'s handles come from; 0 means none, so it starts at 1.
static NEXT_EFFECT_HANDLE: AtomicU64 = AtomicU64::new(1);

/// A handle as the subsystem takes it: a negative one is no effect's.
fn handle(h: i64) -> u64 {
    h.max(0) as u64
}

pub struct ParticleModule;

pub static PARTICLE_MODULE: ParticleModule = ParticleModule;

fn push(call: &mut Call<'_>, what: Particle) {
    let function = call.name();
    call.ctx()
        .commands
        .push(ScriptCommand::Module(ModuleCommand::new(function, what)));
}

impl ScriptModule for ParticleModule {
    fn name(&self) -> &'static str {
        MODULE
    }

    fn enabled_when(&self) -> &'static str {
        "the particles package is in the build"
    }

    fn declare(&self, m: &mut Module<'_>) {
        m.func(
            "emit_burst",
            "Fire a number of particles from the entity's emitter at once.",
        )
        .arg("id", Entity)
        .arg("count", Int)
        .run(|call, (entity, count)| push(call, Particle::Burst { entity, count }));

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
            push(
                call,
                Particle::Play {
                    handle,
                    name: name.to_string(),
                    position: (x, y, z),
                },
            );
            handle as i64
        });

        m.func(
            "stop_effect",
            "Stop a detached effect's emission; particles in flight finish.",
        )
        .arg("handle", Int)
        .run(|call, (h,)| push(call, Particle::Stop { handle: handle(h) }));

        m.func(
            "set_effect_param",
            "Tune a detached effect: emission_scale, scale, playing (above 0.5 is on), x, y or z.",
        )
        .arg("handle", Int)
        .arg("param", Str)
        .arg("value", Num)
        .run(|call, (h, param, value)| {
            push(
                call,
                Particle::Param {
                    handle: handle(h),
                    param: param.to_string(),
                    value,
                },
            )
        });
    }
}

/// Write one field of the entity's `particle_emitter`.
fn set_emitter(
    call: &mut Call<'_>,
    id: EntityId,
    field: &str,
    value: toml::Value,
) -> CallResult<()> {
    if let Some(mut comps) = call.world_mut()?.get_components_mut(id) {
        comps.set_field(comp::PARTICLE_EMITTER, field, value);
    }
    Ok(())
}
