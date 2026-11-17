//! The `test` module (#714): a script's say in a playtest. `test_assert`
//! and `test_fail` fail the run, `test_pass` ends it passing, `test_log`
//! puts a value in its report. They write the run's
//! [`TestRecord`](crate::testing::TestRecord); playing the game, they only
//! log a failure, once a place.

use super::builder::kind::{Any, Bool, Str};
use super::builder::{Call, ScriptApi};
use crate::testing::{dynamic_to_json, Place};

/// Where the call is: its script, entity, line and the game time.
fn place<R>(
    call: &mut Call<'_>,
    f: impl FnOnce(&mut crate::testing::TestRecord, &Place<'_>) -> R,
) -> R {
    let line = call.position().line();
    let ctx = call.ctx();
    let speaker = ctx.speaker.clone();
    let place = Place {
        file: &speaker.file,
        entity: &speaker.entity,
        line,
        at: ctx.total_time,
    };
    f(&mut ctx.test, &place)
}

fn shown(file: &str, line: Option<usize>) -> String {
    match line {
        Some(l) => format!("{file}:{l}"),
        None => file.to_string(),
    }
}

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("test");

    m.func(
        "test_assert",
        "Fail the playtest when cond is false, naming the message, this file and line (counted once a place).",
    )
    .arg("cond", Bool)
    .arg("message", Str)
    .run(|call, (cond, message)| {
        let line = call.position().line();
        let first = place(call, |r, p| r.assert(cond, &message, p));
        if first {
            let file = call.ctx_ref().speaker.file.clone();
            tracing::warn!(
                target: "script::test",
                "[test] {}: assertion failed: {message}",
                shown(&file, line)
            );
        }
    });

    m.func(
        "test_fail",
        "End the playtest failing with the message (the first test_pass or test_fail stands).",
    )
    .arg("message", Str)
    .run(|call, (message,)| {
        let line = call.position().line();
        if place(call, |r, p| r.decide(false, &message, p)) {
            let file = call.ctx_ref().speaker.file.clone();
            tracing::warn!(
                target: "script::test",
                "[test] {}: failed: {message}",
                shown(&file, line)
            );
        }
    });

    m.func(
        "test_pass",
        "End the playtest passing, unless an assertion has failed (the first test_pass or test_fail stands).",
    )
    .run(|call, ()| {
        place(call, |r, p| r.decide(true, "", p));
    });

    m.func(
        "test_log",
        "Put a value in the playtest's report under key (its last value is kept).",
    )
    .arg("key", Str)
    .arg("value", Any)
    .run(|call, (key, value)| {
        let json = dynamic_to_json(&value);
        call.ctx().test.log(&key, json);
    });
}

#[cfg(test)]
mod tests {
    use crate::engine::ScriptEngine;

    #[test]
    fn the_test_functions_write_the_record() {
        let engine = ScriptEngine::new();
        engine
            .rhai_engine()
            .run(
                "for i in 0..3 { test_assert(i < 2, \"small\"); }\n\
                 test_log(\"laps\", 2);\n\
                 test_log(\"where\", #{ x: 1.5 });\n\
                 test_fail(\"fell\");\n\
                 test_pass();",
            )
            .unwrap();
        let ctx = engine.ctx.lock().unwrap();
        let r = &ctx.test;
        assert_eq!(r.assertions.len(), 1);
        let a = &r.assertions[0];
        assert_eq!((a.passed, a.failed, a.line), (2, 1, Some(1)));
        let v = r.verdict.as_ref().unwrap();
        assert!(!v.passed, "the first verdict stands");
        assert_eq!((v.message.as_str(), v.line), ("fell", Some(4)));
        assert_eq!(r.logged["laps"], serde_json::json!(2));
        assert_eq!(r.logged["where"], serde_json::json!({"x": 1.5}));
    }
}
