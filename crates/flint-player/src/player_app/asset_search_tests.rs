//! The player's loaders on the project's one asset search (ADR 0114, #706):
//! each kind is found from a scene two folders deep, and a miss says where
//! it looked.

use super::scene_loading::{
    build_model_load_config, load_animations_from_world, load_audio_from_world,
    load_sequences_from_world, load_sprite_animations_from_world, visible_files,
};
use super::PlayerApp;
use flint_animation::AnimationSystem;
use flint_core::project::{shown, AssetKind, Project};
use flint_core::vfs::{self, MemorySource};
use flint_ecs::FlintWorld;
use flint_render::model_loader::{resolve_model_path, resolve_texture_path};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const DEEP: &str = "game/scenes/act1/cave/a.scene.toml";

const SEQUENCE: &str =
    "name = \"intro\"\n\n[[events]]\ntime = 0.0\nkind = \"cue\"\nname = \"start\"\n";

/// A one-pixel PNG.
fn png() -> Vec<u8> {
    let mut bytes = Vec::new();
    image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

/// A game whose assets are all in its root's folders, with a scene two
/// folders below `scenes/`.
fn game() -> MemorySource {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert(DEEP, "");
    files.insert("game/models/tree.glb", "");
    files.insert("game/textures/wall.png", png());
    files.insert("game/sprites/heart.png", png());
    files.insert("game/animations/intro.sequence.toml", SEQUENCE);
    files.insert("game/animations/door.anim.toml", "");
    files.insert("game/animations/hero.sprite.toml", "");
    files.insert("game/audio/hit.ogg", "not really vorbis");
    files.insert("game/audio/wind.wav", "nor a wave");
    files
}

fn in_game<R>(f: impl FnOnce(&Project) -> R) -> R {
    vfs::scoped(Arc::new(game()), || {
        let project = Project::for_scene(Path::new(DEEP));
        assert_eq!(project.root, Path::new("game"));
        f(&project)
    })
}

#[test]
fn a_deep_scene_finds_the_projects_models_and_textures() {
    in_game(|project| {
        let config = build_model_load_config(project, DEEP, &FlintWorld::new(), None, None);
        assert_eq!(
            resolve_model_path(&config, "tree"),
            Ok(PathBuf::from("game/models/tree.glb"))
        );
        assert_eq!(
            resolve_texture_path(&config, "wall"),
            Ok(PathBuf::from("game/textures/wall.png"))
        );
        assert_eq!(
            resolve_texture_path(&config, "wall.png"),
            Ok(PathBuf::from("game/textures/wall.png"))
        );
    });
}

#[test]
fn a_missing_model_says_every_path_tried_and_what_was_asked_first() {
    in_game(|project| {
        let config = build_model_load_config(project, DEEP, &FlintWorld::new(), None, None);
        let missing = resolve_model_path(&config, "boulder").unwrap_err();
        let text = config.not_found_text(&missing);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "model 'boulder' not found. Tried:");
        assert_eq!(
            lines[1..9],
            [
                "  game/scenes/act1/cave/models/boulder.glb",
                "  game/scenes/act1/cave/models/boulder.gltf",
                "  game/scenes/act1/models/boulder.glb",
                "  game/scenes/act1/models/boulder.gltf",
                "  game/scenes/models/boulder.glb",
                "  game/scenes/models/boulder.gltf",
                "  game/models/boulder.glb",
                "  game/models/boulder.gltf",
            ]
        );
        // No catalog here, so only procgen was asked before the files.
        assert_eq!(
            lines[9],
            "  and before the files: a procgen spec of the same name (specs/<name>.procgen.toml)"
        );
        assert_eq!(lines.len(), 10);

        let missing = resolve_texture_path(&config, "moss").unwrap_err();
        let text = config.not_found_text(&missing);
        assert!(
            text.starts_with("texture 'moss' not found. Tried:"),
            "{text}"
        );
        assert!(text.contains("\n  game/textures/moss_color.png"), "{text}");
    });
}

#[test]
fn a_catalog_path_is_asked_before_the_files() {
    in_game(|project| {
        let mut config = build_model_load_config(project, DEEP, &FlintWorld::new(), None, None);
        config
            .overrides
            .insert("tree".into(), PathBuf::from("game/textures/wall.png"));
        assert_eq!(
            resolve_model_path(&config, "tree"),
            Ok(PathBuf::from("game/textures/wall.png"))
        );
        // An override whose file is gone falls through to the search.
        config
            .overrides
            .insert("tree".into(), PathBuf::from("game/gone.glb"));
        assert_eq!(
            resolve_model_path(&config, "tree"),
            Ok(PathBuf::from("game/models/tree.glb"))
        );
    });
}

#[test]
fn a_deep_scene_loads_the_projects_sequences_and_a_miss_lists_the_folders() {
    in_game(|project| {
        let mut animation = AnimationSystem::new();
        load_sequences_from_world(project, DEEP, &mut animation);
        assert_eq!(animation.sequence_names(), ["intro"]);
        for (kind, file) in [
            (AssetKind::Animations, "game/animations/door.anim.toml"),
            (
                AssetKind::SpriteAnimations,
                "game/animations/hero.sprite.toml",
            ),
            (AssetKind::Sequences, "game/animations/intro.sequence.toml"),
        ] {
            assert_eq!(visible_files(project, kind, DEEP), [PathBuf::from(file)]);
        }
        // The other two loaders read the same folders (the files here are
        // empty, so they warn and register nothing; they must not panic).
        load_animations_from_world(project, DEEP, &mut animation);
        load_sprite_animations_from_world(project, DEEP, &mut animation);
    });
}

#[test]
fn clips_load_farthest_first_so_the_nearest_has_the_last_word() {
    let mut files = game();
    files.insert("game/scenes/animations/intro.sequence.toml", SEQUENCE);
    files.insert("game/scenes/animations/zoom.sequence.toml", SEQUENCE);
    files.insert("game/animations/alpha.sequence.toml", SEQUENCE);
    vfs::scoped(Arc::new(files), || {
        let project = Project::for_scene(Path::new(DEEP));
        let got: Vec<String> = visible_files(&project, AssetKind::Sequences, DEEP)
            .iter()
            .map(|p| shown(p))
            .collect();
        // The root's own (less the `intro` a nearer folder hides), then the
        // nearer folder's, each by name.
        assert_eq!(
            got,
            [
                "game/animations/alpha.sequence.toml",
                "game/scenes/animations/intro.sequence.toml",
                "game/scenes/animations/zoom.sequence.toml",
            ]
        );
    });
}

#[test]
fn a_deep_scene_finds_the_projects_sounds_by_path_and_preloads_its_audio_folder() {
    in_game(|project| {
        let mut world = FlintWorld::new();
        let speaker = world.spawn("speaker").unwrap();
        let source: toml::Table = "file = \"audio/hit.ogg\"".parse().unwrap();
        world
            .set_component(
                speaker,
                flint_core::components::AUDIO_SOURCE,
                toml::Value::Table(source),
            )
            .unwrap();
        // Queued, as the web does: nothing is decoded, so the bytes needn't
        // be sound.
        let mut audio = flint_audio::AudioSystem::new();
        load_audio_from_world(&world, &mut audio, project, DEEP, true, true);
        assert!(audio.engine.is_queued("audio/hit.ogg"));
        assert!(audio.engine.is_queued("audio/wind.wav"));
        assert_eq!(audio.engine.queued_count(), 2);

        // With the preload off, only what a source names.
        let mut audio = flint_audio::AudioSystem::new();
        load_audio_from_world(&world, &mut audio, project, DEEP, false, true);
        assert!(audio.engine.is_queued("audio/hit.ogg"));
        assert_eq!(audio.engine.queued_count(), 1);
    });
}

#[test]
fn a_deep_scene_finds_the_projects_ui_sprites() {
    in_game(|_| {
        let mut app = PlayerApp::new(FlintWorld::new(), DEEP.into(), false, None, None);
        assert_eq!(app.project().root, Path::new("game"));
        assert!(app.load_ui_texture("heart.png"));
        assert!(app.hud.textures.contains_key("heart.png"));
        // A miss is remembered, so a draw command repeating it every frame
        // warns once.
        assert!(!app.load_ui_texture("gone.png"));
        assert!(!app.load_ui_texture("gone.png"));
        assert_eq!(app.hud.sprites_warned.len(), 1);
    });
}
