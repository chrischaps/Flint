//! The `tween` module (#734): ease any numeric component field over game
//! time, the one call for what used to be a hand-rolled `on_update` lerp (a
//! light's intensity, a material's colour, a camera's field of view). The
//! animation package's since #1620 (ADR 0139 section 9): a tween is a
//! two-key property clip made by a call.
//!
//! The call checks what it can at once, so a mistake is a script error at its
//! line: the path's shape, the easing, and (with a world lent) that the field
//! is there and holds what the target is. It then queues a module command
//! ([`TweenField`]); the package's subsystem starts the tween from where the
//! field stands ([`crate::tween::FieldTweens`]) and steps it with the scripts
//! at `Stage::Tweens`.

use crate::tween::{endpoints, FieldPath};
use flint_core::tween::Easing;
use flint_core::EntityId;
use flint_script::api::builder::kind::{Any, Entity, Num, Str};
use flint_script::api::{dynamic_to_toml, Call, CallResult, Module};
use flint_script::context::ScriptCommand;
use flint_script::{ModuleCommand, ScriptModule};
use rhai::Dynamic;

/// The module's name.
pub const MODULE: &str = "tween";

/// `tween(id, path, target, seconds[, easing])`: what the subsystem starts.
#[derive(Debug, Clone, PartialEq)]
pub struct TweenField {
    pub entity: EntityId,
    /// `component.field`, as written
    pub path: String,
    pub target: toml::Value,
    pub seconds: f64,
    pub easing: Easing,
}

/// The `tween` module, declared by the package and enabled by its
/// subsystem, which every scene has.
pub struct TweenModule;

pub static TWEEN_MODULE: TweenModule = TweenModule;

impl ScriptModule for TweenModule {
    fn name(&self) -> &'static str {
        MODULE
    }

    fn enabled_when(&self) -> &'static str {
        "the animation package is in the build (every scene)"
    }

    fn declare(&self, m: &mut Module<'_>) {
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
    let function = call.name();
    let tween = TweenField {
        entity,
        path: field.dotted(),
        target,
        seconds,
        easing,
    };
    call.ctx()
        .commands
        .push(ScriptCommand::Module(ModuleCommand::new(function, tween)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::TweenField;
    use flint_core::tween::Easing;
    use flint_ecs::FlintWorld;
    use flint_script::context::ScriptCommand;
    use flint_script::engine::ScriptEngine;

    /// A mistake is a script error at the call, naming the field; a good
    /// call queues one command with the path as written.
    #[test]
    fn a_tween_says_what_it_cant_ease_at_the_call() {
        let mut world = FlintWorld::new();
        let cube = world.spawn("cube").unwrap();
        let origin = toml::Value::Array(vec![toml::Value::Float(0.0); 3]);
        world
            .set_field(cube, "transform", "position", origin)
            .unwrap();
        let id = cube.raw();
        let run = |world: &mut FlintWorld, call: &str| {
            let mut engine = ScriptEngine::new();
            engine.declare_enabled_module(&super::TWEEN_MODULE);
            engine.errors_mut().set_quiet(true);
            let ast = engine
                .compile(&format!("fn on_init() {{ {call}; }}"))
                .unwrap();
            engine.add_script(cube, ast, "t.rhai".into());
            engine.call_inits(world);
            let errors: Vec<String> = engine
                .errors()
                .all()
                .iter()
                .map(|e| e.message.clone())
                .collect();
            (errors, engine.drain_commands())
        };
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
            let (errors, commands) = run(&mut world, &call);
            assert!(
                errors.len() == 1 && errors[0].contains("tween: ") && errors[0].contains(says),
                "{call}: {errors:?}"
            );
            assert!(commands.is_empty(), "{call}: {commands:?}");
        }
        let good = format!("tween({id}, \"transform.position\", [0, 1, 0], 2, \"ease_out\")");
        let (errors, commands) = run(&mut world, &good);
        assert!(errors.is_empty(), "{errors:?}");
        let [ScriptCommand::Module(command)] = &commands[..] else {
            panic!("{commands:?}");
        };
        assert_eq!(command.function(), "tween");
        let tween = command.payload::<TweenField>().unwrap();
        assert_eq!(tween.path, "transform.position");
        assert_eq!(tween.seconds, 2.0);
        assert_eq!(tween.easing, Easing::EaseOut);
    }
}
