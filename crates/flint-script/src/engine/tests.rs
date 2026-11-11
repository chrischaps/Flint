//! The engine itself: compiling, the callbacks a script defines and how
//! they are called, events, the command queue, script state.

use super::*;
use crate::context::InputSnapshot;
use flint_core::components as comp;
use flint_ecs::FlintWorld;
use flint_runtime::GameEvent;
use test_support::{probe_component, probe_float};

#[test]
fn test_engine_creation() {
    let engine = ScriptEngine::new();
    assert!(engine.scripts.is_empty());
}

#[test]
fn test_compile_valid_script() {
    let engine = ScriptEngine::new();
    let result = engine.compile("fn on_update() { let x = 1 + 2; }");
    assert!(result.is_ok());
}

#[test]
fn test_compile_invalid_script() {
    let engine = ScriptEngine::new();
    let result = engine.compile("fn on_update( { }");
    assert!(result.is_err());
}

#[test]
fn test_callback_detection() {
    let engine = ScriptEngine::new();
    let ast = engine
        .compile(
            r#"
        fn on_init() {}
        fn on_update() {}
        fn on_collision(other) {}
    "#,
        )
        .unwrap();

    let instance = ScriptInstance::new(ast, "test.rhai".into());
    assert!(instance.has_on_init);
    assert!(instance.has_on_update);
    assert!(instance.has_on_collision);
    assert!(!instance.has_on_trigger_enter);
    assert!(!instance.has_on_action);
    assert!(!instance.has_on_interact);
}

#[test]
fn test_entity_api_get_set() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("test_entity").unwrap();

    // Set a component
    world
        .set_component(
            id,
            "health",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("current".into(), toml::Value::Integer(100));
                m
            }),
        )
        .unwrap();

    // Compile a script that reads and writes
    let ast = engine
        .compile(&format!(
            r#"
        fn on_init() {{
            let me = self_entity();
            let hp = get_field(me, "health", "current");
            set_field(me, "health", "current", hp - 25);
        }}
    "#
        ))
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    // Check that health was modified
    let hp = world
        .get_components(id)
        .unwrap()
        .get_field("health", "current")
        .unwrap()
        .as_integer()
        .unwrap();
    assert_eq!(hp, 75);
}

#[test]
fn test_on_update_uses_delta_time() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("mover").unwrap();

    world
        .set_component(
            id,
            "state",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("accumulated".into(), toml::Value::Float(0.0));
                m
            }),
        )
        .unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_update() {
            let dt = delta_time();
            let me = self_entity();
            let acc = get_field(me, "state", "accumulated");
            set_field(me, "state", "accumulated", acc + dt);
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());

    // Simulate 3 frames
    engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
    engine.call_updates(&mut world);
    engine.call_updates(&mut world);
    engine.call_updates(&mut world);

    let acc = world
        .get_components(id)
        .unwrap()
        .get_field("state", "accumulated")
        .unwrap()
        .as_float()
        .unwrap();
    assert!((acc - 0.048).abs() < 1e-10);
}

// ─── Conducted-parameters API (ADR 0020) ──────────────────

#[test]
fn test_script_loads_from_memory_source() {
    // A web build has no disk: the scene's scripts/ folder is found, and
    // the script and the module it imports are read and run, entirely
    // from an in-memory bundle.
    use crate::sync::{load_scripts_from_scene, ScriptSync};
    use flint_core::vfs::{self, MemorySource};

    let mut bundle = MemorySource::new();
    bundle.insert(
        "game/scripts/greeter.rhai",
        r#"fn on_init() {
        import "greet" as greet;
        set_field(self_entity(), "probe", "coh", greet::value());
    }"#,
    );
    bundle.insert("game/scripts/lib/greet.rhai", "fn value() { 3.0 }");
    vfs::scoped(Arc::new(bundle), || {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = world.spawn("greeter").unwrap();
        probe_component(&mut world, id);
        world
            .set_component(
                id,
                comp::SCRIPT,
                toml::Value::Table({
                    let mut m = toml::map::Map::new();
                    m.insert("source".into(), toml::Value::String("greeter.rhai".into()));
                    m
                }),
            )
            .unwrap();

        // levels/ and scripts/ are siblings: with no project marker the
        // search still looks one level above the scene.
        let mut sync = ScriptSync::new();
        let scene = "game/levels/demo.scene.toml";
        let project = flint_core::project::Project::for_scene(std::path::Path::new(scene));
        load_scripts_from_scene(scene, &project, &mut sync);
        engine.set_module_search(sync.module_base().unwrap());
        sync.discover_and_load(&world, &mut engine);
        assert_eq!(engine.scripts.len(), 1, "the script must load");

        engine.call_inits(&mut world);
        assert_eq!(probe_float(&world, id, "coh"), 3.0);

        // A module that is not in the bundle fails like a missing file.
        let err = engine.engine.run(r#"import "nope" as nope;"#).unwrap_err();
        assert!(
            matches!(*err, rhai::EvalAltResult::ErrorModuleNotFound(ref p, _) if p == "nope"),
            "{err}"
        );
    });
}

/// `quit_game()` from `on_update` and from `on_draw_ui` (a pause menu's
/// EXIT row) both reach the player as one `QuitGame` each (#659).
#[test]
fn quit_game_queues_a_quit_from_update_and_draw_ui() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("menu").unwrap();
    let ast = engine
        .compile(
            r#"
        fn on_update() { quit_game(); }
        fn on_draw_ui() { quit_game(); }
    "#,
        )
        .unwrap();
    engine.add_script(id, ast, "menu.rhai".into());

    engine.call_updates(&mut world);
    let commands = engine.drain_commands();
    assert!(
        matches!(commands.as_slice(), [ScriptCommand::QuitGame]),
        "on_update: {commands:?}"
    );

    engine.call_draw_uis(&mut world);
    let commands = engine.drain_commands();
    assert!(
        matches!(commands.as_slice(), [ScriptCommand::QuitGame]),
        "on_draw_ui: {commands:?}"
    );
}

#[test]
fn test_fire_event_command() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("evt_test").unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_init() {
            fire_event("door_opened");
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);

    let commands = engine.drain_commands();
    assert_eq!(commands.len(), 1);
    match &commands[0] {
        ScriptCommand::FireEvent { name, .. } => {
            assert_eq!(name, "door_opened");
        }
        _ => panic!("Expected FireEvent command"),
    }
}

#[test]
fn test_collision_event_routing() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let entity_a = world.spawn("entity_a").unwrap();
    let entity_b = world.spawn("entity_b").unwrap();

    world
        .set_component(
            entity_a,
            "hits",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("count".into(), toml::Value::Integer(0));
                m
            }),
        )
        .unwrap();

    let ast = engine
        .compile(
            r#"
        fn on_collision(other) {
            let me = self_entity();
            let count = get_field(me, "hits", "count");
            set_field(me, "hits", "count", count + 1);
        }
    "#,
        )
        .unwrap();

    engine.add_script(entity_a, ast, "test.rhai".into());

    let events = vec![GameEvent::CollisionStarted { entity_a, entity_b }];
    engine.process_events(&events, &mut world);

    let count = world
        .get_components(entity_a)
        .unwrap()
        .get_field("hits", "count")
        .unwrap()
        .as_integer()
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_module_level_variables_persist() {
    // Verifies that `let x = value;` at module scope is accessible
    // from both on_init and on_update via Scope population.
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("persist_test").unwrap();

    world
        .set_component(
            id,
            "state",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("value".into(), toml::Value::Float(0.0));
                m
            }),
        )
        .unwrap();

    let ast = engine
        .compile(
            r#"
        let counter = 10.0;

        fn on_init() {
            // Modify the module-level variable
            counter = 42.0;
        }

        fn on_update() {
            // Read the value set in on_init
            let me = self_entity();
            set_field(me, "state", "value", counter);
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);
    engine.call_updates(&mut world);

    let value = world
        .get_components(id)
        .unwrap()
        .get_field("state", "value")
        .unwrap()
        .as_float()
        .unwrap();
    assert!(
        (value - 42.0).abs() < 1e-10,
        "Module-level var should persist: got {}",
        value
    );
}

#[test]
fn test_arity_mismatch_still_detects_function() {
    // A script with the wrong arity should still be detected (warning, not error)
    let engine = ScriptEngine::new();
    let ast = engine
        .compile(
            r#"
        fn on_update(dt) {
            // Wrong arity — should warn but still detect
        }
    "#,
        )
        .unwrap();

    let instance = ScriptInstance::new(ast, "test_mismatch.rhai".into());
    assert!(
        instance.has_on_update,
        "has_on_update should be true even with wrong arity"
    );
}

#[test]
fn test_top_level_state_reaches_callbacks_not_helpers() {
    // #628: what the book's "Script state" section says, held here.
    // Top-level `let`s live in the entity's scope: callbacks read and
    // write them and the writes last; a helper `fn` sees none of them; a
    // helper called as a method gets the caller's value as `this` by
    // reference, so it can grow an array without copying it.
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("pool").unwrap();

    let ast = engine
        .compile(
            r#"
        let frames = 0;
        let tiles = [];
        let helper_saw = "unset";

        fn peek() { frames }            // no `frames` in here
        fn add_tile(x) { this.push(x); } // `this` is the caller's array
        fn copy_add(arr, x) { arr.push(x); }  // `arr` is a copy

        fn on_update() {
            frames += 1;
            tiles.add_tile(frames);
            copy_add(tiles, 99);
            try { peek(); helper_saw = "saw it"; } catch { helper_saw = "error"; }
            set_field(self_entity(), "probe", "frames", frames);
            set_field(self_entity(), "probe", "tiles", tiles.len());
            set_field(self_entity(), "probe", "helper", helper_saw);
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast, "test.rhai".into());
    engine.call_inits(&mut world);
    engine.call_updates(&mut world);
    engine.call_updates(&mut world);

    let probe = world.get_component(id, "probe").unwrap();
    assert_eq!(probe.get("frames").unwrap().as_integer(), Some(2));
    assert_eq!(
        probe.get("tiles").unwrap().as_integer(),
        Some(2),
        "copy_add added nothing"
    );
    assert_eq!(probe.get("helper").unwrap().as_str(), Some("error"));
}
