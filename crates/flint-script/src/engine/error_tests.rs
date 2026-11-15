//! Script errors counted, not warned about each frame (#693): a run-time
//! error in a callback, one in a helper, a top-level one, and a script that
//! doesn't compile or isn't there, each one entry naming its entity, file
//! and line.

use crate::engine::test_support::*;
use crate::errors::{COMPILE, LOAD, TOP_LEVEL};
use crate::ScriptSystem;
use flint_ecs::FlintWorld;
use flint_runtime::RuntimeSystem;

/// A folder of its own for a test's scripts.
fn scripts_dir(test: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("flint-693-{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
    dir
}

fn system_in(dir: &std::path::Path) -> ScriptSystem {
    let mut system = ScriptSystem::new();
    system.sync.set_scripts_dir(dir.to_path_buf());
    system.engine.errors.set_quiet(true);
    system
}

#[test]
fn an_error_every_frame_is_one_entry_counted_with_its_entity_and_line() {
    let dir = scripts_dir(
        "every-frame",
        &[(
            "hud.rhai",
            "fn on_update() {\n    let x = 1;\n    drw_text(x);\n}\n",
        )],
    );
    let mut system = system_in(&dir);
    let mut world = FlintWorld::new();
    scripted(&mut world, "hud_controller", "hud.rhai");
    system.initialize(&mut world).unwrap();
    for _ in 0..60 {
        system.update(&mut world, 1.0 / 60.0).unwrap();
    }

    let errors = system.errors();
    assert_eq!(errors.all().len(), 1, "{:?}", errors.all());
    assert_eq!(errors.total(), 60);
    let err = &errors.all()[0];
    assert_eq!(err.count, 60);
    assert_eq!(err.entity_name, "hud_controller");
    assert_eq!(err.callback, "on_update");
    assert_eq!(err.line, Some(3));
    assert!(err.file.ends_with("hud.rhai"), "{}", err.file);
    assert!(err.message.contains("drw_text"), "{}", err.message);
    assert!(!err.message.contains("line 3"), "{}", err.message);
    assert_eq!((err.first_frame, err.last_frame), (1, 60));
    let shown = err.to_string();
    assert!(
        shown.ends_with(&format!(
            "hud.rhai:3:5: hud_controller, on_update: {}",
            err.message
        )),
        "{shown}"
    );
}

#[test]
fn an_error_in_a_helper_names_the_helper_and_its_line() {
    let dir = scripts_dir(
        "helper",
        &[(
            "door.rhai",
            "fn swing(a) {\n    a.nope()\n}\nfn on_init() {\n    swing(1);\n}\n",
        )],
    );
    let mut system = system_in(&dir);
    let mut world = FlintWorld::new();
    scripted(&mut world, "door", "door.rhai");
    system.initialize(&mut world).unwrap();

    let err = &system.errors().all()[0];
    assert_eq!(err.callback, "on_init");
    assert_eq!(err.function.as_deref(), Some("swing"));
    assert_eq!(err.line, Some(2));
    assert!(err.context().contains("door, on_init > swing"));
}

#[test]
fn a_script_that_does_not_compile_or_is_not_there_is_an_error_too() {
    let dir = scripts_dir(
        "load",
        &[
            ("broken.rhai", "fn on_update( {\n}\n"),
            (
                "top.rhai",
                "let x = undefined_thing();\nfn on_update() {}\n",
            ),
        ],
    );
    let mut system = system_in(&dir);
    let mut world = FlintWorld::new();
    scripted(&mut world, "crate_a", "broken.rhai");
    scripted(&mut world, "crate_b", "missing.rhai");
    scripted(&mut world, "crate_c", "top.rhai");
    system.initialize(&mut world).unwrap();

    let errors = system.errors();
    let of = |entity: &str| {
        errors
            .all()
            .iter()
            .find(|e| e.entity_name == entity)
            .unwrap_or_else(|| panic!("no error for {entity}: {:?}", errors.all()))
    };
    let compile = of("crate_a");
    assert_eq!(compile.callback, COMPILE);
    assert!(compile.file.ends_with("broken.rhai"));
    assert_eq!(compile.line, Some(1), "{compile:?}");
    assert!(compile.is_load_error());

    let missing = of("crate_b");
    assert_eq!(missing.callback, LOAD);
    assert_eq!(missing.file, "missing.rhai");

    let top = of("crate_c");
    assert_eq!(top.callback, TOP_LEVEL);
    assert_eq!(top.line, Some(1));
}

#[test]
fn a_script_that_did_not_compile_at_load_loads_once_saved() {
    let dir = scripts_dir("mend", &[("chest.rhai", "fn on_init( {\n}\n")]);
    let mut system = system_in(&dir);
    let mut world = FlintWorld::new();
    let id = scripted(&mut world, "chest", "chest.rhai");
    system.initialize(&mut world).unwrap();
    assert_eq!(system.errors().on_screen().len(), 1);

    // Mended, with a later time than Windows' mtime granularity might give
    let path = dir.join("chest.rhai");
    std::fs::write(
        &path,
        "fn on_init() {\n    set_field(self_entity(), \"probe\", \"lx\", 4.0);\n}\n",
    )
    .unwrap();
    let f = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    drop(f);

    system.update(&mut world, 1.0 / 60.0).unwrap();
    assert_eq!(
        probe_float(&world, id, "lx"),
        4.0,
        "the mended script ran on_init"
    );
    assert!(system.errors().on_screen().is_empty());
    assert_eq!(system.errors().distinct(), 1, "the run still counts it");
}

#[test]
fn a_scene_change_takes_errors_off_the_screen_and_keeps_them_in_the_count() {
    let dir = scripts_dir(
        "clear",
        &[("bad.rhai", "fn on_update() {\n    nope();\n}\n")],
    );
    let mut system = system_in(&dir);
    let mut world = FlintWorld::new();
    scripted(&mut world, "thing", "bad.rhai");
    system.initialize(&mut world).unwrap();
    system.update(&mut world, 1.0 / 60.0).unwrap();
    assert_eq!(system.errors().on_screen().len(), 1);

    system.clear();
    assert!(system.errors().on_screen().is_empty());
    assert_eq!(system.errors().distinct(), 1);
    assert_eq!(
        system.errors().summary()[0],
        "[script] 1 error (1 time in all)"
    );
}
