//! What a scene asks of a package this player was built without (#754).
//!
//! The music, procgen at run time and living props are packages, each a
//! Cargo feature of flint-player. A build without one links none of it,
//! and a scene that uses it is told so in one line at its entry, rather
//! than playing on with a part silently missing: a suite that doesn't
//! sound, a tree that isn't there, a grove that stands still.
//!
//! The line is the kernel's, written from the package's manifest (ADR 0137
//! decision 8, #1585): its `[provides]` components and scene tables,
//! counted in the scene, and its `[without] does` ([`unbuilt`], over
//! `packages::WITHOUT`), so this file names no package. Procgen's line
//! counts what its specs would make (`stage_scene`, [`procgen_line`]).
//! Each is made in every build, so it is tested in the default one, and
//! printed only by a run that lacks its package.

use std::collections::BTreeSet;
use std::path::Path;

use flint_core::package::InUse;
use flint_core::project::{AssetKind, Project};
use flint_ecs::FlintWorld;
use flint_package::{SceneCtx, Subsystem};

use crate::packages::Without;

mod comp {
    pub(super) use flint_core::components::*;
}

/// What the kernel says the scene's parts of a package do in a build
/// without it, when its manifest's `[without]` doesn't say.
pub(crate) const DOES: &str = "do nothing in this build";

/// The kernel's builder for each package the run lacks (one it has a part
/// of, which the launcher didn't register): a scene that uses one hears,
/// once, what it uses of it and what that does here. With the package
/// registered it says nothing; the package's own builder runs the scene.
pub(crate) fn unbuilt(ctx: &mut SceneCtx) -> Option<Box<dyn Subsystem>> {
    for without in crate::packages::WITHOUT {
        if ctx.registry.has(without.package) {
            continue;
        }
        let left_out = ctx.leaves_out(without.package);
        if let Some(line) = unbuilt_line(without, ctx.world, ctx.tables, left_out) {
            tracing::warn!(target: "load", "{line}");
        }
    }
    None
}

/// What brings a package the run lacks: a `flint build` of the project,
/// whose list is then every package (no `[packages]`) or names it. A list
/// that leaves it out is told the line to add first, as the load's
/// `unlisted-package` says.
pub(crate) fn build_hint(package: &str, left_out: Option<&InUse>) -> String {
    match left_out {
        Some(in_use) => format!(
            "{}, then run `flint build` for a player with it",
            in_use.hint_for(package)
        ),
        None => "run `flint build` in the project for a player with it".to_string(),
    }
}

/// What of `without`'s package a scene uses, in the order the line names
/// them: each of its components some entity carries, then each of its
/// scene tables the scene writes (`[wooze]`).
pub(crate) fn uses(without: &Without, world: &FlintWorld, tables: &toml::Table) -> Vec<String> {
    let components = without
        .components
        .iter()
        .filter(|c| !world.entities_with_component(c).is_empty())
        .map(|c| c.to_string());
    let scene_tables = without
        .scene_tables
        .iter()
        .filter(|t| tables.contains_key(**t))
        .map(|t| format!("[{t}]"));
    components.chain(scene_tables).collect()
}

/// The line for a scene that uses `without`'s package, in a run without it:
/// `[living] this player is built without the `living` package: the
/// scene's living_prop, [wooze] stand still; run `flint build` in the
/// project for a player with it`. It is the one thing said of those names:
/// the load's summary leaves their `unknown-component` and `unknown-table`
/// to it (`packages::mark_built_without`, #1601). `left_out` is the list
/// when the project's `[packages]` leaves the package out.
pub(crate) fn unbuilt_line(
    without: &Without,
    world: &FlintWorld,
    tables: &toml::Table,
    left_out: Option<&InUse>,
) -> Option<String> {
    let uses = uses(without, world, tables);
    (!uses.is_empty()).then(|| {
        let package = without.package;
        format!(
            "[{package}] this player is built without the `{package}` package: the scene's {} \
             {}; {}",
            uses.join(", "),
            without.does.unwrap_or(DOES),
            build_hint(package, left_out)
        )
    })
}

/// The names in `world` (`model.asset`, a material's or sprite's
/// `texture`) that a spec the scene can see would have made, in name order.
/// A spec is named by its `[meta] name`, as flint-procgen indexes it, and
/// found by the project's one search (`AssetKind::Specs`).
pub(crate) fn names_a_spec_would_make(
    world: &FlintWorld,
    project: &Project,
    scene_path: &str,
) -> Vec<String> {
    names_made_from(world, project, scene_path, |_| true)
}

/// [`names_a_spec_would_make`], of the spec files `made_from` takes (a
/// package's `[provides] specs`, #1578): the names one package's specs
/// would have made.
pub(crate) fn names_made_from(
    world: &FlintWorld,
    project: &Project,
    scene_path: &str,
    made_from: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut asked: BTreeSet<String> = BTreeSet::new();
    for entity in world.all_entities() {
        // Through the typed view (ADR 0115): a wrong type is reported once
        let model = world.view(entity.id, comp::MODEL).opt_str("asset");
        let textures = [comp::MATERIAL, comp::SPRITE]
            .into_iter()
            .filter_map(|c| world.view(entity.id, c).opt_str("texture"));
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
        .filter(|listed| made_from(&listed.path.to_string_lossy()))
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
pub(crate) fn procgen_line(
    world: &FlintWorld,
    project: &Project,
    scene_path: &str,
) -> Option<String> {
    let names = names_a_spec_would_make(world, project, scene_path);
    (!names.is_empty()).then(|| {
        format!(
            "[procgen] this player is built without the `procgen` feature: {} {UNMADE}",
            made_by_a_spec(&names)
        )
    })
}

/// What a name a spec would have made does when nothing makes it.
const UNMADE: &str = "load from files, where there are any";

/// `2 assets named by a spec (oak, rock)`.
fn made_by_a_spec(names: &[String]) -> String {
    let count = match names.len() {
        1 => "1 asset".to_string(),
        n => format!("{n} assets"),
    };
    format!("{count} named by a spec ({})", names.join(", "))
}

/// The line for a scene that names assets a spec of `package` would make,
/// when the project's `[packages]` leaves `package` out and so its asset
/// maker makes nothing (#1578): `[procgen] [packages] in flint.toml leaves
/// out `procgen`: the scene's 1 asset named by a spec (rock) load from
/// files, where there are any (remove `procgen = false` ...)`. The spec
/// files are those the package's manifest says it makes from
/// (`[provides] specs`); `None` when the scene names none of them.
pub(crate) fn left_out_maker_line(
    in_use: &InUse,
    package: &str,
    world: &FlintWorld,
    project: &Project,
    scene_path: &str,
) -> Option<String> {
    let manifest = in_use.left_out.iter().find(|p| p.name == package)?;
    let names = names_made_from(world, project, scene_path, |file| {
        manifest.provides.makes_from(file)
    });
    if names.is_empty() {
        return None;
    }
    let uses = made_by_a_spec(&names);
    flint_package::left_out_line(in_use, package, &[uses.as_str()], UNMADE)
}

#[cfg(test)]
#[path = "left_out_tests.rs"]
mod left_out_tests;

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

    /// A package with components and scene tables, as a manifest's
    /// `[provides]` and `[without]` would give it.
    const GROVE: Without = Without {
        package: "grove",
        components: &["grove_tree", "grove_moss"],
        scene_tables: &["grove", "breeze"],
        does: Some("stand still"),
        policies: &["grove"],
    };

    #[test]
    fn a_scene_without_the_parts_hears_nothing() {
        let world = world_with(&[("model", "asset = \"crate\"")]);
        assert_eq!(
            unbuilt_line(&GROVE, &world, &toml::Table::new(), None),
            None
        );
        for without in crate::packages::WITHOUT {
            assert_eq!(
                unbuilt_line(without, &world, &toml::Table::new(), None),
                None
            );
        }
    }

    /// Each component the scene's entities carry, then each table it
    /// writes, in the manifest's order, then what they do.
    #[test]
    fn the_line_names_each_part_the_scene_uses() {
        let world = world_with(&[("grove_moss", ""), ("grove_tree", "species = \"oak\"")]);
        let tables: toml::Table = "[breeze]\nstrength = 1.0\n".parse().unwrap();
        assert_eq!(
            uses(&GROVE, &world, &tables),
            ["grove_tree", "grove_moss", "[breeze]"]
        );
        assert_eq!(
            unbuilt_line(&GROVE, &world, &tables, None).unwrap(),
            "[grove] this player is built without the `grove` package: the scene's grove_tree, \
             grove_moss, [breeze] stand still; run `flint build` in the project for a player \
             with it"
        );
        let unsaid = Without {
            does: None,
            ..GROVE
        };
        assert!(unbuilt_line(&unsaid, &world, &tables, None)
            .unwrap()
            .contains(&format!("[breeze] {DOES}; ")));
    }

    /// A list that leaves the package out is told the line to add before
    /// `flint build`, which alone would build without it again.
    #[test]
    fn a_list_that_leaves_it_out_hears_the_line_to_add_first() {
        let in_use = InUse {
            manifest: "game/flint.toml".into(),
            on: Default::default(),
            left_out: Vec::new(),
            off: Default::default(),
        };
        let world = world_with(&[("grove_tree", "")]);
        let line = unbuilt_line(&GROVE, &world, &toml::Table::new(), Some(&in_use)).unwrap();
        assert!(
            line.ends_with(
                "stand still; add `grove = \"*\"` under [packages] in game/flint.toml, then run \
                 `flint build` for a player with it"
            ),
            "{line}"
        );
    }

    /// The table is the manifests': every package this crate has a part
    /// of, each with its `[provides]` and `[without]`, read again here
    /// through flint-core's parser.
    #[test]
    fn the_table_is_the_manifests() {
        let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages");
        let manifests = flint_core::package::Packages::read(&folder);
        let names: Vec<&str> = crate::packages::WITHOUT.iter().map(|w| w.package).collect();
        assert_eq!(names, crate::packages::PLAYER_PACKAGES);
        for without in crate::packages::WITHOUT {
            let m = manifests.get(without.package).unwrap();
            assert_eq!(without.components, m.provides.components);
            assert_eq!(without.scene_tables, m.provides.scene_tables);
            assert_eq!(without.does, m.without.does.as_deref());
            assert_eq!(without.policies, m.without.policies);
        }
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
