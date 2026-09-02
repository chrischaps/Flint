//! Texture pattern generators.
//!
//! This module provides the pattern generation layer — "base shape" producers
//! that turn texture parameters into per-pixel fields (cell IDs, heights, edge
//! distances). These fields are consumed by the map derivation stage to produce
//! albedo, normal, and roughness textures.
//!
//! Three pattern families cover the most common procedural material types:
//!
//! - **Voronoi/Brick** ([`VoronoiBrickPattern`]) — stone walls, cobblestone, cracked earth
//! - **Organic** ([`PerlinOrganicPattern`]) — layered rock, dirt, bark, natural surfaces
//! - **Grid** ([`TilingGridPattern`]) — manufactured tiles, bricks, panels

pub mod field;
pub mod grid;
pub mod maps;
pub mod node_graph;
pub mod ops;
pub mod organic;
pub mod pattern;
pub mod pipeline;
pub mod voronoi;
pub mod voronoi_util;

pub use field::TextureField;
pub use grid::{TilingGridParams, TilingGridPattern};
pub use maps::{derive_maps, TextureMapParams};
pub use ops::{OpPortInfo, TextureOp};
pub use organic::{PerlinOrganicParams, PerlinOrganicPattern};
pub use pattern::{Pattern, PatternCell, PatternField};
pub use voronoi::{VoronoiBrickParams, VoronoiBrickPattern};

use crate::generator::{GenerationCost, Generator};
use crate::output::{GeneratorOutput, OutputKind};
use crate::spec::ProcGenSpec;
use crate::{ProcGenError, Result, SeededRng};

// ─── TextureGenerator ───────────────────────────────────────────────────────

/// Procedural texture generator — produces PBR texture map sets (albedo,
/// normal, roughness) from pattern specifications.
///
/// Registered as `"texture_v1"` in the generator registry. The spec `params`
/// table is parsed via [`TextureMapParams`] for shared map-derivation settings,
/// plus pattern-specific params dispatched by the `pattern` field.
pub struct TextureGenerator;

impl Generator for TextureGenerator {
    fn type_name(&self) -> &str {
        "texture_v1"
    }

    fn output_kind(&self) -> OutputKind {
        OutputKind::Image
    }

    fn generate(&self, spec: &ProcGenSpec, rng: &mut SeededRng) -> Result<GeneratorOutput> {
        // 1. Parse shared map-derivation params
        let map_params = TextureMapParams::from_toml(&spec.params)?;

        // 2. Composable pipeline: ops define the full generation flow
        if map_params.pattern == "pipeline" {
            let mut pipeline_rng = rng.fork("pipeline");
            let maps = pipeline::run_pipeline(&spec.params, &map_params, &mut pipeline_rng)?;
            return Ok(GeneratorOutput::ImageSet(maps));
        }

        // 3. Legacy patterns: build a PatternField, then derive PBR maps
        let mut pattern_rng = rng.fork("pattern");
        let field = match map_params.pattern.as_str() {
            "voronoi_brick" => {
                let params = VoronoiBrickParams::from_toml(&spec.params)?;
                let pattern = VoronoiBrickPattern::new(params);
                pattern.generate(
                    map_params.width,
                    map_params.height,
                    map_params.seamless,
                    &mut pattern_rng,
                )
            }
            "perlin_organic" => {
                let params = PerlinOrganicParams::from_toml(&spec.params)?;
                let pattern = PerlinOrganicPattern::new(params);
                pattern.generate(
                    map_params.width,
                    map_params.height,
                    map_params.seamless,
                    &mut pattern_rng,
                )
            }
            "tiling_grid" => {
                let params = TilingGridParams::from_toml(&spec.params)?;
                let pattern = TilingGridPattern::new(params);
                pattern.generate(
                    map_params.width,
                    map_params.height,
                    map_params.seamless,
                    &mut pattern_rng,
                )
            }
            other => {
                return Err(ProcGenError::InvalidParameter {
                    name: "pattern".into(),
                    reason: format!(
                        "unknown pattern \"{other}\"; expected \"voronoi_brick\", \"perlin_organic\", \"tiling_grid\", or \"pipeline\""
                    ),
                });
            }
        };

        // 4. Derive PBR maps from the pattern field
        let mut derive_rng = rng.fork("derive_maps");
        let maps = derive_maps(&field, &map_params, &mut derive_rng);

        Ok(GeneratorOutput::ImageSet(maps))
    }

    fn param_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "width": { "type": "integer", "minimum": 1, "default": 512 },
                "height": { "type": "integer", "minimum": 1, "default": 512 },
                "pattern": {
                    "type": "string",
                    "enum": ["voronoi_brick", "perlin_organic", "tiling_grid", "pipeline"],
                    "default": "voronoi_brick"
                },
                "output_maps": {
                    "type": "array",
                    "items": { "type": "string", "enum": ["albedo", "normal", "roughness"] },
                    "default": ["albedo", "normal", "roughness"]
                },
                "seamless": { "type": "boolean", "default": true },
                "base_color": { "type": "string", "pattern": "^#?[0-9a-fA-F]{6,8}$", "default": "#735A47" },
                "color_variation": { "type": "number", "minimum": 0.0, "default": 0.06 },
                "mortar_color": { "type": "string", "pattern": "^#?[0-9a-fA-F]{6,8}$", "default": "#40382E" },
                "mortar_threshold": { "type": "number", "minimum": 0.0, "default": 0.05 },
                "roughness_base": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.75 },
                "roughness_variation": { "type": "number", "minimum": 0.0, "default": 0.05 },
                "roughness_mortar": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.5 },
                "normal_strength": { "type": "number", "minimum": 0.0, "default": 1.0 },
                "detail_scale": { "type": "number", "minimum": 0.0, "default": 8.0 },
                "detail_strength": { "type": "number", "minimum": 0.0, "default": 0.1 },
                "cell_count": { "type": "integer", "minimum": 1, "default": 12 },
                "mortar_width": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.02 },
                "height_variation": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.3 },
                "regularity": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.0 },
                "noise_type": { "type": "string", "enum": ["perlin", "simplex"], "default": "perlin" },
                "octaves": { "type": "integer", "minimum": 1, "default": 6 },
                "frequency": { "type": "number", "exclusiveMinimum": 0.0, "default": 4.0 },
                "lacunarity": { "type": "number", "exclusiveMinimum": 0.0, "default": 2.0 },
                "persistence": { "type": "number", "exclusiveMinimum": 0.0, "maximum": 1.0, "default": 0.5 },
                "height_bands": { "type": "integer", "minimum": 1, "default": 8 },
                "columns": { "type": "integer", "minimum": 1, "default": 4 },
                "rows": { "type": "integer", "minimum": 1, "default": 4 },
                "gap_width": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.05 },
                "jitter": { "type": "number", "minimum": 0.0, "maximum": 0.5, "default": 0.0 }
            },
            "required": ["pattern"],
            "additionalProperties": false
        })
    }

    fn estimate_cost(&self, spec: &ProcGenSpec) -> GenerationCost {
        let params = TextureMapParams::from_toml(&spec.params).unwrap_or_default();
        let num_maps = params.output_maps.len() as u64;
        let pixels = params.width as u64 * params.height as u64;
        let texture_bytes = pixels * 4 * num_maps;

        GenerationCost {
            estimated_vertices: 0,
            estimated_triangles: 0,
            estimated_texture_bytes: texture_bytes,
            estimated_generation_ms: pixels as f64 * num_maps as f64 * 0.0001,
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ChannelSemantics;

    fn texture_spec(pattern: &str) -> ProcGenSpec {
        ProcGenSpec::parse(&format!(
            r#"
generator = "texture_v1"
[meta]
name = "test_texture"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
[params]
width = 64
height = 64
pattern = "{pattern}"
"#
        ))
        .unwrap()
    }

    #[test]
    fn texture_generator_object_safety() {
        let _: Box<dyn Generator> = Box::new(TextureGenerator);
    }

    #[test]
    fn texture_generator_type_name_and_output_kind() {
        assert_eq!(TextureGenerator.type_name(), "texture_v1");
        assert_eq!(TextureGenerator.output_kind(), OutputKind::Image);
    }

    #[test]
    fn texture_generator_voronoi_brick_end_to_end() {
        let spec = texture_spec("voronoi_brick");
        let mut rng = SeededRng::new(42);
        let output = TextureGenerator.generate(&spec, &mut rng).unwrap();

        let images = output.as_image_set().unwrap();
        assert_eq!(images.len(), 3);
        assert_eq!(images[0].width, 64);
        assert_eq!(images[0].height, 64);
        assert_eq!(images[0].channel_semantics, ChannelSemantics::Color);
        assert_eq!(images[1].channel_semantics, ChannelSemantics::Normal);
        assert_eq!(images[2].channel_semantics, ChannelSemantics::Roughness);
        for img in images {
            assert!(img.validate().is_ok());
        }
    }

    #[test]
    fn texture_generator_perlin_organic_end_to_end() {
        let spec = texture_spec("perlin_organic");
        let mut rng = SeededRng::new(42);
        let output = TextureGenerator.generate(&spec, &mut rng).unwrap();

        let images = output.as_image_set().unwrap();
        assert_eq!(images.len(), 3);
        for img in images {
            assert_eq!(img.width, 64);
            assert!(img.validate().is_ok());
        }
    }

    #[test]
    fn texture_generator_tiling_grid_end_to_end() {
        let spec = texture_spec("tiling_grid");
        let mut rng = SeededRng::new(42);
        let output = TextureGenerator.generate(&spec, &mut rng).unwrap();

        let images = output.as_image_set().unwrap();
        assert_eq!(images.len(), 3);
        for img in images {
            assert_eq!(img.width, 64);
            assert!(img.validate().is_ok());
        }
    }

    #[test]
    fn texture_generator_determinism() {
        let spec = texture_spec("voronoi_brick");

        let mut rng_a = SeededRng::new(42);
        let output_a = TextureGenerator.generate(&spec, &mut rng_a).unwrap();

        let mut rng_b = SeededRng::new(42);
        let output_b = TextureGenerator.generate(&spec, &mut rng_b).unwrap();

        let images_a = output_a.as_image_set().unwrap();
        let images_b = output_b.as_image_set().unwrap();
        for (a, b) in images_a.iter().zip(images_b.iter()) {
            assert_eq!(
                a.pixels, b.pixels,
                "same seed should produce identical pixels"
            );
        }
    }

    #[test]
    fn texture_generator_single_map_output() {
        let spec = ProcGenSpec::parse(
            r#"
generator = "texture_v1"
[meta]
name = "test_normal_only"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
[params]
width = 32
height = 32
pattern = "voronoi_brick"
output_maps = ["normal"]
"#,
        )
        .unwrap();

        let mut rng = SeededRng::new(42);
        let output = TextureGenerator.generate(&spec, &mut rng).unwrap();

        let images = output.as_image_set().unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].channel_semantics, ChannelSemantics::Normal);
    }

    #[test]
    fn texture_generator_unknown_pattern_error() {
        let spec = ProcGenSpec::parse(
            r#"
generator = "texture_v1"
[meta]
name = "test_bad_pattern"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
[params]
pattern = "marble_veins"
"#,
        )
        .unwrap();

        let mut rng = SeededRng::new(42);
        let result = TextureGenerator.generate(&spec, &mut rng);
        assert!(result.is_err());
    }

    #[test]
    fn texture_generator_defaults_work() {
        // Minimal spec: just the pattern field
        let spec = ProcGenSpec::parse(
            r#"
generator = "texture_v1"
[meta]
name = "test_defaults"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
[params]
pattern = "tiling_grid"
"#,
        )
        .unwrap();

        let mut rng = SeededRng::new(42);
        let output = TextureGenerator.generate(&spec, &mut rng).unwrap();
        let images = output.as_image_set().unwrap();
        assert_eq!(images.len(), 3); // default output_maps = all three
        assert_eq!(images[0].width, 512); // default width
    }

    #[test]
    fn texture_generator_png_encoding_roundtrip() {
        let spec = texture_spec("voronoi_brick");
        let mut rng = SeededRng::new(42);
        let output = TextureGenerator.generate(&spec, &mut rng).unwrap();

        let images = output.as_image_set().unwrap();
        let png_bytes = images[0].to_png_bytes().unwrap();

        // Verify PNG magic bytes
        assert!(png_bytes.len() > 8);
        assert_eq!(&png_bytes[0..4], &[0x89, 0x50, 0x4E, 0x47]);
    }

    #[test]
    #[ignore]
    fn texture_generator_visual_debug_png() {
        use std::path::PathBuf;

        let dir = PathBuf::from(std::env::temp_dir()).join("flint_texture_gen_debug");
        std::fs::create_dir_all(&dir).unwrap();

        let spec = ProcGenSpec::parse(
            r##"
generator = "texture_v1"
[meta]
name = "debug_stone"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
[params]
width = 512
height = 512
pattern = "voronoi_brick"
cell_count = 24
mortar_width = 0.03
height_variation = 0.4
regularity = 0.3
base_color = "#8B6B4F"
mortar_color = "#3A3228"
"##,
        )
        .unwrap();

        let mut rng = SeededRng::new(42);
        let output = TextureGenerator.generate(&spec, &mut rng).unwrap();
        let images = output.into_image_set().unwrap();

        let names = ["albedo", "normal", "roughness"];
        for (img, name) in images.iter().zip(names.iter()) {
            let path = dir.join(format!("{name}.png"));
            img.save_png(&path).unwrap();
            println!("wrote {}", path.display());
        }
        println!("\nTexture debug PNGs written to: {}", dir.display());
    }

    #[test]
    fn texture_generator_param_schema_valid() {
        let schema = TextureGenerator.param_schema();
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["pattern"].is_object());
        assert!(schema["properties"]["width"].is_object());
        assert!(schema["properties"]["cell_count"].is_object());
        assert!(schema["properties"]["noise_type"].is_object());
        assert!(schema["properties"]["columns"].is_object());
        assert_eq!(schema["required"][0], "pattern");
        // Pipeline should be in the enum
        let patterns = &schema["properties"]["pattern"]["enum"];
        assert!(patterns.as_array().unwrap().iter().any(|v| v == "pipeline"));
    }

    #[test]
    fn texture_generator_pipeline_end_to_end() {
        let spec = ProcGenSpec::parse(
            r##"
generator = "texture_v1"
[meta]
name = "test_pipeline"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
[params]
width = 64
height = 64
pattern = "pipeline"

[[params.ops]]
type = "brick_grid"
columns = 4
rows = 8

[[params.ops]]
type = "cell_height"
variation = 0.3

[[params.ops]]
type = "cell_color"
base_color = "#8B5A3A"
variation = 0.12

[[params.ops]]
type = "derive_normal"
strength = 1.5

[[params.ops]]
type = "cell_roughness"
base = 0.75
"##,
        )
        .unwrap();

        let mut rng = SeededRng::new(42);
        let output = TextureGenerator.generate(&spec, &mut rng).unwrap();

        let images = output.as_image_set().unwrap();
        assert_eq!(images.len(), 3);
        assert_eq!(images[0].channel_semantics, ChannelSemantics::Color);
        assert_eq!(images[1].channel_semantics, ChannelSemantics::Normal);
        assert_eq!(images[2].channel_semantics, ChannelSemantics::Roughness);
        for img in images {
            assert_eq!(img.width, 64);
            assert!(img.validate().is_ok());
        }
    }

    #[test]
    fn texture_generator_estimate_cost() {
        let spec = texture_spec("voronoi_brick");
        let cost = TextureGenerator.estimate_cost(&spec);

        // 64x64 with 3 maps = 64*64*4*3 = 49152 bytes
        assert_eq!(cost.estimated_texture_bytes, 64 * 64 * 4 * 3);
        assert_eq!(cost.estimated_vertices, 0);
        assert_eq!(cost.estimated_triangles, 0);
        assert!(cost.estimated_generation_ms > 0.0);
    }
}
