//! Terrain chunk mesh generation

use crate::heightmap::Heightmap;
use crate::terrain::TerrainConfig;

/// A single chunk of terrain geometry
pub struct TerrainChunk {
    /// Grid position (column, row) in the chunk grid
    pub grid_pos: (u32, u32),
    /// Vertex positions in world space
    pub positions: Vec<[f32; 3]>,
    /// Vertex normals
    pub normals: Vec<[f32; 3]>,
    /// UV coordinates (normalized over entire terrain for splat map lookup)
    pub uvs: Vec<[f32; 2]>,
    /// Triangle indices (CCW winding)
    pub indices: Vec<u32>,
    /// AABB minimum corner
    pub aabb_min: [f32; 3],
    /// AABB maximum corner
    pub aabb_max: [f32; 3],
}

/// Generate mesh data for a single terrain chunk.
///
/// `col` and `row` identify which chunk in the grid.
/// `resolution` is the number of quads per chunk edge (vertices = resolution + 1).
/// `chunks_x` and `chunks_z` are the total number of chunks in each direction.
pub fn generate_chunk(
    heightmap: &Heightmap,
    config: &TerrainConfig,
    col: u32,
    row: u32,
    resolution: u32,
    chunks_x: u32,
    chunks_z: u32,
) -> TerrainChunk {
    let verts_per_edge = resolution + 1;
    let vert_count = (verts_per_edge * verts_per_edge) as usize;

    let mut positions = Vec::with_capacity(vert_count);
    let mut normals = Vec::with_capacity(vert_count);
    let mut uvs = Vec::with_capacity(vert_count);

    let mut aabb_min = [f32::MAX; 3];
    let mut aabb_max = [f32::MIN; 3];

    for vz in 0..verts_per_edge {
        for vx in 0..verts_per_edge {
            // Normalized position within the entire terrain [0..1]
            let u = (col as f32 + vx as f32 / resolution as f32) / chunks_x as f32;
            let v = (row as f32 + vz as f32 / resolution as f32) / chunks_z as f32;

            let world_x = u * config.width;
            let world_z = v * config.depth;
            let height = heightmap.sample(u, v) * config.height_scale;

            let normal = heightmap.compute_normal(
                u,
                v,
                config.width,
                config.depth,
                config.height_scale,
            );

            let pos = [world_x, height, world_z];

            // Update AABB
            for i in 0..3 {
                aabb_min[i] = aabb_min[i].min(pos[i]);
                aabb_max[i] = aabb_max[i].max(pos[i]);
            }

            positions.push(pos);
            normals.push(normal);
            uvs.push([u, v]);
        }
    }

    // Generate indices (two triangles per quad, CCW winding)
    let index_count = (resolution * resolution * 6) as usize;
    let mut indices = Vec::with_capacity(index_count);

    for qz in 0..resolution {
        for qx in 0..resolution {
            let tl = qz * verts_per_edge + qx;
            let tr = tl + 1;
            let bl = tl + verts_per_edge;
            let br = bl + 1;

            // First triangle (top-left, bottom-left, bottom-right)
            indices.push(tl);
            indices.push(bl);
            indices.push(br);

            // Second triangle (top-left, bottom-right, top-right)
            indices.push(tl);
            indices.push(br);
            indices.push(tr);
        }
    }

    TerrainChunk {
        grid_pos: (col, row),
        positions,
        normals,
        uvs,
        indices,
        aabb_min,
        aabb_max,
    }
}
