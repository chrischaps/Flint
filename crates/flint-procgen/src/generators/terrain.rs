//! `terrain_v1`: a terrain spec's heightmap and splat map, as a procgen
//! generator (#1168).
//!
//! The maps are `flint_terrain::bake`'s, the one bake the terrain editor and
//! `flint gen` run, so this generator adds nothing to what a terrain looks
//! like: it puts terrain inside the procgen system, where the parameter
//! schema, `flint gen --describe`, `--register`, `--validate`, the disk
//! cache and the AI steps already are.
//!
//! **Two spellings of one spec.** A `.terrain.toml` (the terrain editor's
//! file: `[meta]`, `[seed]`, `[geometry]`, `[[height_layers]]`,
//! `[[splat_rules]]`, `[textures]`, `[grass]`) loads as a `terrain_v1`
//! [`ProcGenSpec`] through [`from_terrain_toml`]: `[meta]` and `[seed]` stay
//! where they are and every other table moves under `[params]`. A
//! `*.procgen.toml` with `generator = "terrain_v1"` is written that way
//! already. [`terrain_spec`] maps either back to the `TerrainSpec` the bake
//! takes.
//!
//! **The output** is an image set of two: the heightmap as 16-bit grey
//! ([`PixelFormat::L16`](crate::PixelFormat), the samples
//! [`flint_terrain::height_samples`] makes, so its PNG is
//! `flint_terrain::encode_heightmap_png`'s byte for byte) with
//! [`ChannelSemantics::Height`], then the RGBA8 splat map with
//! [`ChannelSemantics::Splat`]. `flint gen` writes them as `<stem>_height.png`
//! and `<stem>_splat.png`.
//!
//! `[textures]` and `[grass]` are the terrain component's, not the bake's:
//! the schema describes them, so a typo in them is still reported, but
//! nothing here reads them.

use flint_terrain::{GrassConfig, TerrainSpec};
use serde_json::{json, Value as Json};

use crate::generator::{GenerationCost, Generator};
use crate::output::{GeneratorOutput, OutputKind};
use crate::spec::{ProcGenSpec, SeedConfig, SeedMode, SpecMeta};
use crate::types::{ChannelSemantics, ImageData};
use crate::{ProcGenError, Result, SeededRng};

/// The terrain generator. See the module docs.
pub struct TerrainGenerator;

/// The generator's registered name.
pub const TERRAIN_GENERATOR: &str = "terrain_v1";

/// The ending of a terrain editor's spec file.
pub const TERRAIN_SUFFIX: &str = ".terrain.toml";

/// The widest map a spec may ask for: 4096² is 32 MB of heights and 64 MB
/// of splat, and an agent's typo shouldn't ask for more.
const MAX_RESOLUTION: u32 = 4096;

/// The `[params]` keys a terrain spec has, besides `[meta]` and `[seed]`.
const PARAM_TABLES: [&str; 5] = [
    "geometry",
    "height_layers",
    "splat_rules",
    "textures",
    "grass",
];

/// Whether `path` names a terrain editor's spec (`x.terrain.toml`).
pub fn is_terrain_spec(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(TERRAIN_SUFFIX)
}

/// A `.terrain.toml`'s text as a `terrain_v1` spec: `[meta]` and `[seed]`
/// as a procgen spec has them (a terrain spec's seed defaults to fixed 42,
/// its version to 1.0.0), every other top-level key under `[params]`, where
/// the schema reports any it doesn't know.
pub fn from_terrain_toml(text: &str) -> Result<ProcGenSpec> {
    let parse = |e: String| ProcGenError::SpecParseError(e);
    let raw: toml::Table = toml::from_str(text).map_err(|e| parse(e.to_string()))?;
    let mut meta = None;
    let mut seed = None;
    let mut params = toml::Table::new();
    for (key, value) in raw {
        match key.as_str() {
            "meta" => meta = Some(value),
            "seed" => seed = Some(value),
            _ => {
                params.insert(key, value);
            }
        }
    }

    // What the terrain format itself says of the two, read by its own types
    let terrain_meta: flint_terrain::spec::MetaConfig = meta
        .ok_or_else(|| parse("missing table `[meta]`".into()))?
        .try_into()
        .map_err(|e: toml::de::Error| parse(format!("[meta]: {e}")))?;
    let terrain_seed: flint_terrain::spec::SeedConfig = match seed {
        Some(s) => s
            .try_into()
            .map_err(|e: toml::de::Error| parse(format!("[seed]: {e}")))?,
        None => Default::default(),
    };
    let fixed = terrain_seed.mode == flint_terrain::spec::SeedMode::Fixed;
    Ok(ProcGenSpec {
        meta: SpecMeta {
            name: terrain_meta.name,
            description: None,
            version: terrain_meta.version,
            tags: vec!["terrain".to_string()],
        },
        generator: TERRAIN_GENERATOR.to_string(),
        seed: SeedConfig {
            mode: if fixed {
                SeedMode::Fixed
            } else {
                SeedMode::Random
            },
            value: fixed.then_some(terrain_seed.value),
            derive_from: None,
        },
        params: toml::Value::Table(params),
        lod: None,
        living: None,
    })
}

/// Everything `flint gen` would say of a `.terrain.toml`'s params: each key
/// the bake won't read and each value it can't use, by path. What the
/// terrain editor shows in its status bar, so the two say the same.
pub fn check_terrain_toml(text: &str) -> Result<Vec<crate::ParamIssue>> {
    let spec = from_terrain_toml(text)?;
    Ok(crate::validate_params(&param_schema(), &spec.params))
}

/// The `TerrainSpec` a `terrain_v1` spec bakes: its params' tables under the
/// terrain format's own names, with `seed` as its fixed seed. A value the
/// terrain types can't hold (a wrong type, an unknown `op`) is
/// [`ProcGenError::InvalidParameter`]; [`crate::validate_params`] names the
/// same problems by path first.
pub fn terrain_spec(spec: &ProcGenSpec, seed: u64) -> Result<TerrainSpec> {
    let mut table = toml::Table::new();
    if let Some(params) = spec.params.as_table() {
        for key in PARAM_TABLES {
            if let Some(v) = params.get(key) {
                table.insert(key.to_string(), v.clone());
            }
        }
    }
    let mut meta = toml::Table::new();
    meta.insert("name".into(), spec.meta.name.clone().into());
    meta.insert("version".into(), spec.meta.version.clone().into());
    table.insert("meta".into(), toml::Value::Table(meta));
    let mut terrain: TerrainSpec =
        toml::Value::Table(table)
            .try_into()
            .map_err(|e: toml::de::Error| ProcGenError::InvalidParameter {
                name: "params".into(),
                reason: e.message().to_string(),
            })?;
    terrain.seed.mode = flint_terrain::spec::SeedMode::Fixed;
    terrain.seed.value = seed;
    Ok(terrain)
}

/// The resolution a spec bakes at, as the bake clamps it, without baking.
fn resolution_of(spec: &ProcGenSpec) -> u32 {
    let asked = spec
        .params
        .get("geometry")
        .and_then(|g| g.get("resolution"))
        .and_then(toml::Value::as_integer)
        .unwrap_or(256);
    asked.clamp(2, i64::from(u32::MAX)) as u32
}

impl Generator for TerrainGenerator {
    fn type_name(&self) -> &str {
        TERRAIN_GENERATOR
    }

    fn revision(&self) -> u32 {
        1
    }

    fn output_kind(&self) -> OutputKind {
        OutputKind::Image
    }

    fn generate(&self, spec: &ProcGenSpec, rng: &mut SeededRng) -> Result<GeneratorOutput> {
        let seed = rng.seed();
        let terrain = terrain_spec(spec, seed)?;
        let res = flint_terrain::bake::map_resolution(&terrain);
        if res > MAX_RESOLUTION {
            return Err(ProcGenError::BudgetExceeded(format!(
                "geometry.resolution {res} is over {MAX_RESOLUTION}"
            )));
        }
        let maps = flint_terrain::bake(&terrain, seed);
        let height = ImageData::grey16(
            res,
            res,
            &flint_terrain::height_samples(&maps.heights),
            ChannelSemantics::Height,
        );
        let mut splat = ImageData::new(res, res, ChannelSemantics::Splat);
        splat.pixels = maps.splat;
        height.validate()?;
        splat.validate()?;
        Ok(GeneratorOutput::ImageSet(vec![height, splat]))
    }

    fn param_schema(&self) -> Json {
        param_schema()
    }

    fn estimate_cost(&self, spec: &ProcGenSpec) -> GenerationCost {
        let res = u64::from(resolution_of(spec));
        let texels = res * res;
        // Release build, rolling_meadow's recipe: about 0.25 us a texel for
        // each noise octave and 0.02 us for each erosion iteration.
        let mut ns_per_texel = 30.0; // the splat rules and normalizing
        for layer in spec
            .params
            .get("height_layers")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            let int = |k: &str, d: i64| layer.get(k).and_then(toml::Value::as_integer).unwrap_or(d);
            ns_per_texel += match layer.get("op").and_then(toml::Value::as_str) {
                Some("noise") => 250.0 * int("octaves", 6) as f64,
                Some("erosion") => 20.0 * int("iterations", 50) as f64,
                _ => 2.0,
            };
        }
        GenerationCost {
            estimated_vertices: 0,
            estimated_triangles: 0,
            estimated_texture_bytes: texels * 6,
            estimated_generation_ms: texels as f64 * ns_per_texel * 1e-6,
        }
    }
}

/// A closed table of `properties`, titled for the validator's messages.
fn table(title: &str, description: &str, properties: Json) -> Json {
    json!({
        "type": "object",
        "title": title,
        "description": description,
        "properties": properties,
        "additionalProperties": false
    })
}

/// One `[[height_layers]]` op: its `op` tag and its own keys, closed.
fn height_op(op: &str, description: &str, mut properties: Json) -> Json {
    properties["op"] = json!({ "type": "string", "const": op, "description": description });
    json!({
        "type": "object",
        "title": format!("{op} layer"),
        "properties": properties,
        "required": ["op"],
        "additionalProperties": false
    })
}

/// One `[[splat_rules]]` entry: closed, and its `layer` required.
fn splat_rule(properties: Json) -> Json {
    let mut rule = table(
        "splat rule",
        "One layer's band of height and slope",
        properties,
    );
    rule["required"] = json!(["layer"]);
    rule
}

/// `[grass]`, read off `GrassConfig`'s own defaults, so a field added there
/// is in the schema with nothing more to write. The bake never reads it.
fn grass_schema() -> Json {
    let defaults = toml::Value::try_from(GrassConfig::default()).unwrap_or(toml::Value::Integer(0));
    let mut props = serde_json::Map::new();
    for (key, value) in defaults.as_table().into_iter().flatten() {
        let prop = match value {
            toml::Value::Boolean(b) => json!({ "type": "boolean", "default": b }),
            toml::Value::Integer(i) => json!({ "type": "integer", "minimum": 0, "default": i }),
            toml::Value::Float(f) => json!({ "type": "number", "default": f }),
            toml::Value::String(s) if key == "density_source" => {
                json!({ "type": "string", "enum": ["splat", "map"], "default": s })
            }
            toml::Value::String(s) => json!({ "type": "string", "default": s }),
            toml::Value::Array(_) => json!({ "type": "array", "items": { "type": "number" } }),
            _ => json!({}),
        };
        props.insert(key.clone(), prop);
    }
    table(
        "grass table",
        "The terrain component's GPU grass (see the book's Terrain page). Not read by the bake; leave it out unless the terrain should grow grass",
        Json::Object(props),
    )
}

/// `terrain_v1`'s parameter schema: the terrain format's tables, every key
/// with its default and range.
pub fn param_schema() -> Json {
    let texture_layer = json!({
        "type": "object",
        "title": "texture layer",
        "properties": {
            "path": { "type": "string", "description": "The layer's albedo texture" },
            "label": { "type": "string", "description": "What the layer is (grass, rock); names its share of the splat map" }
        },
        "additionalProperties": false
    });
    json!({
        "type": "object",
        "description": "A heightmap terrain: height layers run in order over a square grid, then splat rules paint four surface layers by height, slope and noise. Heights are normalized to 0..1 after the last layer, so a recipe's shape matters more than its absolute numbers",
        "properties": {
            "geometry": table("geometry table", "The terrain's size and the maps' resolution", json!({
                "width": { "type": "number", "exclusiveMinimum": 0.0, "default": 256.0, "description": "Metres across (x)" },
                "depth": { "type": "number", "exclusiveMinimum": 0.0, "default": 256.0, "description": "Metres deep (z)" },
                "height_scale": { "type": "number", "minimum": 0.0, "default": 50.0, "description": "Metres from height 0 to height 1; also how steep the splat rules' slopes are" },
                "resolution": { "type": "integer", "minimum": 2, "maximum": MAX_RESOLUTION, "default": 256, "description": "Texels a side of both maps" },
                "chunk_resolution": { "type": "integer", "minimum": 1, "default": 64, "description": "Quads a side of each mesh chunk the runtime builds; not read by the bake" }
            })),
            "height_layers": {
                "type": "array",
                "description": "The height recipe, run in order on a grid starting at 0: `noise` adds fractal noise, `flatten` pulls heights toward a level, `erosion` wears them",
                "items": {
                    "type": "object",
                    "required": ["op"],
                    "discriminator": { "propertyName": "op" },
                    "oneOf": [
                        height_op("noise", "Fractal noise, octaves summed and blended into the heights", json!({
                            "noise_type": { "type": "string", "enum": ["perlin", "simplex", "value", "ridged"], "default": "perlin", "description": "perlin and simplex roll; value is blockier; ridged makes sharp crests (mountain ridges)" },
                            "octaves": { "type": "integer", "minimum": 1, "maximum": 16, "default": 6, "description": "Noise layers summed, each finer than the last; more adds small detail" },
                            "frequency": { "type": "number", "exclusiveMinimum": 0.0, "default": 2.0, "description": "Features across the whole terrain in the first octave: 1 to 2 is one or two big hills, 8 is many small ones" },
                            "lacunarity": { "type": "number", "exclusiveMinimum": 0.0, "default": 2.0, "description": "Frequency multiplier from one octave to the next" },
                            "persistence": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.5, "description": "Amplitude multiplier from one octave to the next; lower is smoother" },
                            "amplitude": { "type": "number", "default": 1.0, "description": "Scale of this layer's noise (which runs about -1..1) before it is blended in" },
                            "blend": { "type": "string", "enum": ["add", "multiply", "max", "min", "overlay"], "default": "add", "description": "How the noise combines with the heights so far" }
                        })),
                        height_op("flatten", "Pull heights below (or above) a level toward it: valley floors, plateaus, lake beds", json!({
                            "height": { "type": "number", "default": 0.1, "description": "The level, in the heights' units so far (normalizing comes after the last layer)" },
                            "falloff": { "type": "number", "minimum": 0.0, "default": 0.05, "description": "Distance from the level over which the pull fades in; 0 cuts flat" },
                            "mode": { "type": "string", "enum": ["below", "above"], "default": "below", "description": "below raises what is under the level (a flat floor); above lowers what is over it (a plateau)" }
                        })),
                        height_op("erosion", "Wear the heights: thermal slumps steep slopes, hydraulic carves channels with seeded raindrops", json!({
                            "kind": { "type": "string", "enum": ["thermal", "hydraulic"], "default": "thermal", "description": "thermal softens slopes steeper than talus_angle; hydraulic carves gullies and fans" },
                            "iterations": { "type": "integer", "minimum": 0, "default": 50, "description": "Passes (thermal) or hundreds of droplets (hydraulic); the bake's cost grows with it" },
                            "talus_angle": { "type": "number", "minimum": 0.0, "default": 0.6, "description": "The steepest slope thermal erosion leaves alone; hydraulic doesn't read it" },
                            "strength": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.3, "description": "How much each pass moves" }
                        }))
                    ]
                }
            },
            "splat_rules": {
                "type": "array",
                "description": "Paint the four surface layers: each texel takes every rule whose height and slope it fits, weighted toward the middle of the rule's band, and the weights are normalized. No rules paints all layer 0",
                "items": splat_rule(json!({
                    "layer": { "type": "integer", "minimum": 0, "maximum": 3, "description": "The layer painted: 0 to 3 (the splat map's R, G, B, A)" },
                    "height_min": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.0, "description": "Lowest normalized height the rule paints" },
                    "height_max": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 1.0, "description": "Highest normalized height the rule paints" },
                    "slope_max": { "type": "number", "minimum": 0.0, "default": 1.0, "description": "Steepest slope (rise over run) the rule paints; its weight fades toward it" },
                    "noise_amount": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.0, "description": "How much seeded noise breaks up the rule's edges" },
                    "noise_scale": { "type": "number", "exclusiveMinimum": 0.0, "default": 4.0, "description": "Noise features across the terrain" }
                }))
            },
            "textures": table("textures table", "The terrain component's surface textures and material; not read by the bake", json!({
                "texture_tile": { "type": "number", "exclusiveMinimum": 0.0, "default": 16.0, "description": "Times each layer's texture repeats across the terrain" },
                "metallic": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.0, "description": "Metallic factor" },
                "roughness": { "type": "number", "minimum": 0.0, "maximum": 1.0, "default": 0.85, "description": "Roughness factor" },
                "layers": table("layers table", "Each splat layer's texture", json!({
                    "layer0": texture_layer.clone(),
                    "layer1": texture_layer.clone(),
                    "layer2": texture_layer.clone(),
                    "layer3": texture_layer
                }))
            })),
            "grass": grass_schema()
        },
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{validate_params, GeneratorRegistry, ParamIssue};

    const MEADOW: &str =
        include_str!("../../../../tests/fixtures/scenes/terrain/rolling_meadow.terrain.toml");

    fn small() -> &'static str {
        r#"
[meta]
name = "small"

[seed]
mode = "fixed"
value = 7

[geometry]
resolution = 24

[[height_layers]]
op = "noise"
octaves = 3

[[height_layers]]
op = "erosion"
kind = "hydraulic"
iterations = 40

[[splat_rules]]
layer = 0
height_max = 0.5
noise_amount = 0.2

[[splat_rules]]
layer = 2
height_min = 0.4
"#
    }

    fn generate(spec: &ProcGenSpec) -> Vec<ImageData> {
        GeneratorRegistry::with_built_ins()
            .generate_from_spec(spec)
            .unwrap()
            .into_image_set()
            .unwrap()
    }

    /// Done-when: the heightmap stays 16-bit and is the bake's, byte for byte.
    #[test]
    fn the_maps_are_the_bakes_byte_for_byte() {
        for text in [small(), MEADOW] {
            let (terrain, _) = TerrainSpec::parse_checked(text).unwrap();
            let maps = flint_terrain::bake(&terrain, terrain.seed.value);
            let images = generate(&from_terrain_toml(text).unwrap());
            assert_eq!(images.len(), 2);
            let (height, splat) = (&images[0], &images[1]);
            assert_eq!(height.format, crate::PixelFormat::L16);
            assert_eq!(height.channel_semantics, ChannelSemantics::Height);
            assert_eq!(splat.channel_semantics, ChannelSemantics::Splat);
            assert_eq!(
                height.to_png_bytes().unwrap(),
                maps.heightmap_png().unwrap()
            );
            assert_eq!(splat.to_png_bytes().unwrap(), maps.splat_png().unwrap());
        }
    }

    #[test]
    fn the_written_heightmap_reads_back_as_16_bit() {
        let images = generate(&from_terrain_toml(small()).unwrap());
        let dir = std::env::temp_dir().join(format!("flint_terrain_v1_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("h.png");
        images[0].save_png(&file).unwrap();
        let back = crate::read_png(&file, ChannelSemantics::Height).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(back, images[0]);
    }

    #[test]
    fn a_terrain_file_maps_its_meta_and_seed() {
        let spec = from_terrain_toml(small()).unwrap();
        assert_eq!(spec.generator, "terrain_v1");
        assert_eq!(spec.meta.name, "small");
        assert_eq!(spec.meta.version, "1.0.0");
        assert_eq!(
            (spec.seed.mode, spec.seed.value),
            (SeedMode::Fixed, Some(7))
        );
        let keys: Vec<&String> = spec.params.as_table().unwrap().keys().collect();
        assert_eq!(keys, ["geometry", "height_layers", "splat_rules"]);

        let random =
            from_terrain_toml("[meta]\nname = \"r\"\n[seed]\nmode = \"random\"\n").unwrap();
        assert_eq!(
            (random.seed.mode, random.seed.value),
            (SeedMode::Random, None)
        );
        let unseeded = from_terrain_toml("[meta]\nname = \"u\"\n").unwrap();
        assert_eq!(
            unseeded.seed.value,
            Some(42),
            "a terrain spec's seed defaults to 42"
        );
        assert!(from_terrain_toml("[geometry]\nresolution = 8\n").is_err());
    }

    /// A terrain spec written either way bakes the same maps.
    #[test]
    fn the_procgen_spelling_bakes_the_same_maps() {
        let from_file = from_terrain_toml(small()).unwrap();
        let text = toml::to_string(&from_file).unwrap();
        let reparsed = ProcGenSpec::parse(&text).unwrap();
        assert_eq!(generate(&reparsed), generate(&from_file));
    }

    #[test]
    fn the_fixture_and_the_editors_default_have_nothing_unread() {
        let default = toml::to_string(&TerrainSpec::default_spec("d")).unwrap();
        for text in [MEADOW, small(), default.as_str()] {
            let spec = from_terrain_toml(text).unwrap();
            let issues = validate_params(&param_schema(), &spec.params);
            assert!(issues.is_empty(), "{issues:?}");
        }
    }

    /// The schema knows every key the terrain types read, at every depth: a
    /// spec with every field written (grass and four texture layers too)
    /// validates clean, so a field added to the terrain format without a
    /// schema entry fails here.
    #[test]
    fn the_schema_covers_every_field_of_the_format() {
        let mut full = TerrainSpec::default_spec("full");
        full.height_layers = vec![
            toml::from_str("op = \"noise\"").unwrap(),
            toml::from_str("op = \"flatten\"").unwrap(),
            toml::from_str("op = \"erosion\"").unwrap(),
        ];
        full.splat_rules = vec![toml::from_str("layer = 1").unwrap()];
        full.grass = Some(GrassConfig::default());
        let layer = || {
            Some(flint_terrain::spec::TextureLayer {
                path: "t.png".into(),
                label: "t".into(),
            })
        };
        let l = &mut full.textures.layers;
        (l.layer0, l.layer1, l.layer2, l.layer3) = (layer(), layer(), layer(), layer());
        let spec = from_terrain_toml(&toml::to_string(&full).unwrap()).unwrap();
        let issues = validate_params(&param_schema(), &spec.params);
        assert!(issues.is_empty(), "{issues:?}");
    }

    /// What `parse_checked` reported as unread, the schema reports too.
    #[test]
    fn typos_are_named_by_path() {
        let text = format!(
            "{}\n[grass]\nwind_sped = 2.0\n[[height_layers]]\nop = \"noise\"\noctave = 8\n",
            small()
        );
        let spec = from_terrain_toml(&text).unwrap();
        let keys: Vec<String> = validate_params(&param_schema(), &spec.params)
            .iter()
            .map(|i| i.key().to_string())
            .collect();
        assert_eq!(keys, ["grass.wind_sped", "height_layers[2].octave"]);

        // A top-level key goes before the first table, or a table holds it
        let spec = from_terrain_toml(&format!("heigth_layers = []\n{}", small())).unwrap();
        let issues = validate_params(&param_schema(), &spec.params);
        assert!(
            matches!(&issues[..], [ParamIssue::Unknown { key, suggestion: Some(s), .. }]
                if key == "heigth_layers" && s == "height_layers"),
            "{issues:?}"
        );
    }

    #[test]
    fn values_it_cant_use_are_errors() {
        let spec = from_terrain_toml(
            "[meta]\nname = \"x\"\n[[height_layers]]\nop = \"sculpt\"\n[[splat_rules]]\nlayer = 4\n",
        )
        .unwrap();
        let issues = validate_params(&param_schema(), &spec.params);
        let keys: Vec<&str> = issues
            .iter()
            .filter(|i| i.is_error())
            .map(|i| i.key())
            .collect();
        assert_eq!(keys, ["height_layers[0].op", "splat_rules[0].layer"]);
    }

    #[test]
    fn an_oversized_map_is_refused_before_baking() {
        let spec =
            from_terrain_toml("[meta]\nname = \"x\"\n[geometry]\nresolution = 9000\n").unwrap();
        let err = GeneratorRegistry::with_built_ins()
            .generate_from_spec(&spec)
            .unwrap_err();
        assert!(err.to_string().contains("4096"), "{err}");
    }

    #[test]
    fn the_estimate_grows_with_the_recipe() {
        let g = TerrainGenerator;
        let small = g.estimate_cost(&from_terrain_toml(small()).unwrap());
        let meadow = g.estimate_cost(&from_terrain_toml(MEADOW).unwrap());
        assert_eq!(small.estimated_texture_bytes, 24 * 24 * 6);
        assert!(meadow.estimated_generation_ms > small.estimated_generation_ms);
    }
}
