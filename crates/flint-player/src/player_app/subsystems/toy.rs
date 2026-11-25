#![cfg(test)]
//! A toy subsystem, to hold #724's third box: adding a subsystem is one new
//! file here and touches no other file in `player_app/`. `build.rs` lists this
//! file's `for_scene`; a scene with a `[toy]` table gets one, which counts the
//! frames it hears. A capture (`HeadlessGame`) builds it from the same list
//! (#1252), unless its table says `window = true`: then it is the window's
//! alone, and the capture names it in its notes.

use super::{Lasts, SceneCtx, Stage, StageCtx, Subsystem};
use crate::player_app::PlayerApp;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use std::sync::Arc;

/// It reads no game-state policy.
pub(crate) const POLICY: Option<&str> = None;

/// Its script module, `toy` (#698): `toy_frames()`, enabled in a scene
/// with a toy.
pub(crate) const SCRIPT_MODULES: &[&dyn flint_script::ScriptModule] = &[&ToyModule];

/// Its debug key (#717): F12 says how many frames it has heard, in a scene
/// with a toy; anywhere else F12 is the game's.
pub(crate) const DEBUG_KEYS: flint_runtime::DebugKeyGroup = flint_runtime::DebugKeyGroup {
    owner: "toy",
    when: "a scene with a [toy] table",
    keys: &[flint_runtime::DebugKey {
        action: "toy_frames",
        key: "F12",
        does: "Say how many frames the toy has heard",
    }],
};

/// What the toy hands its module's functions: the frames it has heard.
#[derive(Debug, Default)]
struct ToyFrames(u32);

struct ToyModule;

impl flint_script::ScriptModule for ToyModule {
    fn name(&self) -> &'static str {
        "toy"
    }

    fn enabled_when(&self) -> &'static str {
        "the scene has a [toy] table"
    }

    fn declare(&self, m: &mut flint_script::api::Module<'_>) {
        m.func("toy_frames", "The frames the toy has heard.")
            .returns(flint_script::api::builder::kind::Int, "frames")
            .run(|call, ()| call.ctx().module_state.get_mut::<ToyFrames>().0 as i64);
    }
}

#[derive(Default)]
pub(crate) struct Toy {
    pub(crate) frames: u32,
    /// `[toy] window = true`: a toy only a window shows.
    window: bool,
}

/// A toy for a scene with a `[toy]` table.
pub(crate) fn for_scene(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    let table = ctx.tables.get("toy")?;
    let window = table.get("window").and_then(|w| w.as_bool()) == Some(true);
    Some(Box::new(Toy { frames: 0, window }))
}

impl Subsystem for Toy {
    fn name(&self) -> &'static str {
        "toy"
    }

    fn lasts(&self) -> Lasts {
        Lasts::Scene
    }

    fn script_modules(&self) -> &[&'static str] {
        &["toy"]
    }

    fn update(&mut self, stage: Stage, ctx: &mut StageCtx) {
        if stage == Stage::Sync {
            self.frames += 1;
            ctx.script.set_module_state(ToyFrames(self.frames));
        }
    }

    fn debug_keys(&self) -> &'static [flint_runtime::DebugKey] {
        DEBUG_KEYS.keys
    }

    fn on_debug_key(&mut self, _action: &str, key: &str, ctx: &mut StageCtx) {
        ctx.note.say(key, &format!("{} frames", self.frames));
    }

    fn presentation_only(&self) -> Option<&'static str> {
        self.window.then_some("a toy only a window shows")
    }
}

/// The toy's F12 is a debug key only in a scene with a toy (#717): there it
/// is dispatched and never reaches the game's input, and `--list-keys`'
/// table says it is live; in a scene without one the game gets F12.
#[test]
fn the_toys_key_is_live_only_in_a_scene_with_a_toy() {
    use winit::keyboard::KeyCode;
    let mut files = MemorySource::new();
    files.insert("toys/flint.toml", "");
    files.insert("toys/scenes/with.scene.toml", "[scene]\nname = \"With\"\n\n[toy]\n");
    files.insert("toys/scenes/without.scene.toml", "[scene]\nname = \"Without\"\n");
    vfs::scoped(Arc::new(files), || {
        let mut app = app_in("toys/scenes/with.scene.toml");
        app.tick();
        let live = |app: &PlayerApp| {
            let table = app.debug_key_table();
            table.entries.iter().find(|e| e.action == "toy_frames").unwrap().live
        };
        assert!(live(&app));
        assert!(app.handle_debug_key(KeyCode::F12), "the toy's");
        assert_eq!(app.hud.key_note.showing(), Some(("F12", "1 frames")));
        assert!(!app.input.is_key_down(KeyCode::F12));

        let mut app = app_in("toys/scenes/without.scene.toml");
        assert!(!live(&app), "listed, but not live");
        assert!(!app.handle_debug_key(KeyCode::F12), "the game's");
    });
}

fn app_in(scene: &str) -> PlayerApp {
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(scene, &registry).expect("the scene loads");
    let mut app = PlayerApp::new(world, scene.into(), false, None, None);
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
    app.core.clock.frame_step = Some(0.25);
    app.enter_scene(crate::player_app::scene_entry::SceneEntry::Start)
        .expect("the scene starts");
    app
}

#[test]
fn a_toy_in_its_own_file_comes_with_a_scene_that_asks_and_hears_its_frames() {
    let mut files = MemorySource::new();
    files.insert("toys/flint.toml", "");
    files.insert(
        "toys/scenes/with.scene.toml",
        "[scene]\nname = \"With\"\n\n[toy]\nsize = 1\n",
    );
    files.insert(
        "toys/scenes/without.scene.toml",
        "[scene]\nname = \"Without\"\n",
    );
    vfs::scoped(Arc::new(files), || {
        let mut app = app_in("toys/scenes/with.scene.toml");
        app.tick();
        app.tick();
        let toy = app
            .core
            .subsystems
            .find::<Toy>()
            .expect("a [toy] scene has one");
        assert_eq!(toy.frames, 2);

        assert!(app.execute_scene_transition("without.scene.toml"));
        assert!(
            app.core.subsystems.find::<Toy>().is_none(),
            "it went with its scene"
        );

        let app = app_in("toys/scenes/without.scene.toml");
        assert!(app.core.subsystems.find::<Toy>().is_none());
    });
}

/// A script asks the toy's module from its `on_init` and every frame: the
/// subsystem is built before the scripts load, so the module is enabled
/// for their first call (#698). In a scene with no toy the same call is
/// the "not enabled" error, and nothing it would have written is.
#[test]
fn a_scene_with_a_toy_lets_its_scripts_call_the_toy_module_from_on_init() {
    const COUNTER: &str = r#"
fn on_init() { set_field(self_entity(), "probe", "init", toy_frames()); }
fn on_update() { set_field(self_entity(), "probe", "seen", toy_frames()); }
"#;
    let scripted = |name: &str, toy: bool| {
        format!(
            "[scene]\nname = \"{name}\"\n{}\n[entities.counter]\n[entities.counter.probe]\n\
             init = -1\nseen = -1\n[entities.counter.script]\nsource = \"counter.rhai\"\n",
            if toy { "\n[toy]\nsize = 1\n" } else { "" }
        )
    };
    let mut files = MemorySource::new();
    files.insert("toys/flint.toml", "");
    files.insert("toys/scenes/with.scene.toml", scripted("With", true));
    files.insert("toys/scenes/without.scene.toml", scripted("Without", false));
    files.insert("toys/scripts/counter.rhai", COUNTER);
    vfs::scoped(Arc::new(files), || {
        let probe = |app: &PlayerApp, field: &str| {
            let id = app.core.world.get_id("counter").expect("the counter");
            app.core
                .world
                .get_components(id)
                .and_then(|c| c.get_field("probe", field))
                .and_then(|v| v.as_integer())
        };
        let mut app = app_in("toys/scenes/with.scene.toml");
        assert_eq!(app.core.script.enabled_modules(), ["toy"]);
        assert_eq!(probe(&app, "init"), Some(0), "on_init reached the module");
        app.tick();
        app.tick();
        assert_eq!(probe(&app, "seen"), Some(1), "last frame's count");

        let mut app = app_in("toys/scenes/without.scene.toml");
        assert!(app.core.script.enabled_modules().is_empty());
        app.tick();
        assert_eq!(probe(&app, "init"), Some(-1), "the call was an error");
        assert_eq!(probe(&app, "seen"), Some(-1));
    });
}

/// The capture of `scene` (`flint render --scripts`'s game), its scene's
/// subsystems built from the player's list and its scripts started.
fn capture_of(scene: &str) -> crate::HeadlessGame {
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(scene, &registry).expect("the scene loads");
    let project = flint_core::project::Project::for_scene(std::path::Path::new(scene));
    let mut game = crate::HeadlessGame::new(world, scene, project);
    game.start_subsystems(
        file.scene_tables(),
        crate::CaptureStart {
            scripts: true,
            ..Default::default()
        },
    );
    game.start_scripts(
        true,
        None,
        (320.0, 180.0),
        &flint_render::PostProcessConfig::default(),
    );
    game
}

/// The toy, in its own file and named nowhere else, runs in a capture as in
/// the player (#1252): `HeadlessGame` builds it from the generated list,
/// steps it with the frame and enables its module for `on_init` and every
/// step after. A scene without a `[toy]` table gets none there either.
#[test]
fn a_capture_builds_the_toy_from_the_players_list_and_its_scripts_read_its_frames() {
    const COUNTER: &str = r#"
fn on_init() { set_field(self_entity(), "probe", "init", toy_frames()); }
fn on_update() { set_field(self_entity(), "probe", "seen", toy_frames()); }
"#;
    let scripted = |name: &str, toy: bool| {
        format!(
            "[scene]\nname = \"{name}\"\n{}\n[entities.counter]\n[entities.counter.probe]\n\
             init = -1\nseen = -1\n[entities.counter.script]\nsource = \"counter.rhai\"\n",
            if toy { "\n[toy]\nsize = 1\n" } else { "" }
        )
    };
    let mut files = MemorySource::new();
    files.insert("toys/flint.toml", "");
    files.insert("toys/scenes/with.scene.toml", scripted("With", true));
    files.insert("toys/scenes/without.scene.toml", scripted("Without", false));
    files.insert("toys/scripts/counter.rhai", COUNTER);
    vfs::scoped(Arc::new(files), || {
        let probe = |game: &crate::HeadlessGame, field: &str| {
            let world = game.world();
            let id = world.get_id("counter").expect("the counter");
            world
                .get_components(id)
                .and_then(|c| c.get_field("probe", field))
                .and_then(|v| v.as_integer())
        };
        let mut camera = flint_render::Camera::new();
        let mut game = capture_of("toys/scenes/with.scene.toml");
        assert_eq!(probe(&game, "init"), Some(0), "on_init reached the module");
        for _ in 0..3 {
            game.step(0.25, &mut camera, None, None);
        }
        let toy = game.core.subsystems.find::<Toy>().expect("built");
        assert_eq!(toy.frames, 3, "it heard every step");
        assert_eq!(probe(&game, "seen"), Some(2), "last step's count");
        assert!(game.notes().is_empty(), "{:?}", game.notes());
        assert_eq!(game.script_errors().total(), 0);

        let mut game = capture_of("toys/scenes/without.scene.toml");
        game.step(0.25, &mut camera, None, None);
        assert!(game.core.subsystems.find::<Toy>().is_none());
        assert_eq!(probe(&game, "seen"), Some(-1), "the call was an error");
    });
}

/// A subsystem that is the window's alone is built by the player and left
/// out of a capture, which names it once in its notes, however many steps
/// it takes (#1252).
#[test]
fn a_capture_names_a_subsystem_it_leaves_out_once_in_its_notes() {
    const SCENE: &str = "[scene]\nname = \"Window\"\n\n[toy]\nwindow = true\n";
    let mut files = MemorySource::new();
    files.insert("toys/flint.toml", "");
    files.insert("toys/scenes/window.scene.toml", SCENE);
    vfs::scoped(Arc::new(files), || {
        let app = app_in("toys/scenes/window.scene.toml");
        assert!(app.core.subsystems.find::<Toy>().is_some(), "the player's");

        let mut camera = flint_render::Camera::new();
        let mut game = capture_of("toys/scenes/window.scene.toml");
        for _ in 0..3 {
            game.step(0.25, &mut camera, None, None);
        }
        assert!(game.core.subsystems.find::<Toy>().is_none(), "left out");
        assert_eq!(
            game.notes(),
            ["toy: a headless capture doesn't run this subsystem (a toy only a window shows)"]
        );
    });
}
