//! A script's top-level statements (#692): they run once, at load, with the
//! world lent and `self_entity()` the script's own, and never again before a
//! callback; a top-level `import` still reaches every callback.

use super::*;
use crate::context::{LogLevel, ScriptCommand};
use flint_ecs::FlintWorld;
use test_support::{probe_component, probe_float};

/// An entity named `name` with the probe component.
fn probed(world: &mut FlintWorld, name: &str) -> EntityId {
    let id = world.spawn(name).unwrap();
    probe_component(world, id);
    id
}

fn add(engine: &mut ScriptEngine, id: EntityId, src: &str) {
    let ast = engine.compile(src).unwrap();
    engine.add_script(id, ast, "top.rhai".into());
}

/// The messages scripts logged since the last drain.
fn logged(engine: &ScriptEngine) -> Vec<String> {
    engine
        .drain_commands()
        .into_iter()
        .filter_map(|c| match c {
            ScriptCommand::Log {
                level: LogLevel::Info,
                message,
            } => Some(message),
            _ => None,
        })
        .collect()
}

#[test]
fn a_top_level_get_entity_names_the_entity() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let door = world.spawn("door").unwrap();
    let id = probed(&mut world, "keeper");
    add(
        &mut engine,
        id,
        r#"
        let door = get_entity("door");
        fn on_init() { set_field(self_entity(), "probe", "lx", door.to_float()); }
        fn on_update() { set_field(self_entity(), "probe", "ty", door.to_float()); }
        "#,
    );
    engine.call_inits(&mut world);
    engine.call_updates(&mut world);
    assert_eq!(probe_float(&world, id, "lx"), door.raw() as f64);
    assert_eq!(probe_float(&world, id, "ty"), door.raw() as f64);
}

#[test]
fn a_top_level_self_entity_is_the_scripts_own() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    // Two scripts, so "whichever entity ran last" is the wrong one for one
    let a = probed(&mut world, "a");
    let b = probed(&mut world, "b");
    let src = r#"
        let me = self_entity();
        fn on_init() { set_field(self_entity(), "probe", "lx", me.to_float()); }
    "#;
    add(&mut engine, a, src);
    engine.name_script(a, "a");
    add(&mut engine, b, src);
    engine.name_script(b, "b");
    engine.call_inits(&mut world);
    assert_eq!(probe_float(&world, a, "lx"), a.raw() as f64);
    assert_eq!(probe_float(&world, b, "lx"), b.raw() as f64);
}

#[test]
fn a_top_level_side_effect_runs_once() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = probed(&mut world, "chatty");
    add(
        &mut engine,
        id,
        r#"
        log("top level");
        let frames = 0;
        fn on_init() {}
        fn on_update() { frames += 1; }
        fn on_draw_ui() {}
        fn on_event(name, data) {}
        "#,
    );
    engine.call_inits(&mut world);
    for _ in 0..5 {
        engine.call_updates(&mut world);
        engine.call_draw_uis(&mut world);
    }
    engine.process_events(
        &[flint_runtime::GameEvent::Custom {
            name: "ping".into(),
            data: toml::Value::Table(Default::default()),
        }],
        &mut world,
    );
    assert_eq!(logged(&engine), vec!["top level".to_string()]);
    let frames = engine.scripts[&id].scope.get_value::<i64>("frames");
    assert_eq!(frames, Some(5), "top-level state persists across callbacks");
}

#[test]
fn a_top_level_import_reaches_every_callback() {
    let dir = std::env::temp_dir().join(format!("flint-692-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("helpers.rhai"),
        "log(\"module top level\"); fn twice(x) { x * 2.0 }",
    )
    .unwrap();

    let mut engine = ScriptEngine::new();
    engine.set_module_base(dir.clone());
    let mut world = FlintWorld::new();
    let id = probed(&mut world, "importer");
    add(
        &mut engine,
        id,
        r#"
        import "helpers" as h;
        log("script top level");
        fn on_init() { set_field(self_entity(), "probe", "lx", h::twice(2.0)); }
        fn on_update() { set_field(self_entity(), "probe", "ty", h::twice(3.0)); }
        "#,
    );
    engine.call_inits(&mut world);
    engine.call_updates(&mut world);
    engine.call_updates(&mut world);
    assert_eq!(probe_float(&world, id, "lx"), 4.0);
    assert_eq!(probe_float(&world, id, "ty"), 6.0);
    assert_eq!(
        logged(&engine),
        vec![
            "module top level".to_string(),
            "script top level".to_string()
        ],
        "each top level ran once"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reload_runs_its_new_top_level_with_the_world() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let door = world.spawn("door").unwrap();
    let id = probed(&mut world, "keeper");
    add(&mut engine, id, "fn on_update() {}");
    engine.call_inits(&mut world);

    let v2 = engine
        .compile(
            r#"
            let door = get_entity("door");
            let me = self_entity();
            fn on_update() {
                set_field(self_entity(), "probe", "lx", door.to_float());
                set_field(self_entity(), "probe", "ty", me.to_float());
            }
            "#,
        )
        .unwrap();
    let added = engine.reload_script(&mut world, id, v2);
    assert_eq!(added, vec!["door".to_string(), "me".to_string()]);
    engine.call_updates(&mut world);
    assert_eq!(probe_float(&world, id, "lx"), door.raw() as f64);
    assert_eq!(probe_float(&world, id, "ty"), id.raw() as f64);
}

/// `global::NAME` is Rhai's way for a function to reach a script's
/// top-level `const` (#1012). Rhai fills it in as the top level runs, so it
/// went missing from every callback once the top level ran only once (#692).
#[test]
fn a_global_constant_reaches_every_callback_and_the_functions_they_call() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = probed(&mut world, "glow");
    add(
        &mut engine,
        id,
        r#"
        const GLOW = 0.5;
        const KEYS = [1.0, 2.0, 4.0];
        log("top level");
        fn last_key() { global::KEYS[global::KEYS.len() - 1] }
        fn on_init() { set_field(self_entity(), "probe", "lx", global::GLOW); }
        fn on_update() {
            set_field(self_entity(), "probe", "ty", global::GLOW * 2.0 + last_key());
        }
        "#,
    );
    engine.call_inits(&mut world);
    engine.call_updates(&mut world);
    engine.call_updates(&mut world);
    assert!(engine.errors().is_empty(), "{:?}", engine.errors().all());
    assert_eq!(probe_float(&world, id, "lx"), 0.5);
    assert_eq!(probe_float(&world, id, "ty"), 5.0);
    assert_eq!(logged(&engine), vec!["top level".to_string()]);
}

/// A constant's initializer runs once, with the top level: `global::` reads
/// the value it made then, never a new call of the initializer.
#[test]
fn a_global_constant_keeps_its_value_from_the_one_top_level_run() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let door = world.spawn("door").unwrap();
    let id = probed(&mut world, "keeper");
    add(
        &mut engine,
        id,
        r#"
        let made = 0;
        const DOOR = get_entity("door");
        const FIRST = { made += 1; made };
        fn on_update() {
            set_field(self_entity(), "probe", "lx", global::DOOR.to_float());
            set_field(self_entity(), "probe", "ty", global::FIRST.to_float());
        }
        "#,
    );
    engine.call_inits(&mut world);
    for _ in 0..3 {
        engine.call_updates(&mut world);
    }
    assert!(engine.errors().is_empty(), "{:?}", engine.errors().all());
    assert_eq!(probe_float(&world, id, "lx"), door.raw() as f64);
    assert_eq!(probe_float(&world, id, "ty"), 1.0);
    let made = engine.scripts[&id].scope.get_value::<i64>("made");
    assert_eq!(made, Some(1), "the initializer ran once");
}

/// A script that reads no `global::` constant is called on its imports
/// alone, as before: nothing more runs per callback.
#[test]
fn a_script_with_no_global_reads_calls_on_its_imports_alone() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = probed(&mut world, "plain");
    add(
        &mut engine,
        id,
        "const A = 1.0; fn on_update() { set_field(self_entity(), \"probe\", \"lx\", A); }",
    );
    engine.call_inits(&mut world);
    engine.call_updates(&mut world);
    assert_eq!(probe_float(&world, id, "lx"), 1.0);
    assert!(engine.scripts[&id].calls.statements().is_empty());
}

/// A constant a reload adds is a `global::` constant too.
#[test]
fn a_reload_adds_its_new_global_constants() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = probed(&mut world, "keeper");
    add(&mut engine, id, "const A = 1.0; fn on_update() {}");
    engine.call_inits(&mut world);
    let v2 = engine
        .compile(
            r#"
            const A = 1.0;
            const B = 3.0;
            fn on_update() { set_field(self_entity(), "probe", "lx", global::A + global::B); }
            "#,
        )
        .unwrap();
    engine.reload_script(&mut world, id, v2);
    engine.call_updates(&mut world);
    assert!(engine.errors().is_empty(), "{:?}", engine.errors().all());
    assert_eq!(probe_float(&world, id, "lx"), 4.0);
}
