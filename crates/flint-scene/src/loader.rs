//! Scene loading from TOML files

use crate::format::SceneFile;
use crate::prefab;
use flint_core::Result;
use flint_ecs::FlintWorld;
use flint_schema::{validate_component_data, SchemaRegistry};
use std::fs;
use std::path::Path;

/// Load a scene from a TOML file (with prefab expansion)
pub fn load_scene<P: AsRef<Path>>(
    path: P,
    registry: &SchemaRegistry,
) -> Result<(FlintWorld, SceneFile)> {
    let path = path.as_ref();
    let content = fs::read_to_string(path)?;
    let mut scene_file: SceneFile = toml::from_str(&content)?;

    // Expand prefab instances into entities before building world
    prefab::expand_prefabs(&mut scene_file, path)?;

    let world = build_world(&scene_file, registry)?;
    Ok((world, scene_file))
}

/// Load a scene from a TOML string (no prefab expansion — no filesystem access)
pub fn load_scene_string(
    content: &str,
    registry: &SchemaRegistry,
) -> Result<(FlintWorld, SceneFile)> {
    let scene_file: SceneFile = toml::from_str(content)?;

    if !scene_file.prefabs.is_empty() {
        tracing::warn!("prefabs in scene string will be ignored (no filesystem access)");
    }

    let world = build_world(&scene_file, registry)?;
    Ok((world, scene_file))
}

/// Reload a scene file, updating the world in place (with prefab expansion)
pub fn reload_scene<P: AsRef<Path>>(
    path: P,
    world: &mut FlintWorld,
    registry: &SchemaRegistry,
) -> Result<SceneFile> {
    let path = path.as_ref();
    let content = fs::read_to_string(path)?;
    let mut scene_file: SceneFile = toml::from_str(&content)?;

    // Expand prefab instances
    prefab::expand_prefabs(&mut scene_file, path)?;

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
    let scene_file: SceneFile = toml::from_str(content)?;

    if !scene_file.prefabs.is_empty() {
        tracing::warn!("prefabs in scene string will be ignored (no filesystem access)");
    }

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

/// Two-pass entity creation: spawn all entities, then set components and relationships
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

        // Set archetype
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
            if let Some(components) = world.get_components_mut(id) {
                components.archetype = Some(archetype.clone());
            }
        }

        // Set component data
        for (comp_name, comp_data) in &entity_def.components {
            // Validate against schema (warn, don't fail)
            if let Some(schema) = registry.get_component(comp_name) {
                if let Err(e) = validate_component_data(schema, comp_data) {
                    tracing::warn!("[scene] entity '{}' component '{}': {}", name, comp_name, e);
                }
            }
            world.merge_component(id, comp_name, comp_data.clone())?;
        }

        // Apply component schema defaults for missing fields
        for comp_name in entity_def.components.keys() {
            if let Some(schema) = registry.get_component(comp_name) {
                if let Some(components) = world.get_components_mut(id) {
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
