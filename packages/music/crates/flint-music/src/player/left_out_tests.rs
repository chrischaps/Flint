//! The line a scene with a `music_session` hears when the project's
//! `[packages]` leaves `music` out (#1528), from this tree's manifests: a
//! game in a `MemorySource` with the engine at `engine/`, as a game
//! carries it, so the list resolves as it does on disk.

use flint_core::vfs::{self, MemorySource};
use std::sync::Arc;

/// Every package of this tree, by name, with its manifest.
const MANIFESTS: &[(&str, &str)] = &[
    ("ai", include_str!("../../../../../ai/package.toml")),
    (
        "debug-hud",
        include_str!("../../../../../debug-hud/package.toml"),
    ),
    ("engine", include_str!("../../../../../engine/package.toml")),
    ("living", include_str!("../../../../../living/package.toml")),
    ("music", include_str!("../../../../package.toml")),
    (
        "procgen",
        include_str!("../../../../../procgen/package.toml"),
    ),
    ("rhythm", include_str!("../../../../../rhythm/package.toml")),
];

const SCENE: &str = "game/scenes/band.scene.toml";

/// A game whose `[packages]` lists nothing.
fn game() -> Arc<MemorySource> {
    let mut files = MemorySource::new();
    files.insert("game/engine/crates/flint-core/Cargo.toml", "");
    files.insert(
        "game/engine/schemas/components/transform.toml",
        include_str!("../../../../../../schemas/components/transform.toml"),
    );
    for (name, manifest) in MANIFESTS {
        files.insert(
            format!("game/engine/packages/{name}/package.toml"),
            *manifest,
        );
    }
    files.insert(
        "game/flint.toml",
        "[project]\nschemas = [\"engine/schemas\"]\n\n[packages]\n",
    );
    files.insert(SCENE, "[scene]\nname = \"band\"\n");
    Arc::new(files)
}

#[test]
fn a_left_out_music_session_is_named_with_the_line_to_add() {
    vfs::scoped(game(), || {
        let flags: &[&str] = &[];
        let dirs = flint_core::project::schema_dirs(Some(SCENE.as_ref()), flags).unwrap();
        let in_use = dirs.packages.expect("a [packages] list");
        let mut world = flint_ecs::FlintWorld::new();
        let id = world.spawn("band").unwrap();
        world
            .set_component(id, "music_session", toml::Value::Table(Default::default()))
            .unwrap();
        let line = super::left_out_line(&in_use, &world).unwrap();
        assert_eq!(
            line,
            "[music] [packages] in flint.toml leaves out `music`: the scene's music_session \
             doesn't play, and music_beat() reads -1 (add `music = \"*\"` under [packages] in \
             game/flint.toml)"
        );
        let quiet = flint_ecs::FlintWorld::new();
        assert_eq!(super::left_out_line(&in_use, &quiet), None);
    });
}
