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
    // A function that reads `this` runs, untimed: its time is the callback's
    assert!(
        times.iter().all(|t| !t.function.contains("grow")),
        "{times:#?}"
    );
    // A closure is timed by the line it is made on (#1281), and what it
    // captured (`sum`) it still reads
    assert_eq!(
        named(&times, "scripts/prof.rhai:closure at line 23").calls,
        5
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

/// A folder holding one module, `menu.rhai`, for `import "menu"`: removed
/// when dropped.
struct LibDir(std::path::PathBuf);

impl LibDir {
    fn with_menu(tag: &str, source: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("flint-1281-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let lib = LibDir(dir);
        lib.write_menu(source);
        lib
    }

    fn write_menu(&self, source: &str) {
        std::fs::write(self.0.join("menu.rhai"), source).unwrap();
    }
}

impl Drop for LibDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A module whose function calls its own helper and makes a closure.
const MENU: &str = "\
fn open(n) { helper(n) + 1 }
fn helper(n) { n * 2 }
fn doubled(a) {
    a.map(|x| x * 2)
}
";

/// A script calling the module, and a closure 50 times that writes what it
/// captured, and `sort` calling another.
const IMPORTER: &str = r#"
import "menu" as menu;
let out = [];
fn on_update() {
    let opened = menu::open(3);
    let count = 0;
    let bump = |x| { count += x; count };
    for i in 0..50 { bump.call(1); }
    let sorted = [3, 1, 2];
    sorted.sort(|a, b| a - b);
    out = [opened, count, sorted, menu::doubled([1, 2, 3])];
}
"#;

fn importer(
    engine: &mut ScriptEngine,
    world: &mut FlintWorld,
    lib: &LibDir,
) -> flint_core::EntityId {
    engine.set_module_base(lib.0.clone());
    let id = scripted(world, "imp", "imp.rhai");
    let ast = engine.compile(IMPORTER).unwrap();
    engine.add_script(id, ast, "scripts/imp.rhai".into());
    id
}

#[test]
fn a_module_function_and_a_closure_called_50_times_are_timed() {
    let lib = LibDir::with_menu("records", MENU);
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = importer(&mut engine, &mut world, &lib);
    engine.call_updates(&mut world);
    let plain = out_of(&engine, id);
    assert_eq!(plain, "[7, 50, [1, 2, 3], [2, 4, 6]]");

    engine.set_timing(true);
    engine.call_updates(&mut world);
    assert_eq!(
        out_of(&engine, id),
        plain,
        "a timed callback works the same"
    );
    let times = engine.take_function_times();
    // The module's functions, by the name the import gave, and the call
    // one makes to its own helper
    assert_eq!(named(&times, "menu::open()").calls, 1);
    assert_eq!(named(&times, "menu::helper()").calls, 1);
    assert_eq!(named(&times, "menu::doubled()").calls, 1);
    assert_eq!(named(&times, "menu::closure at line 4").calls, 3);
    // The closure by its line; its writes to `count` reached the callback
    // (50 in `out`), so what it captured is still shared
    let bump = named(&times, "scripts/imp.rhai:closure at line 7");
    assert_eq!(bump.calls, 50);
    assert!(bump.total_ms > 0.0 && bump.worst_ms <= bump.total_ms);
    assert!(named(&times, "scripts/imp.rhai:closure at line 10").calls >= 2);
    // Nothing of the wrapping shows: no renamed body, no wrapper, no anon
    for t in &times {
        assert!(
            !t.function.contains("__flint") && !t.function.contains("anon"),
            "{t:?}"
        );
    }

    // Off again: the plain module is the one imported, untimed
    engine.set_timing(false);
    engine.call_updates(&mut world);
    assert_eq!(out_of(&engine, id), plain);
    assert!(engine.take_function_times().is_empty());
}

#[test]
fn a_profiled_module_exports_what_the_plain_one_does() {
    let lib = LibDir::with_menu("exports", MENU);
    // `open`'s renamed body in the profiled module is private: calling it
    // by that name fails, timed or not
    let inner = format!(
        "{}{}",
        crate::fn_times::INNER_PREFIX,
        crate::fn_times::id_of("menu::open()")
    );
    for timing in [false, true] {
        let mut engine = ScriptEngine::new();
        engine.errors.set_quiet(true);
        let mut world = FlintWorld::new();
        let id = scripted(&mut world, "e", "e.rhai");
        engine.set_module_base(lib.0.clone());
        let ast = engine
            .compile(&format!(
                "import \"menu\" as menu;\n\
                 let out = ();\n\
                 fn on_update() {{ out = menu::open(1); menu::{inner}(1); }}\n"
            ))
            .unwrap();
        engine.add_script(id, ast, "scripts/e.rhai".into());
        engine.set_timing(timing);
        engine.call_updates(&mut world);
        let errors = engine.errors().all();
        assert_eq!(errors.len(), 1, "timing {timing}: {errors:#?}");
        assert!(errors[0].message.contains(&inner), "{:?}", errors[0]);
        assert_eq!(
            engine.scripts[&id].scope.get_value::<i64>("out"),
            Some(3),
            "timing {timing}"
        );
    }
}

#[test]
fn a_timed_closure_kept_after_profiling_still_runs() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = scripted(&mut world, "k", "k.rhai");
    let ast = engine
        .compile(
            "let kept = ();\n\
             let out = 0;\n\
             fn on_update() {\n\
                 if type_of(kept) == \"()\" { kept = |x| x + 1; }\n\
                 out = kept.call(out);\n\
             }\n",
        )
        .unwrap();
    engine.add_script(id, ast, "scripts/k.rhai".into());
    // Made while timed, so `kept` holds the timed closure
    engine.set_timing(true);
    engine.call_updates(&mut world);
    assert_eq!(
        named(
            &engine.take_function_times(),
            "scripts/k.rhai:closure at line 4"
        )
        .calls,
        1
    );
    engine.set_timing(false);
    engine.call_updates(&mut world);
    engine.call_updates(&mut world);
    assert!(
        engine.errors().all().is_empty(),
        "{:#?}",
        engine.errors().all()
    );
    assert_eq!(engine.scripts[&id].scope.get_value::<i64>("out"), Some(3));
    assert!(engine.take_function_times().is_empty());
}

#[test]
fn an_error_in_a_module_or_a_closure_reads_the_same_timed() {
    let lib = LibDir::with_menu("errors", "fn open(n) {\n    n.no_such_thing()\n}\n");
    for (source, line) in [
        ("fn on_update() { menu::open(1); }", 2),
        (
            "fn on_update() {\n    let f = |x| x.nope();\n    f.call(1);\n}",
            2,
        ),
    ] {
        let mut seen = Vec::new();
        for timing in [false, true] {
            let mut engine = ScriptEngine::new();
            engine.errors.set_quiet(true);
            engine.set_module_base(lib.0.clone());
            let mut world = FlintWorld::new();
            let id = scripted(&mut world, "x", "x.rhai");
            let ast = engine
                .compile(&format!("import \"menu\" as menu;\n{source}\n"))
                .unwrap();
            engine.add_script(id, ast, "scripts/x.rhai".into());
            engine.set_timing(timing);
            engine.call_updates(&mut world);
            let err = engine.errors().all()[0].clone();
            seen.push((err.file, err.function, err.line, err.message));
        }
        assert_eq!(seen[0], seen[1], "{source}");
        assert!(seen[0].2.is_some_and(|l| l >= line - 1), "{seen:?}");
    }
}

#[test]
fn a_module_cache_reset_clears_the_profiled_form_too() {
    let lib = LibDir::with_menu("reset", "fn open(n) { n }\n");
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = scripted(&mut world, "r", "r.rhai");
    engine.set_module_base(lib.0.clone());
    let ast = engine
        .compile(
            "import \"menu\" as menu;\nlet out = 0;\nfn on_update() { out = menu::open(5); }\n",
        )
        .unwrap();
    engine.add_script(id, ast, "scripts/r.rhai".into());
    engine.set_timing(true);
    engine.call_updates(&mut world);
    named(&engine.take_function_times(), "menu::open()");
    lib.write_menu("fn open(n) { twice(n) }\nfn twice(n) { n * 2 }\n");
    engine.reset_module_cache();
    engine.call_updates(&mut world);
    let times = engine.take_function_times();
    assert_eq!(named(&times, "menu::twice()").calls, 1);
    assert_eq!(engine.scripts[&id].scope.get_value::<i64>("out"), Some(10));
    // And the plain form is the new file's too
    engine.set_timing(false);
    engine
        .scripts
        .get_mut(&id)
        .unwrap()
        .scope
        .set_value("out", 0_i64);
    engine.call_updates(&mut world);
    assert_eq!(engine.scripts[&id].scope.get_value::<i64>("out"), Some(10));
}

/// The cost of a call of each kind (a script function, an engine function,
/// a closure, an imported module's function), with timing off and on
/// (#1100's done-when, #1281's: off unchanged, on stated). Release:
/// `cargo test -p flint-script --release --lib fn_call_cost -- --ignored --nocapture`
#[test]
#[ignore]
fn fn_call_cost() {
    const N: i64 = 200_000;
    let lib = LibDir::with_menu("cost", "fn f(x) { x + 1 }\n");
    let kinds = [
        (
            "script function",
            "fn f(x) { x + 1 }\nfn on_update() { let s = 0; for i in 0..N { s = f(s); } }",
        ),
        (
            "engine function",
            "fn on_update() { let s = 0.0; for i in 0..N { s += clamp(0.5, 0.0, 1.0); } }",
        ),
        (
            "closure",
            "fn on_update() { let g = |x| x + 1; let s = 0; for i in 0..N { s = g.call(s); } }",
        ),
        (
            "module function",
            "import \"menu\" as m;\nfn on_update() { let s = 0; for i in 0..N { s = m::f(s); } }",
        ),
    ];
    for (which, body) in kinds {
        let source = body.replace("..N", &format!("..{N}"));
        let mut engine = ScriptEngine::new();
        engine.set_module_base(lib.0.clone());
        let mut world = FlintWorld::new();
        let id = scripted(&mut world, "cost", "cost.rhai");
        let ast = engine.compile(&source).unwrap();
        engine.add_script(id, ast, "scripts/cost.rhai".into());
        let mut per_call = |timing: bool| {
            engine.set_timing(timing);
            let mut best = f64::MAX;
            for _ in 0..5 {
                let t = std::time::Instant::now();
                engine.call_updates(&mut world);
                best = best.min(t.elapsed().as_nanos() as f64 / N as f64);
            }
            // Only `take_times`, so the same bench runs on main for the A/B
            engine.take_times();
            best
        };
        let off = per_call(false);
        let on = per_call(true);
        // While the recorder records (#1288): a script function's call a
        // scope, an engine function's calls summed into one record
        flint_core::profile::set_recording(true);
        let recording = per_call(false);
        flint_core::profile::set_recording(false);
        println!(
            "{which}: {off:.0} ns a call (loop included) off, {on:.0} on (+{:.0}), \
             {recording:.0} recording (+{:.0})",
            on - off,
            recording - off
        );
    }
}
