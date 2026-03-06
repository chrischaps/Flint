//! Terrain spec format — `.terrain.toml` parsing and serialization

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Top-level terrain specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerrainSpec {
    pub meta: MetaConfig,
    #[serde(default)]
    pub seed: SeedConfig,
    #[serde(default)]
    pub geometry: GeometryConfig,
    #[serde(default)]
    pub textures: TextureConfig,
    #[serde(default)]
    pub height_layers: Vec<HeightLayer>,
    #[serde(default)]
    pub splat_rules: Vec<SplatRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaConfig {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
}

fn default_version() -> String {
    "1.0.0".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedConfig {
    #[serde(default = "default_seed_mode")]
    pub mode: SeedMode,
    #[serde(default = "default_seed_value")]
    pub value: u64,
}

impl Default for SeedConfig {
    fn default() -> Self {
        Self {
            mode: SeedMode::Fixed,
            value: 42,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeedMode {
    Fixed,
    Random,
}

fn default_seed_mode() -> SeedMode {
    SeedMode::Fixed
}

fn default_seed_value() -> u64 {
    42
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeometryConfig {
    #[serde(default = "default_256f")]
    pub width: f32,
    #[serde(default = "default_256f")]
    pub depth: f32,
    #[serde(default = "default_50f")]
    pub height_scale: f32,
    #[serde(default = "default_256u")]
    pub resolution: u32,
    #[serde(default = "default_64u")]
    pub chunk_resolution: u32,
}

impl Default for GeometryConfig {
    fn default() -> Self {
        Self {
            width: 256.0,
            depth: 256.0,
            height_scale: 50.0,
            resolution: 256,
            chunk_resolution: 64,
        }
    }
}

fn default_256f() -> f32 {
    256.0
}
fn default_50f() -> f32 {
    50.0
}
fn default_256u() -> u32 {
    256
}
fn default_64u() -> u32 {
    64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextureConfig {
    #[serde(default = "default_texture_tile")]
    pub texture_tile: f32,
    #[serde(default)]
    pub metallic: f32,
    #[serde(default = "default_roughness")]
    pub roughness: f32,
    #[serde(default)]
    pub layers: TextureLayers,
}

impl Default for TextureConfig {
    fn default() -> Self {
        Self {
            texture_tile: 16.0,
            metallic: 0.0,
            roughness: 0.85,
            layers: TextureLayers::default(),
        }
    }
}

fn default_texture_tile() -> f32 {
    16.0
}
fn default_roughness() -> f32 {
    0.85
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TextureLayers {
    pub layer0: Option<TextureLayer>,
    pub layer1: Option<TextureLayer>,
    pub layer2: Option<TextureLayer>,
    pub layer3: Option<TextureLayer>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextureLayer {
    pub path: String,
    #[serde(default)]
    pub label: String,
}

/// A procedural height generation layer
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum HeightLayer {
    Noise(NoiseLayer),
    Flatten(FlattenLayer),
    Erosion(ErosionLayer),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoiseLayer {
    #[serde(default = "default_noise_type")]
    pub noise_type: NoiseType,
    #[serde(default = "default_6u")]
    pub octaves: u32,
    #[serde(default = "default_2f")]
    pub frequency: f64,
    #[serde(default = "default_2f")]
    pub lacunarity: f64,
    #[serde(default = "default_half")]
    pub persistence: f64,
    #[serde(default = "default_1f")]
    pub amplitude: f64,
    #[serde(default)]
    pub blend: BlendMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoiseType {
    Perlin,
    Simplex,
    Value,
    Ridged,
}

fn default_noise_type() -> NoiseType {
    NoiseType::Perlin
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlendMode {
    #[default]
    Add,
    Multiply,
    Max,
    Min,
    Overlay,
}

fn default_6u() -> u32 {
    6
}
fn default_2f() -> f64 {
    2.0
}
fn default_half() -> f64 {
    0.5
}
fn default_1f() -> f64 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlattenLayer {
    #[serde(default = "default_flatten_height")]
    pub height: f64,
    #[serde(default = "default_flatten_falloff")]
    pub falloff: f64,
    #[serde(default)]
    pub mode: FlattenMode,
}

fn default_flatten_height() -> f64 {
    0.1
}
fn default_flatten_falloff() -> f64 {
    0.05
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlattenMode {
    #[default]
    Below,
    Above,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErosionLayer {
    #[serde(default)]
    pub kind: ErosionKind,
    #[serde(default = "default_erosion_iterations")]
    pub iterations: u32,
    #[serde(default = "default_talus")]
    pub talus_angle: f64,
    #[serde(default = "default_erosion_strength")]
    pub strength: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ErosionKind {
    #[default]
    Thermal,
    Hydraulic,
}

fn default_erosion_iterations() -> u32 {
    50
}
fn default_talus() -> f64 {
    0.6
}
fn default_erosion_strength() -> f64 {
    0.3
}

/// Rule for automatic splat map generation based on height/slope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplatRule {
    pub layer: u8,
    #[serde(default)]
    pub height_min: f32,
    #[serde(default = "default_1f32")]
    pub height_max: f32,
    #[serde(default = "default_1f32")]
    pub slope_max: f32,
    #[serde(default)]
    pub noise_amount: f32,
    #[serde(default = "default_4f32")]
    pub noise_scale: f32,
}

fn default_1f32() -> f32 {
    1.0
}
fn default_4f32() -> f32 {
    4.0
}

impl TerrainSpec {
    /// Load a terrain spec from a `.terrain.toml` file.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read '{}': {}", path.display(), e))?;
        toml::from_str(&content)
            .map_err(|e| format!("Failed to parse '{}': {}", path.display(), e))
    }

    /// Save the spec to a `.terrain.toml` file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize: {}", e))?;
        std::fs::write(path, content)
            .map_err(|e| format!("Failed to write '{}': {}", path.display(), e))
    }

    /// Create a minimal default spec.
    pub fn default_spec(name: &str) -> Self {
        Self {
            meta: MetaConfig {
                name: name.to_string(),
                version: "1.0.0".to_string(),
            },
            seed: SeedConfig::default(),
            geometry: GeometryConfig::default(),
            textures: TextureConfig::default(),
            height_layers: vec![HeightLayer::Noise(NoiseLayer {
                noise_type: NoiseType::Perlin,
                octaves: 6,
                frequency: 2.0,
                lacunarity: 2.0,
                persistence: 0.5,
                amplitude: 1.0,
                blend: BlendMode::Add,
            })],
            splat_rules: Vec::new(),
        }
    }

    /// Get layer texture paths as a 4-element array.
    pub fn layer_paths(&self) -> [String; 4] {
        [
            self.textures.layers.layer0.as_ref().map(|l| l.path.clone()).unwrap_or_default(),
            self.textures.layers.layer1.as_ref().map(|l| l.path.clone()).unwrap_or_default(),
            self.textures.layers.layer2.as_ref().map(|l| l.path.clone()).unwrap_or_default(),
            self.textures.layers.layer3.as_ref().map(|l| l.path.clone()).unwrap_or_default(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_serialize() {
        let spec = TerrainSpec::default_spec("test_hills");
        let toml_str = toml::to_string_pretty(&spec).unwrap();
        let parsed: TerrainSpec = toml::from_str(&toml_str).unwrap();

        assert_eq!(parsed.meta.name, "test_hills");
        assert_eq!(parsed.geometry.width, 256.0);
        assert_eq!(parsed.height_layers.len(), 1);
    }

    #[test]
    fn parse_minimal_spec() {
        let toml_str = r#"
[meta]
name = "minimal"

[[height_layers]]
op = "noise"
"#;
        let spec: TerrainSpec = toml::from_str(toml_str).unwrap();
        assert_eq!(spec.meta.name, "minimal");
        assert_eq!(spec.geometry.resolution, 256); // default
        assert_eq!(spec.height_layers.len(), 1);
    }

    #[test]
    fn parse_all_layer_types() {
        let toml_str = r#"
[meta]
name = "all_layers"

[[height_layers]]
op = "noise"
noise_type = "ridged"
octaves = 4
frequency = 3.0

[[height_layers]]
op = "flatten"
height = 0.2
mode = "above"

[[height_layers]]
op = "erosion"
kind = "hydraulic"
iterations = 100

[[splat_rules]]
layer = 0
height_min = 0.0
height_max = 0.5
"#;
        let spec: TerrainSpec = toml::from_str(toml_str).unwrap();
        assert_eq!(spec.height_layers.len(), 3);
        assert_eq!(spec.splat_rules.len(), 1);

        match &spec.height_layers[0] {
            HeightLayer::Noise(n) => {
                assert_eq!(n.noise_type, NoiseType::Ridged);
                assert_eq!(n.octaves, 4);
            }
            _ => panic!("Expected noise layer"),
        }
        match &spec.height_layers[1] {
            HeightLayer::Flatten(f) => assert_eq!(f.mode, FlattenMode::Above),
            _ => panic!("Expected flatten layer"),
        }
        match &spec.height_layers[2] {
            HeightLayer::Erosion(e) => assert_eq!(e.kind, ErosionKind::Hydraulic),
            _ => panic!("Expected erosion layer"),
        }
    }

    #[test]
    fn save_and_load_round_trip() {
        let spec = TerrainSpec::default_spec("save_test");
        let tmp = std::env::temp_dir().join("flint_test_spec.terrain.toml");
        spec.save(&tmp).unwrap();

        let loaded = TerrainSpec::from_file(&tmp).unwrap();
        assert_eq!(loaded.meta.name, "save_test");
        assert_eq!(loaded.seed.value, 42);

        let _ = std::fs::remove_file(&tmp);
    }
}
