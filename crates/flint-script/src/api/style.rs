//! The `style` module: an entity's stylize style (#269), whole-frame
//! styles (#369) and looks (#356), and reading a style back (#372).
//! ADR 0116.
//!
//! The setters queue commands the player applies after the frame's
//! scripts, before the renderer reads the world, firing `style_changed` /
//! `post_style_changed` to `on_event`. A param can change every frame: a
//! uniform write, never a GPU allocation. The getters ask the host's
//! `StyleReader` and see the world as the script finds it, so a style set
//! this frame reads back from the next; without a reader every entity is
//! unstyled.

use super::builder::kind::{Any, Bool, Entity, Num, Str};
use super::builder::{Call, CallResult, ScriptApi};
use super::convert::{dynamic_to_toml, toml_to_dynamic};
use crate::context::{LogLevel, ScriptCommand};
use crate::style_reader::StyleReader;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use rhai::{Dynamic, ImmutableString};

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("style");

    // ── An entity's style ──
    m.func(
        "set_entity_style",
        "Draw the entity and its descendants in a style (\"ascii\", ...); settings it had are kept.",
    )
    .arg("id", Entity)
    .arg("style", Str)
    .run(|call, (entity, style)| {
        call.ctx().commands.push(ScriptCommand::SetEntityStyle {
            entity,
            style: style.to_string(),
        });
    });

    m.func(
        "set_style_param",
        "Set one of the entity's style settings: a number, a string, enabled, blend (0..1) or a colour.",
    )
    .arg("id", Entity)
    .arg("param", Str)
    .arg("value", Any)
    .run(|call, (entity, param, value)| {
        let command = match dynamic_to_toml(&value) {
            Some(value) => ScriptCommand::SetStyleParam {
                entity,
                param: param.to_string(),
                value,
            },
            None => warn(format!(
                "set_style_param: `{param}` can't take a {}",
                value.type_name()
            )),
        };
        call.ctx().commands.push(command);
    });

    m.func(
        "tween_style_param",
        "Ease one of the entity's style settings (a number, blend or a colour) to a target over game seconds, smoothly.",
    )
    .arg("id", Entity)
    .arg("param", Str)
    .arg("target", Any)
    .arg("seconds", Num)
    .run(|call, (entity, param, target, seconds)| {
        tween(call, entity, &param, &target, seconds, "smooth");
    });

    m.func(
        "tween_style_param",
        "Ease one of the entity's style settings to a target over game seconds: linear, smooth, ease_in, ease_out or ease_in_out.",
    )
    .arg("id", Entity)
    .arg("param", Str)
    .arg("target", Any)
    .arg("seconds", Num)
    .arg("easing", Str)
    .run(|call, (entity, param, target, seconds, easing)| {
        tween(call, entity, &param, &target, seconds, &easing);
    });

    m.func(
        "clear_entity_style",
        "Take the entity's own style off, and stop its style tweens.",
    )
    .arg("id", Entity)
    .run(|call, (entity,)| {
        call.ctx()
            .commands
            .push(ScriptCommand::ClearEntityStyle { entity });
    });

    // ── Reading it back ──
    m.func(
        "get_entity_style",
        "The style the entity is drawn in: its own, an ancestor's, or the scene's whole-frame style.",
    )
    .arg("id", Entity)
    .returns(Str, "the style's name; \"\" for none")
    .run(|call, (id,)| -> CallResult<String> {
        Ok(read_style(call, id, |r, w, id| r.entity_style(w, id))?
            .map(|s| s.style)
            .unwrap_or_default())
    });

    m.func(
        "get_entity_style_variant",
        "The variant of the style the entity is drawn in, named for the settings that set it apart.",
    )
    .arg("id", Entity)
    .returns(Str, "\"ascii{ramp=blocks}\", \"ascii@barrel\"; \"\" for none")
    .run(|call, (id,)| -> CallResult<String> {
        Ok(read_style(call, id, |r, w, id| r.entity_style(w, id))?
            .map(|s| s.variant)
            .unwrap_or_default())
    });

    m.func(
        "get_style_param",
        "The value one of the entity's style settings has: its own, an ancestor's, the scene's, or the default.",
    )
    .arg("id", Entity)
    .arg("param", Str)
    .returns(
        Any,
        "a number, a string, [r, g, b] or a bool; () when unstyled or the style has no such setting",
    )
    .run(|call, (id, param)| -> CallResult<Dynamic> {
        Ok(read_style(call, id, |r, w, id| r.style_param(w, id, &param))?
            .map(toml_to_dynamic)
            .unwrap_or(Dynamic::UNIT))
    });

    // ── Whole-frame styles ──
    m.func(
        "set_post_style",
        "Switch a whole-frame style ([post_process.styles.<name>]) on or off; settings it had are kept.",
    )
    .arg("style", Str)
    .arg("enabled", Bool)
    .run(|call, (style, enabled)| {
        call.ctx().commands.push(ScriptCommand::SetPostStyle {
            style: style.to_string(),
            enabled,
        });
    });

    m.func(
        "set_post_style_param",
        "Set one of a whole-frame style's settings: a number, a string, enabled, blend (0..1) or a colour.",
    )
    .arg("style", Str)
    .arg("param", Str)
    .arg("value", Any)
    .run(|call, (style, param, value)| {
        let command = match dynamic_to_toml(&value) {
            Some(value) => ScriptCommand::SetPostStyleParam {
                style: style.to_string(),
                param: param.to_string(),
                value,
            },
            None => warn(format!(
                "set_post_style_param: `{param}` can't take a {}",
                value.type_name()
            )),
        };
        call.ctx().commands.push(command);
    });

    m.func(
        "tween_post_style_param",
        "Ease a whole-frame style's number (reveal, blend, a reveal_ key or a param) to a target over game seconds, smoothly.",
    )
    .arg("style", Str)
    .arg("param", Str)
    .arg("target", Num)
    .arg("seconds", Num)
    .run(|call, (style, param, target, seconds)| {
        post_tween(call, &style, &param, target, seconds, "smooth");
    });

    m.func(
        "tween_post_style_param",
        "Ease a whole-frame style's number to a target over game seconds: linear, smooth, ease_in, ease_out or ease_in_out.",
    )
    .arg("style", Str)
    .arg("param", Str)
    .arg("target", Num)
    .arg("seconds", Num)
    .arg("easing", Str)
    .run(|call, (style, param, target, seconds, easing)| {
        post_tween(call, &style, &param, target, seconds, &easing);
    });

    m.func(
        "reset_post_style",
        "Put a whole-frame style back to what the scene authored.",
    )
    .arg("style", Str)
    .run(|call, (style,)| {
        call.ctx().commands.push(ScriptCommand::ResetPostStyle {
            style: style.to_string(),
        });
    });

    m.func(
        "reset_style_history",
        "The next frame doesn't follow on from this one (a cut, a teleport): styles that remember frames start over.",
    )
    .run(|call, ()| {
        call.ctx().commands.push(ScriptCommand::ResetStyleHistory);
    });

    m.func(
        "set_camera_cut_detection",
        "Whether the player resets style history itself when the camera jumps (on by default).",
    )
    .arg("enabled", Bool)
    .run(|call, (enabled,)| {
        call.ctx()
            .commands
            .push(ScriptCommand::SetCameraCutDetection { enabled });
    });

    // ── Looks ──
    m.func(
        "set_look",
        "Lay a post-processing look (styles/<name>.look.toml or a built-in) under the scene's keys; \"\" for none.",
    )
    .arg("name", Str)
    .run(|call, (name,)| {
        call.ctx().commands.push(ScriptCommand::SetLook {
            name: look_name(&name),
        });
    });

    m.func(
        "clear_look",
        "Take the look off: the scene's own post-processing keys alone.",
    )
    .run(|call, ()| {
        call.ctx()
            .commands
            .push(ScriptCommand::SetLook { name: None });
    });
}

/// A warning for the game log, queued in place of a command that can't be.
fn warn(message: String) -> ScriptCommand {
    ScriptCommand::Log {
        level: LogLevel::Warn,
        message,
    }
}

/// `set_look`'s name: trimmed, and none when that leaves nothing.
fn look_name(name: &ImmutableString) -> Option<String> {
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

fn tween(
    call: &mut Call<'_>,
    entity: EntityId,
    param: &str,
    target: &Dynamic,
    seconds: f64,
    easing: &str,
) {
    let command = match dynamic_to_toml(target) {
        Some(target) => ScriptCommand::TweenStyleParam {
            entity,
            param: param.to_string(),
            target,
            seconds,
            easing: easing.to_string(),
        },
        None => warn(format!(
            "tween_style_param: `{param}` can't ease to a {}",
            target.type_name()
        )),
    };
    call.ctx().commands.push(command);
}

fn post_tween(
    call: &mut Call<'_>,
    style: &str,
    param: &str,
    target: f64,
    seconds: f64,
    easing: &str,
) {
    call.ctx()
        .commands
        .push(ScriptCommand::TweenPostStyleParam {
            style: style.to_string(),
            param: param.to_string(),
            target,
            seconds,
            easing: easing.to_string(),
        });
}

/// Ask the host's style reader about `id`; `None` without a reader.
fn read_style<T>(
    call: &mut Call<'_>,
    id: EntityId,
    f: impl FnOnce(&dyn StyleReader, &FlintWorld, EntityId) -> Option<T>,
) -> CallResult<Option<T>> {
    let Some(reader) = call.ctx_ref().style_reader.clone() else {
        return Ok(None);
    };
    Ok(f(reader.as_ref(), call.world()?, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_look_name_is_trimmed_and_empty_is_none() {
        assert_eq!(look_name(&" graphite ".into()), Some("graphite".into()));
        assert_eq!(look_name(&"  ".into()), None);
    }
}
