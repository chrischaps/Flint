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
use crate::terrain_pipeline::{TerrainDrawCall, TerrainPipeline, TerrainUniforms};
use crate::texture_cache::TextureCache;
use flint_core::project::AssetScope;
use flint_core::vfs;
use flint_core::Transform;
use std::path::Path;

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
        chunks: &[flint_terrain::TerrainChunk],
        transform: &Transform,
        texture_tile: f32,
        metallic: f32,
        roughness: f32,
        splat_path: &str,
        layer_paths: &[String; 4],
        scene_dir: &Path,
    ) {
        let (Some(tp), Some(tc)) = (pipeline, texture_cache.as_mut()) else {
            return;
        };

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

        if !splat_path.is_empty() {
            if let Some(path) = find("terrain splat map", splat_path) {
                let _ = tc.load_file(device, queue, SPLAT, &path);
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
        chunks: &[flint_terrain::TerrainChunk],
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
        chunks: &[flint_terrain::TerrainChunk],
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
            enable_tonemapping: 0, // Updated per frame
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
        chunks: &[flint_terrain::TerrainChunk],
        transform: &Transform,
    ) {
        // The old chunks' buffers go before the new ones are made.
        self.draws.clear();

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
            });
        }
    }

    /// Clear terrain draw calls (for scene transitions)
    pub(crate) fn clear(&mut self) {
        self.draws.clear();
        self.entity = None;
        self.material_bind_group = None;
        self.material_buffer = None;
    }
}
