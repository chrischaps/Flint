//! The loaded terrain, owned by [`TerrainRenderer`] (ADR 0118, #736): its
//! per-chunk draws and the one material bind group they share.
//!
//! Three loaders write it. The two full ones (`load_terrain`, which finds
//! the splat map by asset search, and `load_terrain_from_data`, which takes
//! it as raw RGBA) differ only in how the textures reach the cache; both
//! then build the material in [`TerrainRenderer::upload_material`]. All
//! three, the brush-sculpt `reload_terrain_geometry` too, make the chunk
//! draws in [`TerrainRenderer::upload_chunks`] (#918).

use super::helpers::mat4_inv_transpose;
use crate::gpu_counts::CountedDevice;
use crate::pipeline::TransformUniforms;
use crate::terrain_pipeline::{
    TerrainDrawCall, TerrainFrameFlags, TerrainPipeline, TerrainUniforms,
};
use crate::texture_cache::TextureCache;
use flint_core::project::AssetScope;
use flint_core::vfs;
use flint_core::Transform;
use std::path::Path;

/// Where a terrain's splat map comes from (#1697): a file the scene names,
/// found by the project's search, or the pixels a recipe's bake made.
/// Both reach the cache as the same texture (sRGB, one level), so a terrain
/// made from its recipe draws as one made from the PNG its bake wrote.
#[derive(Debug, Clone, Copy)]
pub enum TerrainSplat<'a> {
    /// No splat map: the material reads white.
    None,
    /// A name the project's search takes from the scene's folder.
    File(&'a str),
    /// RGBA8 weights, `size` texels a side.
    Pixels { rgba: &'a [u8], size: u32 },
}

/// The texture cache's name for the terrain's splat map.
const SPLAT: &str = "terrain_splat";
/// The texture cache's names for the four layers the splat map blends.
const LAYERS: [&str; 4] = [
    "terrain_layer0",
    "terrain_layer1",
    "terrain_layer2",
    "terrain_layer3",
];

/// The terrain's chunk draws, the material they share, and the entity it
/// belongs to. Empty until a loader runs; `clear` empties it again.
#[derive(Default)]
pub(crate) struct TerrainRenderer {
    /// One draw per chunk, each with its own vertex, index and transform
    /// buffers. Every loader writes this, through `upload_chunks`.
    pub(crate) draws: Vec<TerrainDrawCall>,
    /// The splat and layer textures and the terrain uniforms, bound once
    /// for every chunk. The two full loaders write it, through
    /// `upload_material`; a geometry reload keeps it.
    pub(crate) material_bind_group: Option<wgpu::BindGroup>,
    pub(crate) material_buffer: Option<wgpu::Buffer>,
    /// Chunks the last full load made, for stats.
    pub(crate) total_chunks: u32,
    /// Chunks the last frame drew after culling, for stats.
    pub(crate) visible_chunks: u32,
    /// The entity the loaded terrain belongs to, so a `style_mask` on it
    /// masks the chunks (#310). Set by [`SceneRenderer::set_terrain_entity`].
    ///
    /// [`SceneRenderer::set_terrain_entity`]: super::SceneRenderer::set_terrain_entity
    pub(crate) entity: Option<flint_core::EntityId>,
    /// Each distinct triangle list among the chunks, kept once (a terrain's
    /// chunks share one: the same grid, raised differently), with its edges
    /// as a line list once a wireframe view has asked (#1692). A chunk's
    /// `shape` indexes it. `upload_chunks` replaces it.
    pub(crate) shapes: Vec<ChunkShape>,
    /// Chunks along x and z (`TerrainChunk::grid_pos`'s extent), which the
    /// wire's chunk borders read.
    pub(crate) grid: [f32; 2],
}

/// One triangle list the terrain's chunks are drawn with, and its edges.
pub(crate) struct ChunkShape {
    triangles: Vec<u32>,
    /// The triangles' edges, each once, as a line list: made the first
    /// frame a wireframe view draws the terrain (`ensure_wires`).
    pub(crate) wire: Option<(wgpu::Buffer, u32)>,
}

impl TerrainRenderer {
    /// Load terrain chunks and create GPU resources for terrain rendering.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn load_terrain(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: Option<&TerrainPipeline>,
        texture_cache: &mut Option<TextureCache>,
        transform_layout: &wgpu::BindGroupLayout,
        scope: &AssetScope,
        chunks: &[flint_geometry::terrain_chunk::TerrainChunk],
        transform: &Transform,
        texture_tile: f32,
        metallic: f32,
        roughness: f32,
        splat: TerrainSplat<'_>,
        layer_paths: &[String; 4],
        scene_dir: &Path,
    ) {
        let (Some(tp), Some(tc)) = (pipeline, texture_cache.as_mut()) else {
            return;
        };
        // A terrain loaded again (its recipe rebaked, #1697, or another
        // scene's) takes its own maps, not the cache's last ones
        tc.remove_texture(SPLAT);
        for name in LAYERS {
            tc.remove_texture(name);
        }

        // The splat map and the layers are paths the scene names: found by
        // the project's one search (`AssetKind::Files`, ADR 0114), and a
        // missing one is a warning listing every path tried.
        let find = |what: &str, name: &str| -> Option<std::path::PathBuf> {
            scope
                .project
                .resolve(flint_core::project::AssetKind::Files, name, scene_dir)
                .map_err(|not_found| tracing::warn!("{what}: {not_found}"))
                .ok()
        };

        match splat {
            TerrainSplat::None | TerrainSplat::File("") => {}
            TerrainSplat::File(name) => {
                if let Some(path) = find("terrain splat map", name) {
                    let _ = tc.load_file(device, queue, SPLAT, &path);
                }
            }
            TerrainSplat::Pixels { rgba, size } => {
                if let Err(e) = tc.upload_rgba(device, queue, SPLAT, size, size, rgba, false) {
                    tracing::warn!("terrain splat map: {e}");
                }
            }
        }
        for (name, layer_path) in LAYERS.iter().zip(layer_paths) {
            if !layer_path.is_empty() {
                if let Some(path) = find("terrain layer texture", layer_path) {
                    let _ = tc.load_file(device, queue, name, &path);
                }
            }
        }

        self.upload_material(device, tp, tc, texture_tile, metallic, roughness);
        self.total_chunks = chunks.len() as u32;
        self.upload_chunks(device, transform_layout, chunks, transform);

        tracing::info!(
            "Loaded {} terrain chunks ({} draw calls)",
            chunks.len(),
            self.draws.len()
        );
    }

    /// Load terrain from raw data (heightmap-generated chunks + raw splat RGBA).
    /// Like `load_terrain()` but accepts raw splat data instead of a file path.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn load_terrain_from_data(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipeline: Option<&TerrainPipeline>,
        texture_cache: &mut Option<TextureCache>,
        transform_layout: &wgpu::BindGroupLayout,
        chunks: &[flint_geometry::terrain_chunk::TerrainChunk],
        transform: &Transform,
        texture_tile: f32,
        metallic: f32,
        roughness: f32,
        splat_data: &[u8],
        splat_res: u32,
        layer_paths: &[String; 4],
        spec_dir: &Path,
    ) {
        let (Some(tp), Some(tc)) = (pipeline, texture_cache.as_mut()) else {
            return;
        };

        tc.remove_texture(SPLAT);
        let _ = tc.upload_rgba(
            device, queue, SPLAT, splat_res, splat_res, splat_data, false,
        );
        for (name, layer_path) in LAYERS.iter().zip(layer_paths) {
            if !layer_path.is_empty() {
                tc.remove_texture(name);
                let path = spec_dir.join(layer_path);
                if vfs::exists(&path) {
                    let _ = tc.load_file(device, queue, name, &path);
                }
            }
        }

        self.upload_material(device, tp, tc, texture_tile, metallic, roughness);
        self.total_chunks = chunks.len() as u32;
        self.upload_chunks(device, transform_layout, chunks, transform);
    }

    /// Reload only the terrain geometry (vertex/index buffers) without rebuilding
    /// the material bind group. Used for brush sculpting where only the mesh changes.
    pub(crate) fn reload_terrain_geometry(
        &mut self,
        device: &wgpu::Device,
        transform_layout: &wgpu::BindGroupLayout,
        chunks: &[flint_geometry::terrain_chunk::TerrainChunk],
        transform: &Transform,
    ) {
        self.upload_chunks(device, transform_layout, chunks, transform);
    }

    /// The material every chunk shares: the terrain uniforms and the splat
    /// and layer textures as the cache holds them, a missing one drawn
    /// white. Replaces the material buffer and bind group.
    fn upload_material(
        &mut self,
        device: &wgpu::Device,
        pipeline: &TerrainPipeline,
        textures: &TextureCache,
        texture_tile: f32,
        metallic: f32,
        roughness: f32,
    ) {
        let uniforms = TerrainUniforms {
            texture_tile,
            metallic,
            roughness,
            // Tonemapping, the debug view and the chunk grid: written each frame
            frame: TerrainFrameFlags::default(),
        };
        let buffer = device.create_buffer_init_counted(&wgpu::util::BufferInitDescriptor {
            label: Some("Terrain Uniform Buffer"),
            contents: bytemuck::cast_slice(&[uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Binding 0 is the uniforms; then each texture is a view and a
        // sampler, the splat map's at 1 and 2, layer i's at 3 + 2i and 4 + 2i.
        let texture = |name| textures.get(name).unwrap_or(&textures.default_white);
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }];
        for (i, tex) in std::iter::once(SPLAT)
            .chain(LAYERS)
            .map(texture)
            .enumerate()
        {
            let binding = 1 + 2 * i as u32;
            entries.push(wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(&tex.view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: binding + 1,
                resource: wgpu::BindingResource::Sampler(&tex.sampler),
            });
        }
        let bind_group = device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
            layout: &pipeline.material_bind_group_layout,
            entries: &entries,
            label: Some("Terrain Material Bind Group"),
        });

        self.material_buffer = Some(buffer);
        self.material_bind_group = Some(bind_group);
    }

    /// One draw per chunk, placed by `transform`: its vertex, index and
    /// transform buffers and its transform bind group, and its bounds in
    /// the world for culling. Replaces every draw; the material stays.
    pub(crate) fn upload_chunks(
        &mut self,
        device: &wgpu::Device,
        transform_layout: &wgpu::BindGroupLayout,
        chunks: &[flint_geometry::terrain_chunk::TerrainChunk],
        transform: &Transform,
    ) {
        // The old chunks' buffers go before the new ones are made.
        self.draws.clear();
        self.shapes.clear();
        let extent = |axis: fn(&(u32, u32)) -> u32| {
            chunks
                .iter()
                .map(|c| axis(&c.grid_pos) + 1)
                .max()
                .unwrap_or(1) as f32
        };
        self.grid = [extent(|p| p.0), extent(|p| p.1)];

        let model = transform.to_matrix();
        let model_inv_transpose = mat4_inv_transpose(&model);
        let offset = [model[3][0], model[3][1], model[3][2]];

        for chunk in chunks {
            let vertices: Vec<crate::primitives::Vertex> = (0..chunk.positions.len())
                .map(|i| crate::primitives::Vertex {
                    position: chunk.positions[i],
                    normal: chunk.normals[i],
                    color: [1.0, 1.0, 1.0, 1.0],
                    uv: chunk.uvs[i],
                })
                .collect();

            let vertex_buffer =
                device.create_buffer_init_counted(&wgpu::util::BufferInitDescriptor {
                    label: Some("Terrain Chunk Vertex Buffer"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });

            let index_buffer =
                device.create_buffer_init_counted(&wgpu::util::BufferInitDescriptor {
                    label: Some("Terrain Chunk Index Buffer"),
                    contents: bytemuck::cast_slice(&chunk.indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

            let transform_uniforms = TransformUniforms {
                view_proj: [[0.0; 4]; 4], // Updated per frame
                model,
                model_inv_transpose,
                camera_pos: [0.0; 3],
                _pad: 0.0,
            };

            let transform_buffer =
                device.create_buffer_init_counted(&wgpu::util::BufferInitDescriptor {
                    label: Some("Terrain Chunk Transform Buffer"),
                    contents: bytemuck::cast_slice(&[transform_uniforms]),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                });

            let transform_bind_group =
                device.create_bind_group_counted(&wgpu::BindGroupDescriptor {
                    layout: transform_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: transform_buffer.as_entire_binding(),
                    }],
                    label: Some("Terrain Chunk Transform Bind Group"),
                });

            let shape = self.shape_of(&chunk.indices);
            self.draws.push(TerrainDrawCall {
                vertex_buffer,
                index_buffer,
                index_count: chunk.indices.len() as u32,
                transform_buffer,
                transform_bind_group,
                model,
                model_inv_transpose,
                aabb_min: std::array::from_fn(|k| chunk.aabb_min[k] + offset[k]),
                aabb_max: std::array::from_fn(|k| chunk.aabb_max[k] + offset[k]),
                shape,
            });
        }
    }

    /// The index of `triangles` among the shapes, added if it is new.
    fn shape_of(&mut self, triangles: &[u32]) -> usize {
        if let Some(n) = self.shapes.iter().position(|s| s.triangles == triangles) {
            return n;
        }
        self.shapes.push(ChunkShape {
            triangles: triangles.to_vec(),
            wire: None,
        });
        self.shapes.len() - 1
    }

    /// Make each shape's edges as a line list, once: what a wireframe view
    /// draws (#1692). A terrain whose chunks share one shape makes one
    /// buffer.
    pub(crate) fn ensure_wires(&mut self, device: &wgpu::Device) {
        for shape in self.shapes.iter_mut().filter(|s| s.wire.is_none()) {
            let lines = crate::primitives::triangles_to_wireframe_indices(&shape.triangles);
            let buffer = device.create_buffer_init_counted(&wgpu::util::BufferInitDescriptor {
                label: Some("Terrain Chunk Wire Index Buffer"),
                contents: bytemuck::cast_slice(&lines),
                usage: wgpu::BufferUsages::INDEX,
            });
            shape.wire = Some((buffer, lines.len() as u32));
        }
    }

    /// Chunk `draw`'s edges, once a wireframe view has made them.
    pub(crate) fn wire_of(&self, draw: &TerrainDrawCall) -> Option<(&wgpu::Buffer, u32)> {
        let (buffer, count) = self.shapes.get(draw.shape)?.wire.as_ref()?;
        Some((buffer, *count))
    }

    /// Clear terrain draw calls (for scene transitions)
    pub(crate) fn clear(&mut self) {
        self.draws.clear();
        self.shapes.clear();
        self.entity = None;
        self.material_bind_group = None;
        self.material_buffer = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A terrain's chunks are one grid raised differently, so they share one
    /// triangle list, kept once: the wireframe views make one edge buffer a
    /// terrain, not one a chunk (#1692).
    #[test]
    fn chunks_with_the_same_triangles_share_one_shape() {
        let mut terrain = TerrainRenderer::default();
        let grid = [0, 1, 2, 0, 2, 3];
        assert_eq!(terrain.shape_of(&grid), 0);
        assert_eq!(terrain.shape_of(&grid), 0);
        assert_eq!(terrain.shape_of(&[0, 2, 1]), 1);
        assert_eq!(terrain.shape_of(&grid), 0);
        assert_eq!(terrain.shapes.len(), 2);
        assert!(
            terrain.shapes.iter().all(|s| s.wire.is_none()),
            "made only when asked"
        );
    }
}
