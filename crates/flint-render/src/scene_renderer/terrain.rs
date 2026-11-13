//! The loaded terrain, owned by [`TerrainRenderer`] (ADR 0118, #736): its
//! per-chunk draws and the one material bind group they share. The three
//! loaders sit side by side, as they were written; #918 folds their shared
//! chunk upload and material bind group into one helper each.

use super::helpers::mat4_inv_transpose;
use crate::pipeline::TransformUniforms;
use crate::terrain_pipeline::{TerrainDrawCall, TerrainPipeline, TerrainUniforms};
use crate::texture_cache::TextureCache;
use flint_core::project::AssetScope;
use flint_core::vfs;
use flint_core::Transform;
use std::path::Path;
use wgpu::util::DeviceExt;

/// The terrain's chunk draws, the material they share, and the entity it
/// belongs to. Empty until a loader runs; `clear` empties it again.
#[derive(Default)]
pub(crate) struct TerrainRenderer {
    /// One draw per chunk, each with its own vertex, index and transform
    /// buffers. Every loader writes this.
    pub(crate) draws: Vec<TerrainDrawCall>,
    /// The splat and layer textures and the terrain uniforms, bound once
    /// for every chunk. The two full loaders write it; a geometry reload
    /// keeps it.
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

        let tp = match pipeline {
            Some(tp) => tp,
            None => return,
        };

        // Load textures into texture cache
        let tc = match texture_cache {
            Some(tc) => tc,
            None => return,
        };

        // Load splat map
        if !splat_path.is_empty() {
            if let Some(path) = find("terrain splat map", splat_path) {
                let _ = tc.load_file(device, queue, "terrain_splat", &path);
            }
        }

        // Load layer textures
        for (i, layer_path) in layer_paths.iter().enumerate() {
            if !layer_path.is_empty() {
                if let Some(path) = find("terrain layer texture", layer_path) {
                    let name = format!("terrain_layer{}", i);
                    let _ = tc.load_file(device, queue, &name, &path);
                }
            }
        }

        // Get texture references (fallback to white)
        let tc = texture_cache.as_ref().unwrap();

        let splat_tex = tc.get("terrain_splat").unwrap_or(&tc.default_white);
        let layer0_tex = tc.get("terrain_layer0").unwrap_or(&tc.default_white);
        let layer1_tex = tc.get("terrain_layer1").unwrap_or(&tc.default_white);
        let layer2_tex = tc.get("terrain_layer2").unwrap_or(&tc.default_white);
        let layer3_tex = tc.get("terrain_layer3").unwrap_or(&tc.default_white);

        // Create terrain uniform buffer
        let terrain_uniforms = TerrainUniforms {
            texture_tile,
            metallic,
            roughness,
            enable_tonemapping: 0, // Will be updated per-frame
        };

        let material_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Terrain Uniform Buffer"),
            contents: bytemuck::cast_slice(&[terrain_uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Create material bind group (shared across all terrain chunks)
        let material_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &tp.material_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: material_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&splat_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&splat_tex.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&layer0_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&layer0_tex.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&layer1_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(&layer1_tex.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&layer2_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&layer2_tex.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(&layer3_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::Sampler(&layer3_tex.sampler),
                },
            ],
            label: Some("Terrain Material Bind Group"),
        });

        self.material_buffer = Some(material_buffer);
        self.material_bind_group = Some(material_bind_group);

        // Create per-chunk draw calls
        self.draws.clear();
        self.total_chunks = chunks.len() as u32;

        let model = transform.to_matrix();
        let model_inv_transpose = mat4_inv_transpose(&model);

        let tx = model[3][0];
        let ty = model[3][1];
        let tz = model[3][2];

        for chunk in chunks {
            // Build Vertex array from chunk data
            let vertices: Vec<crate::primitives::Vertex> = (0..chunk.positions.len())
                .map(|i| crate::primitives::Vertex {
                    position: chunk.positions[i],
                    normal: chunk.normals[i],
                    color: [1.0, 1.0, 1.0, 1.0],
                    uv: chunk.uvs[i],
                })
                .collect();

            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Terrain Chunk Vertex Buffer"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

            let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
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

            let transform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Terrain Chunk Transform Buffer"),
                contents: bytemuck::cast_slice(&[transform_uniforms]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

            let transform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
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
                aabb_min: [
                    chunk.aabb_min[0] + tx,
                    chunk.aabb_min[1] + ty,
                    chunk.aabb_min[2] + tz,
                ],
                aabb_max: [
                    chunk.aabb_max[0] + tx,
                    chunk.aabb_max[1] + ty,
                    chunk.aabb_max[2] + tz,
                ],
            });
        }

        tracing::info!(
            "Loaded {} terrain chunks ({} draw calls)",
            chunks.len(),
            self.draws.len()
        );
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
        // Keep existing material bind group and buffer
        self.draws.clear();

        let model = transform.to_matrix();
        let model_inv_transpose = mat4_inv_transpose(&model);

        let tx = model[3][0];
        let ty = model[3][1];
        let tz = model[3][2];

        for chunk in chunks {
            let vertices: Vec<crate::primitives::Vertex> = (0..chunk.positions.len())
                .map(|i| crate::primitives::Vertex {
                    position: chunk.positions[i],
                    normal: chunk.normals[i],
                    color: [1.0, 1.0, 1.0, 1.0],
                    uv: chunk.uvs[i],
                })
                .collect();

            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Terrain Chunk Vertex Buffer"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

            let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Terrain Chunk Index Buffer"),
                contents: bytemuck::cast_slice(&chunk.indices),
                usage: wgpu::BufferUsages::INDEX,
            });

            let transform_uniforms = TransformUniforms {
                view_proj: [[0.0; 4]; 4],
                model,
                model_inv_transpose,
                camera_pos: [0.0; 3],
                _pad: 0.0,
            };

            let transform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Terrain Chunk Transform Buffer"),
                contents: bytemuck::cast_slice(&[transform_uniforms]),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });

            let transform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
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
                aabb_min: [
                    chunk.aabb_min[0] + tx,
                    chunk.aabb_min[1] + ty,
                    chunk.aabb_min[2] + tz,
                ],
                aabb_max: [
                    chunk.aabb_max[0] + tx,
                    chunk.aabb_max[1] + ty,
                    chunk.aabb_max[2] + tz,
                ],
            });
        }
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
        let tp = match pipeline {
            Some(tp) => tp,
            None => return,
        };

        // Load textures into texture cache
        let tc = match texture_cache {
            Some(tc) => tc,
            None => return,
        };

        // Upload splat map from raw data
        tc.remove_texture("terrain_splat");
        let _ = tc.upload_rgba(
            device,
            queue,
            "terrain_splat",
            splat_res,
            splat_res,
            splat_data,
            false,
        );

        // Load layer textures
        for (i, layer_path) in layer_paths.iter().enumerate() {
            if !layer_path.is_empty() {
                let name = format!("terrain_layer{}", i);
                tc.remove_texture(&name);
                let path = spec_dir.join(layer_path);
                if vfs::exists(&path) {
                    let _ = tc.load_file(device, queue, &name, &path);
                }
            }
        }

        // Get texture references (fallback to white)
        let tc = texture_cache.as_ref().unwrap();

        let splat_tex = tc.get("terrain_splat").unwrap_or(&tc.default_white);
        let layer0_tex = tc.get("terrain_layer0").unwrap_or(&tc.default_white);
        let layer1_tex = tc.get("terrain_layer1").unwrap_or(&tc.default_white);
        let layer2_tex = tc.get("terrain_layer2").unwrap_or(&tc.default_white);
        let layer3_tex = tc.get("terrain_layer3").unwrap_or(&tc.default_white);

        // Create terrain uniform buffer
        let terrain_uniforms = TerrainUniforms {
            texture_tile,
            metallic,
            roughness,
            enable_tonemapping: 0,
        };

        let material_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Terrain Uniform Buffer"),
            contents: bytemuck::cast_slice(&[terrain_uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let material_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &tp.material_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: material_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&splat_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&splat_tex.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&layer0_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&layer0_tex.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&layer1_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(&layer1_tex.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&layer2_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&layer2_tex.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(&layer3_tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::Sampler(&layer3_tex.sampler),
                },
            ],
            label: Some("Terrain Material Bind Group"),
        });

        self.material_buffer = Some(material_buffer);
        self.material_bind_group = Some(material_bind_group);

        // Build chunk draw calls
        self.total_chunks = chunks.len() as u32;
        self.reload_terrain_geometry(device, transform_layout, chunks, transform);
    }

    /// Clear terrain draw calls (for scene transitions)
    pub(crate) fn clear(&mut self) {
        self.draws.clear();
        self.entity = None;
        self.material_bind_group = None;
        self.material_buffer = None;
    }
}
