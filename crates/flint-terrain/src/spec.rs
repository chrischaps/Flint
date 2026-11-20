//! Terrain spec format — `.terrain.toml` parsing and serialization

use crate::grass_config::GrassConfig;
use flint_core::vfs;
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
    #[serde(default)]
    pub grass: Option<GrassConfig>,
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

/// Something in a spec that parses but does nothing: a key no part of the
/// spec reads, or a value the generator skips (#719).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecIssue {
    /// Where it is: `geometry.resolutoin`, `height_layers[1].octave`.
    pub path: String,
    /// What is wrong, and what was probably meant.
    pub message: String,
}

impl std::fmt::Display for SpecIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl TerrainSpec {
    /// Load a terrain spec from a `.terrain.toml` file.
    pub fn from_file(path: &Path) -> Result<Self, String> {
        Self::from_file_checked(path).map(|(spec, _)| spec)
    }

    /// Load a terrain spec and say what in it would be ignored.
    pub fn from_file_checked(path: &Path) -> Result<(Self, Vec<SpecIssue>), String> {
        let content = vfs::read_to_string(path)
            .map_err(|e| format!("Failed to read '{}': {}", path.display(), e))?;
        Self::parse_checked(&content)
            .map_err(|e| format!("Failed to parse '{}': {}", path.display(), e))
    }

    /// Parse a spec from TOML text, with every key no field reads and every
    /// value the generator skips. Serde drops an unknown key without a word,
    /// so a typo (`octave = 8`) would quietly give the default.
    pub fn parse_checked(text: &str) -> Result<(Self, Vec<SpecIssue>), String> {
        let spec: Self = toml::from_str(text).map_err(|e| e.to_string())?;
        let raw: toml::Value = toml::from_str(text).map_err(|e| e.to_string())?;
        // What the spec keeps of the text: every field it has, filled with
        // its defaults. A key the text has and this lacks was read by nothing.
        let kept = toml::Value::try_from(&spec).map_err(|e| e.to_string())?;
        let mut issues = Vec::new();
        unread_keys(&raw, &kept, "", &mut issues);
        for (i, rule) in spec.splat_rules.iter().enumerate() {
            if rule.layer > 3 {
                issues.push(SpecIssue {
                    path: format!("splat_rules[{i}].layer"),
                    message: format!(
                        "layer {} is past the splat map's four (0 to 3); the rule paints nothing",
                        rule.layer
                    ),
                });
            }
        }
        Ok((spec, issues))
    }

    /// Save the spec to a `.terrain.toml` file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let content =
            toml::to_string_pretty(self).map_err(|e| format!("Failed to serialize: {}", e))?;
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
            grass: None,
        }
    }

    /// Get layer texture paths as a 4-element array.
    pub fn layer_paths(&self) -> [String; 4] {
        [
            self.textures
                .layers
                .layer0
                .as_ref()
                .map(|l| l.path.clone())
                .unwrap_or_default(),
            self.textures
                .layers
                .layer1
                .as_ref()
                .map(|l| l.path.clone())
                .unwrap_or_default(),
            self.textures
                .layers
                .layer2
                .as_ref()
                .map(|l| l.path.clone())
                .unwrap_or_default(),
            self.textures
                .layers
                .layer3
                .as_ref()
                .map(|l| l.path.clone())
                .unwrap_or_default(),
        ]
    }
}

/// Every key under `raw` with no counterpart under `kept`, walking tables by
/// key and arrays by index.
fn unread_keys(raw: &toml::Value, kept: &toml::Value, at: &str, out: &mut Vec<SpecIssue>) {
    match (raw, kept) {
        (toml::Value::Table(raw), toml::Value::Table(kept)) => {
            for (key, value) in raw {
                let path = if at.is_empty() {
                    key.clone()
                } else {
                    format!("{at}.{key}")
                };
                match kept.get(key) {
                    Some(k) => unread_keys(value, k, &path, out),
                    None => {
                        let near = flint_core::diagnostic::did_you_mean(
                            key,
                            kept.keys().map(String::as_str),
                        );
                        let place = if at.is_empty() {
                            "a terrain spec".to_string()
                        } else {
                            format!("`{at}`")
                        };
                        out.push(SpecIssue {
                            path,
                            message: match near {
                                Some(near) => format!("unknown key in {place}; {near}"),
                                None => format!("unknown key in {place}; nothing reads it"),
                            },
                        });
                    }
                }
            }
        }
        (toml::Value::Array(raw), toml::Value::Array(kept)) => {
            for (i, (r, k)) in raw.iter().zip(kept).enumerate() {
                unread_keys(r, k, &format!("{at}[{i}]"), out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_keys_are_named_with_their_path() {
        let (spec, issues) = TerrainSpec::parse_checked(
            r#"
colour = "green"

[meta]
name = "typos"

[geometry]
resolutoin = 64

[[height_layers]]
op = "noise"
octave = 8

[[height_layers]]
op = "erosion"
talus_angle = 0.4

[[splat_rules]]
layer = 4
"#,
        )
        .unwrap();
        // The typo gave the default, which is why it has to be said
        assert_eq!(spec.geometry.resolution, 256);
        let paths: Vec<&str> = issues.iter().map(|i| i.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "colour",
                "geometry.resolutoin",
                "height_layers[0].octave",
                "splat_rules[0].layer"
            ]
        );
        assert!(issues[1].message.contains("`resolution`"), "{}", issues[1]);
        assert!(issues[2].message.contains("`octaves`"), "{}", issues[2]);
        assert!(
            issues[0].message.contains("nothing reads it"),
            "{}",
            issues[0]
        );
    }

    #[test]
    fn a_key_of_another_layer_kind_is_unknown_here() {
        let (_, issues) = TerrainSpec::parse_checked(
            "[meta]\nname = \"x\"\n[[height_layers]]\nop = \"noise\"\niterations = 9\n",
        )
        .unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].path, "height_layers[0].iterations");
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // a test walking the checkout's fixtures
    fn the_fixtures_have_nothing_unread() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/scenes/terrain");
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.to_string_lossy().ends_with(".terrain.toml") {
                let (_, issues) = TerrainSpec::from_file_checked(&path).unwrap();
                assert!(issues.is_empty(), "{}: {issues:?}", path.display());
                seen += 1;
            }
        }
        assert!(
            seen > 0,
            "no .terrain.toml in tests/fixtures/scenes/terrain/"
        );
    }

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
    fn parse_spec_with_grass() {
        let toml_str = r#"
[meta]
name = "grassy_hills"

[grass]
enabled = true
density = 12.0
max_distance = 100.0
blade_height = 0.6
wind_strength = 0.2
density_source = "splat"
density_layer = 0
density_threshold = 0.15
"#;
        let spec: TerrainSpec = toml::from_str(toml_str).unwrap();
        let grass = spec.grass.expect("grass section should be present");
        assert!(grass.enabled);
        assert_eq!(grass.density, 12.0);
        assert_eq!(grass.max_distance, 100.0);
        assert_eq!(grass.blade_height, 0.6);
        assert_eq!(grass.wind_strength, 0.2);
        assert_eq!(grass.density_layer, 0);
        assert_eq!(grass.density_threshold, 0.15);
    }

    #[test]
    fn spec_without_grass_has_none() {
        let spec = TerrainSpec::default_spec("no_grass");
        assert!(spec.grass.is_none());
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
