//! Scene file format definitions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Root structure of a scene TOML file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneFile {
    pub scene: SceneMetadata,
    #[serde(default)]
    pub environment: Option<EnvironmentDef>,
    #[serde(default)]
    pub post_process: Option<PostProcessDef>,
    #[serde(default)]
    pub entities: HashMap<String, EntityDef>,
}

/// Post-processing settings for the scene
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostProcessDef {
    #[serde(default = "default_true")]
    pub bloom_enabled: bool,
    #[serde(default = "default_bloom_intensity")]
    pub bloom_intensity: f32,
    #[serde(default = "default_bloom_threshold")]
    pub bloom_threshold: f32,
    #[serde(default)]
    pub vignette_enabled: bool,
    #[serde(default = "default_vignette_intensity")]
    pub vignette_intensity: f32,
    #[serde(default = "default_vignette_smoothness")]
    pub vignette_smoothness: f32,
    #[serde(default = "default_exposure")]
    pub exposure: f32,
}

fn default_true() -> bool {
    true
}

fn default_bloom_intensity() -> f32 {
    0.04
}

fn default_bloom_threshold() -> f32 {
    1.0
}

fn default_vignette_intensity() -> f32 {
    0.3
}

fn default_vignette_smoothness() -> f32 {
    2.0
}

fn default_exposure() -> f32 {
    1.0
}

/// Environment settings for the scene (skybox, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentDef {
    /// Path to equirectangular panorama image for the skybox
    #[serde(default)]
    pub skybox: Option<String>,
}

/// Scene metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneMetadata {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub input_config: Option<String>,
}

fn default_version() -> String {
    "1.0".to_string()
}

/// Definition of an entity in a scene file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityDef {
    /// Optional archetype name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archetype: Option<String>,
    /// Optional parent entity name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Component data - all other fields are treated as components
    #[serde(flatten)]
    pub components: HashMap<String, toml::Value>,
}

impl EntityDef {
    pub fn new() -> Self {
        Self {
            archetype: None,
            parent: None,
            components: HashMap::new(),
        }
    }

    pub fn with_archetype(mut self, archetype: impl Into<String>) -> Self {
        self.archetype = Some(archetype.into());
        self
    }

    pub fn with_parent(mut self, parent: impl Into<String>) -> Self {
        self.parent = Some(parent.into());
        self
    }

    pub fn with_component(mut self, name: impl Into<String>, data: toml::Value) -> Self {
        self.components.insert(name.into(), data);
        self
    }
}

impl Default for EntityDef {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneFile {
    /// Create a new scene file
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            scene: SceneMetadata {
                name: name.into(),
                version: default_version(),
                description: None,
                input_config: None,
            },
            environment: None,
            post_process: None,
            entities: HashMap::new(),
        }
    }

    /// Add an entity to the scene
    pub fn add_entity(&mut self, name: impl Into<String>, entity: EntityDef) {
        self.entities.insert(name.into(), entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scene_file_serialization() {
        let mut scene = SceneFile::new("Test Scene");
        scene.add_entity(
            "door1",
            EntityDef::new()
                .with_archetype("door")
                .with_component("door", toml::toml! { locked = false }.into()),
        );

        let toml_str = toml::to_string_pretty(&scene).unwrap();
        assert!(toml_str.contains("Test Scene"));
        assert!(toml_str.contains("door1"));
    }

    #[test]
    fn test_scene_file_deserialization() {
        let toml_str = r#"
[scene]
name = "Test Scene"
version = "1.0"

[entities.room1]
archetype = "room"

[entities.room1.bounds]
min = [0, 0, 0]
max = [10, 4, 8]
"#;

        let scene: SceneFile = toml::from_str(toml_str).unwrap();
        assert_eq!(scene.scene.name, "Test Scene");
        assert!(scene.entities.contains_key("room1"));
    }
}
