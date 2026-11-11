//! Scene saving to TOML files

use crate::format::{EntityDef, SceneFile, SceneMetadata};
use flint_core::Result;
use flint_ecs::FlintWorld;
use std::fs;
use std::path::Path;

/// Write a world to a new scene file. Refuses a path that's already there
/// (#677): the world holds only entities, so a file written from it has no
/// `[camera]`, `[environment]`, `[post_process]`, `[living]`, `[wooze]`,
/// prefabs, description or comments. An existing scene is changed through
/// [`crate::SceneDocument`] (`patch_field`, `apply_world_diff`), which writes
/// only what changed.
pub fn save_scene<P: AsRef<Path>>(
    path: P,
    world: &FlintWorld,
    name: impl Into<String>,
) -> Result<()> {
    let path = path.as_ref();
    let content = save_scene_string(world, name)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => flint_core::FlintError::IoError(std::io::Error::new(
                e.kind(),
                format!(
                    "{} already exists: a scene is rewritten from its world only when it's new (#677); change one through SceneDocument",
                    path.display()
                ),
            )),
            _ => e.into(),
        })?;
    std::io::Write::write_all(&mut file, content.as_bytes())?;
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
            preload_audio: true,
        },
        camera: None,
        environment: None,
        post_process: None,
        living: None,
        wooze: None,
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
    existing
        .entities
        .retain(|name, _| world_names.contains(name.as_str()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_scene_refuses_to_overwrite_a_scene() {
        // #677: a world-only file would lose everything but the entities
        let dir = std::env::temp_dir().join(format!("flint-save-scene-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("kept.scene.toml");
        let original = "# mine\n[scene]\nname = \"kept\"\n\n[camera]\nfov = 40.0\n";
        std::fs::write(&path, original).unwrap();
        let mut world = FlintWorld::new();
        world.spawn("e").unwrap();
        let err = save_scene(&path, &world, "kept").unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);

        let new = dir.join("new.scene.toml");
        let _ = std::fs::remove_file(&new);
        save_scene(&new, &world, "new").unwrap();
        assert!(std::fs::read_to_string(&new)
            .unwrap()
            .contains("[entities.e]"));
        let _ = std::fs::remove_dir_all(&dir);
    }

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

    #[test]
    fn test_roundtrip_with_parent_hierarchy() {
        use crate::loader::load_scene_string;
        use flint_schema::SchemaRegistry;

        let original = r#"
[scene]
name = "Hierarchy RT"

[entities.parent_e]
[entities.parent_e.transform]
position = [1.0, 2.0, 3.0]

[entities.child_e]
parent = "parent_e"
[entities.child_e.transform]
position = [4.0, 5.0, 6.0]
"#;

        let registry = SchemaRegistry::new();
        let (world, _) = load_scene_string(original, &registry).unwrap();
        let saved = save_scene_string(&world, "Hierarchy RT").unwrap();
        let (world2, _) = load_scene_string(&saved, &registry).unwrap();

        let parent_id = world2.get_id("parent_e").unwrap();
        let child_id = world2.get_id("child_e").unwrap();
        assert_eq!(
            world2.get_parent(child_id),
            Some(parent_id),
            "parent-child should survive roundtrip"
        );
    }

    #[test]
    fn test_roundtrip_with_archetype() {
        use crate::loader::load_scene_string;
        use flint_schema::SchemaRegistry;

        let mut registry = SchemaRegistry::new();
        registry
            .load_archetype_string(
                r#"
[archetype.lamp]
description = "A lamp"
components = ["transform", "light"]

[archetype.lamp.defaults.light]
intensity = 1.0
"#,
            )
            .unwrap();

        let original = r#"
[scene]
name = "Arch RT"

[entities.my_lamp]
archetype = "lamp"

[entities.my_lamp.transform]
position = [0.0, 5.0, 0.0]
"#;

        let (world, _) = load_scene_string(original, &registry).unwrap();
        let saved = save_scene_string(&world, "Arch RT").unwrap();
        let (world2, scene2) = load_scene_string(&saved, &registry).unwrap();

        let entity_def = scene2.entities.get("my_lamp").unwrap();
        assert_eq!(
            entity_def.archetype.as_deref(),
            Some("lamp"),
            "archetype name should survive roundtrip"
        );
        assert!(world2.contains_name("my_lamp"));
    }

    #[test]
    fn test_update_scene_file_removes_deleted_entities() {
        use crate::loader::load_scene_string;
        use flint_schema::SchemaRegistry;

        let original = r#"
[scene]
name = "Update Test"

[entities.keep_me]
[entities.keep_me.transform]
position = [0.0, 0.0, 0.0]

[entities.delete_me]
[entities.delete_me.transform]
position = [1.0, 0.0, 0.0]
"#;

        let registry = SchemaRegistry::new();
        let (mut world, mut scene_file) = load_scene_string(original, &registry).unwrap();

        // Delete one entity from the world
        world.despawn_by_name("delete_me").unwrap();

        update_scene_file(&world, &mut scene_file);

        assert!(scene_file.entities.contains_key("keep_me"));
        assert!(
            !scene_file.entities.contains_key("delete_me"),
            "deleted entity should be removed from scene file"
        );
    }

    #[test]
    fn test_roundtrip_preserves_component_data() {
        use crate::loader::load_scene_string;
        use flint_schema::SchemaRegistry;

        let original = r#"
[scene]
name = "Data RT"

[entities.obj]
[entities.obj.transform]
position = [1.5, 2.5, 3.5]
rotation = [0.0, 45.0, 0.0]
scale = [2.0, 2.0, 2.0]

[entities.obj.material]
color = "blue"
metallic = 0.8
roughness = 0.2
"#;

        let registry = SchemaRegistry::new();
        let (world, _) = load_scene_string(original, &registry).unwrap();
        let saved = save_scene_string(&world, "Data RT").unwrap();
        let (world2, _) = load_scene_string(&saved, &registry).unwrap();

        let id = world2.get_id("obj").unwrap();

        // Transform should survive
        let t = world2.get_transform(id).unwrap();
        assert!((t.position.x - 1.5).abs() < 0.01);
        assert!((t.rotation.y - 45.0).abs() < 0.01);
        assert!((t.scale.x - 2.0).abs() < 0.01);

        // Material should survive
        let mat = world2.get_component(id, "material").unwrap();
        assert_eq!(mat.get("color").and_then(|v| v.as_str()), Some("blue"));
        assert!((mat.get("metallic").and_then(|v| v.as_float()).unwrap() - 0.8).abs() < 0.01);
    }
}
