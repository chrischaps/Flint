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
pub use pattern::{Pattern, PatternCell, PatternField, SeamSteps};
pub use voronoi::{VoronoiBrickParams, VoronoiBrickPattern};

use crate::generator::{GenerationCost, Generator};
use crate::output::{GeneratorOutput, OutputKind};
use crate::spec::ProcGenSpec;
use crate::{ProcGenError, Result, SeededRng};

/// The patterns that build a field and run the map derivation over it
/// (`"pipeline"` runs its ops instead).
pub const LEGACY_PATTERNS: [&str; 3] = ["voronoi_brick", "perlin_organic", "tiling_grid"];

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
        use serde_json::{json, Value};

        // `x-when` says which patterns read a key (see `param_ui::lint_params`):
        // the editor hides the rest and `flint gen` warns about them.
        let only = |patterns: &[&str], mut prop: Value| {
            prop["x-when"] = json!({ "pattern": patterns });
            prop
        };
        let voronoi = |prop| only(&["voronoi_brick"], prop);
        let organic = |prop| only(&["perlin_organic"], prop);
        let grid = |prop| only(&["tiling_grid"], prop);
        // The fixed patterns run the map derivation; a pipeline's ops replace it.
        let derive = |prop| only(&LEGACY_PATTERNS, prop);

        json!({
            "type": "object",
            "properties": {
                "width": { "type": "integer", "minimum": 1, "default": 512, "description": "Image width in pixels" },
                "height": { "type": "integer", "minimum": 1, "default": 512, "description": "Image height in pixels" },
                "pattern": {
                    "type": "string",
                    "enum": ["voronoi_brick", "perlin_organic", "tiling_grid", "pipeline"],
                    "default": "voronoi_brick",
                    "description": "The base shape: Voronoi stones, layered organic noise, a grid of tiles, or a pipeline of ops"
                },
                "output_maps": {
                    "type": "array",
                    "items": { "type": "string", "enum": ["albedo", "normal", "roughness"] },
                    "default": ["albedo", "normal", "roughness"],
                    "description": "Which PBR maps to write; each becomes <stem>_<map>.png"
                },
                "ops": only(&["pipeline"], json!({
                    "type": "array",
                    "items": pipeline::ops_items_schema(),
                    "description": "The pipeline's operations, run in order (or as a graph when an op has an `id`); each type's keys are in `op_schema`"
                })),
                "seamless": { "type": "boolean", "default": true, "description": "Tile without a seam. In a pipeline, noise and repeating ops round their frequencies to whole cycles per tile (simplex cross-fades); false keeps the unwrapped fields" },
                "base_color": derive(json!({ "type": "string", "pattern": "^#?[0-9a-fA-F]{6,8}$", "default": "#735A47", "description": "Albedo of the cells (stones, tiles, bands), sRGB hex; each cell gets its own shade of it" })),
                "color_variation": derive(json!({ "type": "number", "minimum": 0.0, "default": 0.06, "description": "How far each cell's lightness strays from base_color (saturation moves by half that; hue stays)" })),
                "mortar_color": derive(json!({ "type": "string", "pattern": "^#?[0-9a-fA-F]{6,8}$", "default": "#40382E", "description": "Albedo in the grooves between cells (strata lines on perlin_organic), sRGB hex" })),
                "mortar_threshold": derive(json!({ "type": "number", "minimum": 0.0, "description": "distance_to_edge below which mortar colour and roughness blend in; default 0.5 (half the groove) for voronoi_brick and tiling_grid, 0.05 (strata lines) for perlin_organic" })),
                "roughness_base": derive(json!({ "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.75, "description": "Roughness of the cells' faces" })),
                "roughness_variation": derive(json!({ "type": "number", "minimum": 0.0, "default": 0.05, "description": "Per-cell random spread of roughness" })),
                "roughness_mortar": derive(json!({ "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.5, "description": "Roughness in the grooves" })),
                "normal_strength": derive(json!({ "type": "number", "minimum": 0.0, "default": 1.0, "description": "Depth of the normal map's relief; 0 is flat" })),
                "detail_scale": derive(json!({ "type": "number", "minimum": 0.0, "default": 8.0, "description": "Detail lattice cells across the texture width (first octave)" })),
                "detail_octaves": derive(json!({ "type": "integer", "minimum": 1, "default": 4, "description": "Octaves in the detail field, each twice as fine; any finer than 4 px are dropped" })),
                "detail_strength": derive(json!({ "type": "number", "minimum": 0.0, "default": 0.1, "description": "RMS brightness change in the albedo (0.1 = 10%); also scales the detail's relief and roughness" })),
                "cell_count": voronoi(json!({ "type": "integer", "minimum": 1, "default": 12, "description": "Voronoi cells (stones) across the whole texture" })),
                "cell_size": voronoi(json!({ "type": "number", "exclusiveMinimum": 0.0, "description": "Typical stone size in pixels; overrides cell_count" })),
                "irregularity": voronoi(json!({ "type": "number", "minimum": 0.0, "maximum": 1.0, "description": "1 - regularity; give one or the other" })),
                "mortar_width": voronoi(json!({ "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.02, "description": "Full width of the groove between stones, as a fraction of the texture width" })),
                "regularity": voronoi(json!({ "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.0, "description": "0 scatters the stones at random, 1 relaxes them into an even layout" })),
                "height_variation": only(&["voronoi_brick", "tiling_grid"], json!({ "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.3, "description": "Per-cell random height, 0 flat to 1 the most; shows in the normal map" })),
                "noise_type": organic(json!({ "type": "string", "enum": ["perlin", "simplex"], "default": "perlin", "description": "Noise basis: perlin, or simplex (fewer grid artefacts)" })),
                "octaves": organic(json!({ "type": "integer", "minimum": 1, "default": 6, "description": "Noise octaves layered into the bands; more adds finer detail" })),
                "frequency": organic(json!({ "type": "number", "exclusiveMinimum": 0.0, "default": 4.0, "description": "Noise lattice cells across the texture for the first octave; rounded to a whole number when seamless" })),
                "lacunarity": organic(json!({ "type": "number", "exclusiveMinimum": 0.0, "default": 2.0, "description": "Frequency multiplier from one octave to the next" })),
                "persistence": organic(json!({ "type": "number", "exclusiveMinimum": 0.0, "maximum": 1.0, "default": 0.5, "description": "Amplitude multiplier from one octave to the next; lower is smoother" })),
                "height_bands": organic(json!({ "type": "integer", "minimum": 1, "default": 8, "description": "Bands the noise height is cut into; each band is a cell with its own shade" })),
                "columns": grid(json!({ "type": "integer", "minimum": 1, "default": 4, "description": "Tiles across" })),
                "rows": grid(json!({ "type": "integer", "minimum": 1, "default": 4, "description": "Tiles down" })),
                "gap_width": grid(json!({ "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.05, "description": "Grout between tiles, as a fraction of a tile" })),
                "jitter": grid(json!({ "type": "number", "minimum": 0.0, "maximum": 0.5, "default": 0.0, "description": "How far each tile strays from the grid, 0 none to 0.5" })),
                "noise_strength": grid(json!({ "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.03, "description": "Fine surface noise on each tile's height" }))
            },
            "additionalProperties": false
        })
    }

    fn estimate_cost(&self, spec: &ProcGenSpec) -> GenerationCost {
        let params = TextureMapParams::from_toml(&spec.params).unwrap_or_default();
        let num_maps = params.output_maps.len() as u64;
        let pixels = params.width as u64 * params.height as u64;
        let texture_bytes = pixels * 4 * num_maps;

        // Fitted to release builds of the shipped specs (2026-10-31, desktop
        // CPU): the fixed patterns cost ~150 ns per pixel per map, and a
        // pipeline ~90 ns per pixel per op, whatever it outputs. Within ±40%
        // on all 16; the old flat 100 ns/pixel/map was 4x low on pipelines.
        let ms = if params.pattern == "pipeline" {
            let ops = spec
                .params
                .get("ops")
                .and_then(|o| o.as_array())
                .map_or(0, |o| o.len());
            pixels as f64 * (ops as f64 * 0.000_09 + num_maps as f64 * 0.000_01)
        } else {
            pixels as f64 * num_maps as f64 * 0.000_15
        };

        GenerationCost {
            estimated_vertices: 0,
            estimated_triangles: 0,
            estimated_texture_bytes: texture_bytes,
            estimated_generation_ms: ms,
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
        // `pattern` has a default, so it isn't required.
        assert!(schema.get("required").is_none());
        assert_eq!(schema["properties"]["pattern"]["default"], "voronoi_brick");
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

    // ── Task 4.3 acceptance: registry, PNGs, routing, a schema that's true ──

    fn repo_root() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn registry() -> crate::GeneratorRegistry {
        let mut reg = crate::GeneratorRegistry::new();
        crate::register_built_in_generators(&mut reg);
        reg
    }

    fn set(spec: &mut ProcGenSpec, key: &str, value: toml::Value) {
        spec.params
            .as_table_mut()
            .unwrap()
            .insert(key.into(), value);
    }

    #[test]
    fn registry_resolves_and_runs_the_texture_generator() {
        let reg = registry();
        let gen = reg.get("texture_v1").expect("texture_v1 is registered");
        assert_eq!(gen.output_kind(), OutputKind::Image);
        let out = reg
            .generate_from_spec(&texture_spec("tiling_grid"))
            .unwrap();
        assert_eq!(out.as_image_set().unwrap().len(), 3);
    }

    /// The ticket's end-to-end path: `specs/stone_wall.procgen.toml` ->
    /// registry -> PNG files on disk -> decoded back at the spec's size.
    #[test]
    fn stone_wall_spec_to_png_files() {
        let mut spec =
            ProcGenSpec::from_file(&repo_root().join("specs/stone_wall.procgen.toml")).unwrap();
        // Smaller than the shipped 512 so the debug test stays quick.
        set(&mut spec, "width", 96.into());
        set(&mut spec, "height", 80.into());
        let images = registry()
            .generate_from_spec(&spec)
            .unwrap()
            .into_image_set()
            .unwrap();
        assert_eq!(images.len(), 3);

        let dir = std::env::temp_dir().join(format!("flint_tex_e2e_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for img in &images {
            let path = dir.join(format!("{:?}.png", img.channel_semantics));
            img.save_png(&path).unwrap();
            assert!(std::fs::metadata(&path).unwrap().len() > 0);
            let decoded = image::open(&path).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (96, 80));
            assert_eq!(decoded.to_rgba8().into_raw(), img.pixels, "PNG is lossless");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn same_spec_and_seed_give_identical_png_bytes() {
        for pattern in ["voronoi_brick", "perlin_organic", "tiling_grid"] {
            let spec = texture_spec(pattern);
            let png = |seed| {
                TextureGenerator
                    .generate(&spec, &mut SeededRng::new(seed))
                    .unwrap()
                    .into_image_set()
                    .unwrap()
                    .iter()
                    .map(|i| i.to_png_bytes().unwrap())
                    .collect::<Vec<_>>()
            };
            assert_eq!(png(42), png(42), "{pattern}: same seed, different bytes");
            assert_ne!(png(42), png(43), "{pattern}: the seed changes nothing");
        }
    }

    #[test]
    fn each_pattern_routes_to_its_own_generator() {
        let albedo = |spec: &ProcGenSpec| {
            TextureGenerator
                .generate(spec, &mut SeededRng::new(42))
                .unwrap()
                .into_image_set()
                .unwrap()
                .remove(0)
                .pixels
        };
        let v = albedo(&texture_spec("voronoi_brick"));
        let o = albedo(&texture_spec("perlin_organic"));
        let g = albedo(&texture_spec("tiling_grid"));
        assert!(v != o && o != g && v != g);
        // A spec with no pattern runs the schema's default.
        let mut none = texture_spec("voronoi_brick");
        none.params.as_table_mut().unwrap().remove("pattern");
        assert_eq!(albedo(&none), v);
    }

    /// A value different from `current` (or from the schema default) that
    /// the parameter's own range still accepts.
    fn nudge(prop: &serde_json::Value, current: Option<&toml::Value>) -> toml::Value {
        let max = prop["maximum"].as_f64();
        match prop["type"].as_str().unwrap() {
            "boolean" => {
                let now = current
                    .and_then(|v| v.as_bool())
                    .or(prop["default"].as_bool())
                    .unwrap_or(false);
                (!now).into()
            }
            "integer" => {
                let now = current
                    .and_then(|v| v.as_integer())
                    .or(prop["default"].as_i64())
                    .unwrap_or(1);
                let min = prop["minimum"].as_i64().unwrap_or(0);
                // Down, where allowed: more octaves than the size can hold
                // are dropped, so "up" may be no change at all.
                if now > min { now - 1 } else { now + 3 }.into()
            }
            "number" => {
                let unset = if max.is_some() { 0.3 } else { 12.0 };
                let now = current
                    .and_then(|v| v.as_float())
                    .or(prop["default"].as_f64());
                if now.is_none() {
                    return unset.into();
                }
                let now = now.unwrap();
                let up = now * 1.5 + 0.1;
                match max {
                    Some(m) if up > m => (now * 0.5).into(),
                    _ => up.into(),
                }
            }
            "string" if prop["enum"].is_array() => {
                let now = current
                    .and_then(|v| v.as_str())
                    .or(prop["default"].as_str())
                    .unwrap();
                let other = prop["enum"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|v| v.as_str() != Some(now))
                    .unwrap();
                other.as_str().unwrap().into()
            }
            "string" => "#20A0C0".into(),
            t => panic!("no nudge for {t}"),
        }
    }

    /// The schema's `x-when` is a promise: a key it says the pattern reads
    /// changes the output, and a key it says is ignored changes nothing.
    /// Checked by nudging every key under every pattern.
    #[test]
    fn schema_says_exactly_which_keys_each_pattern_reads() {
        let schema = TextureGenerator.param_schema();
        let props = schema["properties"].as_object().unwrap();
        let pipeline = ProcGenSpec::parse(
            r##"
generator = "texture_v1"
[meta]
name = "nudge_pipeline"
version = "1.0.0"
[seed]
mode = "fixed"
value = 7
[params]
width = 48
height = 40
pattern = "pipeline"
[[params.ops]]
type = "brick_grid"
[[params.ops]]
type = "cell_color"
[[params.ops]]
type = "noise_layer"
frequency = 2.5
[[params.ops]]
type = "blend"
target = "height"
[[params.ops]]
type = "derive_normal"
"##,
        )
        .unwrap();

        for pattern in ["voronoi_brick", "perlin_organic", "tiling_grid", "pipeline"] {
            let base = if pattern == "pipeline" {
                pipeline.clone()
            } else {
                let mut s = texture_spec(pattern);
                set(&mut s, "width", 48.into());
                set(&mut s, "height", 40.into());
                // Two detail octaves (24 and 12 px) fit at this size, so
                // dropping one is visible.
                set(&mut s, "detail_scale", 2.0.into());
                set(&mut s, "detail_octaves", 2.into());
                if pattern == "tiling_grid" {
                    // At 48 px the default 5% gap is under a pixel: no mortar
                    // pixels, so nothing for mortar_color to paint.
                    set(&mut s, "gap_width", 0.2.into());
                }
                s
            };
            let run = |spec: &ProcGenSpec| {
                TextureGenerator
                    .generate(spec, &mut SeededRng::new(7))
                    .unwrap()
                    .into_image_set()
                    .unwrap()
            };
            let reference = run(&base);
            for (key, prop) in props {
                if ["width", "height", "pattern", "output_maps", "ops"].contains(&key.as_str()) {
                    continue;
                }
                let mut spec = base.clone();
                let value = nudge(prop, spec.params.get(key));
                set(&mut spec, key, value.clone());
                let applies = crate::parse_param_schema(&schema)
                    .iter()
                    .find(|f| f.name == *key)
                    .unwrap()
                    .applies(&schema, spec.params.as_table().unwrap());
                let changed = run(&spec) != reference;
                assert_eq!(
                    changed,
                    applies,
                    "{pattern}: {key} = {value} {} the output, but the schema says it {}",
                    if changed { "changed" } else { "did not change" },
                    if applies { "applies" } else { "is ignored" }
                );
            }
        }
    }

    /// Every spec the repo ships, for every generator, uses only keys its
    /// generator reads.
    #[test]
    fn shipped_specs_lint_clean() {
        let reg = registry();
        let root = repo_root();
        let mut checked = 0;
        let mut problems = Vec::new();
        for dir in [
            root.clone(),
            root.join("specs"),
            root.join("specs/examples"),
            root.join("showcase_game/specs"),
            root.join("crates/flint-procgen-ai/templates"),
        ] {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if !path.to_string_lossy().ends_with(".procgen.toml") {
                    continue;
                }
                let spec = ProcGenSpec::from_file(&path).unwrap();
                let gen = reg.get(&spec.generator).unwrap();
                for issue in crate::lint_params(&gen.param_schema(), &spec.params) {
                    problems.push(format!("{}: {issue}", path.display()));
                }
                checked += 1;
            }
        }
        assert!(checked >= 32, "found only {checked} specs");
        assert!(problems.is_empty(), "\n{}", problems.join("\n"));
    }

    #[test]
    fn estimate_cost_counts_pipeline_ops() {
        let short = ProcGenSpec::parse(
            "generator = \"texture_v1\"\n[meta]\nname = \"a\"\nversion = \"1\"\n[seed]\nmode = \"fixed\"\nvalue = 1\n[params]\npattern = \"pipeline\"\n[[params.ops]]\ntype = \"brick_grid\"\n",
        )
        .unwrap();
        let mut long = short.clone();
        let ops = long.params["ops"].as_array().unwrap().clone();
        let tripled: Vec<_> = ops.iter().cycle().take(3).cloned().collect();
        set(&mut long, "ops", tripled.into());
        let a = TextureGenerator
            .estimate_cost(&short)
            .estimated_generation_ms;
        let b = TextureGenerator
            .estimate_cost(&long)
            .estimated_generation_ms;
        assert!(
            b > 2.0 * a,
            "3 ops ({b} ms) should cost well over 1 op ({a} ms)"
        );
    }

    // ── Task 4.1 acceptance: every pattern, the same bar ────────────────────

    fn all_patterns() -> Vec<(&'static str, Box<dyn Pattern>)> {
        vec![
            (
                "voronoi_brick",
                Box::new(VoronoiBrickPattern::new(VoronoiBrickParams::default())),
            ),
            (
                "voronoi_cell_size",
                Box::new(VoronoiBrickPattern::new(VoronoiBrickParams {
                    cell_size: Some(24.0),
                    regularity: 0.5,
                    ..Default::default()
                })),
            ),
            (
                "perlin_organic",
                Box::new(PerlinOrganicPattern::new(PerlinOrganicParams::default())),
            ),
            (
                "simplex_organic",
                Box::new(PerlinOrganicPattern::new(PerlinOrganicParams {
                    noise_type: "simplex".into(),
                    ..Default::default()
                })),
            ),
            (
                "tiling_grid",
                Box::new(TilingGridPattern::new(TilingGridParams::default())),
            ),
            (
                "tiling_grid_jitter",
                Box::new(TilingGridPattern::new(TilingGridParams {
                    jitter: 0.35,
                    ..Default::default()
                })),
            ),
        ]
    }

    #[test]
    fn every_pattern_is_valid_deterministic_and_seamless() {
        use pattern::assert_valid;
        for (name, pattern) in all_patterns() {
            for (w, h) in [(128u32, 128u32), (160, 96)] {
                for seed in [3u64, 42] {
                    let a = pattern.generate(w, h, true, &mut SeededRng::new(seed));
                    let b = pattern.generate(w, h, true, &mut SeededRng::new(seed));
                    assert_valid(&a, w, h);
                    assert_eq!(
                        a.cells, b.cells,
                        "{name} {w}x{h} seed {seed}: not deterministic"
                    );
                    let s = a.seam_steps();
                    assert!(s.ratio() < 1.25, "{name} {w}x{h} seed {seed}: seam {s:?}");
                    // Non-seamless output is valid too.
                    assert_valid(
                        &pattern.generate(w, h, false, &mut SeededRng::new(seed)),
                        w,
                        h,
                    );
                }
            }
        }
    }

    /// `cargo test -p flint-procgen --lib -- --ignored pattern_debug_pngs`
    /// writes each pattern's height, cells and edges, and a 2 x 2 repeat of
    /// the height to show the tile has no seam, to `target/test-output/`.
    #[test]
    #[ignore]
    fn pattern_debug_pngs() {
        use pattern::write_debug_png;
        for (name, pattern) in all_patterns() {
            let f = pattern.generate(256, 256, true, &mut SeededRng::new(42));
            write_debug_png(&format!("{name}_height"), &f.to_height_image());
            write_debug_png(&format!("{name}_cells"), &f.to_cell_id_image());
            write_debug_png(&format!("{name}_edges"), &f.to_edge_distance_image());
            let one = f.to_height_image();
            let mut four = crate::types::ImageData::new(512, 512, ChannelSemantics::Height);
            for y in 0..512u32 {
                for x in 0..512u32 {
                    let src = (((y % 256) * 256 + x % 256) * 4) as usize;
                    let dst = ((y * 512 + x) * 4) as usize;
                    four.pixels[dst..dst + 4].copy_from_slice(&one.pixels[src..src + 4]);
                }
            }
            write_debug_png(&format!("{name}_height_2x2"), &four);
        }
    }
}
