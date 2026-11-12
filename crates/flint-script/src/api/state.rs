//! The `state` module: the game state machine (playing, paused, a menu),
//! a stack of states each saying which systems run (ADR 0116). The pushes
//! and pops are commands the host runs after the batch; the reads and
//! `register_state` reach the machine the host lends for the call.

use super::builder::kind::{Array, Map, Str};
use super::builder::ScriptApi;
use crate::context::ScriptCommand;
use flint_runtime::{StateConfig, SystemPolicy};
use rhai::Dynamic;

/// What a host without a state machine is in.
const PLAYING: &str = "playing";

/// `register_state`'s config: each system's policy (`"run"`, the default,
/// `"pause"` or `"hidden"`) and whether the state below shows through.
pub(crate) fn state_config(config: &rhai::Map) -> StateConfig {
    let policy = |key: &str| -> SystemPolicy {
        match config
            .get(key)
            .and_then(|v| v.clone().into_string().ok())
            .as_deref()
        {
            Some("pause") | Some("Pause") => SystemPolicy::Pause,
            Some("hidden") | Some("Hidden") => SystemPolicy::Hidden,
            _ => SystemPolicy::Run,
        }
    };
    StateConfig {
        physics: policy("physics"),
        scripts: policy("scripts"),
        animation: policy("animation"),
        particles: policy("particles"),
        audio: policy("audio"),
        rendering: policy("rendering"),
        transparent: config
            .get("transparent")
            .and_then(|v| v.as_bool().ok())
            .unwrap_or(false),
    }
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
        "Make a state: each of physics, scripts, animation, particles, audio, rendering \"run\", \"pause\" or \"hidden\", and transparent.",
    )
    .arg("name", Str)
    .arg("config", Map)
    .run(|call, (name, config)| {
        if let Some(sm) = call.ctx().try_state_machine_mut() {
            sm.register_state(&name, state_config(&config));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_config_reads_each_policy_and_defaults_to_run() {
        let mut config = rhai::Map::new();
        config.insert("physics".into(), "pause".into());
        config.insert("rendering".into(), "Hidden".into());
        config.insert("audio".into(), "loud".into());
        config.insert("transparent".into(), true.into());
        let c = state_config(&config);
        assert_eq!(c.physics, SystemPolicy::Pause);
        assert_eq!(c.rendering, SystemPolicy::Hidden);
        assert_eq!(c.audio, SystemPolicy::Run);
        assert_eq!(c.scripts, SystemPolicy::Run);
        assert!(c.transparent);
        assert!(!state_config(&rhai::Map::new()).transparent);
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
