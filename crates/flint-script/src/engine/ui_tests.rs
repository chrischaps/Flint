//! The structured UI from a script (#731): the showcase's own pause menu
//! (flint-examples' `scripts/showcase_controller.rhai` and
//! `ui/pause.ui.toml`, frozen in `tests/fixtures/scripts/` since #984, and
//! the engine's `scripts/lib/menu.rhai`, read from the checkout) opened,
//! driven with keys and a tap, and heard in `on_ui`.

use super::*;
use crate::context::{InputSnapshot, ScriptCommand};
use flint_core::vfs::{self, MemorySource};
use flint_ecs::FlintWorld;
use std::path::Path;
use std::sync::Arc;
use test_support::scripted;

/// The repository's file at `rel`, from this crate.
fn repo_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    #[allow(clippy::disallowed_methods)] // a test reads the checkout
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn pressing(actions: &[&str]) -> InputSnapshot {
    let mut snap = InputSnapshot::default();
    for a in actions {
        snap.actions_just_pressed.insert(a.to_string());
        snap.actions_pressed.insert(a.to_string());
    }
    snap
}

fn frame(engine: &mut ScriptEngine, world: &mut FlintWorld, input: InputSnapshot) {
    engine.provide_context(input, 1.0 / 60.0, 0.0);
    engine.call_inits(world);
    engine.call_draw_uis(world);
}

fn shown(engine: &ScriptEngine, id: &str) -> bool {
    crate::lock_or_recover(&engine.ctx).ui_system.is_shown(id)
}

fn focused(engine: &ScriptEngine) -> Option<String> {
    crate::lock_or_recover(&engine.ctx)
        .ui_system
        .focused()
        .map(str::to_string)
}

#[test]
fn the_showcase_pause_menu_works_from_keys_and_touch() {
    let mut files = MemorySource::new();
    // At the paths the frozen copies name them by, from the project root
    for rel in [
        "ui/pause.ui.toml",
        "ui/pause.style.toml",
        "scripts/showcase_controller.rhai",
    ] {
        let frozen = rel.trim_start_matches("scripts/");
        files.insert(rel, repo_file(&format!("tests/fixtures/scripts/{frozen}")));
    }
    files.insert("scripts/lib/menu.rhai", repo_file("scripts/lib/menu.rhai"));
    vfs::scoped(Arc::new(files), || {
        let mut engine = ScriptEngine::new();
        engine.set_module_base("scripts/lib".into());
        let mut world = FlintWorld::new();
        let id = scripted(&mut world, "game_controller", "showcase_controller.rhai");
        let source = vfs::read_to_string(Path::new("scripts/showcase_controller.rhai")).unwrap();
        let ast = engine.compile(&source).unwrap();
        engine.add_script(id, ast, "showcase_controller.rhai".into());

        // Loaded at the top level, hidden until Escape
        frame(&mut engine, &mut world, InputSnapshot::default());
        assert!(!shown(&engine, "pause_menu"));
        frame(&mut engine, &mut world, pressing(&["pause"]));
        assert!(shown(&engine, "pause_menu"));
        // Its autofocus button takes focus the next frame
        frame(&mut engine, &mut world, InputSnapshot::default());
        assert_eq!(focused(&engine).as_deref(), Some("resume"));

        // The arrows (or a pad's d-pad, the same action) walk down
        frame(&mut engine, &mut world, pressing(&["ui_down"]));
        frame(&mut engine, &mut world, pressing(&["ui_down"]));
        assert_eq!(focused(&engine).as_deref(), Some("quit"));
        frame(&mut engine, &mut world, pressing(&["ui_accept"]));
        let commands = engine.drain_commands();
        assert!(
            commands
                .iter()
                .any(|c| matches!(c, ScriptCommand::QuitGame)),
            "Quit pressed: {commands:?}"
        );
        assert!(engine.errors.is_empty(), "{:?}", engine.errors);

        // A tap on Resume closes the menu
        let (x, y, w, h) = crate::lock_or_recover(&engine.ctx)
            .ui_system
            .get_rect("resume", 1280.0, 720.0)
            .unwrap();
        let mut tap = InputSnapshot::default();
        tap.touch_taps.push((
            ((x + w / 2.0) / 1280.0) as f64,
            ((y + h / 2.0) / 720.0) as f64,
        ));
        frame(&mut engine, &mut world, tap);
        assert!(!shown(&engine, "pause_menu"));
        // Input is routed before the scripts answer it, so focus lets go of
        // the menu the script hid at the next frame's routing
        frame(&mut engine, &mut world, InputSnapshot::default());
        assert_eq!(focused(&engine), None, "focus lets go of a hidden menu");
        assert!(engine.errors.is_empty(), "{:?}", engine.errors);
    });
}

#[test]
fn a_bad_layout_is_a_script_error_naming_the_file_and_element() {
    let mut files = MemorySource::new();
    files.insert("ui/bad.ui.toml", "[elements.go]\ntype = \"buttn\"\n");
    vfs::scoped(Arc::new(files), || {
        let mut engine = ScriptEngine::new();
        let mut world = FlintWorld::new();
        let id = scripted(&mut world, "menu", "menu.rhai");
        let ast = engine.compile("load_ui(\"ui/bad.ui.toml\");").unwrap();
        engine.add_script(id, ast, "menu.rhai".into());
        frame(&mut engine, &mut world, InputSnapshot::default());
        let errors = format!("{:?}", engine.errors);
        assert!(errors.contains("ui/bad.ui.toml"), "{errors}");
        assert!(
            errors.contains("element `go`: unknown type `buttn`"),
            "{errors}"
        );
    });
}
