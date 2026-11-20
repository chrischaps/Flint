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

/// A Rhai function's last expression is its value, so a callback ending in
/// a call that returns one (`play_sound`'s handle, #699) returns it. The
/// engine reads no callback's value: it is no failure (#1031).
#[test]
fn a_callback_ending_in_a_value_is_no_failure() {
    let dir = scripts_dir(
        "ends-in-a-value",
        &[(
            "ends.rhai",
            "let t = 0.0;\n\
             fn on_init() { 7 }\n\
             fn on_update() {\n    t += 1.0;\n    if t >= 1.0 { play_sound(\"a.ogg\", 0.5) }\n}\n\
             fn on_draw_ui() { random() }\n",
        )],
    );
    let mut system = system_in(&dir);
    let mut world = FlintWorld::new();
    scripted(&mut world, "ender", "ends.rhai");
    system.initialize(&mut world).unwrap();
    for _ in 0..3 {
        system.update(&mut world, 1.0 / 60.0).unwrap();
        system.call_draw_uis(&mut world);
    }
    assert!(system.errors().is_empty(), "{:?}", system.errors().all());
}

/// A module the scene hasn't enabled, for the tests below.
struct Bells;
impl crate::ScriptModule for Bells {
    fn name(&self) -> &'static str {
        "bells"
    }
    fn enabled_when(&self) -> &'static str {
        "the scene has a belfry"
    }
    fn declare(&self, m: &mut crate::api::Module<'_>) {
        m.func("ring", "Ring a bell.")
            .arg("times", crate::api::builder::kind::Int)
            .run(|_, (_times,)| ());
    }
}

/// Ten seconds at 60 Hz.
const TEN_SECONDS: usize = 600;

/// A scene of one HUD running `dir`'s `hud.rhai` for ten seconds, after
/// `system`'s last scene (if any) is cleared as a transition clears it.
fn play_a_scene(system: &mut ScriptSystem, dir: &std::path::Path) {
    system.clear();
    // A transition's next scene sets its search again
    system.sync.set_scripts_dir(dir.to_path_buf());
    let mut world = FlintWorld::new();
    scripted(&mut world, "hud_controller", "hud.rhai");
    system.initialize(&mut world).unwrap();
    for _ in 0..TEN_SECONDS {
        system.update(&mut world, 1.0 / 60.0).unwrap();
    }
}

/// A call into a module that isn't enabled is logged once a scene (#856),
/// not at 1, 10, 100 calls as other errors are: a shared HUD that rings
/// every frame for 10 s says it once, and once more in the next scene
/// without the module. It is still counted every time, and still fails a
/// run under --strict-scripts.
#[test]
fn a_call_into_a_module_not_enabled_is_logged_once_a_scene() {
    let dir = scripts_dir(
        "once-a-scene",
        &[(
            "hud.rhai",
            "fn on_update() {\n    let n = 1;\n    ring(n);\n}\n",
        )],
    );
    let mut system = system_in(&dir);
    system.declare_module(&Bells);
    play_a_scene(&mut system, &dir);

    let errors = system.errors();
    assert_eq!(errors.all().len(), 1, "{:?}", errors.all());
    let rang = &errors.all()[0];
    assert!(rang.once_a_scene, "{rang:?}");
    assert!(
        rang.message
            .contains("ring: the bells module is not enabled here"),
        "{}",
        rang.message
    );
    assert_eq!(rang.line, Some(3));
    assert_eq!(rang.count, TEN_SECONDS as u64, "counted every frame");
    assert_eq!(errors.lines_logged(), 1, "said once over 10 s");

    play_a_scene(&mut system, &dir);
    let errors = system.errors();
    assert_eq!(errors.lines_logged(), 2, "once more in the next scene");
    assert_eq!(errors.all()[0].count, 2 * TEN_SECONDS as u64);
    assert_eq!(errors.on_screen().len(), 1);
    assert!(errors.strict_failure().is_some());
}

/// Other run-time errors are logged as #693 made them, at 1, 10 and 100
/// calls over the same ten seconds, with the module declared beside them.
#[test]
fn an_ordinary_error_is_still_logged_at_one_ten_and_a_hundred() {
    let dir = scripts_dir(
        "ordinary",
        &[("hud.rhai", "fn on_update() {\n    drw_text(1);\n}\n")],
    );
    let mut system = system_in(&dir);
    system.declare_module(&Bells);
    play_a_scene(&mut system, &dir);
    let errors = system.errors();
    assert!(!errors.all()[0].once_a_scene);
    assert_eq!(errors.all()[0].count, TEN_SECONDS as u64);
    assert_eq!(errors.lines_logged(), 3);
}
