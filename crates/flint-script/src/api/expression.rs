//! The `expression` module: state from outside the process (Flint#518), a
//! timeline played on the game clock (`--expression-timeline`) or
//! newline-delimited JSON on a localhost socket (`--expression-listen`).
//! Flint doesn't know the fields: any JSON object arrives as a map, its
//! numbers as floats. The host delivers each new state before `on_update`,
//! calling `on_expression(state)` on every script that defines it. ADR 0116.

use super::builder::kind::Map;
use super::builder::ScriptApi;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("expression");

    m.func(
        "expression",
        "The latest state from outside the process (a timeline or a socket).",
    )
    .returns(Map, "the state's map; #{} before the first arrives")
    .run(|call, ()| call.ctx_ref().expression.clone());
}
