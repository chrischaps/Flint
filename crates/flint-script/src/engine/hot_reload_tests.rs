//! Hot reload: a script edited, broken and mended, compiled on the worker,
//! and a nearer script that appears later (#633, #706).

use crate::context::InputSnapshot;
use crate::engine::test_support::*;
use crate::engine::ScriptEngine;
use flint_ecs::FlintWorld;
use std::sync::Arc;

#[test]
fn test_hot_reload_and_bad_edit() {
    // Hot-reload picks up a binding edit; a syntax error keeps the old
    // AST running (the compile-error branch in check_hot_reload).
    use crate::sync::ScriptSync;
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("binder").unwrap();
    probe_component(&mut world, id);

    let dir = std::env::temp_dir().join(format!("flint-conducted-reload-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("binder.rhai");
    let v1 = r#"fn on_update() {
        set_field(self_entity(), "probe", "coh", music_beat());
    }"#;
    std::fs::write(&path, v1).unwrap();

    let mut sync = ScriptSync::new();
    sync.set_scripts_dir(dir.clone());
    let ast = engine.compile_file(&path).unwrap();
    engine.add_script(id, ast, "binder.rhai".into());
    sync.check_hot_reload(&mut engine); // records the initial timestamp

    engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
    engine.call_updates(&mut world);
    assert_eq!(probe_float(&world, id, "coh"), -1.0);

    // v2 writes a different field; bump mtime explicitly (Windows mtime
    // granularity would otherwise swallow a fast rewrite).
    let v2 = r#"fn on_update() {
        set_field(self_entity(), "probe", "lx", music_bar().to_float() + 3.0);
    }"#;
    std::fs::write(&path, v2).unwrap();
    let f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    drop(f);
    sync.check_hot_reload(&mut engine);
    engine.call_updates(&mut world);
    assert_eq!(probe_float(&world, id, "lx"), 2.0, "v2 must be live");

    // A broken edit: the old (v2) AST keeps running, no panic.
    std::fs::write(&path, "fn on_update( {").unwrap();
    let f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(10))
        .unwrap();
    drop(f);
    sync.check_hot_reload(&mut engine);
    world
        .set_component(
            id,
            "probe",
            toml::Value::Table({
                let mut m = toml::map::Map::new();
                m.insert("lx".into(), toml::Value::Float(0.0));
                m
            }),
        )
        .unwrap();
    engine.call_updates(&mut world);
    assert_eq!(
        probe_float(&world, id, "lx"),
        2.0,
        "old AST must keep running after a bad edit"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_hot_reload_compiles_on_the_worker() {
    // #633: with the worker, a frame only swaps finished ASTs in. An
    // edited script and an edited lib module both arrive a few calls
    // later, and an import after the lib change takes the worker's AST.
    use crate::sync::ScriptSync;
    let dir = std::env::temp_dir().join(format!("flint-reload-worker-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("lib")).unwrap();
    let script = dir.join("greeter.rhai");
    let module = dir.join("lib/greet.rhai");
    let write = |path: &std::path::Path, text: &str, ahead: u64| {
        std::fs::write(path, text).unwrap();
        // Windows mtime granularity would otherwise swallow a fast rewrite
        let f = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(ahead))
            .unwrap();
    };
    let body = |field: &str| {
        format!(
            r#"fn on_update() {{
                import "greet" as greet;
                set_field(self_entity(), "probe", "{field}", greet::value());
            }}"#
        )
    };
    write(&script, &body("coh"), 0);
    write(&module, "fn value() { 1.0 }", 0);

    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("greeter").unwrap();
    probe_component(&mut world, id);
    let mut sync = ScriptSync::new();
    sync.use_reload_worker();
    sync.set_scripts_dir(dir.clone());
    engine.set_module_search(sync.module_base().unwrap());
    let ast = engine.compile_file(&script).unwrap();
    engine.add_script(id, ast, "greeter.rhai".into());

    // Calls the frame loop makes until `done` holds (the worker's
    // results land on a later call, never during one)
    let mut run_until =
        |engine: &mut ScriptEngine, world: &mut FlintWorld, done: &dyn Fn(&FlintWorld) -> bool| {
            let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                sync.check_hot_reload(engine);
                engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
                engine.call_updates(world);
                if done(world) || std::time::Instant::now() > until {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
    run_until(&mut engine, &mut world, &|w| {
        probe_float(w, id, "coh") == 1.0
    });
    assert_eq!(probe_float(&world, id, "coh"), 1.0);

    // The script changes: it writes another field
    write(&script, &body("lx"), 5);
    run_until(&mut engine, &mut world, &|w| {
        probe_float(w, id, "lx") == 1.0
    });
    assert_eq!(probe_float(&world, id, "lx"), 1.0, "the edited script runs");

    // The module changes: the import resolves to the worker's compile
    write(&module, "fn value() { 2.0 }", 10);
    run_until(&mut engine, &mut world, &|w| {
        probe_float(w, id, "lx") == 2.0
    });
    assert_eq!(probe_float(&world, id, "lx"), 2.0, "the edited module runs");

    // A broken edit keeps the last good script running
    write(&script, "fn on_update( {", 15);
    for _ in 0..40 {
        sync.check_hot_reload(&mut engine);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    engine.call_updates(&mut world);
    assert_eq!(
        probe_float(&world, id, "lx"),
        2.0,
        "old AST after a bad edit"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_a_scripts_folder_beside_the_scene_does_not_hide_the_projects() {
    // ADR 0114: before the one resolver the first `scripts/` that
    // existed was the only one read, so a `scripts/` beside a scene hid
    // every script and module in the project's.
    use crate::sync::{load_scripts_from_scene, ScriptSync};
    use flint_core::vfs::{self, MemorySource};

    let mut bundle = MemorySource::new();
    bundle.insert("game/flint.toml", "");
    bundle.insert(
        "game/scripts/greeter.rhai",
        r#"fn on_init() {
        import "greet" as greet;
        set_field(self_entity(), "probe", "coh", greet::value());
    }"#,
    );
    bundle.insert("game/scripts/lib/greet.rhai", "fn value() { 3.0 }");
    bundle.insert(
        "game/levels/act1/scripts/local.rhai",
        r#"fn on_init() { set_field(self_entity(), "probe", "lx", 7.0); }"#,
    );
    // Two of one name: the nearer one runs.
    bundle.insert(
        "game/scripts/both.rhai",
        r#"fn on_init() { set_field(self_entity(), "probe", "ty", 1.0); }"#,
    );
    bundle.insert(
        "game/levels/act1/scripts/both.rhai",
        r#"fn on_init() { set_field(self_entity(), "probe", "ty", 2.0); }"#,
    );
    vfs::scoped(Arc::new(bundle), || {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let greeter = scripted(&mut world, "greeter", "greeter.rhai");
        let local = scripted(&mut world, "local", "local.rhai");
        let both = scripted(&mut world, "both", "both.rhai");
        let missing = scripted(&mut world, "missing", "nope.rhai");

        // Two folders below the root, with a scripts/ of its own.
        let scene = "game/levels/act1/cave.scene.toml";
        let project = flint_core::project::Project::for_scene(std::path::Path::new(scene));
        let mut sync = ScriptSync::new();
        load_scripts_from_scene(scene, &project, &mut sync);
        engine.set_module_search(sync.module_base().unwrap());
        sync.discover_and_load(&world, &mut engine);
        assert_eq!(engine.scripts.len(), 3, "all but the missing one load");

        engine.call_inits(&mut world);
        assert_eq!(probe_float(&world, greeter, "coh"), 3.0, "project's");
        assert_eq!(probe_float(&world, local, "lx"), 7.0, "the scene's own");
        assert_eq!(probe_float(&world, both, "ty"), 2.0, "the nearer of two");

        // What isn't anywhere says every place it was looked for.
        let err = sync
            .locate_for(
                missing,
                "nope.rhai",
                std::path::Path::new("game/levels/act1"),
            )
            .unwrap_err();
        assert!(
            err.starts_with("script 'nope.rhai' not found. Tried:"),
            "{err}"
        );
        for tried in [
            "game/levels/act1/scripts/nope.rhai",
            "game/levels/scripts/nope.rhai",
            "game/scripts/nope.rhai",
        ] {
            assert!(err.contains(tried), "{tried} in {err}");
        }
    });
}

#[test]
fn test_hot_reload_takes_a_nearer_script_that_appears_later() {
    // The poll follows the resolver's search: a script loaded from the
    // project's scripts/ is replaced by one of the same name written
    // beside the scene afterwards, and a module likewise.
    use crate::sync::{load_scripts_from_scene, ScriptSync};
    let root = std::env::temp_dir().join(format!("flint-reload-nearer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scripts/lib")).unwrap();
    std::fs::create_dir_all(root.join("levels")).unwrap();
    std::fs::write(root.join("flint.toml"), "").unwrap();
    let scene = root.join("levels/a.scene.toml");
    std::fs::write(&scene, "").unwrap();
    let body = |field: &str| {
        format!(
            r#"fn on_update() {{
                import "greet" as greet;
                set_field(self_entity(), "probe", "{field}", greet::value());
            }}"#
        )
    };
    std::fs::write(root.join("scripts/greeter.rhai"), body("coh")).unwrap();
    std::fs::write(root.join("scripts/lib/greet.rhai"), "fn value() { 1.0 }").unwrap();

    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = scripted(&mut world, "greeter", "greeter.rhai");
    let scene = scene.to_string_lossy().into_owned();
    let project = flint_core::project::Project::for_scene(std::path::Path::new(&scene));
    assert_eq!(project.found, flint_core::project::Found::Manifest);
    let mut sync = ScriptSync::new();
    load_scripts_from_scene(&scene, &project, &mut sync);
    engine.set_module_search(sync.module_base().unwrap());
    sync.discover_and_load(&world, &mut engine);
    assert_eq!(engine.scripts.len(), 1);

    let mut frame = |engine: &mut ScriptEngine, world: &mut FlintWorld| {
        // One changed script is recompiled a call
        for _ in 0..3 {
            sync.check_hot_reload(engine);
        }
        engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
        engine.call_updates(world);
    };
    frame(&mut engine, &mut world);
    assert_eq!(probe_float(&world, id, "coh"), 1.0);
    assert_eq!(probe_float(&world, id, "lx"), -1.0);

    // A script of the same name appears beside the scene: it takes over.
    std::fs::create_dir_all(root.join("levels/scripts/lib")).unwrap();
    std::fs::write(root.join("levels/scripts/greeter.rhai"), body("lx")).unwrap();
    frame(&mut engine, &mut world);
    assert_eq!(probe_float(&world, id, "lx"), 1.0, "the nearer script runs");

    // And a module of the same name, nearer than the one imported.
    std::fs::write(
        root.join("levels/scripts/lib/greet.rhai"),
        "fn value() { 5.0 }",
    )
    .unwrap();
    frame(&mut engine, &mut world);
    assert_eq!(probe_float(&world, id, "lx"), 5.0, "the nearer module runs");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn test_hot_reload_preserves_scope() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("reload_test").unwrap();

    // First version: set a persistent variable
    let ast1 = engine
        .compile(
            r#"
        fn on_init() {
            // No persistent state in this version
        }
        fn on_update() {
            let me = self_entity();
            log("v1 running");
        }
    "#,
        )
        .unwrap();

    engine.add_script(id, ast1, "test.rhai".into());
    engine.call_inits(&mut world);

    // Hot-reload with new version
    let ast2 = engine
        .compile(
            r#"
        fn on_update() {
            let me = self_entity();
            log("v2 running");
        }
    "#,
        )
        .unwrap();

    let script = engine.scripts.get_mut(&id).unwrap();
    script.hot_reload(ast2);

    // init_called should still be true
    assert!(script.init_called);
    assert!(script.has_on_update);
    assert!(!script.has_on_init);
}

#[test]
fn test_hot_reload_initialises_new_top_level_lets() {
    // #634: a reload that adds `let boost_s` (read by on_update) used to
    // fail every frame with `Variable not found`. Now the new variable gets
    // its initial value and the old one keeps the value it had reached.
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("reload_lets").unwrap();
    let mut state = toml::map::Map::new();
    state.insert("ticks".into(), toml::Value::Float(-1.0));
    state.insert("boost".into(), toml::Value::Float(-1.0));
    state.insert("limit".into(), toml::Value::Float(-1.0));
    world
        .set_component(id, "state", toml::Value::Table(state))
        .unwrap();

    let v1 = engine
        .compile(
            r#"
        let ticks = 0.0;
        fn on_update() {
            ticks += 1.0;
            set_field(self_entity(), "state", "ticks", ticks);
        }
    "#,
        )
        .unwrap();
    engine.add_script(id, v1, "audio.rhai".into());
    engine.call_inits(&mut world);
    engine.call_updates(&mut world);
    engine.call_updates(&mut world);
    engine.call_updates(&mut world);

    let v2 = engine
        .compile(
            r#"
        let ticks = 0.0;
        let boost_s = 0.5;
        let boost_s = 0.25;
        const LIMIT = 3.0;
        fn on_update() {
            ticks += 1.0;
            boost_s += 1.0;
            let me = self_entity();
            set_field(me, "state", "ticks", ticks);
            set_field(me, "state", "boost", boost_s);
            set_field(me, "state", "limit", LIMIT);
        }
    "#,
        )
        .unwrap();
    let added = engine.reload_script(id, v2);
    assert_eq!(added, vec!["boost_s".to_string(), "LIMIT".to_string()]);
    engine.call_updates(&mut world);
    engine.call_updates(&mut world);

    let field = |name: &str| {
        world
            .get_components(id)
            .unwrap()
            .get_field("state", name)
            .and_then(|v| v.as_float())
            .unwrap()
    };
    assert_eq!(field("ticks"), 5.0, "kept state carries on counting");
    assert_eq!(
        field("boost"),
        2.25,
        "new let starts at its last declaration"
    );
    assert_eq!(field("limit"), 3.0, "new const is there");

    // A second reload of the same source adds nothing and resets nothing.
    let v3 = engine
        .compile("let boost_s = 0.25; fn on_update() {}")
        .unwrap();
    assert!(engine.reload_script(id, v3).is_empty());
    let script = engine.scripts.get(&id).unwrap();
    assert_eq!(script.scope.get_value::<f64>("boost_s"), Some(2.25));
}
