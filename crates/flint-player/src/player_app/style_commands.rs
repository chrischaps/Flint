//! The style commands scripts queue (#269): `set_entity_style`,
//! `set_style_param` and `clear_entity_style`, applied to the world by
//! `flint_render::entity_style`. The renderer reads the world after
//! scripts each frame, so a change shows the frame it was made.
//!
//! The whole-frame ones (#369), `set_post_style`, `set_post_style_param`
//! and `reset_post_style`, edit the renderer's post-process config through
//! `flint_render::post_style` instead.

use flint_core::EntityId;
use flint_ecs::FlintWorld;
use flint_render::{entity_style, post_style, PostProcessConfig};
use flint_runtime::GameEvent;
use flint_script::context::ScriptCommand;

/// The event a style change fires to scripts' `on_event`, with
/// `#{ entity, style }` (`style` is `""` once cleared).
pub const STYLE_CHANGED: &str = "style_changed";

/// The event a whole-frame style change fires (#369), with
/// `#{ style, enabled }`: on `set_post_style` and `reset_post_style`.
pub const POST_STYLE_CHANGED: &str = "post_style_changed";

/// What applying one command did.
#[derive(Debug)]
pub enum StyleApplied {
    /// Not a style command: the caller handles it.
    NotAStyleCommand,
    /// Applied. Setting or clearing a style hands back the `style_changed`
    /// event to fire; a param change fires none (it may come every frame).
    Done(Option<GameEvent>),
    /// Refused, with what to tell the script's author.
    Failed(String),
}

fn changed(entity_id: i64, style: &str) -> GameEvent {
    let mut data = toml::Table::new();
    data.insert("entity".into(), toml::Value::Integer(entity_id));
    data.insert("style".into(), style.into());
    GameEvent::Custom {
        name: STYLE_CHANGED.into(),
        data: toml::Value::Table(data),
    }
}

/// Apply `cmd` to `world` if it's a style command.
pub fn apply_style_command(world: &mut FlintWorld, cmd: &ScriptCommand) -> StyleApplied {
    let id = |e: i64| EntityId(e.max(0) as u64);
    match cmd {
        ScriptCommand::SetEntityStyle { entity_id, style } => {
            match entity_style::set_entity_style(world, id(*entity_id), style) {
                Ok(()) => StyleApplied::Done(Some(changed(*entity_id, style))),
                Err(e) => StyleApplied::Failed(format!("set_entity_style: {e}")),
            }
        }
        ScriptCommand::SetStyleParam {
            entity_id,
            param,
            value,
        } => match entity_style::set_style_param(world, id(*entity_id), param, value.clone()) {
            Ok(_) => StyleApplied::Done(None),
            Err(e) => StyleApplied::Failed(format!("set_style_param: {e}")),
        },
        ScriptCommand::ClearEntityStyle { entity_id } => {
            let had = entity_style::clear_entity_style(world, id(*entity_id));
            StyleApplied::Done(had.then(|| changed(*entity_id, "")))
        }
        _ => StyleApplied::NotAStyleCommand,
    }
}

/// Whether `cmd` is a whole-frame style command (#369), which
/// [`apply_post_style_command`] applies to the renderer's config.
pub fn is_post_style_command(cmd: &ScriptCommand) -> bool {
    matches!(
        cmd,
        ScriptCommand::SetPostStyle { .. }
            | ScriptCommand::SetPostStyleParam { .. }
            | ScriptCommand::ResetPostStyle { .. }
    )
}

fn post_changed(style: &str, config: &PostProcessConfig) -> GameEvent {
    let mut data = toml::Table::new();
    data.insert("style".into(), style.into());
    data.insert("enabled".into(), config.styles.is_enabled(style).into());
    GameEvent::Custom {
        name: POST_STYLE_CHANGED.into(),
        data: toml::Value::Table(data),
    }
}

/// Apply a whole-frame style command (#369) to `config`, the renderer's
/// post-process config, to be handed back with `set_post_process_config`.
/// `authored` gives the scene's own config, asked for only on a reset.
pub fn apply_post_style_command(
    config: &mut PostProcessConfig,
    authored: impl FnOnce() -> PostProcessConfig,
    cmd: &ScriptCommand,
) -> StyleApplied {
    match cmd {
        ScriptCommand::SetPostStyle { style, enabled } => {
            match post_style::set_post_style(config, style, *enabled) {
                Ok(()) => StyleApplied::Done(Some(post_changed(style, config))),
                Err(e) => StyleApplied::Failed(format!("set_post_style: {e}")),
            }
        }
        ScriptCommand::SetPostStyleParam {
            style,
            param,
            value,
        } => match post_style::set_post_style_param(config, style, param, value.clone()) {
            Ok(()) => StyleApplied::Done(None),
            Err(e) => StyleApplied::Failed(format!("set_post_style_param: {e}")),
        },
        ScriptCommand::ResetPostStyle { style } => {
            match post_style::reset_post_style(config, &authored(), style) {
                Ok(()) => StyleApplied::Done(Some(post_changed(style, config))),
                Err(e) => StyleApplied::Failed(format!("reset_post_style: {e}")),
            }
        }
        _ => StyleApplied::NotAStyleCommand,
    }
}
