//! Scene loading from TOML files

use crate::diagnose::{diagnose_scene, diagnose_scene_in};
use crate::format::{PostProcessDef, SceneFile};
use crate::prefab;
use flint_core::diagnostic::{self, Diagnostic};
use flint_core::project::Project;
use flint_core::vfs;
use flint_core::{FlintError, Result};
use flint_ecs::FlintWorld;
use flint_schema::SchemaRegistry;
use std::path::Path;

/// Load a scene from a TOML file (with prefab expansion).
///
/// What is wrong with its entities (an unknown archetype or component, a
/// field of the wrong type, a reference to an entity or an asset that isn't
/// there: [`diagnose_scene_in`]) doesn't stop the load: it is logged once, as
/// a summary line and the first few (ADR 0115).
/// [`load_scene_diagnosed`] returns the list instead.
///
/// The project's `[project.diagnostics] allow` is applied here as in
/// `flint validate` (#795), and `[project] strict` is not: strict is a gate
/// for the commands that pass or fail a scene, and a load never fails on
/// what it promotes, so the summary keeps each code at the severity it is
/// raised with.
pub fn load_scene<P: AsRef<Path>>(
    path: P,
    registry: &SchemaRegistry,
) -> Result<(FlintWorld, SceneFile)> {
    let path = path.as_ref();
    let (world, scene_file, found) = load_scene_diagnosed(path, registry)?;
    diagnostic::log_summary(&flint_core::project::shown(path), &found);
    Ok((world, scene_file))
}

/// What a host says when the scene at `path` didn't load (#1044):
/// `Failed to load scene levels/hall.scene.toml`, the error's cause under
/// it. A load's own errors mostly name entities, not the file (a parent
/// cycle, a missing prefab), so with a folder of scenes or a sweep of
/// variants this line is the one that says which file to open. Every
/// command, the player and the editor word it the same way.
pub fn load_failed<P: AsRef<Path>>(path: P) -> String {
    format!(
        "Failed to load scene {}",
        flint_core::project::shown(path.as_ref())
    )
}

/// [`load_scene`] into a world whose ids start past every id `previous`
/// gave out ([`FlintWorld::after`]): what a host that goes on loads its
/// next scene with (a transition, an editor's reload), so no id held across
/// the change names an entity of the new scene (#688). A plain
/// [`load_scene`] counts from 1, which is what makes a scene's ids the same
/// every load.
pub fn load_scene_after<P: AsRef<Path>>(
    path: P,
    registry: &SchemaRegistry,
    previous: &FlintWorld,
) -> Result<(FlintWorld, SceneFile)> {
    let path = path.as_ref();
    let content = vfs::read_to_string(path)?;
    let (world, scene_file, found) =
        load_text_into(&content, path, registry, FlintWorld::after(previous))?;
    diagnostic::log_summary(&flint_core::project::shown(path), &found);
    Ok((world, scene_file))
}

/// [`load_scene`] that hands back what [`diagnose_scene_in`] found and logs
/// nothing about it, for a host that reports diagnostics its own way
/// (`flint validate`).
pub fn load_scene_diagnosed<P: AsRef<Path>>(
    path: P,
    registry: &SchemaRegistry,
) -> Result<(FlintWorld, SceneFile, Vec<Diagnostic>)> {
    let path = path.as_ref();
    let content = vfs::read_to_string(path)?;
    load_text_diagnosed(&content, path, registry)
}

/// Load scene text as though it were the file at `path`: prefabs resolve
/// from there. What `flint entity create/delete` (#678) runs on
/// the text it is about to write, so an edit that would leave the scene
/// unloadable is refused before the file changes. Diagnostics are logged as
/// [`load_scene`] logs them.
pub fn load_scene_text(
    content: &str,
    path: &Path,
    registry: &SchemaRegistry,
) -> Result<(FlintWorld, SceneFile)> {
    let (world, scene_file, found) = load_text_diagnosed(content, path, registry)?;
    diagnostic::log_summary(&flint_core::project::shown(path), &found);
    Ok((world, scene_file))
}

/// [`load_scene_text`] that hands back what it found and logs nothing: for
/// text that is a world saved after its load (a frame capture's
/// `replay/world.toml`, #1089), whose model parts name sub-meshes the
/// renderer registered rather than files, so every part would be reported
/// missing.
pub fn load_scene_text_diagnosed(
    content: &str,
    path: &Path,
    registry: &SchemaRegistry,
) -> Result<(FlintWorld, SceneFile, Vec<Diagnostic>)> {
    load_text_diagnosed(content, path, registry)
}

/// What [`load_scene_diagnosed`] and [`load_scene_text`] share: parse, expand
/// prefabs, diagnose, build the world.
///
/// The scene-level tables are handed back as the file wrote them. A feature
/// that settles its own after a load does so itself: living props fill in the
/// preset `[wooze] preset` names with `flint_procgen::settle_scene`, which
/// this loader did while those tables were its own (#686).
fn load_text_diagnosed(
    content: &str,
    path: &Path,
    registry: &SchemaRegistry,
) -> Result<(FlintWorld, SceneFile, Vec<Diagnostic>)> {
    load_text_into(content, path, registry, FlintWorld::new())
}

/// [`load_text_diagnosed`] into `world`, an empty world that decides where
/// the ids start.
fn load_text_into(
    content: &str,
    path: &Path,
    registry: &SchemaRegistry,
    mut world: FlintWorld,
) -> Result<(FlintWorld, SceneFile, Vec<Diagnostic>)> {
    let mut scene_file = parse_scene(content, Some(path))?;
    note_post_process(content, Some(path), registry);

    // Expand prefab instances into entities before building world
    prefab::expand_prefabs(&mut scene_file, path)?;
    let found = load_diagnostics(&scene_file, content, path, registry);
    populate_world(&mut world, &scene_file, registry)?;
    Ok((world, scene_file, found))
}

/// What a load says of a scene it parsed from `content`:
/// [`diagnose_scene_in`], less what the project allows, a scene table's
/// mistakes placed by line and column (#1061). Not promoted by `[project]
/// strict` (see [`load_scene`]).
fn load_diagnostics(
    scene: &SceneFile,
    content: &str,
    path: &Path,
    registry: &SchemaRegistry,
) -> Vec<Diagnostic> {
    let project = Project::for_scene(path);
    let mut found = diagnose_scene_in(scene, path, registry, &project);
    diagnostic::drop_allowed(&mut found, &project.allow);
    leave_user_style_components(&mut found, &project, path);
    crate::place_table_diagnostics(content, &mut found);
    found
}

/// Drop the `unknown-component` of each `<name>_style` that a user style
/// the scene can see owns (`styles/<name>.shader.toml`, #414). Such a
/// component has no schema file. Its schema is made from the style's
/// `.shader.toml` when the style loads, and that happens after the scene
/// loads, so the loader can't judge it. The host that draws the scene
/// judges it once its styles are loaded (`bind_scene`'s
/// `user_style_component_warnings`, #556), field by field. Saying it here
/// too called the style's own component unknown, as an error, and asked for
/// a schema file the book never asks for (#1320). `flint check` and
/// `flint validate` don't come through here: they load the styles first and
/// check against the schemas the styles make.
fn leave_user_style_components(found: &mut Vec<Diagnostic>, project: &Project, path: &Path) {
    let style_of = |d: &Diagnostic| {
        (d.code == diagnostic::codes::UNKNOWN_COMPONENT)
            .then_some(d.component.as_deref())
            .flatten()
            .and_then(|component| component.strip_suffix("_style"))
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    };
    if !found.iter().any(|d| style_of(d).is_some()) {
        return;
    }
    let scene_dir = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut seen: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    found.retain(|d| {
        let Some(name) = style_of(d) else {
            return true;
        };
        let owned = *seen.entry(name.clone()).or_insert_with(|| {
            project
                .explain(flint_core::project::AssetKind::Styles, &name, scene_dir)
                .found
                .is_some()
        });
        !owned
    });
}

/// Parse a scene's text. A file that isn't TOML, or isn't a scene, is an
/// error that names the file (#683): the parser's own message has only a
/// line and a column.
fn parse_scene(content: &str, path: Option<&Path>) -> Result<SceneFile> {
    SceneFile::parse(content).map_err(|e: toml::de::Error| match path {
        Some(path) => FlintError::TomlParseError(format!(
            "{}: {}",
            flint_core::project::shown(path),
            e.to_string().trim_end()
        )),
        None => e.into(),
    })
}

/// What a load from a string logs: there is no file to name.
fn log_string_diagnostics(scene_file: &SceneFile, registry: &SchemaRegistry) {
    let found = diagnose_scene(scene_file, None, registry);
    diagnostic::log_summary(&format!("scene '{}'", scene_file.scene.name), &found);
}

/// A `transform.shear` that shears nothing, or folds space flat, says so
/// (#625): `xzz = 0.2` is a typo that would otherwise draw an unsheared tile,
/// and `xy = yx = 1` has no inverse for the normals to use.
fn warn_on_shear(entity: &str, transform: &toml::Value) {
    let Some(shear) = transform.get("shear") else {
        return;
    };
    let unknown = flint_core::Shear::unknown_keys(shear);
    if !unknown.is_empty() {
        tracing::warn!(
            "[scene] entity '{}' transform.shear: unknown term(s) {} (expected xy, xz, yx, yz, zx, zy)",
            entity,
            unknown.join(", ")
        );
    }
    match flint_core::Shear::from_toml(shear) {
        Some(terms) if flint_core::Shear::determinant(&terms).abs() < 1e-3 => tracing::warn!(
            "[scene] entity '{}' transform.shear flattens it (determinant {:.4}); lighting will be wrong",
            entity,
            flint_core::Shear::determinant(&terms)
        ),
        None if !matches!(shear, toml::Value::Table(_)) => tracing::warn!(
            "[scene] entity '{}' transform.shear should be a table like {{ xz = 0.25 }}",
            entity
        ),
        _ => {}
    }
}

/// Load a scene from a TOML string (no prefab expansion — no filesystem access)
pub fn load_scene_string(
    content: &str,
    registry: &SchemaRegistry,
) -> Result<(FlintWorld, SceneFile)> {
    let scene_file = parse_scene(content, None)?;
    note_post_process(content, None, registry);

    if !scene_file.prefabs.is_empty() {
        tracing::warn!("prefabs in scene string will be ignored (no filesystem access)");
    }

    log_string_diagnostics(&scene_file, registry);
    let world = build_world(&scene_file, registry)?;
    Ok((world, scene_file))
}

/// Warn about `[post_process]` keys that are no field (#354): serde drops
/// them silently, so a typo would leave the default in place. The scene
/// still loads. Only when no loaded schema declares `[table.post_process]`:
/// with one (the engine's `schemas/scene/`), the same keys are
/// `unknown-field` diagnostics in the load's one list (#686), and saying it
/// twice helps nobody. ([`SceneFile::parse`] records the keys the block
/// wrote, [`PostProcessDef::authored`], which a look goes under, #356.)
fn note_post_process(content: &str, path: Option<&Path>, registry: &SchemaRegistry) {
    if registry.get_table("post_process").is_some() {
        return;
    }
    let source = path.map(|p| p.display().to_string());
    for warning in PostProcessDef::unknown_field_warnings(content, source.as_deref()) {
        tracing::warn!("{warning}");
    }
}

/// Reload a scene file, updating the world in place (with prefab expansion)
pub fn reload_scene<P: AsRef<Path>>(
    path: P,
    world: &mut FlintWorld,
    registry: &SchemaRegistry,
) -> Result<SceneFile> {
    let path = path.as_ref();
    let content = vfs::read_to_string(path)?;
    let mut scene_file = parse_scene(&content, Some(path))?;
    note_post_process(&content, Some(path), registry);

    // Expand prefab instances
    prefab::expand_prefabs(&mut scene_file, path)?;
    let found = load_diagnostics(&scene_file, &content, path, registry);
    diagnostic::log_summary(&flint_core::project::shown(path), &found);

    // Clear existing world
    world.clear();

    populate_world(world, &scene_file, registry)?;
    Ok(scene_file)
}

/// Reload a scene from a string, updating the world in place
pub fn reload_scene_string(
    content: &str,
    world: &mut FlintWorld,
    registry: &SchemaRegistry,
) -> Result<SceneFile> {
    let scene_file = parse_scene(content, None)?;
    note_post_process(content, None, registry);

    if !scene_file.prefabs.is_empty() {
        tracing::warn!("prefabs in scene string will be ignored (no filesystem access)");
    }
    log_string_diagnostics(&scene_file, registry);

    // Clear existing world
    world.clear();

    populate_world(world, &scene_file, registry)?;
    Ok(scene_file)
}

/// Build a new FlintWorld from a parsed (and prefab-expanded) scene file
fn build_world(scene_file: &SceneFile, registry: &SchemaRegistry) -> Result<FlintWorld> {
    let mut world = FlintWorld::new();
    populate_world(&mut world, scene_file, registry)?;
    Ok(world)
}

/// Two-pass entity creation: spawn all entities, then set components and
/// relationships. Both passes go in name order (`SceneFile::entities` is a
/// `BTreeMap`), so the same scene gives the same names the same ids, and
/// its components land the same way, every load (#688).
fn populate_world(
    world: &mut FlintWorld,
    scene_file: &SceneFile,
    registry: &SchemaRegistry,
) -> Result<()> {
    // First pass: create all entities
    for (name, _) in &scene_file.entities {
        world.spawn(name.clone())?;
    }

    // Second pass: set up components and relationships
    for (name, entity_def) in &scene_file.entities {
        let id = world.get_id(name).unwrap();

        // Set archetype. One the registry doesn't know gives no defaults;
        // `diagnose_scene` has said so (unknown-archetype)
        if let Some(archetype) = &entity_def.archetype {
            // Apply archetype defaults through set_component to maintain index
            if let Some(arch_schema) = registry.get_archetype(archetype) {
                for (comp_name, defaults) in &arch_schema.defaults {
                    if !world.get_components(id).map_or(false, |c| c.has(comp_name)) {
                        let _ = world.set_component(id, comp_name, defaults.clone());
                    }
                }
            }
            // Set archetype name after defaults so get_components check works
            if let Some(mut components) = world.get_components_mut(id) {
                components.set_archetype(Some(archetype.clone()));
            }
        }

        // Set component data
        for (comp_name, comp_data) in &entity_def.components {
            // Checked against its schema by `diagnose_scene`, which the
            // loaders run first: a load warns and never fails on it
            if comp_name == "transform" {
                warn_on_shear(name, comp_data);
            }
            world.merge_component(id, comp_name, comp_data.clone())?;
        }

        // Apply component schema defaults for missing fields
        for comp_name in entity_def.components.keys() {
            if let Some(schema) = registry.get_component(comp_name) {
                if let Some(mut components) = world.get_components_mut(id) {
                    for (field_name, field_schema) in &schema.fields {
                        if let Some(default) = &field_schema.default {
                            if components.get_field(comp_name, field_name).is_none() {
                                components.set_field(comp_name, field_name, default.clone());
                            }
                        }
                    }
                }
            }
        }

        // Set parent relationship
        if let Some(parent_name) = &entity_def.parent {
            world.set_parent_by_name(name, parent_name)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::EntityId;

    #[test]
    fn test_load_scene_string() {
        let toml_str = r#"
[scene]
name = "Test Scene"

[entities.room1]
archetype = "room"

[entities.room1.bounds]
min = [0, 0, 0]
max = [10, 4, 8]

[entities.door1]
archetype = "door"
parent = "room1"

[entities.door1.transform]
position = [5, 0, 0]
"#;

        let registry = SchemaRegistry::new();
        let (world, scene) = load_scene_string(toml_str, &registry).unwrap();

        assert_eq!(scene.scene.name, "Test Scene");
        assert_eq!(world.entity_count(), 2);
        assert!(world.contains_name("room1"));
        assert!(world.contains_name("door1"));

        let door_id = world.get_id("door1").unwrap();
        let room_id = world.get_id("room1").unwrap();
        assert_eq!(world.get_parent(door_id), Some(room_id));
    }

    /// Entities written out of name order, as a scene file often is
    const SHUFFLED: &str = r#"
[scene]
name = "Order"

[entities.zeta]
parent = "alpha"
[entities.zeta.rigidbody]
body_type = "dynamic"

[entities.alpha]
[entities.alpha.rigidbody]
body_type = "dynamic"

[entities.mid]
[entities.mid.transform]
position = [0, 1, 0]

[entities.beta]
[entities.beta.rigidbody]
body_type = "static"
"#;

    fn name_to_id(world: &FlintWorld) -> Vec<(String, u64)> {
        world
            .all_entities()
            .into_iter()
            .map(|e| (e.name, e.id.raw()))
            .collect()
    }

    #[test]
    fn a_scene_gives_the_same_ids_every_load() {
        // #688: ids came from a process-wide counter in HashMap order, so
        // two loads (or two runs) named the same entity differently
        let registry = SchemaRegistry::new();
        let (first, _) = load_scene_string(SHUFFLED, &registry).unwrap();
        let (second, _) = load_scene_string(SHUFFLED, &registry).unwrap();
        assert_eq!(name_to_id(&first), name_to_id(&second));
        // And which ids is fixed by the names alone, so another process
        // gets these too
        assert_eq!(
            name_to_id(&first),
            [
                ("alpha".to_string(), 1),
                ("beta".to_string(), 2),
                ("mid".to_string(), 3),
                ("zeta".to_string(), 4)
            ]
        );
    }

    #[test]
    fn a_component_is_walked_in_name_order_after_a_load() {
        let registry = SchemaRegistry::new();
        let (world, _) = load_scene_string(SHUFFLED, &registry).unwrap();
        let bodies: Vec<&str> = world
            .entities_with_component("rigidbody")
            .iter()
            .map(|id| world.get_name(*id).unwrap())
            .collect();
        assert_eq!(bodies, ["alpha", "beta", "zeta"]);
    }

    #[test]
    fn a_reload_into_the_same_world_takes_new_ids() {
        // An editor's caches keyed by entity must never take a reloaded
        // entity for an old one
        let registry = SchemaRegistry::new();
        let (mut world, _) = load_scene_string(SHUFFLED, &registry).unwrap();
        reload_scene_string(SHUFFLED, &mut world, &registry).unwrap();
        assert_eq!(world.get_id("alpha"), Some(EntityId::from_raw(5)));
        assert_eq!(world.get_id("zeta"), Some(EntityId::from_raw(8)));
    }

    #[test]
    fn a_scene_loaded_after_another_starts_past_its_ids() {
        // A transition: what a script kept of the old scene names nothing new
        let dir = std::env::temp_dir().join(format!("flint-688-after-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("next.scene.toml");
        std::fs::write(&path, SHUFFLED).unwrap();
        let registry = SchemaRegistry::new();

        let (old, _) = load_scene_string(SHUFFLED, &registry).unwrap();
        let (next, _) = load_scene_after(&path, &registry, &old).unwrap();
        let (fresh, _) = load_scene(&path, &registry).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(next.get_id("alpha"), Some(EntityId::from_raw(5)));
        assert!(name_to_id(&old)
            .iter()
            .all(|(_, id)| next.get_name(EntityId::from_raw(*id)).is_none()));
        // A plain load still counts from 1
        assert_eq!(name_to_id(&fresh), name_to_id(&old));
    }

    #[test]
    fn test_reload_scene_string_clears_and_repopulates() {
        let registry = SchemaRegistry::new();

        let scene_a = r#"
[scene]
name = "V1"

[entities.alpha]
[entities.alpha.transform]
position = [1, 0, 0]

[entities.beta]
[entities.beta.transform]
position = [2, 0, 0]
"#;

        let (mut world, _) = load_scene_string(scene_a, &registry).unwrap();
        assert!(world.contains_name("alpha"));
        assert!(world.contains_name("beta"));

        let scene_b = r#"
[scene]
name = "V2"

[entities.beta]
[entities.beta.transform]
position = [20, 0, 0]

[entities.gamma]
[entities.gamma.transform]
position = [30, 0, 0]
"#;

        reload_scene_string(scene_b, &mut world, &registry).unwrap();

        assert!(
            !world.contains_name("alpha"),
            "alpha should be gone after reload"
        );
        assert!(world.contains_name("beta"), "beta should still exist");
        assert!(world.contains_name("gamma"), "gamma should be added");
        assert_eq!(world.entity_count(), 2);
    }

    #[test]
    fn test_load_with_archetype_defaults_and_overrides() {
        let mut registry = SchemaRegistry::new();
        registry
            .load_archetype_string(
                r#"
[archetype.door]
description = "A door"
components = ["transform", "door"]

[archetype.door.defaults.door]
locked = false
style = "hinged"
"#,
            )
            .unwrap();

        let scene = r#"
[scene]
name = "Arch Test"

[entities.my_door]
archetype = "door"

[entities.my_door.door]
locked = true
"#;

        let (world, _) = load_scene_string(scene, &registry).unwrap();
        let id = world.get_id("my_door").unwrap();
        let door = world.get_component(id, "door").unwrap();

        assert_eq!(
            door.get("locked").and_then(|v| v.as_bool()),
            Some(true),
            "entity override should win"
        );
        assert_eq!(
            door.get("style").and_then(|v| v.as_str()),
            Some("hinged"),
            "archetype default should be preserved"
        );
    }

    /// The engine's `ascii_style` (#266) keeps only the fields a scene or
    /// archetype gives it, so the rest fall back on the scene's
    /// `[post_process.styles.ascii]`: an archetype is a preset, and an
    /// entity overrides it field by field.
    #[test]
    fn ascii_style_takes_an_archetype_preset_and_gains_no_defaults() {
        let mut registry = SchemaRegistry::new();
        registry
            .load_component_string(include_str!("../../../schemas/components/ascii_style.toml"))
            .unwrap();
        registry
            .load_archetype_string(
                r#"
[archetype.terminal_prop]
description = "A prop drawn in green phosphor"
components = ["ascii_style"]

[archetype.terminal_prop.defaults.ascii_style]
color = "tint"
tint = [0.35, 1.0, 0.45]
ramp = "blocks"
"#,
            )
            .unwrap();
        let scene = r#"
[scene]
name = "ASCII presets"

[entities.monitor]
archetype = "terminal_prop"

[entities.keyboard]
archetype = "terminal_prop"
[entities.keyboard.ascii_style]
ramp = "minimal"

[entities.chair.ascii_style]
cell = 12
"#;
        let (world, _) = load_scene_string(scene, &registry).unwrap();
        let get = |e: &str| {
            world
                .get_component(world.get_id(e).unwrap(), "ascii_style")
                .unwrap()
                .as_table()
                .unwrap()
                .clone()
        };
        let keyboard = get("keyboard");
        assert_eq!(
            keyboard["ramp"].as_str(),
            Some("minimal"),
            "the entity wins"
        );
        assert_eq!(keyboard["color"].as_str(), Some("tint"), "the preset stays");
        assert_eq!(get("monitor")["ramp"].as_str(), Some("blocks"));
        let chair = get("chair");
        assert_eq!(
            chair.keys().collect::<Vec<_>>(),
            vec!["cell"],
            "no schema default fills in a field the scene would set"
        );
    }

    #[test]
    fn test_load_deep_hierarchy() {
        let registry = SchemaRegistry::new();
        let scene = r#"
[scene]
name = "Hierarchy"

[entities.grandparent]
[entities.grandparent.transform]
position = [0, 0, 0]

[entities.parent_node]
parent = "grandparent"
[entities.parent_node.transform]
position = [0, 0, 0]

[entities.child_node]
parent = "parent_node"
[entities.child_node.transform]
position = [0, 0, 0]
"#;

        let (world, _) = load_scene_string(scene, &registry).unwrap();

        let gp = world.get_id("grandparent").unwrap();
        let p = world.get_id("parent_node").unwrap();
        let c = world.get_id("child_node").unwrap();

        assert_eq!(world.get_parent(p), Some(gp));
        assert_eq!(world.get_parent(c), Some(p));
        assert!(world.get_parent(gp).is_none());
    }

    /// #680: a self-parent, or two entities parented to each other, used
    /// to load and then overflow the stack at the first matrix walk. Now
    /// the load fails, naming the entities.
    #[test]
    fn a_parent_cycle_fails_the_load_naming_the_entities() {
        let registry = SchemaRegistry::new();
        let itself = "[scene]\nname = \"s\"\n\n[entities.lamp]\nparent = \"lamp\"\n";
        let err = load_scene_string(itself, &registry).err().expect("refused");
        assert_eq!(
            err.to_string(),
            "Parent cycle: `lamp` can't be parented to itself"
        );

        let two = "[scene]\nname = \"s\"\n\n[entities.cart]\nparent = \"wheel\"\n\n\
                   [entities.wheel]\nparent = \"cart\"\n";
        let err = load_scene_string(two, &registry).err().expect("refused");
        // Which link closes the loop depends on the load order; both are named
        let text = err.to_string();
        assert!(text.starts_with("Parent cycle: `"), "{text}");
        assert!(
            text.contains("`cart`") && text.contains("`wheel`"),
            "{text}"
        );
    }

    #[test]
    fn test_load_camera_metadata() {
        let registry = SchemaRegistry::new();
        let scene = r#"
[scene]
name = "Camera Test"

[camera]
position = [0.0, 10.0, 5.0]
target = [0.0, 0.0, 0.0]
fov = 60.0
"#;

        let (_, scene_file) = load_scene_string(scene, &registry).unwrap();
        let cam = scene_file.camera.expect("camera should be present");
        assert_eq!(cam.position, Some([0.0, 10.0, 5.0]));
        assert_eq!(cam.target, Some([0.0, 0.0, 0.0]));
        assert_eq!(cam.fov, Some(60.0));
    }

    #[test]
    fn test_load_environment_metadata() {
        let registry = SchemaRegistry::new();
        let scene = r#"
[scene]
name = "Env Test"

[environment]
skybox = "sky_sunset.hdr"
"#;

        let (_, scene_file) = load_scene_string(scene, &registry).unwrap();
        let env = scene_file
            .environment
            .expect("environment should be present");
        assert_eq!(env.skybox, Some("sky_sunset.hdr".into()));
        assert!(
            env.casts_shadows() && env.uses_default_lights(),
            "absent = true"
        );
    }

    #[test]
    fn test_environment_no_shadows_no_default_lights() {
        let registry = SchemaRegistry::new();
        let scene = r#"
[scene]
name = "Unlit"

[environment]
shadows = false
default_lights = false
"#;
        let (_, scene_file) = load_scene_string(scene, &registry).unwrap();
        let env = scene_file.environment.unwrap();
        assert!(!env.casts_shadows());
        assert!(!env.uses_default_lights());
    }

    #[test]
    fn test_load_post_process_metadata() {
        let registry = SchemaRegistry::new();
        let scene = r#"
[scene]
name = "PP Test"

[post_process]
bloom_enabled = true
bloom_intensity = 0.08
fog_enabled = true
fog_density = 0.05
chromatic_aberration = 0.2
radial_blur = 0.4
desaturate = 0.85
dof_strength = 0.5
dof_focus_distance = 22.0
dof_focus_range = 3.5
"#;

        let (_, scene_file) = load_scene_string(scene, &registry).unwrap();
        let pp = scene_file
            .post_process
            .expect("post_process should be present");
        assert!(pp.bloom_enabled);
        assert!((pp.bloom_intensity - 0.08).abs() < 0.001);
        assert!(pp.fog_enabled);
        assert!((pp.fog_density - 0.05).abs() < 0.001);
        assert!((pp.chromatic_aberration - 0.2).abs() < 0.001);
        assert!((pp.radial_blur - 0.4).abs() < 0.001);
        assert!((pp.desaturate - 0.85).abs() < 0.001);
        assert!((pp.dof_strength - 0.5).abs() < 0.001);
        assert!((pp.dof_focus_distance - 22.0).abs() < 0.001);
        assert!((pp.dof_focus_range - 3.5).abs() < 0.001);
    }

    /// A writer the test's subscriber logs into.
    #[derive(Clone, Default)]
    struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn unknown_post_process_field_warns_with_file_and_still_loads() {
        let dir = std::env::temp_dir().join(format!("flint_pp_typo_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("typo.scene.toml");
        std::fs::write(
            &path,
            "[scene]\nname = \"Typo\"\n\n[post_process]\nbloom_intensty = 0.5\nexposure = 1.5\n",
        )
        .unwrap();

        let log = Captured::default();
        let writer = log.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();
        let loaded = tracing::subscriber::with_default(subscriber, || {
            load_scene(&path, &SchemaRegistry::new())
        });
        std::fs::remove_dir_all(&dir).ok();

        let (_, scene_file) = loaded.expect("a typo doesn't stop the load");
        let pp = scene_file.post_process.unwrap();
        assert!((pp.exposure - 1.5).abs() < 1e-6, "known fields still read");
        assert!(
            (pp.bloom_intensity - 0.04).abs() < 1e-6,
            "the typo leaves the default"
        );

        let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        assert!(text.contains("WARN"), "{text}");
        assert!(
            text.contains(
                "[post_process] has no field 'bloom_intensty', did you mean 'bloom_intensity'?"
            ),
            "{text}"
        );
        assert!(text.contains("typo.scene.toml"), "names the file: {text}");
    }

    #[test]
    fn test_post_process_effect_fields_default_to_zero() {
        let registry = SchemaRegistry::new();
        let scene = r#"
[scene]
name = "PP Defaults"

[post_process]
bloom_enabled = false
"#;

        let (_, scene_file) = load_scene_string(scene, &registry).unwrap();
        let pp = scene_file
            .post_process
            .expect("post_process should be present");
        assert_eq!(pp.chromatic_aberration, 0.0);
        assert_eq!(pp.radial_blur, 0.0);
        assert_eq!(pp.desaturate, 0.0);
        assert_eq!(pp.dof_strength, 0.0);
    }

    #[test]
    fn test_load_empty_scene() {
        let registry = SchemaRegistry::new();
        let scene = r#"
[scene]
name = "Empty"
"#;

        let (world, scene_file) = load_scene_string(scene, &registry).unwrap();
        assert_eq!(world.entity_count(), 0);
        assert!(scene_file.entities.is_empty());
    }

    #[test]
    fn test_transform_parsing_pipeline() {
        let toml_str = r#"
[scene]
name = "Transform Test"

[entities.bar_counter]
archetype = "furniture"

[entities.bar_counter.transform]
position = [-4, 0, 0]

[entities.kitchen]
archetype = "room"

[entities.kitchen.transform]
position = [0, 0, -9]

[entities.table]
archetype = "furniture"

[entities.table.transform]
position = [2.5, 1.0, -3.5]
"#;

        let registry = SchemaRegistry::new();
        let (world, _) = load_scene_string(toml_str, &registry).unwrap();

        // Verify bar_counter transform
        let bar_id = world.get_id("bar_counter").unwrap();
        let bar_transform = world.get_transform(bar_id);
        let bar_t = bar_transform.expect("bar_counter should have a transform");
        assert!(
            (bar_t.position.x - (-4.0)).abs() < 0.001,
            "bar x={}, expected -4",
            bar_t.position.x
        );
        assert!(
            (bar_t.position.y - 0.0).abs() < 0.001,
            "bar y={}, expected 0",
            bar_t.position.y
        );
        assert!(
            (bar_t.position.z - 0.0).abs() < 0.001,
            "bar z={}, expected 0",
            bar_t.position.z
        );

        // Verify kitchen transform
        let kitchen_id = world.get_id("kitchen").unwrap();
        let kitchen_transform = world.get_transform(kitchen_id);
        let kitchen_t = kitchen_transform.expect("kitchen should have a transform");
        assert!(
            (kitchen_t.position.z - (-9.0)).abs() < 0.001,
            "kitchen z={}, expected -9",
            kitchen_t.position.z
        );

        // Verify table transform with float values
        let table_id = world.get_id("table").unwrap();
        let table_transform = world.get_transform(table_id);
        let table_t = table_transform.expect("table should have a transform");
        assert!(
            (table_t.position.x - 2.5).abs() < 0.001,
            "table x={}, expected 2.5",
            table_t.position.x
        );
        assert!(
            (table_t.position.y - 1.0).abs() < 0.001,
            "table y={}, expected 1.0",
            table_t.position.y
        );
        assert!(
            (table_t.position.z - (-3.5)).abs() < 0.001,
            "table z={}, expected -3.5",
            table_t.position.z
        );
    }
}
