//! The `log` module: messages to the host's log at a level (ADR 0116).
//!
//! A message is any value, shown as Rhai prints it, so `log(42)` and
//! `log(3.5)` log the number. `log` shares its name with Rhai's base-10
//! logarithm of a float, which Rhai would find first for `log(3.5)`, so the
//! float signature is registered exactly too
//! ([`Func::also_exact`](super::builder::Func::also_exact)), as is the
//! string one, the usual call. The logarithm is still `log(x, 10.0)`.

use super::builder::kind::Any;
use super::builder::{Call, ScriptApi};
use crate::context::{LogLevel, ScriptCommand};
use rhai::{Dynamic, ImmutableString};
use std::any::TypeId;

fn push(call: &mut Call<'_>, level: LogLevel, message: Dynamic) {
    // A string's Display is its text, unquoted; a number's is the number
    let message = message.to_string();
    call.ctx()
        .commands
        .push(ScriptCommand::Log { level, message });
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("log");
    let exact = |t: TypeId| [t];

    m.func("log", "Log a message (any value) at info level.")
        .arg("message", Any)
        .also_exact(&exact(TypeId::of::<ImmutableString>()))
        .also_exact(&exact(TypeId::of::<rhai::FLOAT>()))
        .run(|call, (message,)| push(call, LogLevel::Info, message));

    m.func(
        "log_info",
        "Log a message at info level (log's other name).",
    )
    .arg("message", Any)
    .also_exact(&exact(TypeId::of::<ImmutableString>()))
    .run(|call, (message,)| push(call, LogLevel::Info, message));

    m.func("log_warn", "Log a message at warning level.")
        .arg("message", Any)
        .also_exact(&exact(TypeId::of::<ImmutableString>()))
        .run(|call, (message,)| push(call, LogLevel::Warn, message));

    m.func("log_error", "Log a message at error level.")
        .arg("message", Any)
        .also_exact(&exact(TypeId::of::<ImmutableString>()))
        .run(|call, (message,)| push(call, LogLevel::Error, message));
}

#[cfg(test)]
mod tests {
    use crate::context::ScriptCommand;
    use crate::engine::ScriptEngine;

    #[test]
    fn log_takes_any_value_and_a_float_is_not_a_logarithm() {
        let engine = ScriptEngine::new();
        engine
            .rhai_engine()
            .run(
                "log(\"hello\"); log(42); log(3.5); log_warn(7); log_error(true); \
                 let ten = log(100.0, 10.0); log_info(ten);",
            )
            .unwrap();
        let cmds = std::mem::take(&mut engine.ctx.lock().unwrap().commands);
        let got: Vec<(String, String)> = cmds
            .into_iter()
            .map(|c| match c {
                ScriptCommand::Log { level, message } => (format!("{level:?}"), message),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("Info".to_string(), "hello".to_string()),
                ("Info".to_string(), "42".to_string()),
                ("Info".to_string(), "3.5".to_string()),
                ("Warn".to_string(), "7".to_string()),
                ("Error".to_string(), "true".to_string()),
                ("Info".to_string(), "2.0".to_string()),
            ]
        );
    }
}
