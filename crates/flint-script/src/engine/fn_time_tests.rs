//! Time inside a script, by function (#1100): a callback timed with
//! `set_timing` names the script functions it called and the engine
//! functions, each with its calls, and runs the same with it as without.

use super::output::FnTime;
use super::ScriptEngine;
use crate::engine::test_support::*;
use flint_ecs::FlintWorld;

/// A helper with a loop, an engine function in a loop, an early return,
/// recursion, a closure and a method on `this`: each kind of call a
/// wrapper must leave as it was. `out` is what the callback worked out.
const PROFILED: &str = r#"
let out = [];

fn slow_helper(n) {
    let total = 0;
    for i in 0..n { total += i; }
    total
}

fn early(x) {
    if x > 0 { return x * 2; }
    x - 1
}

fn depth(n) { if n <= 0 { 0 } else { 1 + depth(n - 1) } }

fn grow() { this += 1; }

fn on_update() {
    let sum = slow_helper(2000);
    let kept = 0.0;
    for i in 0..400 { kept += clamp(i * 0.01, 0.0, 1.0); }
    let add = |v| v + sum;
    let closed = 0;
    for i in 0..5 { closed += add.call(i); }
    let n = 1;
    n.grow();
    out = [sum, kept, early(3), early(-4), depth(5), closed, n];
}
"#;

fn profiled_script(engine: &mut ScriptEngine, world: &mut FlintWorld) -> flint_core::EntityId {
    let id = scripted(world, "prof", "prof.rhai");
    let ast = engine.compile(PROFILED).unwrap();
    engine.add_named_script(
        id,
        ast,
        "scripts/prof.rhai".into(),
        "prof",
        Some("scripts/prof.rhai"),
    );
    id
}

fn out_of(engine: &ScriptEngine, id: flint_core::EntityId) -> String {
    let script = &engine.scripts[&id];
    format!(
        "{:?}",
        script.scope.get_value::<rhai::Array>("out").unwrap()
    )
}

fn named<'a>(times: &'a [FnTime], function: &str) -> &'a FnTime {
    times
        .iter()
        .find(|t| t.function == function)
        .unwrap_or_else(|| panic!("no {function} in {times:#?}"))
}

#[test]
fn a_timed_callback_names_its_helpers_and_engine_calls_with_their_counts() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = profiled_script(&mut engine, &mut world);
    engine.call_updates(&mut world);
    assert!(
        engine.take_function_times().is_empty(),
        "nothing until timing is on"
    );
    let plain = out_of(&engine, id);

    engine.set_timing(true);
    engine.call_updates(&mut world);
    assert_eq!(
        out_of(&engine, id),
        plain,
        "a timed callback works the same"
    );
    let times = engine.take_function_times();
    for t in &times {
        assert_eq!(t.callback, "scripts/prof.rhai:on_update", "{t:?}");
    }

    let helper = named(&times, "scripts/prof.rhai:slow_helper()");
    assert_eq!(helper.calls, 1);
    assert!(helper.total_ms > 0.0 && helper.worst_ms == helper.total_ms);
    // An engine function is one record by name, however many calls
    assert_eq!(named(&times, "clamp()").calls, 400);
    assert_eq!(named(&times, "scripts/prof.rhai:early()").calls, 2);
    // Every level of a recursion is a call; its time is the outermost's
    let depth = named(&times, "scripts/prof.rhai:depth()");
    assert_eq!(depth.calls, 6);
    assert_eq!(depth.worst_ms, depth.total_ms);
    // A closure and a function that reads `this` run, untimed: their time
    // is the callback's
    assert!(
        times.iter().all(|t| !t.function.contains("grow")),
        "{times:#?}"
    );
    assert!(
        times.iter().all(|t| !t.function.contains("anon")),
        "{times:#?}"
    );
    // The callback itself is the callback's time, not a function's
    assert!(times.iter().all(|t| !t.function.contains("on_update")));

    // A second call adds to the first until the take
    engine.call_updates(&mut world);
    engine.call_updates(&mut world);
    let times = engine.take_function_times();
    assert_eq!(named(&times, "clamp()").calls, 800);
    assert!(
        engine.take_function_times().is_empty(),
        "a take empties them"
    );

    engine.set_timing(false);
    engine.call_updates(&mut world);
    assert_eq!(out_of(&engine, id), plain);
    assert!(engine.take_function_times().is_empty());
}

#[test]
fn a_slow_helper_is_where_the_callbacks_time_went() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = scripted(&mut world, "slow", "slow.rhai");
    let ast = engine
        .compile(
            "fn wait(ms) { let t = timestamp(); while t.elapsed * 1000.0 < ms {} }\n\
             fn quick() { 1 }\n\
             fn on_update() { quick(); wait(20.0); quick(); clamp(2.0, 0.0, 1.0); }\n",
        )
        .unwrap();
    engine.add_script(id, ast, "scripts/slow.rhai".into());
    engine.set_timing(true);
    engine.call_updates(&mut world);
    let callback = engine.take_times()[0].1;
    let times = engine.take_function_times();
    let wait = named(&times, "scripts/slow.rhai:wait()");
    assert!(wait.total_ms >= 20.0, "{wait:?}");
    assert!(wait.total_ms <= callback, "{wait:?} inside {callback} ms");
    assert!(named(&times, "scripts/slow.rhai:quick()").total_ms < wait.total_ms);
    // Rhai's own functions (`timestamp`, `elapsed`) are Rhai's, not timed
    assert!(times.iter().all(|t| !t.function.contains("timestamp")));
    assert_eq!(named(&times, "clamp()").calls, 1);
}

#[test]
fn a_recursion_fails_at_the_depth_it_would_untimed() {
    let levels = ScriptEngine::new().rhai_engine().max_call_levels() as i64;
    for timing in [false, true] {
        for (n, fails) in [(levels - 2, false), (levels + 2, true)] {
            let mut engine = ScriptEngine::new();
            engine.errors.set_quiet(true);
            let mut world = FlintWorld::new();
            let id = scripted(&mut world, "d", "d.rhai");
            let source = format!(
                "let out = 0;\n\
                 fn depth(n) {{ if n <= 0 {{ 0 }} else {{ 1 + depth(n - 1) }} }}\n\
                 fn on_update() {{ out = depth({n}); }}\n"
            );
            let ast = engine.compile(&source).unwrap();
            engine.add_script(id, ast, "scripts/d.rhai".into());
            engine.set_timing(timing);
            engine.call_updates(&mut world);
            let out = engine.scripts[&id].scope.get_value::<i64>("out").unwrap();
            assert_eq!(
                engine.errors().all().is_empty(),
                !fails,
                "timing {timing}, {n} deep"
            );
            if !fails {
                assert_eq!(out, n, "timing {timing}");
            }
        }
    }
}

#[test]
fn an_error_in_a_timed_helper_names_the_helper() {
    let mut engine = ScriptEngine::new();
    engine.errors.set_quiet(true);
    let mut world = FlintWorld::new();
    let id = scripted(&mut world, "bad", "bad.rhai");
    let ast = engine
        .compile("fn boom(x) {\n    x.no_such_thing()\n}\nfn on_update() { boom(1); }\n")
        .unwrap();
    engine.add_script(id, ast, "scripts/bad.rhai".into());
    engine.set_timing(true);
    engine.call_updates(&mut world);
    let err = &engine.errors().all()[0];
    assert_eq!(err.function.as_deref(), Some("boom"), "{err:?}");
    assert_eq!(err.line, Some(2), "{err:?}");
    // The guard went with the error: the next call counts afresh
    engine.take_function_times();
    engine.call_updates(&mut world);
    let times = engine.take_function_times();
    assert_eq!(named(&times, "scripts/bad.rhai:boom()").calls, 1);
}

#[test]
fn a_hot_reload_times_the_new_source() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = scripted(&mut world, "r", "r.rhai");
    let ast = engine
        .compile("fn a() { 1 }\nfn on_update() { a(); }\n")
        .unwrap();
    engine.add_script(id, ast, "scripts/r.rhai".into());
    engine.set_timing(true);
    engine.call_updates(&mut world);
    named(&engine.take_function_times(), "scripts/r.rhai:a()");
    let ast = engine
        .compile("fn b() { 2 }\nfn on_update() { b(); b(); }\n")
        .unwrap();
    engine.reload_script(&mut world, id, ast);
    engine.call_updates(&mut world);
    let times = engine.take_function_times();
    assert_eq!(named(&times, "scripts/r.rhai:b()").calls, 2);
    assert!(times.iter().all(|t| t.function != "scripts/r.rhai:a()"));
}

/// The cost of a script function call and of an engine call, with timing
/// off and on (#1100's done-when: off unchanged, on stated). Release:
/// `cargo test -p flint-script --release --lib fn_call_cost -- --ignored --nocapture`
#[test]
#[ignore]
fn fn_call_cost() {
    const N: i64 = 200_000;
    let source = format!(
        "fn f(x) {{ x + 1 }}\n\
         fn on_update() {{ let s = 0; for i in 0..{N} {{ s = f(s); }} }}\n\
         fn on_draw_ui() {{ let s = 0.0; for i in 0..{N} {{ s += clamp(0.5, 0.0, 1.0); }} }}\n"
    );
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = scripted(&mut world, "cost", "cost.rhai");
    let ast = engine.compile(&source).unwrap();
    engine.add_script(id, ast, "scripts/cost.rhai".into());
    let mut per_call = |timing: bool, which: &str| {
        engine.set_timing(timing);
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            if which == "script" {
                engine.call_updates(&mut world);
            } else {
                engine.call_draw_uis(&mut world);
            }
            best = best.min(t.elapsed().as_nanos() as f64 / N as f64);
        }
        // Only `take_times`, so the same bench runs on main for the A/B
        engine.take_times();
        best
    };
    for which in ["script", "engine"] {
        let off = per_call(false, which);
        let on = per_call(true, which);
        // While the recorder records (#1288): a script function's call a
        // scope, an engine function's calls summed into one record
        flint_core::profile::set_recording(true);
        let recording = per_call(false, which);
        flint_core::profile::set_recording(false);
        println!(
            "{which} function: {off:.0} ns a call (loop included) off, {on:.0} on (+{:.0}), \
             {recording:.0} recording (+{:.0})",
            on - off,
            recording - off
        );
    }
}
