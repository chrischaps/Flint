//! The `scene` module: changing scene, quitting, and the transition
//! between scenes (ADR 0116). The changes are commands the host runs after
//! the batch; a transition's progress is the host's, read back.

use super::builder::kind::{Bool, Num, Str};
use super::builder::ScriptApi;
use crate::context::ScriptCommand;
use flint_core::events::TRANSITION_COMPLETE;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("scene");

    m.func(
        "load_scene",
        "Leave this scene for another, by its path, after the frame.",
    )
    .arg("path", Str)
    .run(|call, (path,)| {
        call.ctx().commands.push(ScriptCommand::LoadScene {
            path: path.to_string(),
        });
    });

    m.func(
        "reload_scene",
        "Load this scene again from its file, after the frame.",
    )
    .run(|call, ()| call.ctx().commands.push(ScriptCommand::ReloadScene));

    m.func(
        "quit_game",
        "End the run as closing the window does: saves flushed, exit 0 (on the web, back to the game list).",
    )
    .run(|call, ()| call.ctx().commands.push(ScriptCommand::QuitGame));

    for (name, old) in [
        ("get_current_scene", None),
        ("current_scene", Some("use get_current_scene")),
    ] {
        m.func(name, "The path of the scene loaded now.")
            .returns(Str, "the path as the host loaded it")
            .deprecated(old)
            .run(|call, ()| call.ctx_ref().current_scene_path.clone());
    }

    m.func(
        "complete_transition",
        "Advance the scene transition to its next phase (the script's fade is done).",
    )
    .run(|call, ()| {
        call.ctx().commands.push(ScriptCommand::FireEvent {
            name: TRANSITION_COMPLETE.to_string(),
            data: toml::Value::Table(toml::map::Map::new()),
        });
    });

    m.func(
        "transition_progress",
        "How far through its current phase a scene transition is.",
    )
    .returns(Num, "0..1 during a transition; -1.0 when none is running")
    .run(|call, ()| call.ctx_ref().transition_progress);

    m.func("is_transitioning", "Whether a scene transition is running.")
        .returns(Bool, "")
        .run(|call, ()| call.ctx_ref().transition_progress >= 0.0);

    m.func(
        "transition_phase",
        "The phase of the scene transition running.",
    )
    .returns(Str, "\"idle\", \"exiting\", \"loading\" or \"entering\"")
    .run(|call, ()| call.ctx_ref().transition_phase.clone());
}
