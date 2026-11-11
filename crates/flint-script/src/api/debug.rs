//! The `debug` module: the look-development calls that are the debug
//! panels' actions (ADR 0116). Living Props' A/B comparison (#238), the
//! panel's F5/F6/F7, and its commit to the scene file (#249). Tools for
//! tuning a scene, not for a shipped game: #698 turns this module off in
//! shipped builds.

use super::builder::kind::{Bool, Str};
use super::builder::ScriptApi;
use crate::context::ScriptCommand;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("debug");

    m.func(
        "commit_living",
        "Write every Living Props value the scene file doesn't say into it, one line each (an error on the web).",
    )
    .run(|call, ()| call.ctx().commands.push(ScriptCommand::CommitLiving));

    m.func(
        "compare_living",
        "Put two presets on the same grove, showing A; \"\" is the knobs as they stand.",
    )
    .arg("a", Str)
    .arg("b", Str)
    .run(|call, (a, b)| {
        call.ctx().commands.push(ScriptCommand::CompareLiving {
            a: a.to_string(),
            b: b.to_string(),
        });
    });

    m.func(
        "flip_living",
        "Show the comparison's other side, on the same clock and camera.",
    )
    .run(|call, ()| call.ctx().commands.push(ScriptCommand::FlipLiving));

    m.func(
        "split_living",
        "Show both sides of the comparison at once, A left and B right, or one at a time.",
    )
    .arg("split", Bool)
    .run(|call, (split,)| {
        call.ctx()
            .commands
            .push(ScriptCommand::SplitLiving { split });
    });

    m.func(
        "stop_comparing_living",
        "End the comparison, keeping the side that's shown.",
    )
    .run(|call, ()| {
        call.ctx().commands.push(ScriptCommand::StopComparingLiving);
    });
}
