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
