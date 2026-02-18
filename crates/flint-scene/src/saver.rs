//! Scene saving to TOML files

use crate::format::{EntityDef, SceneFile, SceneMetadata};
use flint_core::Result;
use flint_ecs::FlintWorld;
use std::fs;
use std::path::Path;

/// Save a world to a scene file
pub fn save_scene<P: AsRef<Path>>(
    path: P,
    world: &FlintWorld,
    name: impl Into<String>,
) -> Result<()> {
    let content = save_scene_string(world, name)?;
    fs::write(path, content)?;
    Ok(())
}

/// Save a world to a TOML string
pub fn save_scene_string(world: &FlintWorld, name: impl Into<String>) -> Result<String> {
    let scene_file = world_to_scene_file(world, name);
    let content = toml::to_string_pretty(&scene_file)?;
    Ok(content)
}

/// Convert a FlintWorld to a SceneFile
pub fn world_to_scene_file(world: &FlintWorld, name: impl Into<String>) -> SceneFile {
    let mut scene = SceneFile {
        scene: SceneMetadata {
            name: name.into(),
            version: "1.0".to_string(),
            description: None,
            input_config: None,
        },
        environment: None,
        post_process: None,
        prefabs: std::collections::HashMap::new(),
        entities: std::collections::HashMap::new(),
    };

    for info in world.all_entities() {
        let components = world.get_components(info.id);

        let entity_def = EntityDef {
            archetype: components.and_then(|c| c.archetype.clone()),
            parent: info.parent,
            components: components.map(|c| c.data.clone()).unwrap_or_default(),
        };

        scene.entities.insert(info.name, entity_def);
    }

    scene
}

/// Update an existing scene file with changes from the world
pub fn update_scene_file(world: &FlintWorld, existing: &mut SceneFile) {
    // Update entities
    for info in world.all_entities() {
        let components = world.get_components(info.id);

        let entity_def = EntityDef {
            archetype: components.and_then(|c| c.archetype.clone()),
            parent: info.parent,
            components: components.map(|c| c.data.clone()).unwrap_or_default(),
        };

        existing.entities.insert(info.name, entity_def);
    }

    // Remove entities that no longer exist
    let world_names: std::collections::HashSet<_> = world.entity_names().collect();
    existing.entities.retain(|name, _| world_names.contains(name.as_str()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_scene_string() {
        let mut world = FlintWorld::new();
        world.spawn("entity1").unwrap();
        world.spawn("entity2").unwrap();

        let toml_str = save_scene_string(&world, "Test Scene").unwrap();

        assert!(toml_str.contains("Test Scene"));
        assert!(toml_str.contains("entity1"));
        assert!(toml_str.contains("entity2"));
    }

    #[test]
    fn test_roundtrip() {
        use crate::loader::load_scene_string;
        use flint_schema::SchemaRegistry;

        let original_toml = r#"
[scene]
name = "Roundtrip Test"

[entities.room]
archetype = "room"

[entities.room.bounds]
min = [0, 0, 0]
max = [10, 4, 8]
"#;

        let registry = SchemaRegistry::new();
        let (world, _) = load_scene_string(original_toml, &registry).unwrap();

        let saved = save_scene_string(&world, "Roundtrip Test").unwrap();

        // Load the saved version
        let (world2, scene2) = load_scene_string(&saved, &registry).unwrap();

        assert_eq!(scene2.scene.name, "Roundtrip Test");
        assert!(world2.contains_name("room"));
    }
}
