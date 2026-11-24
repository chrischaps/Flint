//! Terrain configuration and orchestration

use crate::chunk::{generate_chunk, TerrainChunk};
use crate::grass_config::GrassConfig;
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
    /// Optional grass rendering configuration
    pub grass: Option<GrassConfig>,
}

impl TerrainConfig {
    /// Read a `terrain` component through the typed view (ADR 0115): a
    /// wrong type reads as the default and is reported once by entity,
    /// component and field. `grass` is left for [`GrassConfig::from_component`].
    pub fn from_component(view: &flint_ecs::ComponentView<'_>) -> Self {
        let text = |key: &str| view.str(key, "").to_string();
        Self {
            heightmap_path: text("heightmap"),
            width: view.f32("width", 256.0),
            depth: view.f32("depth", 256.0),
            height_scale: view.f32("height_scale", 50.0),
            chunk_resolution: view.i64("chunk_resolution", 64) as i32 as u32,
            texture_tile: view.f32("texture_tile", 16.0),
            splat_map_path: text("splat_map"),
            layer_textures: [
                text("layer0_texture"),
                text("layer1_texture"),
                text("layer2_texture"),
                text("layer3_texture"),
            ],
            metallic: view.f32("metallic", 0.0),
            roughness: view.f32("roughness", 0.85),
            grass: None,
        }
    }
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
        flint_core::scope!("terrain::generate");
        let res = config.chunk_resolution.max(2);

        // Determine how many chunks we need based on heightmap resolution
        // Each chunk covers `res` quads, so we need ceil(hm_pixels / res) chunks
        let chunks_x = ((heightmap.width - 1) as f32 / res as f32).ceil().max(1.0) as u32;
        let chunks_z = ((heightmap.depth - 1) as f32 / res as f32).ceil().max(1.0) as u32;

        let mut chunks = Vec::with_capacity((chunks_x * chunks_z) as usize);

        for row in 0..chunks_z {
            for col in 0..chunks_x {
                let chunk = generate_chunk(heightmap, config, col, row, res, chunks_x, chunks_z);
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

    /// The chunks' vertices as one grid of heights, for a physics
    /// heightfield (#1219). Neighbouring chunks share their edge vertices,
    /// so the grid is `chunks_x * resolution + 1` columns (along X) by
    /// `chunks_z * resolution + 1` rows (along Z), each height exactly the
    /// one [`trimesh_data`](Self::trimesh_data) puts there. Each quad of the
    /// mesh is split along the diagonal from its (x0, z0) corner to its
    /// (x1, z1) corner, which a heightfield must do too to match it.
    pub fn height_grid(&self) -> HeightGrid {
        let Some(first) = self.chunks.first() else {
            return HeightGrid {
                columns: 0,
                rows: 0,
                heights: Vec::new(),
                width: 0.0,
                depth: 0.0,
            };
        };
        let edge = (first.positions.len() as f64).sqrt().round() as usize;
        let res = edge - 1;
        let columns = self.chunks_x as usize * res + 1;
        let rows = self.chunks_z as usize * res + 1;
        let mut heights = vec![0.0; columns * rows];
        let (mut width, mut depth) = (0.0f32, 0.0f32);
        for chunk in &self.chunks {
            let (col, row) = (chunk.grid_pos.0 as usize, chunk.grid_pos.1 as usize);
            for (k, p) in chunk.positions.iter().enumerate() {
                let (vx, vz) = (k % edge, k / edge);
                heights[(row * res + vz) * columns + col * res + vx] = p[1];
                width = width.max(p[0]);
                depth = depth.max(p[2]);
            }
        }
        HeightGrid {
            columns,
            rows,
            heights,
            width,
            depth,
        }
    }
}

/// A terrain's heights on its vertex grid ([`Terrain::height_grid`]), in the
/// terrain's own frame: column 0 at x = 0, the last at x = `width`; row 0 at
/// z = 0, the last at z = `depth`.
#[derive(Debug, Clone, PartialEq)]
pub struct HeightGrid {
    /// Samples along X.
    pub columns: usize,
    /// Samples along Z.
    pub rows: usize,
    /// World heights, row by row: `heights[row * columns + column]`.
    pub heights: Vec<f32>,
    /// The X extent the columns span.
    pub width: f32,
    /// The Z extent the rows span.
    pub depth: f32,
}

#[cfg(test)]
mod height_grid_tests {
    use super::*;

    fn config(res: u32) -> TerrainConfig {
        TerrainConfig {
            heightmap_path: String::new(),
            width: 40.0,
            depth: 30.0,
            height_scale: 8.0,
            chunk_resolution: res,
            texture_tile: 1.0,
            splat_map_path: String::new(),
            layer_textures: Default::default(),
            metallic: 0.0,
            roughness: 1.0,
            grass: None,
        }
    }

    #[test]
    fn the_grid_holds_every_trimesh_vertex_where_the_chunks_put_it() {
        let (w, d) = (13u32, 9u32);
        let heights = (0..w * d)
            .map(|i| ((i * 37 % 101) as f32) / 100.0)
            .collect();
        let hm = Heightmap::from_raw(heights, w, d);
        let cfg = config(4);
        let terrain = Terrain::generate(&hm, &cfg);
        assert_eq!((terrain.chunks_x, terrain.chunks_z), (3, 2));

        let grid = terrain.height_grid();
        assert_eq!((grid.columns, grid.rows), (13, 9));
        assert_eq!((grid.width, grid.depth), (40.0, 30.0));
        let (verts, _) = terrain.trimesh_data();
        for v in verts {
            let column = (v[0] / grid.width * (grid.columns - 1) as f32).round() as usize;
            let row = (v[2] / grid.depth * (grid.rows - 1) as f32).round() as usize;
            assert_eq!(grid.heights[row * grid.columns + column], v[1], "{v:?}");
        }
    }
}
