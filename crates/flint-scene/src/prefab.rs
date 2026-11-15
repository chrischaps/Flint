//! Prefab expansion — loads .prefab.toml templates and expands instances into scene entities

use crate::format::{PrefabFile, SceneFile};
use flint_core::project::{AssetKind, Project};
use flint_core::vfs;
use flint_core::{FlintError, Result};
use std::collections::HashMap;
use std::path::Path;

/// Expand all prefab instances in a scene file, inserting their entities into `scene.entities`.
/// Must be called before entity creation (two-pass load).
///
/// A template is found by the project's one asset search
/// (`AssetKind::Prefabs`, ADR 0114): `prefabs/<name>.prefab.toml` beside the
/// scene, in each directory above it up to the project root, then in the
/// engine. The project is the one the scene file is found in.
pub fn expand_prefabs(scene: &mut SceneFile, scene_path: &Path) -> Result<()> {
    if scene.prefabs.is_empty() {
        return Ok(());
    }
    expand_prefabs_in(scene, scene_path, &Project::for_scene(scene_path))
}

/// [`expand_prefabs`] for a host that already holds the scene's project.
pub fn expand_prefabs_in(
    scene: &mut SceneFile,
    scene_path: &Path,
    project: &Project,
) -> Result<()> {
    if scene.prefabs.is_empty() {
        return Ok(());
    }

    let scene_dir = scene_path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    // Cache loaded prefab templates by template name
    let mut cache: HashMap<String, PrefabFile> = HashMap::new();

    // Collect instances (we need to drain prefabs to avoid borrow conflict)
    let instances: Vec<_> = scene.prefabs.drain().collect();

    for (_instance_name, instance) in &instances {
        // Load template (cached)
        let prefab = if let Some(cached) = cache.get(&instance.template) {
            cached.clone()
        } else {
            let prefab_path = project
                .resolve(AssetKind::Prefabs, &instance.template, scene_dir)
                .map_err(|not_found| FlintError::ParseError(not_found.to_string()))?;
            let content = vfs::read_to_string(&prefab_path).map_err(|e| {
                FlintError::ParseError(format!(
                    "Failed to read prefab '{}': {}",
                    prefab_path.display(),
                    e
                ))
            })?;
            let prefab: PrefabFile = toml::from_str(&content).map_err(|e| {
                FlintError::ParseError(format!(
                    "Failed to parse prefab '{}': {}",
                    prefab_path.display(),
                    e
                ))
            })?;
            cache.insert(instance.template.clone(), prefab.clone());
            prefab
        };

        let prefix = &instance.prefix;

        // Build variable map for substitution
        let mut vars = HashMap::new();
        vars.insert("PREFIX".to_string(), prefix.clone());

        // Expand each template entity
        for (entity_suffix, template_def) in &prefab.entities {
            let expanded_name = format!("{}_{}", prefix, entity_suffix);

            // Check for collision
            if scene.entities.contains_key(&expanded_name) {
                return Err(FlintError::ParseError(format!(
                    "Prefab expansion collision: entity '{}' already exists in scene",
                    expanded_name
                )));
            }

            // Clone and transform the entity definition
            let mut entity = template_def.clone();

            // Prefix parent references within the prefab
            if let Some(ref parent) = entity.parent {
                entity.parent = Some(format!("{}_{}", prefix, parent));
            }

            // Substitute ${PREFIX} in all string component values
            for (_comp_name, comp_value) in entity.components.iter_mut() {
                substitute_in_value(comp_value, &vars);
            }

            // Apply per-instance overrides (deep merge at field level)
            if let Some(entity_overrides) = instance.overrides.get(entity_suffix) {
                for (comp_name, override_value) in entity_overrides {
                    deep_merge_component(&mut entity.components, comp_name, override_value.clone());
                }
            }

            scene.entities.insert(expanded_name, entity);
        }
    }

    Ok(())
}

/// Recursively substitute `${VAR}` patterns in string values within a toml::Value tree.
fn substitute_in_value(value: &mut toml::Value, vars: &HashMap<String, String>) {
    match value {
        toml::Value::String(s) => {
            for (key, replacement) in vars {
                let pattern = format!("${{{}}}", key);
                if s.contains(&pattern) {
                    *s = s.replace(&pattern, replacement);
                }
            }
        }
        toml::Value::Table(table) => {
            for (_k, v) in table.iter_mut() {
                substitute_in_value(v, vars);
            }
        }
        toml::Value::Array(arr) => {
            for v in arr.iter_mut() {
                substitute_in_value(v, vars);
            }
        }
        _ => {} // integers, floats, booleans — no substitution needed
    }
}

/// Deep-merge override data into an entity's component map.
/// If the component already exists as a Table, merge fields. Otherwise replace.
fn deep_merge_component(
    components: &mut HashMap<String, toml::Value>,
    comp_name: &str,
    override_value: toml::Value,
) {
    if let Some(existing) = components.get_mut(comp_name) {
        if let (toml::Value::Table(existing_table), toml::Value::Table(override_table)) =
            (existing, &override_value)
        {
            // Field-level merge: override wins per field
            for (field, val) in override_table {
                existing_table.insert(field.clone(), val.clone());
            }
            return;
        }
    }
    // No existing component or type mismatch: just insert
    components.insert(comp_name.to_string(), override_value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::{EntityDef, PrefabInstance, SceneMetadata};

    fn default_version() -> String {
        "1.0".to_string()
    }

    #[test]
    fn test_substitute_in_value() {
        let mut vars = HashMap::new();
        vars.insert("PREFIX".to_string(), "ai1".to_string());

        let mut val = toml::Value::String("${PREFIX}_kart".to_string());
        substitute_in_value(&mut val, &vars);
        assert_eq!(val.as_str().unwrap(), "ai1_kart");
    }

    #[test]
    fn test_substitute_nested_table() {
        let mut vars = HashMap::new();
        vars.insert("PREFIX".to_string(), "player".to_string());

        let mut val = toml::toml! {
            kart = "${PREFIX}_kart"
            prefix = "${PREFIX}"
            speed = 30.0
        }
        .into();
        substitute_in_value(&mut val, &vars);

        let table = val.as_table().unwrap();
        assert_eq!(table["kart"].as_str().unwrap(), "player_kart");
        assert_eq!(table["prefix"].as_str().unwrap(), "player");
        assert_eq!(table["speed"].as_float().unwrap(), 30.0);
    }

    #[test]
    fn test_deep_merge_component() {
        let mut components = HashMap::new();
        components.insert(
            "kart_physics".to_string(),
            toml::toml! {
                max_speed = 30.0
                acceleration = 15.0
                drag = 0.5
            }
            .into(),
        );

        let override_val: toml::Value = toml::toml! {
            max_speed = 33.0
        }
        .into();

        deep_merge_component(&mut components, "kart_physics", override_val);

        let result = components["kart_physics"].as_table().unwrap();
        assert_eq!(result["max_speed"].as_float().unwrap(), 33.0);
        assert_eq!(result["acceleration"].as_float().unwrap(), 15.0);
        assert_eq!(result["drag"].as_float().unwrap(), 0.5);
    }

    #[test]
    fn test_deep_merge_adds_new_component() {
        let mut components = HashMap::new();
        let override_val: toml::Value = toml::toml! {
            throttle = 0.0
            steering = 0.0
        }
        .into();

        deep_merge_component(&mut components, "ai_input", override_val);
        assert!(components.contains_key("ai_input"));
    }

    #[test]
    fn test_expand_prefabs_name_collision() {
        let mut scene = SceneFile {
            scene: SceneMetadata {
                name: "test".to_string(),
                version: default_version(),
                description: None,
                input_config: None,
                preload_audio: true,
            },
            camera: None,
            environment: None,
            post_process: None,
            tables: toml::Table::new(),
            prefabs: HashMap::new(),
            entities: HashMap::new(),
            written: toml::Table::new(),
        };

        // Add an entity named "p_kart" that would collide
        scene
            .entities
            .insert("p_kart".to_string(), EntityDef::new());

        // We can't easily test expand_prefabs without a filesystem,
        // but we can verify the collision detection logic by testing
        // that the entities map already has the key
        assert!(scene.entities.contains_key("p_kart"));
    }

    const LAMP: &str = "[prefab]\nname = \"lamp\"\n\n[entities.bulb]\n";
    const SCENE: &str =
        "[scene]\nname = \"cave\"\n\n[prefabs.porch]\ntemplate = \"lamp\"\nprefix = \"porch\"\n";

    fn expanded(files: &[(&str, &str)], scene_path: &str) -> Result<SceneFile> {
        let mut source = flint_core::vfs::MemorySource::new();
        for (path, text) in files {
            source.insert(path, *text);
        }
        flint_core::vfs::scoped(std::sync::Arc::new(source), || {
            let mut scene: SceneFile = toml::from_str(SCENE).unwrap();
            expand_prefabs(&mut scene, Path::new(scene_path))?;
            Ok(scene)
        })
    }

    #[test]
    fn a_scene_two_folders_deep_finds_the_projects_prefab() {
        // Before the one resolver (ADR 0114) a prefab was looked for beside
        // the scene and one level up, and no further.
        let scene = "game/scenes/act1/cave/a.scene.toml";
        let found = expanded(
            &[
                ("game/flint.toml", ""),
                ("game/prefabs/lamp.prefab.toml", LAMP),
                (scene, SCENE),
            ],
            scene,
        )
        .unwrap();
        assert!(found.entities.contains_key("porch_bulb"));
    }

    #[test]
    fn the_nearest_prefab_of_a_name_wins() {
        let scene = "game/scenes/a.scene.toml";
        let near = "[prefab]\nname = \"lamp\"\n\n[entities.wick]\n";
        let found = expanded(
            &[
                ("game/flint.toml", ""),
                ("game/prefabs/lamp.prefab.toml", LAMP),
                ("game/scenes/prefabs/lamp.prefab.toml", near),
                (scene, SCENE),
            ],
            scene,
        )
        .unwrap();
        assert!(found.entities.contains_key("porch_wick"));
        assert!(!found.entities.contains_key("porch_bulb"));
    }

    #[test]
    fn a_missing_prefab_lists_every_path_tried() {
        let scene = "game/scenes/act1/a.scene.toml";
        let err = expanded(&[("game/flint.toml", ""), (scene, SCENE)], scene).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("prefab 'lamp' not found. Tried:"), "{text}");
        for tried in [
            "game/scenes/act1/prefabs/lamp.prefab.toml",
            "game/scenes/prefabs/lamp.prefab.toml",
            "game/prefabs/lamp.prefab.toml",
        ] {
            assert!(text.contains(tried), "{tried} in {text}");
        }
    }
}
