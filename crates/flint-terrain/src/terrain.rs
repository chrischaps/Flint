//! Terrain configuration and orchestration

use crate::chunk::{generate_chunk, TerrainChunk};
use crate::heightmap::Heightmap;

/// Configuration for terrain generation, parsed from the `terrain` component
pub struct TerrainConfig {
    /// Path to the heightmap PNG
    pub heightmap_path: String,
    /// World-space X extent
    pub width: f32,
    /// World-space Z extent
    pub depth: f32,
    /// Maximum Y height (heightmap 1.0 maps to this)
    pub height_scale: f32,
    /// Number of quads per chunk edge (vertices = resolution + 1)
    pub chunk_resolution: u32,
    /// UV tiling factor for layer textures
    pub texture_tile: f32,
    /// Path to the RGBA splat map
    pub splat_map_path: String,
    /// Paths to 4 layer textures (R, G, B, A channels of splat map)
    pub layer_textures: [String; 4],
    /// PBR metallic factor
    pub metallic: f32,
    /// PBR roughness factor
    pub roughness: f32,
}

/// A complete terrain composed of chunks
pub struct Terrain {
    /// The source heightmap
    pub heightmap: Heightmap,
    /// Number of chunks along X axis
    pub chunks_x: u32,
    /// Number of chunks along Z axis
    pub chunks_z: u32,
    /// All generated chunks
    pub chunks: Vec<TerrainChunk>,
}

impl Terrain {
    /// Generate all terrain chunks from a heightmap and config.
    ///
    /// The chunk grid is sized so that each chunk covers approximately
    /// `chunk_resolution` quads, with the heightmap distributed across
    /// all chunks. For small heightmaps, a single chunk is used.
    pub fn generate(heightmap: &Heightmap, config: &TerrainConfig) -> Self {
        let res = config.chunk_resolution.max(2);

        // Determine how many chunks we need based on heightmap resolution
        // Each chunk covers `res` quads, so we need ceil(hm_pixels / res) chunks
        let chunks_x = ((heightmap.width - 1) as f32 / res as f32).ceil().max(1.0) as u32;
        let chunks_z = ((heightmap.depth - 1) as f32 / res as f32).ceil().max(1.0) as u32;

        let mut chunks = Vec::with_capacity((chunks_x * chunks_z) as usize);

        for row in 0..chunks_z {
            for col in 0..chunks_x {
                let chunk = generate_chunk(
                    heightmap, config, col, row, res, chunks_x, chunks_z,
                );
                chunks.push(chunk);
            }
        }

        Self {
            heightmap: Heightmap::from_raw(
                // Clone the heightmap data so Terrain owns it
                heightmap.clone_heights(),
                heightmap.width,
                heightmap.depth,
            ),
            chunks_x,
            chunks_z,
            chunks,
        }
    }

    /// Sample world-space height at (x, z).
    /// Returns the interpolated Y value at that position.
    pub fn sample_height(&self, x: f32, z: f32, config: &TerrainConfig) -> f32 {
        self.heightmap.sample_world(x, z, config)
    }

    /// Export all terrain geometry as a single trimesh for physics.
    /// Returns (vertices, triangle_indices) suitable for Rapier's TriMesh collider.
    pub fn trimesh_data(&self) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let total_verts: usize = self.chunks.iter().map(|c| c.positions.len()).sum();
        let total_tris: usize = self.chunks.iter().map(|c| c.indices.len() / 3).sum();

        let mut vertices = Vec::with_capacity(total_verts);
        let mut triangles = Vec::with_capacity(total_tris);
        let mut base_index: u32 = 0;

        for chunk in &self.chunks {
            vertices.extend_from_slice(&chunk.positions);

            for tri in chunk.indices.chunks(3) {
                triangles.push([
                    tri[0] + base_index,
                    tri[1] + base_index,
                    tri[2] + base_index,
                ]);
            }

            base_index += chunk.positions.len() as u32;
        }

        (vertices, triangles)
    }
}
