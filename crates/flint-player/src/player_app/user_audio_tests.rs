#![allow(clippy::disallowed_methods)] // reads back the user directory the player wrote, which is the disk, never the vfs
//! #1217: a player's bus volume outlives the run and the game's fades.
//!
//! Run one: a settings menu's script sets the music bus to 0.4, then the
//! game fades music to 0.5, which the mixer is told is 0.2; the run ends
//! and the file is written. Run two, a fresh player on the same user
//! directory: music opens at 0.4 before any frame, and `on_init` reads it.
//! The mixer is the browser's worklet link, read here, so the numbers are
//! the ones the web hears; the file goes through `user_files`, which is
//! `localStorage` there. No window, no GPU, no sound card.

use super::scene_entry::SceneEntry;
use super::PlayerApp;
use flint_audio::engine::{amplitude_to_db, AudioEngine};
use flint_audio::remote::protocol::{Command, Reader};
use flint_audio::remote::Link;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MANIFEST: &str = "[project]\nid = \"radio_play\"\n";

/// Frame 1: the player's slider; frame 3: the game's fade.
const SETTINGS: &str = r#"
let frame = 0;
fn on_init() {
    test_log("music_at_start", user_bus_volume("music"));
}
fn on_update() {
    if frame == 1 {
        set_user_bus_volume("music", 0.4);
        test_log("read_back", user_bus_volume("music"));
    }
    if frame == 3 { set_bus_volume("music", 0.5); }
    frame += 1;
}
"#;

const SCENE: &str = r#"[scene]
name = "Settings"

[entities.menu.script]
source = "settings.rhai"
"#;

/// The last volume the mixer was given for the music bus, in dB.
fn music_db(link: &Link) -> Option<f32> {
    let out = link.take();
    let mut reader = Reader::new(&out.batch);
    let db = reader
        .by_ref()
        .filter_map(|c| match c {
            Command::BusVolume { bus: 1, db, .. } => Some(db),
            _ => None,
        })
        .last();
    assert_eq!(reader.error, None);
    db
}

/// A player on the scene, mixing into `link`, its user directory `config`.
fn start(config: &Path, link: &Link) -> PlayerApp {
    let path = "game/scenes/settings.scene.toml";
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
    app.saves.user_config_dir = Some(config.to_path_buf());
    let mut engine = AudioEngine::silent();
    engine.start_remote(link.clone());
    app.audio.engine = engine;
    app.enter_scene(SceneEntry::Start)
        .expect("the scene starts");
    app
}

fn logged(app: &PlayerApp, key: &str) -> Option<f64> {
    app.core.script.test_record().logged.get(key)?.as_f64()
}

#[test]
fn a_players_music_volume_is_kept_for_the_next_run_and_a_fade_is_heard_under_it() {
    let config: PathBuf =
        std::env::temp_dir().join(format!("flint-1217-user-audio-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&config);
    let file = config.join("radio_play").join("audio.toml");

    let mut files = MemorySource::new();
    files.insert("game/flint.toml", MANIFEST);
    files.insert("game/scenes/settings.scene.toml", SCENE);
    files.insert("game/scripts/settings.rhai", SETTINGS);
    vfs::scoped(Arc::new(files), || {
        // ── Run one ──
        let link = Link::default();
        let mut app = start(&config, &link);
        assert_eq!(
            logged(&app, "music_at_start"),
            Some(1.0),
            "nothing saved yet"
        );
        link.take();
        app.tick();
        app.tick(); // frame 1: the slider
        assert_eq!(
            logged(&app, "read_back"),
            Some(0.4),
            "read back the same frame"
        );
        assert_eq!(music_db(&link), Some(amplitude_to_db(0.4).0));
        app.tick();
        app.tick(); // frame 3: the game's fade to 0.5
        assert_eq!(
            music_db(&link),
            Some(amplitude_to_db(0.2).0),
            "0.5 x 0.4 is heard"
        );
        assert_eq!(
            app.audio.engine.user_bus_volume("music"),
            0.4,
            "the fade kept it"
        );
        assert!(!file.exists(), "not written until the debounce or the exit");
        // The exit (`exiting`'s call)
        app.flush_persistent_store(true, "exit");
        let text = std::fs::read_to_string(&file).expect("the file is written");
        assert!(text.contains("[buses]\nmusic = 0.4\n"), "{text}");
        drop(app);

        // ── Run two: the same user directory ──
        let link = Link::default();
        let app = start(&config, &link);
        assert_eq!(
            music_db(&link),
            Some(amplitude_to_db(0.4).0),
            "music opens at 0.4 before the first frame"
        );
        assert_eq!(
            app.audio.engine.bus_volume(flint_audio::Bus::MUSIC),
            Some(1.0)
        );
        assert_eq!(
            logged(&app, "music_at_start"),
            Some(0.4),
            "on_init reads it"
        );
    });
    let _ = std::fs::remove_dir_all(&config);
}

/// A playtest keeps saves in memory, and so the player's volumes: nothing
/// is read or written, however the script sets them.
#[test]
fn a_run_with_saves_in_memory_touches_no_audio_file() {
    let config: PathBuf =
        std::env::temp_dir().join(format!("flint-1217-in-memory-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&config);
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", MANIFEST);
    files.insert("game/scenes/settings.scene.toml", SCENE);
    files.insert("game/scripts/settings.rhai", SETTINGS);
    vfs::scoped(Arc::new(files), || {
        let path = "game/scenes/settings.scene.toml";
        let registry = flint_schema::SchemaRegistry::new();
        let (world, file) = flint_scene::load_scene(path, &registry).unwrap();
        let mut app = PlayerApp::new(world, path.into(), false, None, None);
        app.apply_scene_settings(&file);
        app.core.clock.frame_step = Some(1.0 / 30.0);
        app.saves.user_config_dir = Some(config.clone());
        app.launch.saves_in_memory = true;
        app.enter_scene(SceneEntry::Start).unwrap();
        for _ in 0..3 {
            app.tick();
        }
        assert_eq!(app.audio.engine.user_bus_volume("music"), 0.4);
        app.flush_persistent_store(true, "exit");
    });
    assert!(
        !config.exists(),
        "nothing written under {}",
        config.display()
    );
}
