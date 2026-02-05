//! Scene loading from TOML files

use crate::format::SceneFile;
use flint_core::Result;
use flint_ecs::FlintWorld;
use flint_schema::SchemaRegistry;
use std::fs;
use std::path::Path;

/// Load a scene from a TOML file
pub fn load_scene<P: AsRef<Path>>(
    path: P,
    registry: &SchemaRegistry,
) -> Result<(FlintWorld, SceneFile)> {
    let content = fs::read_to_string(path)?;
    load_scene_string(&content, registry)
}

/// Load a scene from a TOML string
pub fn load_scene_string(
    content: &str,
    registry: &SchemaRegistry,
) -> Result<(FlintWorld, SceneFile)> {
    let scene_file: SceneFile = toml::from_str(content)?;
    let mut world = FlintWorld::new();

    // First pass: create all entities
    for (name, _) in &scene_file.entities {
        world.spawn(name.clone())?;
    }

    // Second pass: set up components and relationships
    for (name, entity_def) in &scene_file.entities {
        let id = world.get_id(name).unwrap();

        // Set archetype
        if let Some(archetype) = &entity_def.archetype {
            let components = world.get_components_mut(id).unwrap();
            components.archetype = Some(archetype.clone());

            // Apply archetype defaults
            if let Some(arch_schema) = registry.get_archetype(archetype) {
                for (comp_name, defaults) in &arch_schema.defaults {
                    if !components.has(comp_name) {
                        components.set(comp_name.clone(), defaults.clone());
                    }
                }
            }
        }

        // Set component data
        for (comp_name, comp_data) in &entity_def.components {
            world.set_component(id, comp_name, comp_data.clone())?;
        }

        // Set parent relationship
        if let Some(parent_name) = &entity_def.parent {
            world.set_parent_by_name(name, parent_name)?;
        }
    }

    Ok((world, scene_file))
}

/// Reload a scene file, updating the world in place
pub fn reload_scene<P: AsRef<Path>>(
    path: P,
    world: &mut FlintWorld,
    registry: &SchemaRegistry,
) -> Result<SceneFile> {
    let content = fs::read_to_string(path)?;
    reload_scene_string(&content, world, registry)
}

/// Reload a scene from a string, updating the world in place
pub fn reload_scene_string(
    content: &str,
    world: &mut FlintWorld,
    registry: &SchemaRegistry,
) -> Result<SceneFile> {
    let scene_file: SceneFile = toml::from_str(content)?;

    // Clear existing world
    world.clear();

    // First pass: create all entities
    for (name, _) in &scene_file.entities {
        world.spawn(name.clone())?;
    }

    // Second pass: set up components and relationships
    for (name, entity_def) in &scene_file.entities {
        let id = world.get_id(name).unwrap();

        // Set archetype
        if let Some(archetype) = &entity_def.archetype {
            let components = world.get_components_mut(id).unwrap();
            components.archetype = Some(archetype.clone());

            // Apply archetype defaults
            if let Some(arch_schema) = registry.get_archetype(archetype) {
                for (comp_name, defaults) in &arch_schema.defaults {
                    if !components.has(comp_name) {
                        components.set(comp_name.clone(), defaults.clone());
                    }
                }
            }
        }

        // Set component data
        for (comp_name, comp_data) in &entity_def.components {
            world.set_component(id, comp_name, comp_data.clone())?;
        }

        // Set parent relationship
        if let Some(parent_name) = &entity_def.parent {
            world.set_parent_by_name(name, parent_name)?;
        }
    }

    Ok(scene_file)
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
        eprintln!("bar_counter transform: {:?}", bar_transform);
        let bar_t = bar_transform.expect("bar_counter should have a transform");
        assert!((bar_t.position.x - (-4.0)).abs() < 0.001, "bar x={}, expected -4", bar_t.position.x);
        assert!((bar_t.position.y - 0.0).abs() < 0.001, "bar y={}, expected 0", bar_t.position.y);
        assert!((bar_t.position.z - 0.0).abs() < 0.001, "bar z={}, expected 0", bar_t.position.z);

        // Verify kitchen transform
        let kitchen_id = world.get_id("kitchen").unwrap();
        let kitchen_transform = world.get_transform(kitchen_id);
        eprintln!("kitchen transform: {:?}", kitchen_transform);
        let kitchen_t = kitchen_transform.expect("kitchen should have a transform");
        assert!((kitchen_t.position.z - (-9.0)).abs() < 0.001, "kitchen z={}, expected -9", kitchen_t.position.z);

        // Verify table transform with float values
        let table_id = world.get_id("table").unwrap();
        let table_transform = world.get_transform(table_id);
        eprintln!("table transform: {:?}", table_transform);
        let table_t = table_transform.expect("table should have a transform");
        assert!((table_t.position.x - 2.5).abs() < 0.001, "table x={}, expected 2.5", table_t.position.x);
        assert!((table_t.position.y - 1.0).abs() < 0.001, "table y={}, expected 1.0", table_t.position.y);
        assert!((table_t.position.z - (-3.5)).abs() < 0.001, "table z={}, expected -3.5", table_t.position.z);
    }
}
