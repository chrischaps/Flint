//! What a scene asks of a feature this player was built without (#754).
//!
//! The music, procgen at run time and living props are Cargo features of
//! flint-player. A build without one links none of it, and a scene that
//! uses it is told so in one line at its entry, rather than playing on with
//! a part silently missing: a suite that doesn't sound, a tree that isn't
//! there, a grove that stands still.
//!
//! The lines are made here, in every build, so they are tested in the
//! default one; each is printed only by a build that lacks its feature
//! (`subsystems/music.rs`, `subsystems/living.rs`, `stage_scene`).

use std::collections::BTreeSet;
use std::path::Path;

use flint_core::components as comp;
use flint_core::project::{AssetKind, Project};
use flint_ecs::FlintWorld;
use flint_terrain::GrassConfig;

/// The line for a scene with a `music_session`, in a build without `music`.
#[cfg_attr(feature = "music", allow(dead_code))]
pub(crate) fn music_line(world: &FlintWorld) -> Option<String> {
    if world
        .entities_with_component(comp::MUSIC_SESSION)
        .is_empty()
    {
        return None;
    }
    Some(
        "[music] this player is built without the `music` feature: the scene's \
         music_session doesn't play, and music_beat() reads -1, as with no music"
            .to_string(),
    )
}

/// What of living props a scene uses, in the order the line names them.
#[cfg_attr(feature = "living", allow(dead_code))]
pub(crate) fn living_uses(
    world: &FlintWorld,
    tables: &toml::Table,
    grass: Option<&GrassConfig>,
) -> Vec<&'static str> {
    let has = |name: &str| !world.entities_with_component(name).is_empty();
    let mut uses = Vec::new();
    if has(comp::LIVING_PROP) {
        uses.push("living props");
    }
    if tables.contains_key("living") {
        uses.push("[living]");
    }
    if tables.contains_key("wooze") {
        uses.push("[wooze]");
    }
    if has(comp::SHALLOW_WOOZE) {
        uses.push("shallow_wooze meshes");
    }
    if grass.is_some_and(|c| c.enabled && c.wooze) {
        uses.push("woozy grass");
    }
    uses
}

/// The line for a scene that uses living props, in a build without `living`.
#[cfg_attr(feature = "living", allow(dead_code))]
pub(crate) fn living_line(
    world: &FlintWorld,
    tables: &toml::Table,
    grass: Option<&GrassConfig>,
) -> Option<String> {
    let uses = living_uses(world, tables, grass);
    (!uses.is_empty()).then(|| {
        format!(
            "[living] this player is built without the `living` feature: the scene's {} \
             stand still",
            uses.join(", ")
        )
    })
}

/// The names in `world` (`model.asset`, a material's or sprite's
/// `texture`) that a spec the scene can see would have made, in name order.
/// A spec is named by its `[meta] name`, as flint-procgen indexes it, and
/// found by the project's one search (`AssetKind::Specs`).
#[cfg_attr(feature = "procgen", allow(dead_code))]
pub(crate) fn names_a_spec_would_make(
    world: &FlintWorld,
    project: &Project,
    scene_path: &str,
) -> Vec<String> {
    let mut asked: BTreeSet<String> = BTreeSet::new();
    for entity in world.all_entities() {
        let Some(comps) = world.get_components(entity.id) else {
            continue;
        };
        let model = comps
            .get(comp::MODEL)
            .and_then(|m| m.get("asset"))
            .and_then(|v| v.as_str());
        let textures = [comp::MATERIAL, comp::SPRITE].into_iter().filter_map(|c| {
            comps
                .get(c)
                .and_then(|t| t.get("texture"))
                .and_then(|v| v.as_str())
        });
        asked.extend(model.into_iter().chain(textures).map(String::from));
    }
    if asked.is_empty() {
        return Vec::new();
    }
    let scene_dir = Path::new(scene_path)
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut made: Vec<String> = project
        .list(AssetKind::Specs, scene_dir)
        .into_iter()
        .filter_map(|listed| {
            let text = flint_core::vfs::read_to_string(&listed.path).ok()?;
            let spec = text.parse::<toml::Table>().ok()?;
            Some(spec.get("meta")?.get("name")?.as_str()?.to_string())
        })
        .filter(|name| asked.contains(name))
        .collect();
    made.sort();
    made.dedup();
    made
}

/// The line for a scene that names assets a spec would make, in a build
/// without `procgen`.
#[cfg_attr(feature = "procgen", allow(dead_code))]
pub(crate) fn procgen_line(
    world: &FlintWorld,
    project: &Project,
    scene_path: &str,
) -> Option<String> {
    let names = names_a_spec_would_make(world, project, scene_path);
    (!names.is_empty()).then(|| {
        let count = match names.len() {
            1 => "1 asset".to_string(),
            n => format!("{n} assets"),
        };
        format!(
            "[procgen] this player is built without the `procgen` feature: {count} named by a \
             spec ({}) load from files, where there are any",
            names.join(", ")
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::vfs::{self, MemorySource};
    use std::sync::Arc;

    fn world_with(components: &[(&str, &str)]) -> FlintWorld {
        let mut world = FlintWorld::new();
        for (i, (component, table)) in components.iter().enumerate() {
            let id = world.spawn(format!("e{i}")).unwrap();
            let value: toml::Value = table.parse::<toml::Table>().unwrap().into();
            world.set_component(id, component, value).unwrap();
        }
        world
    }

    #[test]
    fn a_scene_without_the_parts_hears_nothing() {
        let world = world_with(&[("model", "asset = \"crate\"")]);
        assert_eq!(music_line(&world), None);
        assert_eq!(living_line(&world, &toml::Table::new(), None), None);
    }

    #[test]
    fn a_music_session_is_named() {
        let world = world_with(&[("music_session", "manifest = \"suite.toml\"")]);
        let line = music_line(&world).unwrap();
        assert!(line.starts_with("[music] "), "{line}");
        assert!(line.contains("`music` feature"), "{line}");
    }

    #[test]
    fn living_names_each_part_the_scene_uses() {
        let world = world_with(&[("living_prop", "species = \"oak\""), ("shallow_wooze", "")]);
        let tables: toml::Table = "[wooze]\namplitude = 1.0\n".parse().unwrap();
        let grass = GrassConfig {
            enabled: true,
            wooze: true,
            ..Default::default()
        };
        assert_eq!(
            living_uses(&world, &tables, Some(&grass)),
            [
                "living props",
                "[wooze]",
                "shallow_wooze meshes",
                "woozy grass"
            ]
        );
        let line = living_line(&world, &tables, Some(&grass)).unwrap();
        assert!(
            line.contains("living props, [wooze], shallow_wooze meshes, woozy grass stand still"),
            "{line}"
        );
    }

    #[test]
    fn procgen_names_only_what_a_spec_would_make() {
        let mut files = MemorySource::new();
        files.insert(
            "game/specs/oak.procgen.toml",
            "[meta]\nname = \"oak_tree\"\ngenerator = \"tree_v1\"\n",
        );
        files.insert(
            "game/specs/rock.procgen.toml",
            "[meta]\nname = \"rock\"\ngenerator = \"rock_v1\"\n",
        );
        files.insert("game/flint.toml", "[project]\n");
        vfs::scoped(Arc::new(files), || {
            let project = Project::discover(Path::new("game/demo.scene.toml")).unwrap();
            let world = world_with(&[
                ("model", "asset = \"oak_tree\""),
                ("model", "asset = \"barrel\""),
                ("material", "texture = \"oak_tree\""),
            ]);
            let scene = "game/demo.scene.toml";
            assert_eq!(
                names_a_spec_would_make(&world, &project, scene),
                ["oak_tree"]
            );
            let line = procgen_line(&world, &project, scene).unwrap();
            assert!(
                line.contains("1 asset named by a spec (oak_tree)"),
                "{line}"
            );
            let plain = world_with(&[("model", "asset = \"barrel\"")]);
            assert_eq!(procgen_line(&plain, &project, scene), None);
        });
    }
}
