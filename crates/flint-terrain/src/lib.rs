//! Flint Terrain - Heightmap-based terrain generation
//!
//! Provides heightmap loading, chunked mesh generation, and trimesh export
//! for physics collision. Does not depend on flint-render — outputs raw
//! vertex data (positions, normals, UVs, indices) for the renderer to consume.

pub mod chunk;
pub mod heightmap;
pub mod terrain;

pub use chunk::TerrainChunk;
pub use heightmap::Heightmap;
pub use terrain::{Terrain, TerrainConfig};

#[cfg(test)]
mod tests {
    use super::*;

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
        let heights = vec![
            0.0, 0.0, 0.0,
            0.0, 1.0, 0.0,
            0.0, 0.0, 0.0,
        ];
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
