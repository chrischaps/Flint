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
