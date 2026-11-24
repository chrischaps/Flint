//! A trace of a callback that calls an imported module's function and a
//! closure (#1281): each is a scope inside the callback's. Its own test
//! binary, so turning the recorder on (global) never reaches the library's
//! tests, which run in parallel and count a module's top-level runs.

use flint_core::profile;
use flint_ecs::FlintWorld;
use flint_script::engine::ScriptEngine;

const MENU: &str = "fn open(n) { helper(n) + 1 }\nfn helper(n) { n * 2 }\n";

const SCRIPT: &str = r#"
import "menu" as menu;
fn on_update() {
    let opened = menu::open(3);
    let count = 0;
    let bump = |x| { count += x; count };
    for i in 0..50 { bump.call(1); }
}
"#;

#[test]
fn a_module_function_and_a_closure_are_scopes_inside_the_callbacks() {
    if !profile::COMPILED_IN {
        return;
    }
    let dir = std::env::temp_dir().join(format!("flint-1281-trace-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("menu.rhai"), MENU).unwrap();

    let mut engine = ScriptEngine::new();
    engine.set_module_base(dir.clone());
    let mut world = FlintWorld::new();
    let id = world.spawn("traced").unwrap();
    let ast = engine.compile(SCRIPT).unwrap();
    engine.add_script(id, ast, "scripts/traced.rhai".into());

    profile::set_recording(true);
    engine.call_updates(&mut world);
    profile::set_recording(false);
    let _ = std::fs::remove_dir_all(&dir);

    let me = std::thread::current();
    let thread = me.name().unwrap();
    let scopes = profile::snapshot(None)
        .into_iter()
        .find(|t| t.thread == thread)
        .expect("this thread recorded")
        .scopes;
    let callback = scopes
        .iter()
        .find(|s| s.name == "scripts/traced.rhai:on_update")
        .unwrap_or_else(|| panic!("no callback scope in {scopes:#?}"));
    let inside = |s: &&profile::TakenScope| {
        s.depth == callback.depth + 1
            && s.start_ns >= callback.start_ns
            && s.start_ns + s.dur_ns <= callback.start_ns + callback.dur_ns
    };
    let open: Vec<_> = scopes.iter().filter(|s| s.name == "menu::open()").collect();
    assert_eq!(open.len(), 1, "{scopes:#?}");
    assert!(inside(&open[0]), "{:?} inside {callback:?}", open[0]);
    // The module's own call to its helper is a scope inside `open`'s
    let helper = scopes
        .iter()
        .find(|s| s.name == "menu::helper()")
        .expect("menu::helper()");
    assert_eq!(helper.depth, callback.depth + 2);
    let closures: Vec<_> = scopes
        .iter()
        .filter(|s| s.name == "scripts/traced.rhai:closure at line 6")
        .collect();
    assert_eq!(closures.len(), 50, "{scopes:#?}");
    assert!(closures.iter().all(inside), "{closures:#?}");
}
