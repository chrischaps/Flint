//! The `tween` module (#734): ease any numeric component field over game
//! time, the one call for what used to be a hand-rolled `on_update` lerp (a
//! light's intensity, a material's colour, a camera's field of view).
//!
//! The call checks what it can at once, so a mistake is a script error at its
//! line: the path's shape, the easing, and (with a world lent) that the field
//! is there and holds what the target is. It then queues a command; the host
//! starts the tween from where the field stands (`crate::tween::FieldTweens`)
//! and steps it with the scripts.

use super::builder::kind::{Any, Entity, Num, Str};
use super::builder::{Call, CallResult, ScriptApi};
use super::convert::dynamic_to_toml;
use crate::context::ScriptCommand;
use crate::tween::{endpoints, FieldPath};
use flint_core::tween::Easing;
use flint_core::EntityId;
use rhai::Dynamic;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("tween");

    m.func(
        "tween",
        "Ease a numeric component field (\"light.intensity\", \"material.color\") to a target over game seconds, smoothly; fires tween_done.",
    )
    .arg("id", Entity)
    .arg("path", Str)
    .arg("target", Any)
    .arg("seconds", Num)
    .run(|call, (entity, path, target, seconds)| {
        tween(call, entity, &path, &target, seconds, Easing::Smooth)
    });

    m.func(
        "tween",
        "Ease a numeric component field to a target over game seconds: linear, smooth, ease_in, ease_out or ease_in_out.",
    )
    .arg("id", Entity)
    .arg("path", Str)
    .arg("target", Any)
    .arg("seconds", Num)
    .arg("easing", Str)
    .run(|call, (entity, path, target, seconds, easing)| {
        let easing =
            Easing::from_name(&easing).ok_or_else(|| call.error(Easing::refusal(&easing)))?;
        tween(call, entity, &path, &target, seconds, easing)
    });
}

fn tween(
    call: &mut Call<'_>,
    entity: EntityId,
    path: &str,
    target: &Dynamic,
    seconds: f64,
    easing: Easing,
) -> CallResult<()> {
    let field = FieldPath::parse(path).map_err(|e| call.error(e))?;
    let target = dynamic_to_toml(target)
        .ok_or_else(|| call.error(format!("`{path}` can't ease to a {}", target.type_name())))?;
    // A script's top level at load has no world: the host checks then
    let checked = call
        .world()
        .ok()
        .map(|world| endpoints(world, entity, &field, &target));
    if let Some(Err(e)) = checked {
        return Err(call.error(e));
    }
    call.ctx().commands.push(ScriptCommand::Tween {
        entity,
        path: field.dotted(),
        target,
        seconds,
        easing,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::context::{ScriptCommand, WorldScope};
    use crate::engine::ScriptEngine;
    use flint_core::tween::Easing;
    use flint_ecs::FlintWorld;

    fn run(engine: &ScriptEngine, world: &mut FlintWorld, src: &str) -> Result<(), String> {
        let _scope = WorldScope::new(&engine.ctx, world);
        (engine.rhai_engine().eval::<()>(src)).map_err(|e| e.to_string())
    }

    /// A mistake is a script error at the call, naming the field; a good
    /// call queues one command with the path as written.
    #[test]
    fn a_tween_says_what_it_cant_ease_at_the_call() {
        let engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let cube = world.spawn("cube").unwrap();
        let origin = toml::Value::Array(vec![toml::Value::Float(0.0); 3]);
        world
            .set_field(cube, "transform", "position", origin)
            .unwrap();
        let id = cube.raw();
        for (call, says) in [
            (
                format!("tween({id}, \"position\", 1.0, 1.0)"),
                "component.field",
            ),
            (
                format!("tween({id}, \"light.intensity\", 1.0, 1.0)"),
                "no `light` component",
            ),
            (
                format!("tween({id}, \"transform.scale\", 2.0, 1.0)"),
                "no such field",
            ),
            (
                format!("tween({id}, \"transform.position\", 2.0, 1.0)"),
                "a list of 3 numbers",
            ),
            (
                format!("tween({id}, \"transform.position\", \"up\", 1.0)"),
                "can't ease to \"up\"",
            ),
            (
                format!("tween({id}, \"transform.position\", [0, 1, 0], 1.0, \"bouncy\")"),
                "no easing `bouncy`",
            ),
        ] {
            let err = run(&engine, &mut world, &call).unwrap_err();
            assert!(
                err.contains("tween: ") && err.contains(says),
                "{call}: {err}"
            );
        }
        assert!(crate::lock_or_recover(&engine.ctx).commands.is_empty());
        let good = format!("tween({id}, \"transform.position\", [0, 1, 0], 2, \"ease_out\")");
        run(&engine, &mut world, &good).unwrap();
        let c = crate::lock_or_recover(&engine.ctx);
        let [ScriptCommand::Tween {
            path,
            seconds,
            easing,
            ..
        }] = &c.commands[..]
        else {
            panic!("{:?}", c.commands);
        };
        assert_eq!(path, "transform.position");
        assert_eq!(*seconds, 2.0);
        assert_eq!(*easing, Easing::EaseOut);
    }
}
