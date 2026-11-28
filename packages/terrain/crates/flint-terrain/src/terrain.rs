//! Terrain configuration and orchestration

use crate::chunk::{generate_chunk, TerrainChunk};
use crate::grass_config::GrassConfig;
use crate::heightmap::{Heightmap, HeightmapFiles};

/// Configuration for terrain generation, parsed from the `terrain` component
/// (over its recipe's settings when it names one: #1697)
#[derive(Debug, Clone)]
pub struct TerrainConfig {
    /// Path to the heightmap PNG
    pub heightmap_path: String,
    /// The `.terrain.toml` the terrain is made from (#1697); empty for none
    pub spec_path: String,
    /// The seed the recipe bakes at, when the component writes one
    pub seed: Option<u64>,
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

impl Default for TerrainConfig {
    /// What a component that writes nothing draws: the numbers the
    /// component's schema gave as defaults until #1697 took them out, so
    /// that a key a scene leaves out can be its recipe's.
    fn default() -> Self {
        Self {
            heightmap_path: String::new(),
            spec_path: String::new(),
            seed: None,
            width: 256.0,
            depth: 256.0,
            height_scale: 50.0,
            chunk_resolution: 64,
            texture_tile: 16.0,
            splat_map_path: String::new(),
            layer_textures: Default::default(),
            metallic: 0.0,
            roughness: 0.85,
            grass: None,
        }
    }
}

/// The component's keys a recipe also sets (#1697): each with the spec's
/// place for it, which `flint check` names when the two disagree.
pub const SPEC_KEYS: [(&str, &str); 11] = [
    ("width", "[geometry] width"),
    ("depth", "[geometry] depth"),
    ("height_scale", "[geometry] height_scale"),
    ("chunk_resolution", "[geometry] chunk_resolution"),
    ("texture_tile", "[textures] texture_tile"),
    ("metallic", "[textures] metallic"),
    ("roughness", "[textures] roughness"),
    ("layer0_texture", "[textures.layers.layer0] path"),
    ("layer1_texture", "[textures.layers.layer1] path"),
    ("layer2_texture", "[textures.layers.layer2] path"),
    ("layer3_texture", "[textures.layers.layer3] path"),
];

impl TerrainConfig {
    /// Read a `terrain` component through the typed view (ADR 0115): a
    /// wrong type reads as the default and is reported once by entity,
    /// component and field. `grass` is left for [`GrassConfig::from_component`].
    pub fn from_component(view: &flint_ecs::ComponentView<'_>) -> Self {
        Self::over(view, Self::default())
    }

    /// The component's keys over `base` (#1697): each key it writes wins,
    /// each it leaves out is `base`'s (a recipe's settings, or the
    /// defaults). `grass` is `base`'s, for [`GrassConfig::over`].
    pub fn over(view: &flint_ecs::ComponentView<'_>, base: Self) -> Self {
        let text = |key: &str, base: &str| view.opt_str(key).unwrap_or(base).to_string();
        let [l0, l1, l2, l3] = &base.layer_textures;
        Self {
            heightmap_path: text("heightmap", &base.heightmap_path),
            spec_path: text("spec", &base.spec_path),
            seed: view.opt_i64("seed").map(|s| s as u64).or(base.seed),
            width: view.f32("width", base.width),
            depth: view.f32("depth", base.depth),
            height_scale: view.f32("height_scale", base.height_scale),
            chunk_resolution: view.i64("chunk_resolution", base.chunk_resolution as i64) as i32
                as u32,
            texture_tile: view.f32("texture_tile", base.texture_tile),
            splat_map_path: text("splat_map", &base.splat_map_path),
            layer_textures: [
                text("layer0_texture", l0),
                text("layer1_texture", l1),
                text("layer2_texture", l2),
                text("layer3_texture", l3),
            ],
            metallic: view.f32("metallic", base.metallic),
            roughness: view.f32("roughness", base.roughness),
            grass: base.grass,
        }
    }

    /// A recipe's settings as a terrain's (#1697): its `[geometry]`, its
    /// `[textures]` (each path, which is from the spec's folder as the
    /// terrain editor reads it, made a name the project's search takes from
    /// `scene_dir`) and its `[grass]`. No heightmap or splat map: the bake
    /// is the terrain's.
    pub fn from_spec(source: &crate::source::SpecSource, scene_dir: &std::path::Path) -> Self {
        let spec = &source.spec;
        let (geo, tex) = (&spec.geometry, &spec.textures);
        let layer = |path: String| {
            if path.is_empty() {
                path
            } else {
                crate::source::from_scene_dir(&source.dir().join(path), scene_dir)
            }
        };
        let [l0, l1, l2, l3] = spec.layer_paths();
        Self {
            width: geo.width,
            depth: geo.depth,
            height_scale: geo.height_scale,
            chunk_resolution: geo.chunk_resolution,
            texture_tile: tex.texture_tile,
            layer_textures: [layer(l0), layer(l1), layer(l2), layer(l3)],
            metallic: tex.metallic,
            roughness: tex.roughness,
            grass: spec.grass.clone(),
            ..Self::default()
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

/// The grid's type is flint-geometry's since #1623, the plain data physics'
/// heightfield and the kernel's [`SceneGround`](flint_geometry::ground::SceneGround) carry.
pub use flint_geometry::heightmap::HeightGrid;

#[cfg(test)]
mod height_grid_tests {
    use super::*;

    fn config(res: u32) -> TerrainConfig {
        TerrainConfig {
            heightmap_path: String::new(),
            spec_path: String::new(),
            seed: None,
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
