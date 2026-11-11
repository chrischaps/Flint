//! The look and style kinds on the one resolver (palettes, glyph sets,
//! bitmap fonts, font files, looks, paper and stroke images, user styles):
//! what the old searches did that the one rule doesn't, which is mostly the
//! working directory, and that the one-release warning says so.

use super::*;
use crate::vfs::MemorySource;
use std::sync::Arc;

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

/// A name of each kind the old searches also looked for in the working
/// directory, and the file there it stood for. In these tests the working
/// directory is the top of the file source.
const IN_THE_WORKING_DIRECTORY: &[(AssetKind, &str, &str)] = &[
    (
        AssetKind::Palettes,
        "sunset",
        "palettes/sunset.palette.toml",
    ),
    (
        AssetKind::Palettes,
        "my/sunset.palette.toml",
        "my/sunset.palette.toml",
    ),
    (
        AssetKind::GlyphSets,
        "terminal",
        "fonts/terminal.glyphs.toml",
    ),
    (AssetKind::Looks, "noir", "styles/noir.look.toml"),
    (
        AssetKind::Looks,
        "out/tuned.look.toml",
        "out/tuned.look.toml",
    ),
    (AssetKind::Papers, "cotton", "papers/cotton.png"),
    (AssetKind::Papers, "scans/linen.jpg", "scans/linen.jpg"),
    (AssetKind::Strokes, "hb", "strokes/hb.png"),
];

#[test]
fn a_file_in_the_working_directory_is_no_longer_found_and_the_warning_names_it() {
    for &(kind, name, file) in IN_THE_WORKING_DIRECTORY {
        // The game is a folder below the working directory; the file is in
        // the working directory, outside the game.
        with_files(
            &["game/flint.toml", "game/levels/a.scene.toml", file],
            || {
                let p = project("game/levels/a.scene.toml");
                let r = p.explain(kind, name, Path::new("game/levels"));
                assert_eq!(r.found, None, "{kind} {name}");
                assert!(
                    !r.tried.iter().any(|t| t == Path::new(file)),
                    "{kind} {name}: the working directory is not searched: {:?}",
                    r.tried
                );
                let change = r.order_change.expect("the old search found it");
                assert_eq!(change.old, Path::new(file), "{kind} {name}");
                assert_eq!(change.new, None);
                assert!(change.to_string().contains("outside the project"));
            },
        );
        // The same with the game's own copy: the one rule finds that, the
        // old search found it too (the scene's folders came first), quiet.
        let own = format!("game/{file}");
        with_files(
            &["game/flint.toml", "game/levels/a.scene.toml", file, &own],
            || {
                let p = project("game/levels/a.scene.toml");
                let r = p.explain(kind, name, Path::new("game/levels"));
                if kind == AssetKind::Looks && name.ends_with(".toml") {
                    // A look named by a path was only ever read from the
                    // working directory: here the pick changes, and the
                    // warning names both files.
                    let change = r.order_change.expect("the pick changed");
                    assert_eq!(change.old, Path::new(file));
                    assert_eq!(change.new, Some(PathBuf::from(&own)));
                } else {
                    assert_eq!(r.order_change, None, "{kind} {name}");
                }
                assert_eq!(r.found, Some(PathBuf::from(&own)), "{kind} {name}");
            },
        );
    }
}

#[test]
fn run_from_the_project_root_the_working_directory_is_the_root_and_nothing_changes() {
    // What a game does: `flint render levels/a.scene.toml` from its root.
    // The old search's `.` and the walk's last step are one folder.
    for &(kind, name, file) in IN_THE_WORKING_DIRECTORY {
        with_files(&["flint.toml", "levels/deep/a.scene.toml", file], || {
            let p = project("levels/deep/a.scene.toml");
            let r = p.explain(kind, name, Path::new("levels/deep"));
            assert_eq!(r.found, Some(PathBuf::from(file)), "{kind} {name}");
            assert_eq!(r.order_change, None, "{kind} {name}");
        });
    }
}

#[test]
fn a_bitmap_fonts_old_search_is_the_first_step_of_the_new_one() {
    // `ui_text.font = "fonts/ui.font.toml"` was read from the scene's
    // directory alone, which is where the one rule looks first: its order
    // cannot change, it can only find more.
    with_files(
        &[
            "game/flint.toml",
            "game/fonts/ui.font.toml",
            "game/levels/fonts/ui.font.toml",
            "game/levels/a.scene.toml",
            "game/levels/cave/b.scene.toml",
        ],
        || {
            let p = project("game/levels/a.scene.toml");
            let k = AssetKind::Fonts;
            let r = p.explain(k, "fonts/ui.font.toml", Path::new("game/levels"));
            assert_eq!(
                r.found,
                Some(PathBuf::from("game/levels/fonts/ui.font.toml"))
            );
            assert_eq!(r.tried, Vec::<PathBuf>::new());
            assert_eq!(r.order_change, None);
            // From a folder down the old search found none; now the
            // nearest above is found.
            let r = p.explain(k, "fonts/ui.font.toml", Path::new("game/levels/cave"));
            assert_eq!(
                r.found,
                Some(PathBuf::from("game/levels/fonts/ui.font.toml"))
            );
            assert_eq!(r.order_change, None);
        },
    );
}

#[test]
fn font_files_are_listed_from_every_fonts_folder_where_one_folder_was_read() {
    with_files(
        &[
            "game/flint.toml",
            "game/fonts/Body.ttf",
            "game/fonts/Display.otf",
            "game/fonts/fonts.toml",
            "game/fonts/notes.txt",
            "game/levels/fonts/Display.otf",
            "game/levels/a.scene.toml",
        ],
        || {
            let p = project("game/levels/a.scene.toml");
            let dir = Path::new("game/levels");
            let listed: Vec<(String, String)> = p
                .list(AssetKind::FontFaces, dir)
                .into_iter()
                .map(|l| (l.name, shown(&l.path)))
                .collect();
            // The scene's Display hides the game's; the game's Body, which
            // the old one-folder search never reached, is listed too.
            assert_eq!(
                listed,
                [
                    (
                        "Display.otf".to_string(),
                        "game/levels/fonts/Display.otf".to_string()
                    ),
                    ("Body.ttf".to_string(), "game/fonts/Body.ttf".to_string()),
                ]
            );
            // Nothing the old search loaded is lost or swapped: quiet.
            assert!(p.list_order_changes(AssetKind::FontFaces, dir).is_empty());
        },
    );
    // A `fonts/` above the project's root was read when the scene sat at
    // the root; the one rule doesn't reach it.
    with_files(
        &[
            "work/fonts/Body.ttf",
            "work/game/flint.toml",
            "work/game/a.scene.toml",
        ],
        || {
            let p = project("work/game/a.scene.toml");
            let changes = p.list_order_changes(AssetKind::FontFaces, Path::new("work/game"));
            assert_eq!(changes.len(), 1);
            assert_eq!(changes[0].old, Path::new("work/fonts/Body.ttf"));
            assert_eq!(changes[0].new, None);
        },
    );
}

#[test]
fn the_nearest_styles_folder_wins_a_user_style_of_one_name() {
    // #426: two folders with a `glow.shader.toml` each. The rule's answer
    // is the nearest to the scene, which is also what one scene's load did
    // before (its own `styles/` was read first); within a folder a style is
    // its file name, so there is one of a name.
    with_files(
        &[
            "game/flint.toml",
            "game/styles/glow.shader.toml",
            "game/styles/haze.shader.toml",
            "game/styles/noir.look.toml",
            "game/levels/styles/glow.shader.toml",
            "game/levels/a.scene.toml",
            "game/levels/act2/styles/glow.shader.toml",
            "game/levels/act2/cave/b.scene.toml",
            "game/engine/styles/glow.shader.toml",
            "game/engine/styles/stock.shader.toml",
        ],
        || {
            let p = project("game/levels/a.scene.toml");
            let names = |dir: &str| -> Vec<(String, String)> {
                p.list(AssetKind::Styles, Path::new(dir))
                    .into_iter()
                    .map(|l| (l.name, shown(&l.path)))
                    .collect()
            };
            let pair = |n: &str, f: &str| (n.to_string(), f.to_string());
            assert_eq!(
                names("game/levels"),
                [
                    pair("glow", "game/levels/styles/glow.shader.toml"),
                    pair("haze", "game/styles/haze.shader.toml"),
                    pair("stock", "game/engine/styles/stock.shader.toml"),
                ]
            );
            // Two folders down, past a folder with no `styles/`.
            assert_eq!(
                names("game/levels/act2/cave"),
                [
                    pair("glow", "game/levels/act2/styles/glow.shader.toml"),
                    pair("haze", "game/styles/haze.shader.toml"),
                    pair("stock", "game/engine/styles/stock.shader.toml"),
                ]
            );
            // The look in `styles/` is not a user style, and the styles are
            // not looks.
            let looks = p.list(AssetKind::Looks, Path::new("game/levels"));
            assert_eq!(looks.len(), 1);
            assert_eq!(looks[0].name, "noir");
            // For the scene in levels/ both searches load the same files.
            assert!(p
                .list_order_changes(AssetKind::Styles, Path::new("game/levels"))
                .is_empty());
        },
    );
}

#[test]
fn a_user_style_above_the_root_was_loaded_and_is_not_now() {
    with_files(
        &[
            "work/styles/glow.shader.toml",
            "work/game/flint.toml",
            "work/game/engine/styles/glow.shader.toml",
            "work/game/a.scene.toml",
        ],
        || {
            let p = project("work/game/a.scene.toml");
            let changes = p.list_order_changes(AssetKind::Styles, Path::new("work/game"));
            assert_eq!(changes.len(), 1, "{changes:?}");
            assert_eq!(changes[0].old, Path::new("work/styles/glow.shader.toml"));
            assert_eq!(
                changes[0].new,
                Some(PathBuf::from("work/game/engine/styles/glow.shader.toml"))
            );
        },
    );
}

#[test]
fn listed_names_are_the_names_a_scene_writes() {
    with_files(
        &[
            "game/flint.toml",
            "game/palettes/sunset.palette.toml",
            "game/palettes/readme.md",
            "game/fonts/terminal.glyphs.toml",
            "game/fonts/terminal.ttf",
            "game/fonts/ui.font.toml",
            "game/papers/cotton.png",
            "game/papers/linen.jpg",
            "game/strokes/hb.png",
            "game/loose.palette.toml",
            "game/levels/a.scene.toml",
        ],
        || {
            let p = project("game/levels/a.scene.toml");
            let dir = Path::new("game/levels");
            let names =
                |kind| -> Vec<String> { p.list(kind, dir).into_iter().map(|l| l.name).collect() };
            // A palette loose in a directory of the walk is found by name
            // but, like every file outside a named folder, not listed.
            assert_eq!(names(AssetKind::Palettes), ["sunset"]);
            assert_eq!(
                p.resolve(AssetKind::Palettes, "loose", dir),
                Ok(PathBuf::from("game/loose.palette.toml"))
            );
            assert_eq!(names(AssetKind::GlyphSets), ["terminal"]);
            assert_eq!(names(AssetKind::Fonts), ["ui"]);
            assert_eq!(names(AssetKind::FontFaces), ["terminal.ttf"]);
            assert_eq!(names(AssetKind::Papers), ["cotton", "linen"]);
            assert_eq!(names(AssetKind::Strokes), ["hb"]);
            // Each listed name resolves to the listed file.
            for kind in [
                AssetKind::Palettes,
                AssetKind::GlyphSets,
                AssetKind::Fonts,
                AssetKind::FontFaces,
                AssetKind::Papers,
                AssetKind::Strokes,
            ] {
                for l in p.list(kind, dir) {
                    assert_eq!(p.resolve(kind, &l.name, dir), Ok(l.path), "{kind}");
                }
            }
        },
    );
}

#[test]
fn a_scope_is_a_project_and_a_directory() {
    with_files(
        &[
            "game/flint.toml",
            "game/palettes/sunset.palette.toml",
            "game/scenes/act1/cave/a.scene.toml",
        ],
        || {
            let scope = AssetScope::new(
                project("game/scenes/act1/cave/a.scene.toml"),
                "game/scenes/act1/cave",
            );
            let r = scope.explain(AssetKind::Palettes, "sunset");
            assert_eq!(
                r.found,
                Some(PathBuf::from("game/palettes/sunset.palette.toml"))
            );
            // What hot reload watches: each nearer place, then the file.
            let watched: Vec<String> = r.watched().map(|p| shown(p)).collect();
            assert_eq!(watched.len(), r.tried.len() + 1);
            assert_eq!(
                watched[0],
                "game/scenes/act1/cave/palettes/sunset.palette.toml"
            );
            assert_eq!(watched.last().unwrap(), "game/palettes/sunset.palette.toml");
            assert_eq!(scope.resolve(AssetKind::Palettes, "sunset").ok(), r.found);
            assert_eq!(scope.list(AssetKind::Palettes).len(), 1);
        },
    );
}
