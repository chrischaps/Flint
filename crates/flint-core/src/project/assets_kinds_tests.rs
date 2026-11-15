//! Each kind on the one resolver: that a scene two folders deep finds the
//! project's file, that the as-written step and a folder of `""` do what
//! the module docs say, and that the one-release order-change warning fires
//! exactly where a kind's old search picked another file.

use super::*;
use crate::vfs::MemorySource;
use std::sync::Arc;

/// Run `f` over a memory source holding `files`.
fn with_files<R>(files: &[&str], f: impl FnOnce() -> R) -> R {
    let mut source = MemorySource::new();
    for path in files {
        source.insert(path, "");
    }
    vfs::scoped(Arc::new(source), f)
}

fn project(start: &str) -> Project {
    Project::discover_with_home(Path::new(start), None).unwrap()
}

/// A name of each kind as a scene writes it, and the file under a project
/// root that it stands for.
const NAMED: &[(AssetKind, &str, &str)] = &[
    (
        AssetKind::Particles,
        "fire",
        "particles/fire.particles.toml",
    ),
    (AssetKind::Models, "tree", "models/tree.glb"),
    (AssetKind::Models, "rock", "models/rock.gltf"),
    (AssetKind::Textures, "wall", "textures/wall.png"),
    (AssetKind::Textures, "wall.jpg", "textures/wall.jpg"),
    (AssetKind::Textures, "brick", "textures/brick_color.png"),
    (AssetKind::Textures, "raw", "textures/raw"),
    (AssetKind::Sprites, "heart.png", "sprites/heart.png"),
    (AssetKind::Sprites, "ui/coin.png", "sprites/ui/coin.png"),
    (AssetKind::Animations, "door", "animations/door.anim.toml"),
    (
        AssetKind::SpriteAnimations,
        "hero",
        "animations/hero.sprite.toml",
    ),
    (
        AssetKind::Sequences,
        "intro",
        "animations/intro.sequence.toml",
    ),
    (AssetKind::Audio, "audio/hit.ogg", "audio/hit.ogg"),
    (AssetKind::Audio, "hit.ogg", "audio/hit.ogg"),
    (AssetKind::Audio, "music/theme.flac", "music/theme.flac"),
    (AssetKind::Scripts, "hud.rhai", "scripts/hud.rhai"),
    (AssetKind::Scripts, "hud", "scripts/hud.rhai"),
    (AssetKind::Scripts, "ui/menu.rhai", "scripts/ui/menu.rhai"),
    // What `import "util"` asks for.
    (AssetKind::Scripts, "lib/util.rhai", "scripts/lib/util.rhai"),
    (AssetKind::Prefabs, "lamp", "prefabs/lamp.prefab.toml"),
    (AssetKind::WoozePresets, "misty", "wooze/misty.wooze.toml"),
    (AssetKind::Files, "config/input.toml", "config/input.toml"),
    (AssetKind::Files, "terrain/height.png", "terrain/height.png"),
    (
        AssetKind::ParticleTextures,
        "spark.png",
        "particles/spark.png",
    ),
    (AssetKind::ParticleTextures, "fx/smoke.png", "fx/smoke.png"),
    (
        AssetKind::Scenes,
        "levels/two.scene.toml",
        "levels/two.scene.toml",
    ),
    (
        AssetKind::Palettes,
        "sunset",
        "palettes/sunset.palette.toml",
    ),
    (
        AssetKind::Palettes,
        "palettes/sunset.palette.toml",
        "palettes/sunset.palette.toml",
    ),
    (
        AssetKind::GlyphSets,
        "terminal",
        "fonts/terminal.glyphs.toml",
    ),
    (
        AssetKind::GlyphSets,
        "fonts/terminal.glyphs.toml",
        "fonts/terminal.glyphs.toml",
    ),
    (AssetKind::Fonts, "fonts/ui.font.toml", "fonts/ui.font.toml"),
    (AssetKind::Fonts, "ui", "fonts/ui.font.toml"),
    (AssetKind::FontFaces, "Barlow.ttf", "fonts/Barlow.ttf"),
    (AssetKind::FontFaces, "Mono.otf", "fonts/Mono.otf"),
    (AssetKind::Looks, "noir", "styles/noir.look.toml"),
    (
        AssetKind::Looks,
        "out/tuned.look.toml",
        "out/tuned.look.toml",
    ),
    (AssetKind::Papers, "cotton", "papers/cotton.png"),
    (AssetKind::Papers, "papers/cotton.png", "papers/cotton.png"),
    (AssetKind::Papers, "scans/linen.jpg", "scans/linen.jpg"),
    (AssetKind::Strokes, "hb", "strokes/hb.png"),
    (AssetKind::Styles, "glow", "styles/glow.shader.toml"),
];

#[test]
fn a_scene_two_folders_deep_finds_every_kind_in_the_project() {
    for &(kind, name, file) in NAMED {
        let at_root = format!("game/{file}");
        with_files(
            &[
                "game/flint.toml",
                &at_root,
                "game/scenes/act1/cave/a.scene.toml",
                "game/scenes/b.scene.toml",
            ],
            || {
                let deep = project("game/scenes/act1/cave/a.scene.toml");
                let shallow = project("game/scenes/b.scene.toml");
                let want = PathBuf::from(&at_root);
                assert_eq!(
                    deep.resolve(kind, name, Path::new("game/scenes/act1/cave")),
                    Ok(want.clone()),
                    "{kind} {name} from two folders down"
                );
                assert_eq!(
                    shallow.resolve(kind, name, Path::new("game/scenes")),
                    Ok(want),
                    "{kind} {name} from scenes/"
                );
            },
        );
    }
}

#[test]
fn every_kind_is_found_in_the_mounted_engine_last() {
    for &(kind, name, file) in NAMED {
        let in_engine = format!("game/engine/{file}");
        with_files(
            &["game/flint.toml", &in_engine, "game/scenes/a.scene.toml"],
            || {
                let p = project("game/scenes/a.scene.toml");
                let r = p.explain(kind, name, Path::new("game/scenes"));
                assert_eq!(r.found, Some(PathBuf::from(&in_engine)), "{kind} {name}");
                // The project's own places were tried before the engine's.
                assert!(r.tried[0].starts_with("game/scenes"), "{:?}", r.tried);
                assert!(r.tried.iter().any(|t| t.starts_with("game")
                    && !t.starts_with("game/scenes")
                    && !t.starts_with("game/engine")));
            },
        );
    }
}

#[test]
fn every_kinds_not_found_lists_the_paths_tried() {
    for &(kind, name, _) in NAMED {
        with_files(&["game/flint.toml", "game/scenes/a.scene.toml"], || {
            let p = project("game/scenes/a.scene.toml");
            let err = p.resolve(kind, name, Path::new("game/scenes")).unwrap_err();
            let info = kind.info();
            // A path a folder, a file name and a level of the walk.
            let per_level = info.folders.len() * info.file_names(name).len();
            assert_eq!(err.tried.len(), 2 * per_level, "{kind} {name}");
            assert_eq!(
                err.tried,
                p.candidates(kind, name, Path::new("game/scenes")),
                "{kind} {name}"
            );
            let text = err.to_string();
            assert_eq!(text.lines().count(), 1 + err.tried.len());
            assert!(text.starts_with(&format!("{} '{name}' not found", info.noun)));
        });
    }
}

#[test]
fn the_order_is_the_same_for_every_kind_with_its_folders_and_extensions_inside_a_level() {
    with_files(&["game/flint.toml", "game/scenes/a.scene.toml"], || {
        let p = project("game/scenes/a.scene.toml");
        let dir = Path::new("game/scenes");
        let tried = |kind, name| -> Vec<String> {
            p.candidates(kind, name, dir)
                .iter()
                .map(|t| shown(t))
                .collect()
        };
        assert_eq!(
            tried(AssetKind::Models, "tree"),
            [
                "game/scenes/models/tree.glb",
                "game/scenes/models/tree.gltf",
                "game/models/tree.glb",
                "game/models/tree.gltf",
            ]
        );
        // A folder of "" is the directory of the walk itself.
        assert_eq!(
            tried(AssetKind::Textures, "wall"),
            [
                "game/scenes/wall",
                "game/scenes/wall.png",
                "game/scenes/wall_color.png",
                "game/scenes/textures/wall",
                "game/scenes/textures/wall.png",
                "game/scenes/textures/wall_color.png",
                "game/wall",
                "game/wall.png",
                "game/wall_color.png",
                "game/textures/wall",
                "game/textures/wall.png",
                "game/textures/wall_color.png",
            ]
        );
        // A file name is taken as written, whatever its extension.
        assert_eq!(
            tried(AssetKind::Textures, "sky.hdr"),
            [
                "game/scenes/sky.hdr",
                "game/scenes/textures/sky.hdr",
                "game/sky.hdr",
                "game/textures/sky.hdr",
            ]
        );
        assert_eq!(
            tried(AssetKind::Sprites, "heart.png"),
            [
                "game/scenes/sprites/heart.png",
                "game/scenes/heart.png",
                "game/sprites/heart.png",
                "game/heart.png",
            ]
        );
        assert_eq!(
            tried(AssetKind::Audio, "audio/hit.ogg"),
            [
                "game/scenes/audio/hit.ogg",
                "game/scenes/audio/audio/hit.ogg",
                "game/audio/hit.ogg",
                "game/audio/audio/hit.ogg",
            ]
        );
        // A sound with no extension tries each the engine can decode.
        assert_eq!(
            tried(AssetKind::Audio, "hit")[..5],
            [
                "game/scenes/hit.ogg",
                "game/scenes/hit.wav",
                "game/scenes/hit.mp3",
                "game/scenes/hit.flac",
                "game/scenes/audio/hit.ogg",
            ]
        );
    });
}

#[test]
fn a_folder_of_nothing_finds_a_file_beside_the_scene_and_beside_each_level() {
    with_files(
        &[
            "game/flint.toml",
            "game/scenes/act1/near.png",
            "game/scenes/mid.png",
            "game/far.png",
            "game/scenes/act1/a.scene.toml",
        ],
        || {
            let p = project("game/scenes/act1/a.scene.toml");
            let dir = Path::new("game/scenes/act1");
            for (name, want) in [
                ("near.png", "game/scenes/act1/near.png"),
                ("mid.png", "game/scenes/mid.png"),
                ("far.png", "game/far.png"),
                ("near", "game/scenes/act1/near.png"),
            ] {
                assert_eq!(
                    p.resolve(AssetKind::Textures, name, dir),
                    Ok(PathBuf::from(want))
                );
            }
            // Beside the scene beats the scene's textures/, which beats
            // anything a level up.
            assert_eq!(
                p.resolve(AssetKind::Sprites, "mid.png", dir),
                Ok(PathBuf::from("game/scenes/mid.png"))
            );
        },
    );
}

#[test]
fn a_listing_is_of_named_folders_only() {
    with_files(
        &[
            "game/flint.toml",
            "game/scenes/loose.ogg",
            "game/scenes/audio/near.ogg",
            "game/audio/near.ogg",
            "game/audio/far.wav",
            "game/audio/notes.txt",
            "game/audio/LOUD.OGG",
            "game/scenes/a.scene.toml",
        ],
        || {
            let p = project("game/scenes/a.scene.toml");
            let dir = Path::new("game/scenes");
            let got: Vec<(String, String)> = p
                .list(AssetKind::Audio, dir)
                .into_iter()
                .map(|l| (l.name, shown(&l.path)))
                .collect();
            // By whole file name; the file beside the scene is not in a
            // named folder; an extension in capitals is another extension.
            assert_eq!(
                got,
                [
                    (
                        "near.ogg".to_string(),
                        "game/scenes/audio/near.ogg".to_string()
                    ),
                    ("far.wav".to_string(), "game/audio/far.wav".to_string()),
                ]
            );
            // The loose file is still found by name.
            assert_eq!(
                p.resolve(AssetKind::Audio, "loose.ogg", dir),
                Ok(PathBuf::from("game/scenes/loose.ogg"))
            );
            // Textures and sprites list their folders, not the scene's.
            assert!(p.list(AssetKind::Sprites, dir).is_empty());
        },
    );
}

#[test]
fn a_name_written_as_a_path_is_tried_as_written_first() {
    with_files(
        &[
            "game/flint.toml",
            "game/shared/x.png",
            "game/scenes/local/tree.glb",
            "game/models/tree.glb",
            "game/scenes/a.scene.toml",
        ],
        || {
            let p = project("game/scenes/a.scene.toml");
            let dir = Path::new("game/scenes");
            // `../` from the scene, found at once.
            let r = p.explain(AssetKind::Textures, "../shared/x.png", dir);
            assert_eq!(r.found, Some(PathBuf::from("game/scenes/../shared/x.png")));
            assert!(r.tried.is_empty());
            // `./` with the kind's extensions, ahead of the kind's folder.
            let r = p.explain(AssetKind::Models, "./local/tree", dir);
            assert_eq!(r.found, Some(PathBuf::from("game/scenes/./local/tree.glb")));
            assert!(r.tried.is_empty());
            // A miss lists the as-written paths first, then the walk, and
            // no path twice (a texture's first folder is the scene's own).
            let r = p.explain(AssetKind::Models, "./gone", dir);
            let tried: Vec<String> = r.tried.iter().map(|t| shown(t)).collect();
            assert_eq!(
                tried,
                [
                    "game/scenes/./gone.glb",
                    "game/scenes/./gone.gltf",
                    "game/scenes/models/./gone.glb",
                    "game/scenes/models/./gone.gltf",
                    "game/models/./gone.glb",
                    "game/models/./gone.gltf",
                ]
            );
            let r = p.explain(AssetKind::Textures, "./gone.png", dir);
            let tried: Vec<String> = r.tried.iter().map(|t| shown(t)).collect();
            assert_eq!(
                tried,
                [
                    "game/scenes/./gone.png",
                    "game/scenes/textures/./gone.png",
                    "game/./gone.png",
                    "game/textures/./gone.png",
                ]
            );
            // A plain name gets no such step.
            let r = p.explain(AssetKind::Models, "local/tree", dir);
            assert_eq!(r.found, None);
        },
    );
}

#[test]
fn an_absolute_name_is_that_one_file_and_no_search() {
    with_files(
        &[
            "game/flint.toml",
            "game/scenes/a.scene.toml",
            "/abs/art/wall.png",
            "/abs/art/tree.glb",
        ],
        || {
            let p = project("game/scenes/a.scene.toml");
            let dir = Path::new("game/scenes");
            assert_eq!(
                p.resolve(AssetKind::Textures, "/abs/art/wall.png", dir),
                Ok(PathBuf::from("/abs/art/wall.png"))
            );
            assert_eq!(
                p.resolve(AssetKind::Models, "/abs/art/tree", dir),
                Ok(PathBuf::from("/abs/art/tree.glb"))
            );
            let err = p
                .resolve(AssetKind::Textures, "/abs/art/gone.png", dir)
                .unwrap_err();
            assert_eq!(err.tried, [PathBuf::from("/abs/art/gone.png")]);
            let err = p
                .resolve(AssetKind::Models, "/abs/art/gone", dir)
                .unwrap_err();
            assert_eq!(
                err.tried,
                [
                    PathBuf::from("/abs/art/gone.glb"),
                    PathBuf::from("/abs/art/gone.gltf")
                ]
            );
        },
    );
}

// ── The one-release order-change warning, kind by kind ────────────────────

/// A scene at its project's root, with `file` one level above: outside the
/// project, where every old search still looked.
fn above_the_root(kind: AssetKind, name: &str, file: &str) -> Resolution {
    let outside = format!("work/{file}");
    with_files(
        &["work/game/flint.toml", "work/game/a.scene.toml", &outside],
        || {
            let p = project("work/game/a.scene.toml");
            p.explain(kind, name, Path::new("work/game"))
        },
    )
}

#[test]
fn every_kinds_order_change_fires_when_its_old_search_reached_outside_the_project() {
    for &(kind, name, file) in NAMED {
        // `hit.ogg` was only ever found as a path from the scene, never
        // in `audio/`: the one name here the old searches could not find.
        // And a script was only ever found by its whole file name. A
        // `.font.toml` was only ever read beside the scene, and a look
        // named by a path was a path from the working directory.
        let old_found_it = !matches!(
            (kind, name),
            (AssetKind::Audio, "hit.ogg")
                | (AssetKind::Scripts, "hud")
                | (AssetKind::Fonts, _)
                | (AssetKind::Looks, "out/tuned.look.toml")
        );
        let r = above_the_root(kind, name, file);
        assert_eq!(r.found, None, "{kind} {name}");
        match r.order_change {
            Some(change) => {
                assert!(old_found_it, "{kind} {name}: {change}");
                assert_eq!(change.old, Path::new("work").join(file), "{kind} {name}");
                assert_eq!(change.new, None);
                assert!(change.to_string().contains("outside the project"));
                assert!(change.to_string().contains(kind.info().noun));
            }
            None => assert!(!old_found_it, "{kind} {name}: the old search found it"),
        }
    }
}

#[test]
fn every_kinds_order_change_stays_quiet_in_an_ordinary_project() {
    // scenes/ under the root, the asset in the root's folder: what both the
    // old two-level search and the one rule find.
    for &(kind, name, file) in NAMED {
        let at_root = format!("game/{file}");
        let beside = format!("game/scenes/{file}");
        with_files(
            &["game/flint.toml", &at_root, "game/scenes/a.scene.toml"],
            || {
                let p = project("game/scenes/a.scene.toml");
                let dir = Path::new("game/scenes");
                let r = p.explain(kind, name, dir);
                assert_eq!(r.found, Some(PathBuf::from(&at_root)), "{kind} {name}");
                assert_eq!(r.order_change, None, "{kind} {name}");
                assert!(p.list_order_changes(kind, dir).is_empty(), "{kind} {name}");
            },
        );
        // And with a nearer copy beside the scene hiding it.
        with_files(
            &[
                "game/flint.toml",
                &at_root,
                &beside,
                "game/scenes/a.scene.toml",
            ],
            || {
                let p = project("game/scenes/a.scene.toml");
                let dir = Path::new("game/scenes");
                let r = p.explain(kind, name, dir);
                assert_eq!(r.found, Some(PathBuf::from(&beside)), "{kind} {name}");
                assert_eq!(r.order_change, None, "{kind} {name}");
                assert!(p.list_order_changes(kind, dir).is_empty(), "{kind} {name}");
            },
        );
    }
}

#[test]
fn a_model_two_levels_up_is_new_and_not_a_change_of_order() {
    with_files(
        &[
            "game/flint.toml",
            "game/models/tree.glb",
            "game/scenes/act1/cave/a.scene.toml",
        ],
        || {
            let p = project("game/scenes/act1/cave/a.scene.toml");
            let r = p.explain(
                AssetKind::Models,
                "tree",
                Path::new("game/scenes/act1/cave"),
            );
            assert!(r.found.is_some());
            assert_eq!(r.order_change, None);
        },
    );
}

#[test]
fn the_sprite_order_changed_where_a_file_beside_the_scene_meets_sprites_a_level_up() {
    // Before: scenes/sprites, ../sprites, then beside the scene. Now each
    // level is finished before the next, so the file beside the scene wins.
    with_files(
        &[
            "game/flint.toml",
            "game/sprites/heart.png",
            "game/scenes/heart.png",
            "game/scenes/a.scene.toml",
        ],
        || {
            let p = project("game/scenes/a.scene.toml");
            let r = p.explain(AssetKind::Sprites, "heart.png", Path::new("game/scenes"));
            assert_eq!(r.found, Some(PathBuf::from("game/scenes/heart.png")));
            let change = r.order_change.expect("the old search took sprites/ first");
            assert_eq!(change.old, PathBuf::from("game/sprites/heart.png"));
            assert_eq!(change.new, Some(PathBuf::from("game/scenes/heart.png")));
        },
    );
}

#[test]
fn the_texture_and_audio_orders_changed_only_where_a_named_folder_meets_the_level_above() {
    // A sound named `hit.ogg`: before, beside the scene then beside its
    // parent; now `audio/` beside the scene comes between.
    with_files(
        &[
            "game/flint.toml",
            "game/hit.ogg",
            "game/scenes/audio/hit.ogg",
            "game/scenes/a.scene.toml",
        ],
        || {
            let p = project("game/scenes/a.scene.toml");
            let r = p.explain(AssetKind::Audio, "hit.ogg", Path::new("game/scenes"));
            assert_eq!(r.found, Some(PathBuf::from("game/scenes/audio/hit.ogg")));
            let change = r.order_change.expect("the old search took ../hit.ogg");
            assert_eq!(change.old, PathBuf::from("game/hit.ogg"));
        },
    );
    // Textures had the same four places in the same order: no change even
    // with a copy in every one.
    with_files(
        &[
            "game/flint.toml",
            "game/wall.png",
            "game/textures/wall.png",
            "game/scenes/textures/wall.png",
            "game/scenes/a.scene.toml",
        ],
        || {
            let p = project("game/scenes/a.scene.toml");
            let r = p.explain(AssetKind::Textures, "wall", Path::new("game/scenes"));
            assert_eq!(
                r.found,
                Some(PathBuf::from("game/scenes/textures/wall.png"))
            );
            assert_eq!(r.order_change, None);
        },
    );
}

#[test]
fn a_kind_loaded_as_a_whole_compares_the_two_lists() {
    // The old loaders took every clip from one folder; from a scene at its
    // project's root that folder could be outside the project. Nothing is
    // listed now, and nothing asks by name, so only the lists can tell.
    for (kind, file) in [
        (AssetKind::Animations, "animations/door.anim.toml"),
        (AssetKind::SpriteAnimations, "animations/hero.sprite.toml"),
        (AssetKind::Sequences, "animations/intro.sequence.toml"),
        (AssetKind::Audio, "audio/hit.ogg"),
    ] {
        let outside = format!("work/{file}");
        with_files(
            &["work/game/flint.toml", "work/game/a.scene.toml", &outside],
            || {
                let p = project("work/game/a.scene.toml");
                let dir = Path::new("work/game");
                assert!(p.list(kind, dir).is_empty());
                let changes = p.list_order_changes(kind, dir);
                assert_eq!(changes.len(), 1, "{kind}");
                assert_eq!(changes[0].old, PathBuf::from(&outside));
                assert_eq!(changes[0].new, None);
                assert_eq!(changes[0].kind, kind);
            },
        );
    }
    // The old animation loaders read ONE folder: beside the scene if it
    // existed, and then nothing from the project's. The one rule reads
    // both, which adds files and moves none.
    with_files(
        &[
            "game/flint.toml",
            "game/animations/door.anim.toml",
            "game/animations/gate.anim.toml",
            "game/scenes/animations/door.anim.toml",
            "game/scenes/a.scene.toml",
        ],
        || {
            let p = project("game/scenes/a.scene.toml");
            let dir = Path::new("game/scenes");
            let names: Vec<String> = p
                .list(AssetKind::Animations, dir)
                .into_iter()
                .map(|l| shown(&l.path))
                .collect();
            assert_eq!(
                names,
                [
                    "game/scenes/animations/door.anim.toml",
                    "game/animations/gate.anim.toml"
                ]
            );
            assert!(p.list_order_changes(AssetKind::Animations, dir).is_empty());
        },
    );
    // The old audio preload merged two folders, first name kept: the same
    // files the one rule lists from an ordinary project.
    with_files(
        &[
            "game/flint.toml",
            "game/audio/hit.ogg",
            "game/audio/wind.wav",
            "game/scenes/audio/hit.ogg",
            "game/scenes/a.scene.toml",
        ],
        || {
            let p = project("game/scenes/a.scene.toml");
            let dir = Path::new("game/scenes");
            assert_eq!(p.list(AssetKind::Audio, dir).len(), 2);
            assert!(p.list_order_changes(AssetKind::Audio, dir).is_empty());
        },
    );
}

#[test]
fn file_names_and_listed_names_per_kind() {
    let models = AssetKind::Models.info();
    assert_eq!(models.file_names("tree"), ["tree.glb", "tree.gltf"]);
    assert_eq!(models.file_names("tree.gltf"), ["tree.gltf"]);
    // A stem may have a dot of its own.
    assert_eq!(
        models.file_names("tree.v2"),
        ["tree.v2.glb", "tree.v2.gltf"]
    );
    // Extensions are matched as spelled.
    assert_eq!(
        models.file_names("tree.GLB"),
        ["tree.GLB.glb", "tree.GLB.gltf"]
    );
    assert_eq!(
        models.name_of(Path::new("m/tree.glb")).as_deref(),
        Some("tree")
    );

    let textures = AssetKind::Textures.info();
    assert_eq!(
        textures.file_names("wall"),
        ["wall", "wall.png", "wall_color.png"]
    );
    assert_eq!(textures.file_names("wall.jpg"), ["wall.jpg"]);
    assert_eq!(
        textures.file_names("../t.v2/wall"),
        ["../t.v2/wall", "../t.v2/wall.png", "../t.v2/wall_color.png"]
    );

    let sprites = AssetKind::Sprites.info();
    assert_eq!(sprites.file_names("heart"), ["heart"]);
    assert_eq!(
        sprites.name_of(Path::new("s/heart.png")).as_deref(),
        Some("heart.png")
    );

    let audio = AssetKind::Audio.info();
    assert_eq!(audio.file_names("audio/hit.ogg"), ["audio/hit.ogg"]);
    assert_eq!(
        audio.name_of(Path::new("a/hit.ogg")).as_deref(),
        Some("hit.ogg")
    );
    assert_eq!(audio.name_of(Path::new("a/hit.OGG")), None);
    assert_eq!(audio.name_of(Path::new("a/notes.txt")), None);

    for label in [
        "models",
        "textures",
        "sprites",
        "animations",
        "sprite-animations",
        "sequences",
        "audio",
        "scripts",
        "specs",
        "prefabs",
        "wooze",
        "files",
        "particle-textures",
        "scenes",
    ] {
        assert!(AssetKind::from_label(label).is_some(), "{label}");
    }
}

// ── Scripts ──────────────────────────────────────────────────────────────

#[test]
fn a_scripts_folder_beside_the_scene_no_longer_hides_the_projects() {
    // Before, the first `scripts/` that existed was the only one read: with
    // one beside the scene, `hud.rhai` in the project's was "File not found".
    with_files(
        &[
            "game/flint.toml",
            "game/scripts/hud.rhai",
            "game/scripts/lib/util.rhai",
            "game/scripts/shared.rhai",
            "game/scenes/scripts/local.rhai",
            "game/scenes/scripts/shared.rhai",
            "game/scenes/a.scene.toml",
        ],
        || {
            let p = project("game/scenes/a.scene.toml");
            let dir = Path::new("game/scenes");
            let k = AssetKind::Scripts;
            for (name, file) in [
                ("hud.rhai", "game/scripts/hud.rhai"),
                ("lib/util.rhai", "game/scripts/lib/util.rhai"),
                ("local.rhai", "game/scenes/scripts/local.rhai"),
                // Of two of one name the nearer wins, as it did.
                ("shared.rhai", "game/scenes/scripts/shared.rhai"),
            ] {
                let r = p.explain(k, name, dir);
                assert_eq!(r.found, Some(PathBuf::from(file)), "{name}");
                // Found where nothing was, or the same file as before:
                // neither is a change of order.
                assert_eq!(r.order_change, None, "{name}");
            }
            // The listing goes into subfolders, each file under the name a
            // scene (or an import) uses for it.
            let names: Vec<String> = p.list(k, dir).into_iter().map(|l| l.name).collect();
            assert_eq!(
                names,
                ["local.rhai", "shared.rhai", "hud.rhai", "lib/util.rhai"]
            );
            assert!(p.list_order_changes(k, dir).is_empty());
        },
    );
}

#[test]
fn the_script_order_changed_only_where_the_old_folder_is_outside_the_project() {
    // A scene at the project's root whose scripts sat one level up, outside
    // it: the old search read that folder, the one rule reads the engine's.
    with_files(
        &[
            "work/scripts/hud.rhai",
            "work/game/flint.toml",
            "work/game/engine/scripts/hud.rhai",
            "work/game/a.scene.toml",
        ],
        || {
            let p = project("work/game/a.scene.toml");
            let r = p.explain(AssetKind::Scripts, "hud.rhai", Path::new("work/game"));
            let change = r
                .order_change
                .expect("the old folder is outside the project");
            assert_eq!(change.old, Path::new("work/scripts/hud.rhai"));
            assert_eq!(
                change.new.as_deref(),
                Some(Path::new("work/game/engine/scripts/hud.rhai"))
            );
            let text = change.to_string();
            assert!(text.contains("work/scripts/hud.rhai"), "{text}");
            assert!(text.contains("work/game/engine/scripts/hud.rhai"), "{text}");
        },
    );
}

// ── Procgen specs: found by the name inside ──────────────────────────────

/// Run `f` over a memory source holding `files` with their text.
fn with_texts<R>(files: &[(&str, &str)], f: impl FnOnce() -> R) -> R {
    let mut source = MemorySource::new();
    for (path, text) in files {
        source.insert(path, *text);
    }
    vfs::scoped(Arc::new(source), f)
}

const OAK: &str = "generator = \"mock\"\n[meta]\nname = \"oak_tree\"\n";
const ROCK: &str = "[meta]\nname = \"rock\"\n";
const PEBBLE: &str = "[meta]\nname = \"pebble\"\n";

#[test]
fn a_spec_is_found_by_the_name_inside_it_from_two_folders_down() {
    with_texts(
        &[
            ("game/flint.toml", ""),
            // Its file is called something else, and sits in a subfolder.
            ("game/specs/trees/big_oak.procgen.toml", OAK),
            ("game/models/rock.procgen.toml", ROCK),
            ("game/specs/broken.procgen.toml", "not toml ["),
            ("game/specs/notes.txt", OAK),
            ("game/engine/specs/pebble.procgen.toml", PEBBLE),
            ("game/scenes/act1/cave/a.scene.toml", ""),
        ],
        || {
            let p = project("game/scenes/act1/cave/a.scene.toml");
            let dir = Path::new("game/scenes/act1/cave");
            let k = AssetKind::Specs;
            assert_eq!(
                p.resolve(k, "oak_tree", dir),
                Ok(PathBuf::from("game/specs/trees/big_oak.procgen.toml"))
            );
            assert_eq!(
                p.resolve(k, "rock", dir),
                Ok(PathBuf::from("game/models/rock.procgen.toml"))
            );
            assert_eq!(
                p.resolve(k, "pebble", dir),
                Ok(PathBuf::from("game/engine/specs/pebble.procgen.toml"))
            );
            // Not by file name, and a file that states no name is not one.
            assert!(p.resolve(k, "big_oak", dir).is_err());
            let names: Vec<String> = p.list(k, dir).into_iter().map(|l| l.name).collect();
            assert_eq!(names, ["oak_tree", "rock", "pebble"]);
            assert!(p.list_order_changes(k, dir).is_empty());

            // Not found: every folder read, in order, each on its own line.
            let err = p.resolve(k, "birch", dir).unwrap_err();
            let tried: Vec<String> = err.tried.iter().map(|t| shown(t)).collect();
            assert_eq!(
                tried,
                [
                    "game/scenes/act1/cave/specs",
                    "game/scenes/act1/cave/models",
                    "game/scenes/act1/specs",
                    "game/scenes/act1/models",
                    "game/scenes/specs",
                    "game/scenes/models",
                    "game/specs",
                    "game/models",
                    "game/engine/specs",
                    "game/engine/models",
                ]
            );
            assert!(err
                .to_string()
                .starts_with("procgen spec 'birch' not found. Tried:"));
            // Found: the folders read before the one that had it.
            let r = p.explain(k, "rock", dir);
            assert_eq!(r.tried.len(), 7);
        },
    );
}

#[test]
fn a_spec_beside_the_scene_beats_the_games_under_both_rules() {
    // The old search read the parent's specs/ first and let the scene's
    // overwrite them: the scene's won then, and wins now. No change.
    with_texts(
        &[
            ("game/flint.toml", ""),
            ("game/specs/oak.procgen.toml", OAK),
            ("game/levels/specs/my_oak.procgen.toml", OAK),
            ("game/levels/a.scene.toml", ""),
        ],
        || {
            let p = project("game/levels/a.scene.toml");
            let dir = Path::new("game/levels");
            let r = p.explain(AssetKind::Specs, "oak_tree", dir);
            assert_eq!(
                r.found,
                Some(PathBuf::from("game/levels/specs/my_oak.procgen.toml"))
            );
            assert_eq!(r.order_change, None);
            assert!(p.list_order_changes(AssetKind::Specs, dir).is_empty());
        },
    );
}

#[test]
fn the_spec_order_changed_where_models_met_specs_beside_the_scene() {
    // Beside the scene, the old search read specs/ then models/, the later
    // overwriting: models/ won. The one rule reads a kind's folders in the
    // order it names them, specs/ first.
    with_texts(
        &[
            ("game/flint.toml", ""),
            ("game/levels/specs/oak.procgen.toml", OAK),
            ("game/levels/models/oak.procgen.toml", OAK),
            ("game/levels/a.scene.toml", ""),
        ],
        || {
            let p = project("game/levels/a.scene.toml");
            let dir = Path::new("game/levels");
            let r = p.explain(AssetKind::Specs, "oak_tree", dir);
            let new = PathBuf::from("game/levels/specs/oak.procgen.toml");
            assert_eq!(r.found, Some(new.clone()));
            let change = r.order_change.expect("models/ used to win");
            assert_eq!(change.old, Path::new("game/levels/models/oak.procgen.toml"));
            assert_eq!(change.new, Some(new));
            let text = change.to_string();
            assert!(text.contains("procgen spec 'oak_tree'"), "{text}");
            assert!(
                text.contains("game/levels/models/oak.procgen.toml"),
                "{text}"
            );
            assert!(
                text.contains("game/levels/specs/oak.procgen.toml"),
                "{text}"
            );
            // The loader lists; the list says the same, once.
            let listed = p.list_order_changes(AssetKind::Specs, dir);
            assert_eq!(listed, [change]);
        },
    );
}

#[test]
fn the_spec_order_changed_where_two_files_in_a_folder_state_one_name() {
    // The old index kept the one read later; the one rule keeps the first
    // by file name.
    with_texts(
        &[
            ("game/flint.toml", ""),
            ("game/specs/a_oak.procgen.toml", OAK),
            ("game/specs/b_oak.procgen.toml", OAK),
            ("game/levels/a.scene.toml", ""),
        ],
        || {
            let p = project("game/levels/a.scene.toml");
            let r = p.explain(AssetKind::Specs, "oak_tree", Path::new("game/levels"));
            assert_eq!(
                r.found,
                Some(PathBuf::from("game/specs/a_oak.procgen.toml"))
            );
            let change = r.order_change.expect("the later file used to win");
            assert_eq!(change.old, Path::new("game/specs/b_oak.procgen.toml"));
        },
    );
}

#[test]
fn a_spec_outside_the_project_is_no_longer_read() {
    with_texts(
        &[
            ("work/specs/oak.procgen.toml", OAK),
            ("work/game/flint.toml", ""),
            ("work/game/a.scene.toml", ""),
        ],
        || {
            let p = project("work/game/a.scene.toml");
            let dir = Path::new("work/game");
            let r = p.explain(AssetKind::Specs, "oak_tree", dir);
            assert_eq!(r.found, None);
            let change = r.order_change.expect("the old search read it");
            assert_eq!(change.old, Path::new("work/specs/oak.procgen.toml"));
            assert_eq!(change.new, None);
            assert_eq!(p.list_order_changes(AssetKind::Specs, dir), [change]);
        },
    );
}

// ── Prefabs ──────────────────────────────────────────────────────────────

#[test]
fn a_prefab_three_levels_up_is_new_and_not_a_change_of_order() {
    with_files(
        &[
            "game/flint.toml",
            "game/prefabs/lamp.prefab.toml",
            "game/scenes/act1/cave/a.scene.toml",
        ],
        || {
            let p = project("game/scenes/act1/cave/a.scene.toml");
            let dir = Path::new("game/scenes/act1/cave");
            let r = p.explain(AssetKind::Prefabs, "lamp", dir);
            assert_eq!(
                r.found,
                Some(PathBuf::from("game/prefabs/lamp.prefab.toml"))
            );
            assert_eq!(r.order_change, None);
            assert_eq!(r.tried.len(), 3);
        },
    );
}

// ── Path-named files, particle textures, transition targets ──────────────

#[test]
fn a_file_named_by_a_path_is_found_from_each_level_and_says_where_it_looked() {
    with_files(
        &[
            "game/flint.toml",
            "game/terrain/height.png",
            "game/scenes/act1/terrain/splat.png",
            "game/scenes/act1/cave/a.scene.toml",
        ],
        || {
            let p = project("game/scenes/act1/cave/a.scene.toml");
            let dir = Path::new("game/scenes/act1/cave");
            assert_eq!(
                p.file_or_beside("terrain heightmap", "terrain/height.png", dir),
                Path::new("game/terrain/height.png")
            );
            assert_eq!(
                p.file_or_beside("terrain splat map", "terrain/splat.png", dir),
                Path::new("game/scenes/act1/terrain/splat.png")
            );
            // A miss: the path beside the scene, for the loader's own error;
            // the warning lists what `resolve` tried.
            assert_eq!(
                p.file_or_beside("skybox", "sky.hdr", dir),
                Path::new("game/scenes/act1/cave/sky.hdr")
            );
            let err = p.resolve(AssetKind::Files, "sky.hdr", dir).unwrap_err();
            let tried: Vec<String> = err.tried.iter().map(|t| shown(t)).collect();
            assert_eq!(
                tried,
                [
                    "game/scenes/act1/cave/sky.hdr",
                    "game/scenes/act1/sky.hdr",
                    "game/scenes/sky.hdr",
                    "game/sky.hdr",
                ]
            );
        },
    );
}

#[test]
fn the_particle_texture_order_changed_where_particles_a_level_up_met_a_file_beside_the_scene() {
    // Before: the one particles/ effects came from (here the game's), then
    // beside the scene. Now: particles/ and the directory itself, level by
    // level, so the file beside the scene is nearer.
    with_files(
        &[
            "game/flint.toml",
            "game/particles/spark.png",
            "game/scenes/spark.png",
            "game/scenes/a.scene.toml",
        ],
        || {
            let p = project("game/scenes/a.scene.toml");
            let r = p.explain(
                AssetKind::ParticleTextures,
                "spark.png",
                Path::new("game/scenes"),
            );
            assert_eq!(r.found, Some(PathBuf::from("game/scenes/spark.png")));
            let change = r.order_change.expect("particles/ a level up used to win");
            assert_eq!(change.old, Path::new("game/particles/spark.png"));
        },
    );
    // With only the one, or with particles/ beside the scene: no change.
    with_files(
        &[
            "game/flint.toml",
            "game/particles/spark.png",
            "game/scenes/particles/spark.png",
            "game/scenes/a.scene.toml",
        ],
        || {
            let p = project("game/scenes/a.scene.toml");
            let r = p.explain(
                AssetKind::ParticleTextures,
                "spark.png",
                Path::new("game/scenes"),
            );
            assert_eq!(
                r.found,
                Some(PathBuf::from("game/scenes/particles/spark.png"))
            );
            assert_eq!(r.order_change, None);
        },
    );
}

#[test]
fn a_transition_target_written_from_the_working_directory_is_seen_by_the_old_rule() {
    // The old search tried the target as written first, which is a path
    // from the working directory (here, the top of the file source).
    with_files(
        &[
            "levels/two.scene.toml",
            "game/flint.toml",
            "game/levels/two.scene.toml",
            "game/levels/one.scene.toml",
        ],
        || {
            let p = project("game/levels/one.scene.toml");
            let dir = Path::new("game/levels");
            // Named from the game's root: the walk finds the game's own,
            // the old rule found the other one first.
            let r = p.explain(AssetKind::Scenes, "levels/two.scene.toml", dir);
            assert_eq!(r.found, Some(PathBuf::from("game/levels/two.scene.toml")));
            let change = r.order_change.expect("the working directory came first");
            assert_eq!(change.old, Path::new("levels/two.scene.toml"));
            // Named beside the current scene: both rules agree.
            let r = p.explain(AssetKind::Scenes, "two.scene.toml", dir);
            assert_eq!(r.found, Some(PathBuf::from("game/levels/two.scene.toml")));
            assert_eq!(r.order_change, None);
        },
    );
}

/// #706's done-when, over `AssetKind::ALL` itself, so a kind added later is
/// held to it without anyone remembering: a miss lists the paths tried, and
/// the message prints every one of them.
#[test]
fn every_kind_there_is_says_where_it_looked_when_nothing_is_found() {
    for &kind in AssetKind::ALL {
        with_files(
            &["game/flint.toml", "game/scenes/act1/a.scene.toml"],
            || {
                let dir = Path::new("game/scenes/act1");
                let p = project("game/scenes/act1/a.scene.toml");
                let err = p.resolve(kind, "no_such_thing", dir).unwrap_err();
                assert!(!err.tried.is_empty(), "{kind}: nothing tried");
                let text = err.to_string();
                for tried in &err.tried {
                    assert!(text.contains(&shown(tried)), "{kind}: {text}");
                }
                // From the scene's own folder up to the root
                assert!(err.tried[0].starts_with(dir), "{kind}: {:?}", err.tried);
                assert!(
                    err.tried.iter().any(|t| !t.starts_with("game/scenes")),
                    "{kind}: the root was not tried: {:?}",
                    err.tried
                );
            },
        );
    }
}

/// The table the every-kind tests above walk names every kind there is, so
/// "a scene two folders deep finds the same assets as one at `scenes/`"
/// holds for all of them. (A kind whose files name themselves, a spec, has
/// the same test of its own below.)
#[test]
fn the_every_kind_tests_cover_every_kind() {
    for &kind in AssetKind::ALL {
        assert!(
            NAMED.iter().any(|&(named, _, _)| named == kind) || kind.info().named_inside.is_some(),
            "{kind} has no row in NAMED"
        );
    }
}
