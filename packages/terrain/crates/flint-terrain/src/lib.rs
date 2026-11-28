//! Flint Terrain - Heightmap-based terrain generation
//!
//! Provides heightmap loading, chunked mesh generation, and trimesh export
//! for physics collision. Outputs raw vertex data (positions, normals, UVs,
//! indices) as flint-geometry's [`TerrainChunk`], and the grass settings as
//! its [`GrassConfig`]: plain data the renderer draws without linking this
//! crate (#1615).
//!
//! The terrain package's crate (#1623, `packages/terrain/`). The `render`
//! feature adds [`scene`]: a scene's terrain built into a renderer, after
//! `SceneRenderer::bind_scene` (the renderer built it itself until #1615).
//! `player` is the package's player part ([`player::PACKAGE`]: that build
//! as the kernel's scene binder, and the `terrain` script module), `cli` its
//! CLI part (the terrain editor). Without a feature this crate links no
//! renderer, so procgen's `terrain_v1` doesn't either.

/// The package's name, as its manifest gives it.
pub const PACKAGE_NAME: &str = "terrain";

pub mod bake;
pub mod brush;
pub mod chunk;
/// The grass settings, flint-geometry's since #1615.
pub use flint_geometry::grass as grass_config;
#[cfg(feature = "cli")]
pub mod cli;
pub mod heightmap;
#[cfg(feature = "player")]
pub mod player;
pub mod procgen;
#[cfg(feature = "render")]
pub mod scene;
pub mod source;
pub mod spec;
pub mod splatgen;
pub mod terrain;

pub use bake::{bake, encode_heightmap_png, encode_splat_png, height_samples, TerrainMaps};
pub use brush::{
    apply_height_brush, apply_splat_brush, ray_heightmap_intersect, BrushParams, HeightBrushMode,
    SplatBrushMode,
};
pub use chunk::TerrainChunk;
/// The kernel's ground, flint-geometry's since #1623.
pub use flint_geometry::ground::{GroundField, SceneGround};
pub use grass_config::GrassConfig;
pub use heightmap::{Heightmap, HeightmapFiles};
pub use procgen::generate_heightmap;
pub use source::{baked, Baked, SpecSource};
pub use spec::{SpecIssue, TerrainSpec};
pub use splatgen::generate_splat_map;
pub use terrain::{HeightGrid, Terrain, TerrainConfig};

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::components as comp;
    use flint_ecs::FlintWorld;

    fn world_with(component: &str, text: &str) -> (FlintWorld, flint_core::EntityId) {
        let mut world = FlintWorld::new();
        let id = world.spawn("hills").unwrap();
        let data: toml::Value = toml::from_str(text).unwrap();
        world.set_component(id, component, data).unwrap();
        (world, id)
    }

    /// The schema's fields carry no `default` since #1697 (a key left out
    /// is the recipe's); each description ends with the number it stands
    /// for, which must be the reader's: `(default 8)`, `(the spec's, else
    /// 256)`. Loading a scene before #1697 wrote those defaults in, so a
    /// scene with no recipe draws as it did only while the two agree.
    #[test]
    fn the_described_defaults_are_the_readers() {
        let schema: toml::Table =
            toml::from_str(include_str!("../../../schemas/components/terrain.toml")).unwrap();
        let fields = schema["component"]["terrain"]["fields"].as_table().unwrap();
        let config = TerrainConfig::default();
        let grass = toml::Value::try_from(GrassConfig::default()).unwrap();
        let reader = |key: &str| -> Option<toml::Value> {
            let float = |v: f32| Some(toml::Value::Float(v as f64));
            match key {
                "width" => float(config.width),
                "depth" => float(config.depth),
                "height_scale" => float(config.height_scale),
                "chunk_resolution" => Some(toml::Value::Integer(config.chunk_resolution as i64)),
                "texture_tile" => float(config.texture_tile),
                "metallic" => float(config.metallic),
                "roughness" => float(config.roughness),
                _ => grass.get(key.strip_prefix("grass.")?).cloned(),
            }
        };
        let mut held = 0;
        for (key, field) in fields {
            assert!(field.get("default").is_none(), "{key} has a default again");
            let text = field["description"].as_str().unwrap();
            let Some(said) = text
                .rsplit_once("(default ")
                .or_else(|| text.rsplit_once("else "))
                .and_then(|(_, rest)| rest.strip_suffix(')'))
            else {
                continue;
            };
            if said == "none" {
                continue;
            }
            let said: toml::Value = toml::from_str::<toml::Table>(&format!("v = {said}"))
                .map(|t| t["v"].clone())
                .unwrap_or_else(|_| toml::Value::String(said.to_string()));
            let reads = reader(key).unwrap_or_else(|| panic!("{key}: no reader default"));
            let reads = match reads {
                // The enum reads back as its word
                toml::Value::String(s) => toml::Value::String(s.to_lowercase()),
                v => v,
            };
            assert!(
                flint_core::toml_util::reads_same(&said, &reads),
                "{key}: the schema says {said}, the reader {reads}"
            );
            held += 1;
        }
        assert!(held >= 27, "only {held} described defaults found");
    }

    #[test]
    fn a_component_key_wins_over_its_recipe_and_a_left_out_one_is_the_recipes() {
        let dir = std::env::temp_dir().join(format!("flint-terrain-over-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("land")).unwrap();
        std::fs::write(
            dir.join("land/hill.terrain.toml"),
            "[meta]\nname = \"hill\"\n[seed]\nvalue = 3\n\
             [geometry]\nwidth = 90.0\ndepth = 70.0\nheight_scale = 12.0\nresolution = 9\n\
             [textures]\ntexture_tile = 4.0\n[textures.layers.layer1]\npath = \"dirt.png\"\n\
             [grass]\nenabled = true\ndensity = 3.0\n\
             [[height_layers]]\nop = \"noise\"\noctaves = 2\n",
        )
        .unwrap();
        let project = flint_core::project::Project::for_scene(&dir.join("hill.scene.toml"));
        let (world, id) = world_with(
            comp::TERRAIN,
            "spec = \"land/hill.terrain.toml\"\nwidth = 50.0\n\"grass.blade_height\" = 0.9",
        );
        let made = source::make_terrain(&world.view(id, comp::TERRAIN), &project, &dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        // Written: the component's. Left out: the recipe's
        assert_eq!(made.config.width, 50.0);
        assert_eq!(made.config.depth, 70.0);
        assert_eq!(made.config.height_scale, 12.0);
        assert_eq!(made.config.texture_tile, 4.0);
        assert_eq!(made.config.layer_textures[1], "./land/dirt.png");
        assert!(made.grass.enabled);
        assert_eq!((made.grass.density, made.grass.blade_height), (3.0, 0.9));
        // The bake's maps, at the recipe's resolution and seed
        assert_eq!((made.heightmap.width, made.heightmap.depth), (9, 9));
        let recipe = made.recipe.unwrap();
        assert_eq!(recipe.seed, 3);
        assert!(matches!(made.splat, source::SplatMap::Baked(_)));
    }

    #[test]
    fn a_wrong_type_in_a_terrain_is_the_default_and_reported_once() {
        let (world, id) = world_with(
            comp::TERRAIN,
            "heightmap = \"h.png\"\nwidth = \"wide\"\nchunk_resolution = 32\n\
             \"grass.enabled\" = true\n\"grass.density\" = [4]\n\"grass.color_tip\" = \"green\"",
        );
        for _ in 0..3 {
            let terrain = TerrainConfig::from_component(&world.view(id, comp::TERRAIN));
            assert_eq!(terrain.width, 256.0);
            assert_eq!(terrain.chunk_resolution, 32);
            assert_eq!(terrain.heightmap_path, "h.png");
            let grass = GrassConfig::from_component(&world.view(id, comp::TERRAIN), false);
            assert!(grass.enabled);
            assert_eq!(grass.density, 8.0);
            assert_eq!(grass.color_tip, GrassConfig::default().color_tip);
        }
        let said: Vec<String> = world
            .read_reports()
            .snapshot()
            .iter()
            .map(|d| d.to_string())
            .collect();
        assert_eq!(
            said,
            [
                "error[bad-type] entity 'hills' terrain.width: expected a number, got string \"wide\"",
                "error[bad-type] entity 'hills' terrain.grass.density: expected a number, got array [4]",
                "error[bad-type] entity 'hills' terrain.grass.color_tip: expected 3 numbers, \
                 [x, y, z], got string \"green\"",
            ]
        );
    }

    #[test]
    fn flat_heightmap_generates_correct_mesh() {
        let hm = Heightmap::from_raw(vec![0.5; 16], 4, 4);
        let config = TerrainConfig {
            heightmap_path: String::new(),
            spec_path: String::new(),
            seed: None,
            width: 10.0,
            depth: 10.0,
            height_scale: 20.0,
            chunk_resolution: 3,
            texture_tile: 1.0,
            splat_map_path: String::new(),
            layer_textures: [String::new(), String::new(), String::new(), String::new()],
            metallic: 0.0,
            roughness: 0.85,
            grass: None,
        };

        let terrain = Terrain::generate(&hm, &config);
        assert_eq!(terrain.chunks.len(), 1); // 4x4 heightmap with res=3 => 1 chunk

        let chunk = &terrain.chunks[0];
        // res=3 means 4 verts per edge
        assert_eq!(chunk.positions.len(), 4 * 4);
        assert_eq!(chunk.indices.len(), 3 * 3 * 6); // 3*3 quads * 2 tris * 3 indices
    }

    #[test]
    fn height_sampling_returns_correct_values() {
        // 3x3 heightmap: center pixel is 1.0, edges are 0.0
        let heights = vec![0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        let hm = Heightmap::from_raw(heights, 3, 3);

        // Center of heightmap (u=0.5, v=0.5) should be 1.0
        let center = hm.sample(0.5, 0.5);
        assert!((center - 1.0).abs() < 0.01);

        // Corner should be 0.0
        let corner = hm.sample(0.0, 0.0);
        assert!((corner - 0.0).abs() < 0.01);
    }

    #[test]
    fn world_height_sampling() {
        let hm = Heightmap::from_raw(vec![0.5; 4], 2, 2);
        let config = TerrainConfig {
            heightmap_path: String::new(),
            spec_path: String::new(),
            seed: None,
            width: 100.0,
            depth: 100.0,
            height_scale: 40.0,
            chunk_resolution: 8,
            texture_tile: 1.0,
            splat_map_path: String::new(),
            layer_textures: [String::new(), String::new(), String::new(), String::new()],
            metallic: 0.0,
            roughness: 0.85,
            grass: None,
        };

        // At the origin (0,0), height should be 0.5 * 40.0 = 20.0
        let h = hm.sample_world(0.0, 0.0, &config);
        assert!((h - 20.0).abs() < 0.01);
    }

    #[test]
    fn trimesh_data_produces_valid_output() {
        let hm = Heightmap::from_raw(vec![0.0; 9], 3, 3);
        let config = TerrainConfig {
            heightmap_path: String::new(),
            spec_path: String::new(),
            seed: None,
            width: 10.0,
            depth: 10.0,
            height_scale: 5.0,
            chunk_resolution: 2,
            texture_tile: 1.0,
            splat_map_path: String::new(),
            layer_textures: [String::new(), String::new(), String::new(), String::new()],
            metallic: 0.0,
            roughness: 0.85,
            grass: None,
        };

        let terrain = Terrain::generate(&hm, &config);
        let (verts, tris) = terrain.trimesh_data();

        assert!(!verts.is_empty());
        assert!(!tris.is_empty());

        // All triangle indices should be in range
        for tri in &tris {
            assert!((tri[0] as usize) < verts.len());
            assert!((tri[1] as usize) < verts.len());
            assert!((tri[2] as usize) < verts.len());
        }
    }

    #[test]
    fn normal_computation_on_flat_terrain() {
        let hm = Heightmap::from_raw(vec![0.5; 9], 3, 3);
        let normal = hm.compute_normal(0.5, 0.5, 10.0, 10.0, 10.0);
        // Flat terrain should have normal pointing straight up
        assert!((normal[0]).abs() < 0.01);
        assert!((normal[1] - 1.0).abs() < 0.01);
        assert!((normal[2]).abs() < 0.01);
    }
}
