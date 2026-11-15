//! The `panels` module (#880, ADR 0120): lay the frame out as comic panels,
//! each with its own camera and look, and cut between layouts. ADR 0116.
//!
//! A layout is the scene's `[panels]` table as a map; a panel is one of its
//! `[panels.panel.<name>]` tables. The commands queue: the next frame is
//! drawn as the new page.

use super::builder::kind::{Map, Str};
use super::builder::ScriptApi;
use super::convert::map_to_toml;
use crate::context::ScriptCommand;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("panels");

    m.func(
        "set_panels",
        "Lay the frame out as comic panels: a map shaped like the scene's [panels] table (gutter, border, and panel: a map of panels by name).",
    )
    .arg("layout", Map)
    .run(|call, (layout,)| {
        if let toml::Value::Table(layout) = map_to_toml(&layout) {
            call.ctx().commands.push(ScriptCommand::SetPanels { layout });
        }
    });

    m.func(
        "set_panel",
        "Set one panel's keys (rect, position, target, fov, follow, look, post_process, order); a new name adds a panel.",
    )
    .arg("name", Str)
    .arg("keys", Map)
    .run(|call, (name, keys)| {
        if let toml::Value::Table(keys) = map_to_toml(&keys) {
            call.ctx().commands.push(ScriptCommand::SetPanel {
                name: name.to_string(),
                keys,
            });
        }
    });

    m.func(
        "remove_panel",
        "Take one panel off the page; with the last gone the frame is one view again.",
    )
    .arg("name", Str)
    .run(|call, (name,)| {
        call.ctx().commands.push(ScriptCommand::RemovePanel {
            name: name.to_string(),
        });
    });

    m.func(
        "clear_panels",
        "Draw the frame as one view again, with no panels.",
    )
    .run(|call, ()| {
        call.ctx().commands.push(ScriptCommand::ClearPanels);
    });
}
