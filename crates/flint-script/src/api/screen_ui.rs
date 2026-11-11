//! The `screen_ui` module: the screen-space UI components on entities
//! (`ui_fill`, `ui_text`, `screen_anchor`), set from a script (ADR 0116).

use super::builder::kind::{Entity, Num, Str};
use super::builder::ScriptApi;
use flint_core::components as comp;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("screen_ui");

    m.func(
        "ui_set_value",
        "Set an entity's ui_fill value (a bar's fill, 0..1).",
    )
    .arg("id", Entity)
    .arg("value", Num)
    .run(|call, (id, value)| {
        if let Some(comps) = call.world_mut()?.get_components_mut(id) {
            comps.set_field(comp::UI_FILL, "value", toml::Value::Float(value));
        }
        Ok(())
    });

    m.func("set_text", "Set an entity's ui_text text.")
        .arg("id", Entity)
        .arg("text", Str)
        .run(|call, (id, text)| {
            if let Some(comps) = call.world_mut()?.get_components_mut(id) {
                comps.set_field(comp::UI_TEXT, "text", toml::Value::String(text.to_string()));
            }
            Ok(())
        });

    m.func("set_text_color", "Set an entity's ui_text colour, 0..1.")
        .arg("id", Entity)
        .arg("r", Num)
        .arg("g", Num)
        .arg("b", Num)
        .arg("a", Num)
        .run(|call, (id, r, g, b, a)| {
            if let Some(comps) = call.world_mut()?.get_components_mut(id) {
                let color =
                    toml::Value::Array([r, g, b, a].into_iter().map(toml::Value::Float).collect());
                comps.set_field(comp::UI_TEXT, "color", color);
            }
            Ok(())
        });

    m.func(
        "set_anchor",
        "Set an entity's screen_anchor anchor (\"top-left\", \"center\", \"bottom-right\", ...).",
    )
    .arg("id", Entity)
    .arg("anchor", Str)
    .run(|call, (id, anchor)| {
        if let Some(comps) = call.world_mut()?.get_components_mut(id) {
            comps.set_field(
                comp::SCREEN_ANCHOR,
                "anchor",
                toml::Value::String(anchor.to_string()),
            );
        }
        Ok(())
    });

    m.func(
        "set_anchor_offset",
        "Set an entity's screen_anchor offset from its anchor, in world units.",
    )
    .arg("id", Entity)
    .arg("x", Num)
    .arg("y", Num)
    .run(|call, (id, x, y)| {
        if let Some(comps) = call.world_mut()?.get_components_mut(id) {
            comps.set_field(comp::SCREEN_ANCHOR, "offset_x", toml::Value::Float(x));
            comps.set_field(comp::SCREEN_ANCHOR, "offset_y", toml::Value::Float(y));
        }
        Ok(())
    });
}
