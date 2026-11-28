//! The `sprite` module: an entity's `sprite` component (its source
//! rectangle, flip, tint, layer, visibility) (ADR 0116). Its clips are the
//! flipbook package's `flipbook` module since #1620 (`play_sprite` and its
//! family, ADR 0139 section 7). The sprite setters change an entity that has a
//! `sprite` and leave one without it alone.

use super::builder::kind::{Bool, Entity, Int, Num};
use super::builder::ScriptApi;
use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;

/// Set fields of the entity's `sprite` table, when it has one.
fn set_sprite(world: &mut FlintWorld, id: EntityId, fields: &[(&str, toml::Value)]) {
    let table = world
        .get_component_mut(id, comp::SPRITE)
        .and_then(|sprite| sprite.as_table_mut());
    if let Some(table) = table {
        for (key, value) in fields {
            table.insert(key.to_string(), value.clone());
        }
    }
}

fn floats(v: [f64; 4]) -> toml::Value {
    toml::Value::Array(v.into_iter().map(toml::Value::Float).collect())
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("sprite");

    m.func(
        "set_sprite_source_rect",
        "Set the part of its image an entity's sprite shows, in pixels.",
    )
    .arg("id", Entity)
    .arg("x", Num)
    .arg("y", Num)
    .arg("w", Num)
    .arg("h", Num)
    .run(|call, (id, x, y, w, h)| {
        set_sprite(
            call.world_mut()?,
            id,
            &[("source_rect", floats([x, y, w, h]))],
        );
        Ok(())
    });

    m.func(
        "set_sprite_flip",
        "Mirror an entity's sprite across x, y or both.",
    )
    .arg("id", Entity)
    .arg("flip_x", Bool)
    .arg("flip_y", Bool)
    .run(|call, (id, flip_x, flip_y)| {
        set_sprite(
            call.world_mut()?,
            id,
            &[
                ("flip_x", toml::Value::Boolean(flip_x)),
                ("flip_y", toml::Value::Boolean(flip_y)),
            ],
        );
        Ok(())
    });

    m.func("set_sprite_tint", "Tint an entity's sprite, 0..1.")
        .arg("id", Entity)
        .arg("r", Num)
        .arg("g", Num)
        .arg("b", Num)
        .arg("a", Num)
        .run(|call, (id, r, g, b, a)| {
            set_sprite(call.world_mut()?, id, &[("tint", floats([r, g, b, a]))]);
            Ok(())
        });

    m.func(
        "set_sprite_layer",
        "Set an entity's sprite layer (higher draws in front).",
    )
    .arg("id", Entity)
    .arg("layer", Int)
    .run(|call, (id, layer)| {
        set_sprite(
            call.world_mut()?,
            id,
            &[("layer", toml::Value::Integer(layer))],
        );
        Ok(())
    });

    m.func("set_sprite_visible", "Show or hide an entity's sprite.")
        .arg("id", Entity)
        .arg("visible", Bool)
        .run(|call, (id, visible)| {
            set_sprite(
                call.world_mut()?,
                id,
                &[("visible", toml::Value::Boolean(visible))],
            );
            Ok(())
        });

    m.func("get_sprite_layer", "An entity's sprite layer.")
        .arg("id", Entity)
        .returns(Int, "0 without a sprite or a layer")
        .run(|call, (id,)| Ok(call.world()?.view(id, comp::SPRITE).i64("layer", 0)));
}
