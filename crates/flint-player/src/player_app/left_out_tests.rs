//! A package the project's `[packages]` leaves out, in the player (#1528):
//! its load summary words a left-out component as `flint check` does, and
//! a build that has the package runs none of it.
//!
//! The game is in a `MemorySource`, with the engine at `engine/` as a game
//! carries it, and this tree's own package manifests and living schemas, so
//! the list resolves as it does on disk.

use crate::player_app::scene_entry::SceneEntry;
use crate::player_app::PlayerApp;
use flint_core::vfs::{self, MemorySource};
use std::sync::{Arc, Mutex};

/// Every package of this tree, by name, with its manifest.
const MANIFESTS: &[(&str, &str)] = &[
    ("ai", include_str!("../../../../packages/ai/package.toml")),
    (
        "debug-hud",
        include_str!("../../../../packages/debug-hud/package.toml"),
    ),
    (
        "engine",
        include_str!("../../../../packages/engine/package.toml"),
    ),
    (
        "living",
        include_str!("../../../../packages/living/package.toml"),
    ),
    (
        "music",
        include_str!("../../../../packages/music/package.toml"),
    ),
    (
        "procgen",
        include_str!("../../../../packages/procgen/package.toml"),
    ),
    (
        "rhythm",
        include_str!("../../../../packages/rhythm/package.toml"),
    ),
];

/// A grove of two props, the scene's `[living]` table and a script that
/// writes down whether it may call the `living` module.
const GROVE: &str = r#"
[scene]
name = "Grove"

[living]
stagger = "per_branch"

[entities.oak]
[entities.oak.transform]
position = [0, 0, 0]
[entities.oak.living_prop]
lineage_seed = 7

[entities.birch]
[entities.birch.transform]
position = [3, 0, 0]
[entities.birch.living_prop]
lineage_seed = 8

[entities.keeper]
[entities.keeper.script]
source = "keeper.rhai"
"#;

const KEEPER: &str = r#"
fn on_init() {
    persist_set("living", module_enabled("living"));
}
"#;

/// The game, listing `packages` under `[packages]`.
fn game(packages: &str) -> Arc<MemorySource> {
    let mut files = MemorySource::new();
    files.insert("game/engine/crates/flint-core/Cargo.toml", "");
    files.insert(
        "game/engine/schemas/components/transform.toml",
        include_str!("../../../../schemas/components/transform.toml"),
    );
    files.insert(
        "game/engine/schemas/components/script.toml",
        include_str!("../../../../schemas/components/script.toml"),
    );
    for (name, manifest) in MANIFESTS {
        files.insert(
            format!("game/engine/packages/{name}/package.toml"),
            *manifest,
        );
    }
    files.insert(
        "game/engine/packages/living/schemas/components/living_prop.toml",
        include_str!("../../../../packages/living/schemas/components/living_prop.toml"),
    );
    files.insert(
        "game/engine/packages/living/schemas/scene/living.toml",
        include_str!("../../../../packages/living/schemas/scene/living.toml"),
    );
    files.insert(
        "game/flint.toml",
        format!("[project]\nschemas = [\"engine/schemas\"]\n\n[packages]\n{packages}"),
    );
    files.insert("game/scenes/grove.scene.toml", GROVE);
    files.insert("game/scripts/keeper.rhai", KEEPER);
    Arc::new(files)
}

const SCENE: &str = "game/scenes/grove.scene.toml";

/// The player brought up as `prepare` brings it: the one schema resolver,
/// the scene loaded with its registry, then the first scene entered.
fn play() -> PlayerApp {
    let flags: &[&str] = &[];
    let (registry, dirs) =
        flint_schema::SchemaRegistry::resolve(Some(SCENE.as_ref()), flags).expect("schemas");
    let (world, file) = flint_scene::load_scene(SCENE, &registry).expect("the scene loads");
    let mut app = PlayerApp::new(world, SCENE.into(), false, None, None);
    app.apply_scene_settings(&file);
    app.set_schemas(registry, dirs);
    app.core.clock.frame_step = Some(0.25);
    app.enter_scene(SceneEntry::Start)
        .expect("the scene starts");
    app.tick();
    app
}

/// What `run` logged at warn and above, each line without decoration.
fn warned<T>(run: impl FnOnce() -> T) -> (T, String) {
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let log = Captured::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .with_level(false)
        .finish();
    let out = tracing::subscriber::with_default(subscriber, run);
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    (out, text)
}

/// What the keeper's script wrote down: `module_enabled("living")`.
#[cfg(feature = "living")]
fn living_enabled(app: &PlayerApp) -> Option<bool> {
    app.core.store.get("living").and_then(|v| v.as_bool())
}

/// The ticket's first box: the load summary of a scene with living props in
/// a project that lists no `living` says `unlisted-package` with the line
/// to add, once for both props, and never `unknown-component`.
#[test]
fn a_left_out_packages_component_is_unlisted_in_the_load_summary() {
    vfs::scoped(game(""), || {
        let (_app, log) = warned(play);
        let summary: Vec<&str> = log
            .lines()
            .map(str::trim)
            .filter(|l| !l.starts_with("[living] "))
            .collect();
        assert_eq!(
            summary,
            [
                "game/scenes/grove.scene.toml: 1 error (the full list: flint validate \
                 game/scenes/grove.scene.toml)",
                "error[unlisted-package] entity 'birch' component 'living_prop': the component \
                 `living_prop` is the `living` package's, and [packages] doesn't list it (1 \
                 other entity too) (add `living = \"*\"` under [packages] in game/flint.toml)",
            ],
            "{log}"
        );
        assert!(!log.contains("unknown-component"), "{log}");
    });
}

/// The ticket's second box: there, `module_enabled("living")` is false, no
/// living subsystem runs, and one line names `[packages]` and what stands
/// still.
#[cfg(feature = "living")]
#[test]
fn a_build_with_living_runs_none_of_it_where_the_project_leaves_it_out() {
    vfs::scoped(game(""), || {
        let (app, log) = warned(play);
        assert_eq!(living_enabled(&app), Some(false));
        assert!(!app.core.subsystems.names().contains(&"living"));
        assert!(!app.core.script.enabled_modules().contains(&"living"));
        assert!(app.core.living_props.is_empty(), "no prop grows");
        let lines: Vec<&str> = log
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("[living] "))
            .collect();
        assert_eq!(
            lines,
            [
                "[living] [packages] in flint.toml leaves out `living`: the scene's living props, \
              [living] stand still (add `living = \"*\"` under [packages] in game/flint.toml)"
            ],
            "{log}"
        );
    });
}

/// The same game listing `living`: the subsystem and its module are back,
/// and nothing is called unlisted.
#[cfg(feature = "living")]
#[test]
fn listing_living_brings_it_back() {
    vfs::scoped(game("living = \"*\"\n"), || {
        let (app, log) = warned(play);
        assert_eq!(living_enabled(&app), Some(true));
        assert!(app.core.subsystems.names().contains(&"living"));
        assert!(!log.contains("unlisted-package"), "{log}");
        assert!(!log.contains("leaves out"), "{log}");
    });
}

/// With no `[packages]` table every package is in, as before the key.
#[cfg(feature = "living")]
#[test]
fn no_packages_table_leaves_nothing_out() {
    let mut files = (*game("")).clone();
    files.insert(
        "game/flint.toml",
        "[project]\nschemas = [\"engine/schemas\"]\n",
    );
    vfs::scoped(Arc::new(files), || {
        let (app, log) = warned(play);
        assert_eq!(living_enabled(&app), Some(true));
        assert!(!log.contains("unlisted-package"), "{log}");
    });
}

/// The line the music's builder says in a project that leaves `music` out.
#[test]
fn a_left_out_music_session_is_named_with_the_line_to_add() {
    vfs::scoped(game(""), || {
        let flags: &[&str] = &[];
        let dirs = flint_core::project::schema_dirs(Some(SCENE.as_ref()), flags).unwrap();
        let in_use = dirs.packages.expect("a [packages] list");
        let mut world = flint_ecs::FlintWorld::new();
        let id = world.spawn("band").unwrap();
        world
            .set_component(id, "music_session", toml::Value::Table(Default::default()))
            .unwrap();
        let line = super::music_left_out_line(&in_use, &world).unwrap();
        assert_eq!(
            line,
            "[music] [packages] in flint.toml leaves out `music`: the scene's music_session \
             doesn't play, and music_beat() reads -1 (add `music = \"*\"` under [packages] in \
             game/flint.toml)"
        );
        let quiet = flint_ecs::FlintWorld::new();
        assert_eq!(super::music_left_out_line(&in_use, &quiet), None);
    });
}
