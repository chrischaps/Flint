//! The `living` module: Living Props' knobs (#218), the F3 panel's by
//! another door, for every prop at once or one species (#246), and the
//! clock everything woozy runs on (ADR 0116). The A/B comparison and the
//! commit to the scene file are the `debug` module's. A subsystem's
//! module: #698 gates it behind living props.
//!
//! Each is a command the player runs after the batch; what it can't use is
//! logged, naming the key, and the rest still happens.

use super::builder::kind::{Array, Bool, Map, Str};
use super::builder::ScriptApi;
use super::convert::map_to_toml;
use crate::context::ScriptCommand;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("living");

    m.func(
        "tune_living",
        "Turn Living Props knobs: what a .wooze.toml holds as a map, or one species' own values with species: name.",
    )
    .arg("knobs", Map)
    .run(|call, (knobs,)| {
        call.ctx().commands.push(ScriptCommand::TuneLiving {
            knobs: map_to_toml(&knobs),
        });
    });

    m.func(
        "clear_living",
        "Give all of a species' own values back to the layers below.",
    )
    .arg("species", Str)
    .run(|call, (species,)| {
        call.ctx().commands.push(ScriptCommand::ClearLiving {
            species: species.to_string(),
            keys: Vec::new(),
        });
    });

    m.func(
        "clear_living",
        "Give the keys named of a species' own values back to the layers below ([\"step\", \"echoes.count\"]).",
    )
    .arg("species", Str)
    .arg("keys", Array)
    .run(|call, (species, keys)| {
        call.ctx().commands.push(ScriptCommand::ClearLiving {
            species: species.to_string(),
            keys: keys.iter().map(|key| key.to_string()).collect(),
        });
    });

    m.func(
        "freeze_living",
        "Stop or start the clock everything woozy runs on; the game goes on.",
    )
    .arg("frozen", Bool)
    .run(|call, (frozen,)| {
        call.ctx()
            .commands
            .push(ScriptCommand::FreezeLiving { frozen });
    });

    m.func(
        "step_living",
        "Move the woozy clock on by exactly one beat, for use while frozen.",
    )
    .run(|call, ()| call.ctx().commands.push(ScriptCommand::StepLiving));

    m.func(
        "save_living_preset",
        "Save the knobs the props agree on, and each species' own, as wooze/<name>.wooze.toml (not on the web).",
    )
    .arg("name", Str)
    .run(|call, (name,)| {
        call.ctx().commands.push(ScriptCommand::SaveLivingPreset {
            name: name.to_string(),
        });
    });

    m.func(
        "load_living_preset",
        "Load a .wooze.toml preset or a built-in, live.",
    )
    .arg("name", Str)
    .run(|call, (name,)| {
        call.ctx().commands.push(ScriptCommand::LoadLivingPreset {
            name: name.to_string(),
        });
    });
}
