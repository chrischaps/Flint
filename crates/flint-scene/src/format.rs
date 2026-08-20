//! Scene file format definitions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Root structure of a scene TOML file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneFile {
    pub scene: SceneMetadata,
    #[serde(default)]
    pub camera: Option<CameraDef>,
    #[serde(default)]
    pub environment: Option<EnvironmentDef>,
    #[serde(default)]
    pub post_process: Option<PostProcessDef>,
    #[serde(default)]
    pub prefabs: HashMap<String, PrefabInstance>,
    #[serde(default)]
    pub entities: HashMap<String, EntityDef>,
}

/// A prefab template file (.prefab.toml)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefabFile {
    pub prefab: PrefabMetadata,
    #[serde(default)]
    pub entities: HashMap<String, EntityDef>,
}

/// Metadata for a prefab template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefabMetadata {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// An instance of a prefab in a scene
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrefabInstance {
    pub template: String,
    pub prefix: String,
    #[serde(default)]
    pub overrides: HashMap<String, HashMap<String, toml::Value>>,
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
    #[serde(default = "default_true")]
    pub ssao_enabled: bool,
    #[serde(default = "default_ssao_radius")]
    pub ssao_radius: f32,
    #[serde(default = "default_ssao_intensity")]
    pub ssao_intensity: f32,
    #[serde(default)]
    pub fog_enabled: bool,
    #[serde(default = "default_fog_color")]
    pub fog_color: [f32; 3],
    #[serde(default = "default_fog_density")]
    pub fog_density: f32,
    #[serde(default = "default_fog_start")]
    pub fog_start: f32,
    #[serde(default = "default_fog_end")]
    pub fog_end: f32,
    #[serde(default)]
    pub fog_height_enabled: bool,
    #[serde(default = "default_fog_height_falloff")]
    pub fog_height_falloff: f32,
    #[serde(default)]
    pub fog_height_origin: f32,
    #[serde(default)]
    pub dither_enabled: bool,
    #[serde(default = "default_dither_intensity")]
    pub dither_intensity: f32,
    #[serde(default)]
    pub volumetric_enabled: bool,
    #[serde(default = "default_volumetric_samples")]
    pub volumetric_samples: u32,
    #[serde(default = "default_volumetric_density")]
    pub volumetric_density: f32,
    #[serde(default = "default_volumetric_max_distance")]
    pub volumetric_max_distance: f32,
    #[serde(default = "default_volumetric_decay")]
    pub volumetric_decay: f32,
    #[serde(default)]
    pub chromatic_aberration: f32,
    #[serde(default)]
    pub radial_blur: f32,
    #[serde(default)]
    pub desaturate: f32,
    #[serde(default)]
    pub dof_strength: f32,
    #[serde(default = "default_dof_focus_distance")]
    pub dof_focus_distance: f32,
    #[serde(default = "default_dof_focus_range")]
    pub dof_focus_range: f32,
    #[serde(default)]
    pub kuwahara_enabled: bool,
    #[serde(default = "default_kuwahara_radius")]
    pub kuwahara_radius: u32,
    #[serde(default = "default_kuwahara_sharpness")]
    pub kuwahara_sharpness: f32,
    #[serde(default = "default_kuwahara_hardness")]
    pub kuwahara_hardness: f32,
    #[serde(default = "default_kuwahara_anisotropy")]
    pub kuwahara_anisotropy: f32,
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

fn default_ssao_radius() -> f32 {
    0.5
}

fn default_ssao_intensity() -> f32 {
    1.0
}

fn default_fog_color() -> [f32; 3] {
    [0.7, 0.75, 0.82]
}

fn default_fog_density() -> f32 {
    0.02
}

fn default_fog_start() -> f32 {
    5.0
}

fn default_fog_end() -> f32 {
    100.0
}

fn default_fog_height_falloff() -> f32 {
    0.1
}

fn default_dof_focus_distance() -> f32 {
    10.0
}

fn default_dof_focus_range() -> f32 {
    5.0
}

fn default_dither_intensity() -> f32 {
    0.03
}

fn default_volumetric_samples() -> u32 {
    32
}

fn default_volumetric_density() -> f32 {
    1.0
}

fn default_volumetric_max_distance() -> f32 {
    100.0
}

fn default_volumetric_decay() -> f32 {
    0.98
}

fn default_kuwahara_radius() -> u32 {
    4
}

fn default_kuwahara_sharpness() -> f32 {
    8.0
}

fn default_kuwahara_hardness() -> f32 {
    8.0
}

fn default_kuwahara_anisotropy() -> f32 {
    1.0
}

/// Camera configuration for the scene
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraDef {
    /// Projection type: "perspective" or "orthographic"
    #[serde(default = "default_projection")]
    pub projection: String,
    /// Orthographic half-height in world units (only used when projection = "orthographic")
    #[serde(default)]
    pub ortho_height: f32,
    /// Camera position [x, y, z]
    #[serde(default)]
    pub position: Option<[f32; 3]>,
    /// Camera look-at target [x, y, z]
    #[serde(default)]
    pub target: Option<[f32; 3]>,
    /// Field of view in degrees (perspective only)
    #[serde(default)]
    pub fov: Option<f32>,
    /// Near clipping plane
    #[serde(default)]
    pub near: Option<f32>,
    /// Far clipping plane
    #[serde(default)]
    pub far: Option<f32>,
}

fn default_projection() -> String {
    "perspective".to_string()
}

/// Environment settings for the scene (skybox, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentDef {
    /// Path to equirectangular panorama image for the skybox
    #[serde(default)]
    pub skybox: Option<String>,
    /// Hemisphere ambient sky color [r, g, b] (linear). Absent keeps the
    /// renderer's built-in default.
    #[serde(default)]
    pub ambient_sky: Option<[f32; 3]>,
    /// Hemisphere ambient ground color [r, g, b] (linear). Absent keeps the
    /// renderer's built-in default.
    #[serde(default)]
    pub ambient_ground: Option<[f32; 3]>,
    /// Diffuse terminator wrap (0 = physically sharp, ~0.2-0.5 = soft matte /
    /// subsurface-ish). Absent = 0 = exact legacy shading.
    #[serde(default)]
    pub diffuse_wrap: Option<f32>,
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
            camera: None,
            environment: None,
            post_process: None,
            prefabs: HashMap::new(),
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
