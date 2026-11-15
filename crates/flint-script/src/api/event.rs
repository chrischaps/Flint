//! The `event` module: game events a script fires, which every script's
//! `on_event(name, data)` hears next frame (ADR 0116).

use super::builder::kind::{Map, Str};
use super::builder::ScriptApi;
use crate::context::ScriptCommand;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("event");

    m.func(
        "fire_event",
        "Fire a named game event with an empty data map, heard by every on_event next frame.",
    )
    .arg("name", Str)
    .run(|call, (name,)| {
        call.ctx().commands.push(ScriptCommand::FireEvent {
            name: name.to_string(),
            data: toml::Value::Table(toml::map::Map::new()),
        });
    });

    m.func(
        "fire_event_data",
        "Fire a named game event with a data map (booleans, numbers, strings, arrays, maps; () is dropped).",
    )
    .arg("name", Str)
    .arg("data", Map)
    .run(|call, (name, data)| {
        let data = super::map_to_toml(&data);
        call.ctx().commands.push(ScriptCommand::FireEvent {
            name: name.to_string(),
            data,
        });
    });
}
