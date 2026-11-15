//! The `state` module: the game state machine (playing, paused, a menu),
//! a stack of states each saying which systems run (ADR 0116). The pushes
//! and pops are commands the host runs after the batch; the reads and
//! `register_state` reach the machine the host lends for the call.
//! `register_state` is a script error for a key no system answers to, or one
//! that was documented and never did anything (`rendering`, `transparent`,
//! `"hidden"`; #727).

use super::builder::kind::{Array, Map, Str};
use super::builder::ScriptApi;
use crate::context::ScriptCommand;
use flint_runtime::{StateConfig, StateError};
use rhai::Dynamic;

/// What a host without a state machine is in.
const PLAYING: &str = "playing";

/// `register_state`'s config: a policy, `"run"` or `"pause"`, for each
/// system it names; the systems it leaves out run. Which names a system
/// answers to is the host's machine's to check.
pub(crate) fn state_config(config: &rhai::Map) -> Result<StateConfig, StateError> {
    let entries: Vec<(String, String)> = config
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    StateConfig::parse(entries.iter().map(|(k, v)| (k.as_str(), v.as_str())))
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("state");

    m.func(
        "push_state",
        "Push a registered state on top of the stack (a pause menu over play).",
    )
    .arg("name", Str)
    .run(|call, (name,)| {
        call.ctx().commands.push(ScriptCommand::PushState {
            name: name.to_string(),
        });
    });

    m.func("pop_state", "Pop the top state off the stack.")
        .run(|call, ()| call.ctx().commands.push(ScriptCommand::PopState));

    m.func("replace_state", "Replace the top state with another.")
        .arg("name", Str)
        .run(|call, (name,)| {
            call.ctx().commands.push(ScriptCommand::ReplaceState {
                name: name.to_string(),
            });
        });

    // Each under its name and, for one release, its old one (#702)
    for (name, old) in [
        ("get_current_state", None),
        ("current_state", Some("use get_current_state")),
    ] {
        m.func(name, "The state on top of the stack.")
            .returns(Str, "its name; \"playing\" without a state machine")
            .deprecated(old)
            .run(|call, ()| match call.ctx_ref().try_state_machine() {
                Some(sm) => sm.current_state().to_string(),
                None => PLAYING.to_string(),
            });
    }

    for (name, old) in [
        ("get_state_stack", None),
        ("state_stack", Some("use get_state_stack")),
    ] {
        m.func(name, "Every state on the stack, bottom first.")
            .returns(Array, "their names; [\"playing\"] without a state machine")
            .deprecated(old)
            .run(|call, ()| -> rhai::Array {
                match call.ctx_ref().try_state_machine() {
                    Some(sm) => sm
                        .stack_names()
                        .into_iter()
                        .map(|s| Dynamic::from(s.to_string()))
                        .collect(),
                    None => vec![Dynamic::from(PLAYING.to_string())],
                }
            });
    }

    m.func(
        "register_state",
        "Make a state: a system's name (physics, scripts, animation, particles, audio, living) to \"run\" or \"pause\"; one left out runs. An unknown name is an error.",
    )
    .arg("name", Str)
    .arg("config", Map)
    .run(|call, (name, config)| {
        let refused = |call: &super::builder::Call, e: StateError| {
            call.error(format_args!("state \"{name}\": {e}"))
        };
        let config = state_config(&config).map_err(|e| refused(call, e))?;
        let registered = match call.ctx().try_state_machine_mut() {
            Some(sm) => sm.register_state(&name, config),
            None => Ok(()),
        };
        registered.map_err(|e| refused(call, e))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use flint_runtime::state::{AUDIO, PHYSICS, SCRIPTS};
    use flint_runtime::{GameStateMachine, PersistentStore};

    #[test]
    fn a_state_config_reads_each_policy_and_the_rest_run() {
        let mut config = rhai::Map::new();
        config.insert("physics".into(), "pause".into());
        config.insert("audio".into(), "Run".into());
        let c = state_config(&config).unwrap();
        assert!(!c.runs(PHYSICS));
        assert!(c.runs(AUDIO));
        assert!(c.runs(SCRIPTS));
    }

    #[test]
    fn a_key_or_policy_nothing_implements_is_refused() {
        for (key, value) in [
            ("rendering", Dynamic::from("run")),
            ("transparent", Dynamic::from(true)),
            ("physics", Dynamic::from("hidden")),
            ("audio", Dynamic::from("loud")),
        ] {
            let mut config = rhai::Map::new();
            config.insert(key.into(), value);
            let err = state_config(&config).expect_err(key);
            assert!(err.to_string().contains(key), "{err}");
        }
    }

    /// The script's error names the state, the key and why, at the call.
    fn register_error(sm: &mut GameStateMachine, script: &str) -> String {
        let engine = crate::engine::ScriptEngine::new();
        let mut store = PersistentStore::new();
        let physics = flint_physics::PhysicsSystem::new();
        let _scope = crate::context::StateScope::new(&engine.ctx, sm, &mut store, &physics);
        match engine.rhai_engine().eval::<()>(script) {
            Ok(()) => String::new(),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn register_state_is_a_script_error_naming_what_it_refused() {
        let mut sm = GameStateMachine::new();
        let err = register_error(
            &mut sm,
            "register_state(\"menu\", #{ physics: \"pause\", transparent: true })",
        );
        assert!(err.contains("register_state: state \"menu\""), "{err}");
        assert!(err.contains("`transparent` is not implemented"), "{err}");
        assert!(!sm.push_state("menu"), "nothing was registered");

        let err = register_error(&mut sm, "register_state(\"menu\", #{ grass: \"pause\" })");
        assert!(err.contains("`grass` is not a system"), "{err}");
        assert!(!sm.push_state("menu"));

        let err = register_error(&mut sm, "register_state(\"menu\", #{ physics: \"pause\" })");
        assert_eq!(err, "");
        assert!(sm.push_state("menu"));
        assert!(!sm.active_config().runs(PHYSICS));
    }

    #[test]
    fn without_a_state_machine_the_game_is_playing() {
        let engine = crate::engine::ScriptEngine::new();
        let state: String = engine.rhai_engine().eval("current_state()").unwrap();
        assert_eq!(state, "playing");
        let stack: rhai::Array = engine.rhai_engine().eval("state_stack()").unwrap();
        assert_eq!(stack.len(), 1);
        let _: () = engine
            .rhai_engine()
            .eval("register_state(\"menu\", #{ physics: \"pause\" })")
            .unwrap();
    }
}
