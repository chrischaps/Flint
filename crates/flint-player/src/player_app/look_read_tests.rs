//! #497: a script asks which look the scene draws with (`get_look()`) and
//! which it could (`list_looks()`). It reads back a look it set (from the
//! next read after the player applied the queued command), one F4 picked
//! and one a transition's scene brought, without keeping a copy of its own.
//!
//! The project is a `MemorySource`; no window, no GPU.

use super::scene_entry::SceneEntry;
use super::PlayerApp;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use flint_script::ScriptHost;
use std::sync::Arc;

/// Scene A's director: notes what it reads at `on_init` and each frame, and
/// asks for watercolor on its first frame.
const DIRECTOR: &str = r#"
let asked = false;
fn on_init() {
    persist_set("init_look", get_look());
    let looks = list_looks();
    persist_set("has_dusk", looks.contains("dusk"));
    persist_set("has_graphite", looks.contains("graphite"));
    persist_set("first_look", looks[0]);
}
fn on_update() {
    persist_set("seen", get_look());
    if !asked {
        asked = true;
        set_look("watercolor");
        persist_set("same_call", get_look());
    }
}
fn on_draw_ui() {
    persist_set("drawn", get_look());
}
"#;

/// Scene B's: reads its scene's own look, then takes the look off.
const KEEPER: &str = r#"
let cleared = false;
fn on_init() { persist_set("b_init_look", get_look()); }
fn on_update() {
    persist_set("seen", get_look());
    if !cleared { cleared = true; clear_look(); }
}
"#;

fn scene(name: &str, look: &str, script: &str) -> String {
    format!(
        "[scene]\nname = \"{name}\"\n\n[post_process]\nlook = \"{look}\"\n\n\
         [entities.director.script]\nsource = \"{script}\"\n"
    )
}

/// A look of the project's own, beside the built-ins.
const DUSK: &str = r#"
[look]
name = "dusk"
description = "A dim, warm frame"

[post_process]
exposure = 0.6
"#;

fn game() -> MemorySource {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert(
        "game/scenes/a.scene.toml",
        scene("A", "graphite", "director.rhai"),
    );
    files.insert(
        "game/scenes/b.scene.toml",
        scene("B", "matrix", "keeper.rhai"),
    );
    files.insert("game/scripts/director.rhai", DIRECTOR);
    files.insert("game/scripts/keeper.rhai", KEEPER);
    files.insert("game/styles/dusk.look.toml", DUSK);
    files
}

/// Started in scene A, saving (the scripts' `persist_set`) to a folder of
/// the test's own under the temp dir, never into the crate.
fn started() -> PlayerApp {
    let path = "game/scenes/a.scene.toml";
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(path, &registry).expect("the scene loads");
    let mut app = PlayerApp::new(world, path.into(), false, None, None);
    app.apply_scene_settings(&file);
    app.set_schemas(
        registry,
        SchemaDirs {
            dirs: Vec::new(),
            source: SchemaSource::Flags,
            dropped: Vec::new(),
            packages: None,
        },
    );
    app.core.clock.frame_step = Some(1.0 / 30.0);
    app.launch.save_dir =
        Some(std::env::temp_dir().join(format!("flint_497_{}", std::process::id())));
    app.enter_scene(SceneEntry::Start)
        .expect("the scene starts");
    app
}

fn stored(app: &PlayerApp, key: &str) -> toml::Value {
    app.core
        .store
        .get(key)
        .cloned()
        .unwrap_or_else(|| panic!("the script stored no {key}"))
}

fn text(app: &PlayerApp, key: &str) -> String {
    stored(app, key).as_str().unwrap_or_default().to_string()
}

#[test]
fn a_script_reads_back_the_look_it_set_f4_picked_and_a_transition_brought() {
    vfs::scoped(Arc::new(game()), || {
        let mut app = started();
        assert_eq!(
            text(&app, "init_look"),
            "graphite",
            "on_init sees the look the scene names"
        );
        assert_eq!(stored(&app, "has_dusk").as_bool(), Some(true));
        assert_eq!(stored(&app, "has_graphite").as_bool(), Some(true));
        assert_eq!(
            text(&app, "first_look"),
            "ascii_noir",
            "the list is sorted by name"
        );

        // Its own set_look: a queued command, so the call after it still
        // reads graphite; the player applies it after on_update, so this
        // frame's on_draw_ui and every later read see watercolor
        app.tick();
        assert_eq!(text(&app, "same_call"), "graphite");
        assert_eq!(text(&app, "drawn"), "watercolor");
        app.tick();
        assert_eq!(text(&app, "seen"), "watercolor");

        // F4's Look picker: its pick reaches `fade_to_look` at the top of
        // the next render (`style.pending_look`), with its Fade time; a
        // fade's target is the look from its first frame
        app.fade_to_look(Some("ink_wash"), 2.0).unwrap();
        app.tick();
        assert_eq!(text(&app, "seen"), "ink_wash");

        // A transition to a scene that names its own look
        ScriptHost::load_scene(&mut app, "b.scene.toml".into());
        app.advance_transition();
        assert!(app.core.scene_path.ends_with("b.scene.toml"));
        assert_eq!(
            text(&app, "b_init_look"),
            "matrix",
            "the next scene's on_init sees its own look, not the last scene's"
        );
        app.tick();
        assert_eq!(text(&app, "seen"), "matrix");

        // clear_look: none, read as ""
        app.tick();
        assert_eq!(text(&app, "seen"), "");
        assert_eq!(app.current_look(), None);
    });
}

#[test]
fn a_look_that_cant_be_found_leaves_get_look_where_it_was() {
    vfs::scoped(Arc::new(game()), || {
        let mut app = started();
        app.tick();
        assert_eq!(text(&app, "drawn"), "watercolor");
        // What a script's set_look("no_such_look") reaches
        ScriptHost::set_look(&mut app, Some("no_such_look".into()), 0.0);
        app.tick();
        assert_eq!(text(&app, "seen"), "watercolor");
        assert_eq!(app.current_look(), Some("watercolor"));
    });
}
