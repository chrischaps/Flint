//! A terrain chunk's mesh: plain data a terrain makes and the renderer
//! uploads (#1615). It was flint-terrain's, which re-exports it; here the
//! renderer takes it as a slice without linking a terrain crate.

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
