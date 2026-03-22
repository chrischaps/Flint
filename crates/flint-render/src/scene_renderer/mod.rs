//! Scene renderer - converts FlintWorld entities to GPU meshes

mod extract;
mod helpers;
mod render_passes;

use helpers::{identity_matrix, mat4_inv_transpose};

use crate::billboard_pipeline::BillboardPipeline;
use crate::bitmap_font::BitmapFont;
use crate::camera::Camera;
use crate::context::RenderContext;
use crate::debug::{DebugMode, DebugState};
use crate::gpu_mesh::MeshCache;
use crate::grass_pipeline::{
    GrassComputeUniforms, GrassEntityPosition, GrassInstanceGpu, GrassPipeline,
    GrassRenderUniforms, MAX_GRASS_ENTITIES,
};
use crate::particle_pipeline::{ParticleDrawCall, ParticleDrawData, ParticlePipeline};
use crate::pipeline::{
    BlendMode, DirectionalLight, LightUniforms, MaterialUniforms, PointLight, RenderPipeline,
    SpotLight, TransformUniforms, MAX_DIRECTIONAL_LIGHTS, MAX_POINT_LIGHTS, MAX_SPOT_LIGHTS,
};
use crate::postprocess::{
    PostProcessConfig, PostProcessPipeline, PostProcessResources, HDR_FORMAT,
};
use crate::primitives::create_grid_mesh;
use crate::shadow::{ShadowPass, DEFAULT_SHADOW_RESOLUTION};
use crate::skinned_pipeline::SkinnedPipeline;
use crate::skybox_pipeline::{SkyboxPipeline, SkyboxUniforms};
use crate::sprite2d_pipeline::{Sprite2dInstanceGpu, Sprite2dPipeline};
use crate::terrain_pipeline::{TerrainDrawCall, TerrainPipeline, TerrainUniforms};
use crate::texture_cache::TextureCache;
use flint_core::components as comp;
use flint_core::toml_util::toml_f32;
use flint_core::{Transform, Vec3};
use flint_ecs::FlintWorld;
use flint_import::ImportResult;
use std::collections::HashMap;
use std::path::Path;
use wgpu::util::DeviceExt;

/// Visual representation for an archetype
#[derive(Clone)]
pub struct ArchetypeVisual {
    pub color: [f32; 4],
    pub wireframe: bool,
    pub default_size: [f32; 3],
}

impl Default for ArchetypeVisual {
    fn default() -> Self {
        Self {
            color: [0.5, 0.5, 0.5, 1.0],
            wireframe: false,
            default_size: [1.0, 1.0, 1.0],
        }
    }
}

/// A single draw call with its own GPU resources
#[allow(dead_code)]
struct DrawCall {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    is_wireframe: bool,
    transform_buffer: wgpu::Buffer,
    transform_bind_group: wgpu::BindGroup,
    material_buffer: wgpu::Buffer, // kept alive for bind group
    material_bind_group: wgpu::BindGroup,
    model: [[f32; 4]; 4],
    model_inv_transpose: [[f32; 4]; 4],
    entity_id: Option<flint_core::EntityId>,
    blend_mode: BlendMode,
    sort_depth: f32,
}

/// A draw call for a skinned mesh (has bone bind group)
#[allow(dead_code)]
struct SkinnedDrawCall {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    transform_buffer: wgpu::Buffer,
    transform_bind_group: wgpu::BindGroup,
    material_buffer: wgpu::Buffer,
    material_bind_group: wgpu::BindGroup,
    bone_bind_group: wgpu::BindGroup,
    model: [[f32; 4]; 4],
    model_inv_transpose: [[f32; 4]; 4],
    entity_id: Option<flint_core::EntityId>,
    blend_mode: BlendMode,
    sort_depth: f32,
}

/// Configuration for creating a SceneRenderer
pub struct RendererConfig {
    /// Show the ground-plane grid (useful for debug/inspection modes)
    pub show_grid: bool,
}

impl Default for RendererConfig {
    fn default() -> Self {
        Self { show_grid: false }
    }
}

/// Renders a FlintWorld to the screen
pub struct SceneRenderer {
    pipeline: RenderPipeline,
    skinned_pipeline: Option<SkinnedPipeline>,
    billboard_pipeline: Option<BillboardPipeline>,
    archetype_visuals: HashMap<String, ArchetypeVisual>,
    mesh_cache: MeshCache,
    grid_draw: Option<DrawCall>,
    entity_draws: Vec<DrawCall>,
    skinned_entity_draws: Vec<SkinnedDrawCall>,
    transparent_draws: Vec<DrawCall>,
    transparent_skinned_draws: Vec<SkinnedDrawCall>,
    billboard_draws: Vec<crate::billboard_pipeline::BillboardDrawCall>,
    debug_state: DebugState,
    wireframe_overlay_draws: Vec<DrawCall>,
    normal_arrow_draws: Vec<DrawCall>,
    skeleton_overlay_draws: Vec<DrawCall>,
    tonemapping_enabled: bool,
    light_buffer: wgpu::Buffer,
    light_bind_group: wgpu::BindGroup,
    light_uniforms: LightUniforms,
    texture_cache: Option<TextureCache>,
    shadow_pass: Option<ShadowPass>,
    selected_entity: Option<flint_core::EntityId>,
    // Skybox
    skybox_pipeline: Option<SkyboxPipeline>,
    skybox_uniform_buffer: Option<wgpu::Buffer>,
    skybox_uniform_bind_group: Option<wgpu::BindGroup>,
    skybox_texture_bind_group: Option<wgpu::BindGroup>,
    // Terrain
    terrain_pipeline: Option<TerrainPipeline>,
    terrain_draws: Vec<TerrainDrawCall>,
    terrain_material_bind_group: Option<wgpu::BindGroup>,
    terrain_material_buffer: Option<wgpu::Buffer>,
    terrain_total_chunks: u32,
    terrain_visible_chunks: u32,
    camera_frustum: Option<crate::frustum::Frustum>,
    // Grass
    grass_pipeline: Option<GrassPipeline>,
    grass_instance_buffer: Option<wgpu::Buffer>,
    grass_instance_count: u32,
    grass_max_instances: u32,
    grass_counter_buffer: Option<wgpu::Buffer>,
    grass_staging_buffer: Option<wgpu::Buffer>,
    grass_compute_uniform_buffer: Option<wgpu::Buffer>,
    grass_compute_uniform_bind_group: Option<wgpu::BindGroup>,
    grass_compute_texture_bind_group: Option<wgpu::BindGroup>,
    grass_compute_storage_bind_group: Option<wgpu::BindGroup>,
    grass_render_uniform_buffer: Option<wgpu::Buffer>,
    grass_render_uniform_bind_group: Option<wgpu::BindGroup>,
    grass_render_instance_bind_group: Option<wgpu::BindGroup>,
    grass_entity_buffer: Option<wgpu::Buffer>,
    grass_entity_count: u32,
    grass_config: Option<flint_terrain::GrassConfig>,
    grass_terrain_offset: [f32; 3],
    grass_terrain_width: f32,
    grass_terrain_depth: f32,
    grass_terrain_height_scale: f32,
    // Particles
    particle_pipeline: Option<ParticlePipeline>,
    particle_draws: Vec<ParticleDrawCall>,
    // 2D sprites
    sprite2d_pipeline: Option<Sprite2dPipeline>,
    sprite2d_batches: Vec<crate::sprite2d_pipeline::Sprite2dBatch>,
    // Post-processing
    postprocess_pipeline: Option<PostProcessPipeline>,
    postprocess_resources: Option<PostProcessResources>,
    postprocess_config: PostProcessConfig,
    #[allow(dead_code)]
    surface_format: wgpu::TextureFormat,
    // Parallax: camera offset in world units + orthographic viewport height
    pub camera_offset: [f32; 2],
    pub ortho_height: f32,
    pub aspect_ratio: f32,
    // Bitmap font cache for ui_text rendering
    bitmap_font_cache: HashMap<String, BitmapFont>,
    /// Scene directory for resolving relative font/texture paths
    pub scene_dir: Option<std::path::PathBuf>,
    /// Time in seconds, used for grass wind animation. Set by the player before render().
    pub grass_time: f32,
    /// Set when GPU device is lost (e.g. driver crash); skips all rendering
    device_lost: bool,
}

impl SceneRenderer {
    pub fn new(context: &RenderContext, config: RendererConfig) -> Self {
        let surface_format = context.config.format;
        // Scene geometry renders to HDR; the composite pass tonemaps to the surface.
        let scene_format = HDR_FORMAT;

        let pipeline = RenderPipeline::new(&context.device, scene_format);
        let archetype_visuals = Self::default_archetype_visuals();
        let texture_cache = TextureCache::new(&context.device, &context.queue);

        // Create grid draw call (only for debug/inspection modes)
        let grid_draw = if config.show_grid {
            let grid = create_grid_mesh(40.0, 40, [0.3, 0.3, 0.3, 0.5]);
            Some(Self::create_draw_call(
                &context.device,
                &pipeline,
                &grid,
                true,
                TransformUniforms::new(),
                MaterialUniforms::procedural(),
                &texture_cache,
            ))
        } else {
            None
        };

        let shadow_pass = ShadowPass::new(&context.device, DEFAULT_SHADOW_RESOLUTION);

        let light_uniforms = LightUniforms::default_scene_lights();
        let (light_buffer, light_bind_group) =
            Self::create_light_bind(&context.device, &pipeline, &light_uniforms, &shadow_pass);

        let skinned_pipeline = SkinnedPipeline::new(
            &context.device,
            scene_format,
            &pipeline.transform_bind_group_layout,
            &pipeline.material_bind_group_layout,
            &pipeline.light_bind_group_layout,
        );

        let billboard_pipeline = BillboardPipeline::new(&context.device, scene_format);
        let terrain_pipeline = TerrainPipeline::new(
            &context.device,
            scene_format,
            &pipeline.transform_bind_group_layout,
            &pipeline.light_bind_group_layout,
        );

        // Graceful degradation: wrap in catch_unwind like the Kuwahara pipeline.
        // If compute shaders aren't supported, grass is silently disabled.
        let grass_pipeline = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            GrassPipeline::new(
                &context.device,
                scene_format,
                &pipeline.transform_bind_group_layout,
                &pipeline.light_bind_group_layout,
            )
        }))
        .unwrap_or_else(|_| {
            tracing::warn!("Grass pipeline creation failed — grass disabled");
            None
        });

        let particle_pipeline = ParticlePipeline::new(&context.device, scene_format);
        let sprite2d_pipeline = Sprite2dPipeline::new(&context.device, scene_format);
        let skybox_pipeline = SkyboxPipeline::new(&context.device, scene_format);

        // Create post-processing pipeline and resources
        let postprocess_config = PostProcessConfig::default();
        let postprocess_pipeline = PostProcessPipeline::new(
            &context.device,
            &context.queue,
            surface_format,
            postprocess_config.kuwahara_enabled,
        );
        let postprocess_resources = PostProcessResources::new(
            &context.device,
            context.config.width,
            context.config.height,
            postprocess_config.kuwahara_enabled,
        );

        Self {
            pipeline,
            skinned_pipeline: Some(skinned_pipeline),
            billboard_pipeline: Some(billboard_pipeline),
            archetype_visuals,
            mesh_cache: MeshCache::new(),
            grid_draw,
            entity_draws: Vec::new(),
            skinned_entity_draws: Vec::new(),
            transparent_draws: Vec::new(),
            transparent_skinned_draws: Vec::new(),
            billboard_draws: Vec::new(),
            debug_state: DebugState::default(),
            wireframe_overlay_draws: Vec::new(),
            normal_arrow_draws: Vec::new(),
            skeleton_overlay_draws: Vec::new(),
            tonemapping_enabled: true,
            light_buffer,
            light_bind_group,
            light_uniforms,
            texture_cache: Some(texture_cache),
            shadow_pass: Some(shadow_pass),
            selected_entity: None,
            skybox_pipeline: Some(skybox_pipeline),
            skybox_uniform_buffer: None,
            skybox_uniform_bind_group: None,
            skybox_texture_bind_group: None,
            terrain_pipeline: Some(terrain_pipeline),
            terrain_draws: Vec::new(),
            terrain_material_bind_group: None,
            terrain_material_buffer: None,
            terrain_total_chunks: 0,
            terrain_visible_chunks: 0,
            camera_frustum: None,
            grass_pipeline,
            grass_instance_buffer: None,
            grass_instance_count: 0,
            grass_max_instances: 0,
            grass_counter_buffer: None,
            grass_staging_buffer: None,
            grass_compute_uniform_buffer: None,
            grass_compute_uniform_bind_group: None,
            grass_compute_texture_bind_group: None,
            grass_compute_storage_bind_group: None,
            grass_render_uniform_buffer: None,
            grass_render_uniform_bind_group: None,
            grass_render_instance_bind_group: None,
            grass_entity_buffer: None,
            grass_entity_count: 0,
            grass_config: None,
            grass_terrain_offset: [0.0; 3],
            grass_terrain_width: 0.0,
            grass_terrain_depth: 0.0,
            grass_terrain_height_scale: 0.0,
            particle_pipeline: Some(particle_pipeline),
            particle_draws: Vec::new(),
            sprite2d_pipeline: Some(sprite2d_pipeline),
            sprite2d_batches: Vec::new(),
            postprocess_pipeline: Some(postprocess_pipeline),
            postprocess_resources: Some(postprocess_resources),
            postprocess_config,
            surface_format,
            camera_offset: [0.0, 0.0],
            ortho_height: 10.0,
            aspect_ratio: 16.0 / 9.0,
            bitmap_font_cache: HashMap::new(),
            scene_dir: None,
            grass_time: 0.0,
            device_lost: false,
        }
    }

    /// Upload particle instance data from the simulation and create draw calls.
    /// Called each frame after ParticleSystem::update().
    pub fn update_particles(&mut self, device: &wgpu::Device, draw_data: Vec<ParticleDrawData>) {
        self.particle_draws.clear();

        let pp = match &self.particle_pipeline {
            Some(pp) => pp,
            None => return,
        };

        for data in &draw_data {
            if data.instances.is_empty() {
                continue;
            }

            // Create storage buffer with instance data
            let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Particle Instance Buffer"),
                contents: bytemuck::cast_slice(data.instances),
                usage: wgpu::BufferUsages::STORAGE,
            });

            // Create instance bind group
            let instance_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &pp.instance_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: instance_buffer.as_entire_binding(),
                }],
                label: Some("Particle Instance Bind Group"),
            });

            // Resolve texture (use white fallback if none specified)
            let texture_bind_group = if !data.texture.is_empty() {
                if let Some(tc) = &self.texture_cache {
                    if let Some(tex) = tc.get(data.texture) {
                        device.create_bind_group(&wgpu::BindGroupDescriptor {
                            layout: &pp.texture_bind_group_layout,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::TextureView(&tex.view),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: wgpu::BindingResource::Sampler(&tex.sampler),
                                },
                            ],
                            label: Some("Particle Texture Bind Group"),
                        })
                    } else {
                        self.create_white_particle_texture_bind_group(device, pp)
                    }
                } else {
                    self.create_white_particle_texture_bind_group(device, pp)
                }
            } else {
                self.create_white_particle_texture_bind_group(device, pp)
            };

            self.particle_draws.push(ParticleDrawCall {
                instance_buffer,
                instance_count: data.instances.len() as u32,
                texture_bind_group,
                instance_bind_group,
                additive: data.additive,
            });
        }
    }

    fn create_white_particle_texture_bind_group(
        &self,
        device: &wgpu::Device,
        pp: &ParticlePipeline,
    ) -> wgpu::BindGroup {
        if let Some(tc) = &self.texture_cache {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &pp.texture_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&tc.default_white.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&tc.default_white.sampler),
                    },
                ],
                label: Some("Particle White Texture Bind Group"),
            })
        } else {
            panic!("TextureCache required for particle rendering");
        }
    }

    /// Load terrain chunks and create GPU resources for terrain rendering.
    pub fn load_terrain(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        chunks: &[flint_terrain::TerrainChunk],
        transform: &Transform,
        texture_tile: f32,
        metallic: f32,
        roughness: f32,
        splat_path: &str,
        layer_paths: &[String; 4],
        scene_dir: &Path,
    ) {
        let tp = match &self.terrain_pipeline {
            Some(tp) => tp,
            None => return,
        };

        // Load textures into texture cache
        let tc = match &mut self.texture_cache {
            Some(tc) => tc,
            None => return,
        };

        // Load splat map
        if !splat_path.is_empty() {
            let path = scene_dir.join(splat_path);
            if path.exists() {
                let _ = tc.load_file(device, queue, "terrain_splat", &path);
            }
        }

        // Load layer textures
        for (i, layer_path) in layer_paths.iter().enumerate() {
            if !layer_path.is_empty() {
                let path = scene_dir.join(layer_path);
                if path.exists() {
                    let name = format!("terrain_layer{}", i);
                    let _ = tc.load_file(device, queue, &name, &path);
                }
            }
        }

        // Get texture references (fallback to white)
        let tc = self.texture_cache.as_ref().unwrap();

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

        self.terrain_material_buffer = Some(material_buffer);
        self.terrain_material_bind_group = Some(material_bind_group);

        // Create per-chunk draw calls
        self.terrain_draws.clear();
        self.terrain_total_chunks = chunks.len() as u32;

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
                layout: &self.pipeline.transform_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: transform_buffer.as_entire_binding(),
                }],
                label: Some("Terrain Chunk Transform Bind Group"),
            });

            self.terrain_draws.push(TerrainDrawCall {
                vertex_buffer,
                index_buffer,
                index_count: chunk.indices.len() as u32,
                transform_buffer,
                transform_bind_group,
                model,
                model_inv_transpose,
                aabb_min: [chunk.aabb_min[0] + tx, chunk.aabb_min[1] + ty, chunk.aabb_min[2] + tz],
                aabb_max: [chunk.aabb_max[0] + tx, chunk.aabb_max[1] + ty, chunk.aabb_max[2] + tz],
            });
        }

        tracing::info!(
            "Loaded {} terrain chunks ({} draw calls)",
            chunks.len(),
            self.terrain_draws.len()
        );
    }

    /// Reload only the terrain geometry (vertex/index buffers) without rebuilding
    /// the material bind group. Used for brush sculpting where only the mesh changes.
    pub fn reload_terrain_geometry(
        &mut self,
        device: &wgpu::Device,
        chunks: &[flint_terrain::TerrainChunk],
        transform: &Transform,
    ) {
        // Keep existing material bind group and buffer
        self.terrain_draws.clear();

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
                layout: &self.pipeline.transform_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: transform_buffer.as_entire_binding(),
                }],
                label: Some("Terrain Chunk Transform Bind Group"),
            });

            self.terrain_draws.push(TerrainDrawCall {
                vertex_buffer,
                index_buffer,
                index_count: chunk.indices.len() as u32,
                transform_buffer,
                transform_bind_group,
                model,
                model_inv_transpose,
                aabb_min: [chunk.aabb_min[0] + tx, chunk.aabb_min[1] + ty, chunk.aabb_min[2] + tz],
                aabb_max: [chunk.aabb_max[0] + tx, chunk.aabb_max[1] + ty, chunk.aabb_max[2] + tz],
            });
        }
    }

    /// Load terrain from raw data (heightmap-generated chunks + raw splat RGBA).
    /// Like `load_terrain()` but accepts raw splat data instead of a file path.
    pub fn load_terrain_from_data(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
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
        let tp = match &self.terrain_pipeline {
            Some(tp) => tp,
            None => return,
        };

        // Load textures into texture cache
        let tc = match &mut self.texture_cache {
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
                if path.exists() {
                    let _ = tc.load_file(device, queue, &name, &path);
                }
            }
        }

        // Get texture references (fallback to white)
        let tc = self.texture_cache.as_ref().unwrap();

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

        self.terrain_material_buffer = Some(material_buffer);
        self.terrain_material_bind_group = Some(material_bind_group);

        // Build chunk draw calls
        self.terrain_total_chunks = chunks.len() as u32;
        self.reload_terrain_geometry(device, chunks, transform);
    }

    /// Clear terrain draw calls (for scene transitions)
    pub fn clear_terrain(&mut self) {
        self.terrain_draws.clear();
        self.terrain_material_bind_group = None;
        self.terrain_material_buffer = None;
    }

    /// Initialize grass GPU resources for the loaded terrain.
    /// Call after load_terrain() when grass is enabled.
    #[allow(clippy::too_many_arguments)]
    pub fn load_grass(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: &flint_terrain::GrassConfig,
        heightmap_data: &[f32],
        heightmap_width: u32,
        heightmap_depth: u32,
        splat_data: &[u8],
        splat_width: u32,
        splat_height: u32,
        terrain_offset: [f32; 3],
        terrain_width: f32,
        terrain_depth: f32,
        height_scale: f32,
    ) {
        let grass_pipeline = match &self.grass_pipeline {
            Some(p) => p,
            None => return,
        };

        let max_instances = config.max_instances(terrain_width, terrain_depth);
        let instance_buffer_size =
            (max_instances as u64) * std::mem::size_of::<GrassInstanceGpu>() as u64;

        // Instance storage buffer (compute writes, render reads)
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Instance Buffer"),
            size: instance_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Atomic counter buffer (u32)
        let counter_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Counter Buffer"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Staging buffer for reading counter back to CPU
        let staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Staging Buffer"),
            size: 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Compute uniform buffer
        let compute_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Compute Uniform Buffer"),
            size: std::mem::size_of::<GrassComputeUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let compute_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.compute_uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: compute_uniform_buffer.as_entire_binding(),
            }],
            label: Some("Grass Compute Uniform Bind Group"),
        });

        // Upload heightmap as R32Float texture
        let hm_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Grass Heightmap Texture"),
            size: wgpu::Extent3d {
                width: heightmap_width,
                height: heightmap_depth,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &hm_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(heightmap_data),
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(heightmap_width * 4),
                rows_per_image: Some(heightmap_depth),
            },
            wgpu::Extent3d {
                width: heightmap_width,
                height: heightmap_depth,
                depth_or_array_layers: 1,
            },
        );

        // Upload splat map as RGBA8 texture
        let splat_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Grass Splat Texture"),
            size: wgpu::Extent3d {
                width: splat_width,
                height: splat_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &splat_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            splat_data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(splat_width * 4),
                rows_per_image: Some(splat_height),
            },
            wgpu::Extent3d {
                width: splat_width,
                height: splat_height,
                depth_or_array_layers: 1,
            },
        );

        // Heightmap uses nearest (non-filtering) sampler since R32Float is not filterable
        let nearest_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Grass Nearest Sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Grass Linear Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let hm_view = hm_texture.create_view(&Default::default());
        let splat_view = splat_texture.create_view(&Default::default());

        let compute_texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.compute_texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&hm_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&nearest_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&splat_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&linear_sampler),
                },
            ],
            label: Some("Grass Compute Texture Bind Group"),
        });

        let compute_storage_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.compute_storage_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: instance_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: counter_buffer.as_entire_binding(),
                },
            ],
            label: Some("Grass Compute Storage Bind Group"),
        });

        // Render uniform buffer
        let render_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Render Uniform Buffer"),
            size: std::mem::size_of::<GrassRenderUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let render_uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.render_grass_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: render_uniform_buffer.as_entire_binding(),
            }],
            label: Some("Grass Render Uniform Bind Group"),
        });

        // Entity positions buffer
        let entity_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Entity Buffer"),
            size: (MAX_GRASS_ENTITIES * std::mem::size_of::<GrassEntityPosition>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let render_instance_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &grass_pipeline.render_instance_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: instance_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: entity_buffer.as_entire_binding(),
                },
            ],
            label: Some("Grass Render Instance Bind Group"),
        });

        // Store everything
        self.grass_instance_buffer = Some(instance_buffer);
        self.grass_instance_count = 0;
        self.grass_max_instances = max_instances;
        self.grass_counter_buffer = Some(counter_buffer);
        self.grass_staging_buffer = Some(staging_buffer);
        self.grass_compute_uniform_buffer = Some(compute_uniform_buffer);
        self.grass_compute_uniform_bind_group = Some(compute_uniform_bind_group);
        self.grass_compute_texture_bind_group = Some(compute_texture_bind_group);
        self.grass_compute_storage_bind_group = Some(compute_storage_bind_group);
        self.grass_render_uniform_buffer = Some(render_uniform_buffer);
        self.grass_render_uniform_bind_group = Some(render_uniform_bind_group);
        self.grass_render_instance_bind_group = Some(render_instance_bind_group);
        self.grass_entity_buffer = Some(entity_buffer);
        self.grass_config = Some(config.clone());
        self.grass_terrain_offset = terrain_offset;
        self.grass_terrain_width = terrain_width;
        self.grass_terrain_depth = terrain_depth;
        self.grass_terrain_height_scale = height_scale;

        tracing::info!(
            "Grass loaded: max {} instances, {:.1}MB buffer",
            max_instances,
            instance_buffer_size as f64 / (1024.0 * 1024.0)
        );
    }

    /// Clear all grass GPU resources.
    pub fn unload_grass(&mut self) {
        self.grass_instance_buffer = None;
        self.grass_instance_count = 0;
        self.grass_max_instances = 0;
        self.grass_counter_buffer = None;
        self.grass_staging_buffer = None;
        self.grass_compute_uniform_buffer = None;
        self.grass_compute_uniform_bind_group = None;
        self.grass_compute_texture_bind_group = None;
        self.grass_compute_storage_bind_group = None;
        self.grass_render_uniform_buffer = None;
        self.grass_render_uniform_bind_group = None;
        self.grass_render_instance_bind_group = None;
        self.grass_entity_buffer = None;
        self.grass_entity_count = 0;
        self.grass_config = None;
    }

    /// Update grass config without buffer reallocation.
    /// Compute and render passes read `grass_config` fresh each frame to build
    /// uniforms, so changes take effect on the next `render()` call.
    pub fn set_grass_config(&mut self, config: flint_terrain::GrassConfig) {
        self.grass_config = Some(config);
    }

    /// Reallocate the grass instance buffer for a new density value,
    /// reusing existing heightmap/splat GPU textures.
    /// Call this when `GrassConfig.density` changes (affects buffer capacity).
    pub fn reload_grass_config(
        &mut self,
        device: &wgpu::Device,
        config: flint_terrain::GrassConfig,
    ) {
        let grass_pipeline = match &self.grass_pipeline {
            Some(p) => p,
            None => return,
        };

        let max_instances = config.max_instances(self.grass_terrain_width, self.grass_terrain_depth);
        let instance_buffer_size =
            (max_instances as u64) * std::mem::size_of::<GrassInstanceGpu>() as u64;

        // Reallocate instance buffer
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Instance Buffer"),
            size: instance_buffer_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Reallocate counter buffer
        let counter_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Counter Buffer"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Reallocate staging buffer
        let staging_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Grass Staging Buffer"),
            size: 4,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Recreate compute storage bind group (binds instance buffer at binding 0)
        let compute_storage_bind_group =
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &grass_pipeline.compute_storage_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: instance_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: counter_buffer.as_entire_binding(),
                    },
                ],
                label: Some("Grass Compute Storage Bind Group"),
            });

        // Recreate render instance bind group (binds instance buffer at binding 0)
        let entity_buffer = self.grass_entity_buffer.as_ref().expect("grass entity buffer");
        let render_instance_bind_group =
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &grass_pipeline.render_instance_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: instance_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: entity_buffer.as_entire_binding(),
                    },
                ],
                label: Some("Grass Render Instance Bind Group"),
            });

        // Update stored state
        // Note: grass_compute_texture_bind_group, compute_uniform_bind_group,
        // render_uniform_bind_group, and entity_buffer are intentionally NOT
        // recreated — they don't reference the instance buffer.
        self.grass_instance_buffer = Some(instance_buffer);
        self.grass_instance_count = 0;
        self.grass_max_instances = max_instances;
        self.grass_counter_buffer = Some(counter_buffer);
        self.grass_staging_buffer = Some(staging_buffer);
        self.grass_compute_storage_bind_group = Some(compute_storage_bind_group);
        self.grass_render_instance_bind_group = Some(render_instance_bind_group);
        self.grass_config = Some(config);

        tracing::info!(
            "Grass reloaded: max {} instances, {:.1}MB buffer",
            max_instances,
            instance_buffer_size as f64 / (1024.0 * 1024.0)
        );
    }

    /// Read-only access to the current grass config (if loaded).
    pub fn grass_config(&self) -> Option<&flint_terrain::GrassConfig> {
        self.grass_config.as_ref()
    }

    /// Update entity positions for grass bend-on-contact.
    /// Also updates entity_count in the render uniform buffer.
    pub fn update_grass_entities(
        &mut self,
        queue: &wgpu::Queue,
        positions: &[GrassEntityPosition],
    ) {
        let count = positions.len().min(MAX_GRASS_ENTITIES);
        self.grass_entity_count = count as u32;
        if let Some(buf) = &self.grass_entity_buffer {
            if count > 0 {
                queue.write_buffer(buf, 0, bytemuck::cast_slice(&positions[..count]));
            }
        }
        // Note: entity_count is written to the render uniform buffer in dispatch_grass_compute,
        // which uses self.grass_entity_count to preserve the value set here.
    }

    /// Get an immutable reference to the texture cache.
    pub fn texture_cache(&self) -> Option<&TextureCache> {
        self.texture_cache.as_ref()
    }

    /// Clear all model/mesh data so a new model can be loaded cleanly.
    /// Used by the preview command when drag-and-dropping a replacement model.
    pub fn clear_model_data(&mut self) {
        self.mesh_cache.clear();
        if let Some(tc) = &mut self.texture_cache {
            tc.clear_user_textures();
        }
        self.entity_draws.clear();
        self.skinned_entity_draws.clear();
        self.transparent_draws.clear();
        self.transparent_skinned_draws.clear();
        self.billboard_draws.clear();
        self.sprite2d_batches.clear();
        self.wireframe_overlay_draws.clear();
        self.normal_arrow_draws.clear();
        self.skeleton_overlay_draws.clear();
        self.bitmap_font_cache.clear();
    }

    /// Create a renderer for headless (offscreen) use with explicit device and format
    pub fn new_headless(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        config: RendererConfig,
    ) -> Self {
        let surface_format = format;
        // Scene geometry renders to HDR; composite tonemaps to the readback surface.
        let scene_format = HDR_FORMAT;

        let pipeline = RenderPipeline::new(device, scene_format);
        let archetype_visuals = Self::default_archetype_visuals();
        let texture_cache = TextureCache::new(device, queue);

        // Create grid draw call (only for debug/inspection modes)
        let grid_draw = if config.show_grid {
            let grid = create_grid_mesh(40.0, 40, [0.3, 0.3, 0.3, 0.5]);
            Some(Self::create_draw_call(
                device,
                &pipeline,
                &grid,
                true,
                TransformUniforms::new(),
                MaterialUniforms::procedural(),
                &texture_cache,
            ))
        } else {
            None
        };

        let shadow_pass = ShadowPass::new(device, DEFAULT_SHADOW_RESOLUTION);

        let light_uniforms = LightUniforms::default_scene_lights();
        let (light_buffer, light_bind_group) =
            Self::create_light_bind(device, &pipeline, &light_uniforms, &shadow_pass);

        let skinned_pipeline = SkinnedPipeline::new(
            device,
            scene_format,
            &pipeline.transform_bind_group_layout,
            &pipeline.material_bind_group_layout,
            &pipeline.light_bind_group_layout,
        );

        let terrain_pipeline = TerrainPipeline::new(
            device,
            scene_format,
            &pipeline.transform_bind_group_layout,
            &pipeline.light_bind_group_layout,
        );

        let grass_pipeline = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            GrassPipeline::new(
                device,
                scene_format,
                &pipeline.transform_bind_group_layout,
                &pipeline.light_bind_group_layout,
            )
        }))
        .unwrap_or_else(|_| {
            tracing::warn!("Grass pipeline creation failed — grass disabled");
            None
        });

        let billboard_pipeline = BillboardPipeline::new(device, scene_format);
        let sprite2d_pipeline = Sprite2dPipeline::new(device, scene_format);
        let skybox_pipeline = SkyboxPipeline::new(device, scene_format);

        // Create post-processing pipeline and resources for headless
        let postprocess_config = PostProcessConfig::default();
        let postprocess_pipeline = PostProcessPipeline::new(
            device,
            queue,
            surface_format,
            postprocess_config.kuwahara_enabled,
        );
        let postprocess_resources =
            PostProcessResources::new(device, width, height, postprocess_config.kuwahara_enabled);

        Self {
            pipeline,
            skinned_pipeline: Some(skinned_pipeline),
            billboard_pipeline: Some(billboard_pipeline),
            archetype_visuals,
            mesh_cache: MeshCache::new(),
            grid_draw,
            entity_draws: Vec::new(),
            skinned_entity_draws: Vec::new(),
            transparent_draws: Vec::new(),
            transparent_skinned_draws: Vec::new(),
            billboard_draws: Vec::new(),
            debug_state: DebugState::default(),
            wireframe_overlay_draws: Vec::new(),
            normal_arrow_draws: Vec::new(),
            skeleton_overlay_draws: Vec::new(),
            tonemapping_enabled: true,
            light_buffer,
            light_bind_group,
            light_uniforms,
            texture_cache: Some(texture_cache),
            shadow_pass: Some(shadow_pass),
            selected_entity: None,
            skybox_pipeline: Some(skybox_pipeline),
            skybox_uniform_buffer: None,
            skybox_uniform_bind_group: None,
            skybox_texture_bind_group: None,
            terrain_pipeline: Some(terrain_pipeline),
            terrain_draws: Vec::new(),
            terrain_material_bind_group: None,
            terrain_material_buffer: None,
            terrain_total_chunks: 0,
            terrain_visible_chunks: 0,
            camera_frustum: None,
            grass_pipeline,
            grass_instance_buffer: None,
            grass_instance_count: 0,
            grass_max_instances: 0,
            grass_counter_buffer: None,
            grass_staging_buffer: None,
            grass_compute_uniform_buffer: None,
            grass_compute_uniform_bind_group: None,
            grass_compute_texture_bind_group: None,
            grass_compute_storage_bind_group: None,
            grass_render_uniform_buffer: None,
            grass_render_uniform_bind_group: None,
            grass_render_instance_bind_group: None,
            grass_entity_buffer: None,
            grass_entity_count: 0,
            grass_config: None,
            grass_terrain_offset: [0.0; 3],
            grass_terrain_width: 0.0,
            grass_terrain_depth: 0.0,
            grass_terrain_height_scale: 0.0,
            particle_pipeline: None, // No particles in headless mode
            particle_draws: Vec::new(),
            sprite2d_pipeline: Some(sprite2d_pipeline),
            sprite2d_batches: Vec::new(),
            postprocess_pipeline: Some(postprocess_pipeline),
            postprocess_resources: Some(postprocess_resources),
            postprocess_config,
            surface_format,
            camera_offset: [0.0, 0.0],
            ortho_height: 10.0,
            aspect_ratio: 16.0 / 9.0,
            bitmap_font_cache: HashMap::new(),
            scene_dir: None,
            grass_time: 0.0,
            device_lost: false,
        }
    }

    /// Load an imported model into the GPU mesh cache
    pub fn load_model(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        import_result: &ImportResult,
    ) {
        let default_color = self
            .archetype_visuals
            .get("furniture")
            .map(|v| v.color)
            .unwrap_or([0.5, 0.5, 0.5, 1.0]);

        self.mesh_cache
            .upload_imported(device, name, import_result, default_color);

        // Upload textures referenced by materials, namespaced by asset name
        if let Some(cache) = &mut self.texture_cache {
            for texture in &import_result.textures {
                let namespaced = format!("{}::{}", name, texture.name);
                cache.upload(device, queue, &namespaced, texture);
            }
        }

        // Patch material texture references on cached meshes to use namespaced names
        if let Some(gpu_meshes) = self.mesh_cache.get_mut(name) {
            for mesh in gpu_meshes.iter_mut() {
                if let Some(ref tex) = mesh.material.base_color_texture {
                    mesh.material.base_color_texture = Some(format!("{}::{}", name, tex));
                }
                if let Some(ref tex) = mesh.material.normal_texture {
                    mesh.material.normal_texture = Some(format!("{}::{}", name, tex));
                }
                if let Some(ref tex) = mesh.material.metallic_roughness_texture {
                    mesh.material.metallic_roughness_texture = Some(format!("{}::{}", name, tex));
                }
            }
        }
    }

    /// Load skinned meshes from an imported model into the GPU mesh cache
    pub fn load_skinned_model(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        import_result: &ImportResult,
    ) {
        let default_color = self
            .archetype_visuals
            .get("character")
            .map(|v| v.color)
            .unwrap_or([0.5, 0.5, 0.5, 1.0]);

        if let Some(sp) = &self.skinned_pipeline {
            self.mesh_cache.upload_skinned(
                device,
                name,
                import_result,
                default_color,
                &sp.bone_bind_group_layout,
            );
        }

        // Also upload textures, namespaced by asset name
        if let Some(cache) = &mut self.texture_cache {
            for texture in &import_result.textures {
                let namespaced = format!("{}::{}", name, texture.name);
                cache.upload(device, queue, &namespaced, texture);
            }
        }

        // Patch material texture references on cached skinned meshes
        if let Some(gpu_meshes) = self.mesh_cache.get_skinned_mut(name) {
            for mesh in gpu_meshes.iter_mut() {
                if let Some(ref tex) = mesh.material.base_color_texture {
                    mesh.material.base_color_texture = Some(format!("{}::{}", name, tex));
                }
                if let Some(ref tex) = mesh.material.normal_texture {
                    mesh.material.normal_texture = Some(format!("{}::{}", name, tex));
                }
                if let Some(ref tex) = mesh.material.metallic_roughness_texture {
                    mesh.material.metallic_roughness_texture = Some(format!("{}::{}", name, tex));
                }
            }
        }
    }

    /// Update bone matrices for a skinned mesh asset on the GPU
    pub fn update_bone_matrices(
        &mut self,
        queue: &wgpu::Queue,
        asset_name: &str,
        matrices: &[[[f32; 4]; 4]],
    ) {
        if self.device_lost {
            return;
        }
        if let Some(skinned_meshes) = self.mesh_cache.get_skinned_mut(asset_name) {
            for mesh in skinned_meshes.iter_mut() {
                queue.write_buffer(&mesh.bone_buffer, 0, bytemuck::cast_slice(matrices));
            }
        }
    }

    /// Load a texture from an image file into the texture cache
    pub fn load_texture_file(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        path: &Path,
    ) -> Result<bool, String> {
        if let Some(cache) = &mut self.texture_cache {
            cache.load_file(device, queue, name, path)
        } else {
            Err("Texture cache not initialized".to_string())
        }
    }

    /// Load raw RGBA8 pixel data into the texture cache.
    ///
    /// Returns `Ok(true)` if newly uploaded, `Ok(false)` if already cached.
    pub fn load_texture_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        name: &str,
        width: u32,
        height: u32,
        data: &[u8],
        is_normal_map: bool,
    ) -> Result<bool, String> {
        if let Some(cache) = &mut self.texture_cache {
            cache.upload_rgba(device, queue, name, width, height, data, is_normal_map)
        } else {
            Err("Texture cache not initialized".to_string())
        }
    }

    /// Load a procedural mesh from raw vertex/index data into the GPU mesh cache
    pub fn load_procedural_mesh(
        &mut self,
        device: &wgpu::Device,
        name: &str,
        vertices: &[crate::primitives::Vertex],
        indices: &[u32],
        material: flint_import::ImportedMaterial,
    ) {
        self.mesh_cache
            .upload_procedural(device, name, vertices, indices, material);
    }

    /// Get a mutable reference to the mesh cache
    pub fn mesh_cache_mut(&mut self) -> &mut MeshCache {
        &mut self.mesh_cache
    }

    /// Get a reference to the mesh cache
    pub fn mesh_cache(&self) -> &MeshCache {
        &self.mesh_cache
    }

    /// Read-only access to the current debug state
    pub fn debug_state(&self) -> &DebugState {
        &self.debug_state
    }

    /// Mutable access to the debug state
    pub fn debug_state_mut(&mut self) -> &mut DebugState {
        &mut self.debug_state
    }

    /// Set the shading debug mode
    pub fn set_debug_mode(&mut self, mode: DebugMode) {
        self.debug_state.mode = mode;
    }

    /// Toggle wireframe overlay on/off, returns the new state
    pub fn toggle_wireframe_overlay(&mut self) -> bool {
        self.debug_state.wireframe_overlay = !self.debug_state.wireframe_overlay;
        self.debug_state.wireframe_overlay
    }

    /// Collect rendering statistics for the current frame.
    ///
    /// Timing (`fps`, `frame_time_ms`) and `resolution` are left at zero —
    /// the caller fills these from their own timing and context.
    pub fn collect_stats(&self) -> crate::render_stats::RenderStats {
        use crate::grass_pipeline::BLADE_INDEX_COUNT;

        let entity_draws = self.entity_draws.len() as u32;
        let skinned_draws = self.skinned_entity_draws.len() as u32;
        let terrain_draws = self.terrain_visible_chunks;
        let transparent_draws =
            (self.transparent_draws.len() + self.transparent_skinned_draws.len()) as u32;
        let billboard_draws = self.billboard_draws.len() as u32;
        let particle_draws = self.particle_draws.len() as u32;
        let sprite_batches = self.sprite2d_batches.len() as u32;
        let grass_draw_calls = if self.grass_instance_count > 0 { 1u32 } else { 0 };

        let draw_calls = entity_draws
            + skinned_draws
            + terrain_draws
            + transparent_draws
            + billboard_draws
            + particle_draws
            + sprite_batches
            + grass_draw_calls;

        // Triangles: sum index_count/3 for types that have it, fixed counts for others
        let mut triangles: u32 = 0;
        for d in &self.entity_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.skinned_entity_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.transparent_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.transparent_skinned_draws {
            triangles += d.index_count / 3;
        }
        for d in &self.terrain_draws {
            triangles += d.index_count / 3;
        }
        // Billboards: each is a fixed quad (2 triangles)
        triangles += billboard_draws * 2;
        // Particles: instanced quads (2 triangles per instance)
        let particle_instances: u32 = self.particle_draws.iter().map(|d| d.instance_count).sum();
        triangles += particle_instances * 2;
        // Sprites: instanced quads (2 triangles per instance)
        let sprite_instances: u32 =
            self.sprite2d_batches.iter().map(|b| b.instance_count).sum();
        triangles += sprite_instances * 2;
        // Grass: BLADE_INDEX_COUNT indices per instance
        triangles += self.grass_instance_count * BLADE_INDEX_COUNT / 3;

        // Shadow stats: estimate as main pass × CASCADE_COUNT
        let cascade_count = crate::shadow::CASCADE_COUNT as u32;
        let shadow_entity_draws = entity_draws + skinned_draws + terrain_draws;
        let shadow_draw_calls = shadow_entity_draws * cascade_count;
        let shadow_triangles = {
            let mut t: u32 = 0;
            for d in &self.entity_draws {
                t += d.index_count / 3;
            }
            for d in &self.skinned_entity_draws {
                t += d.index_count / 3;
            }
            for d in &self.terrain_draws {
                t += d.index_count / 3;
            }
            t * cascade_count
        };

        crate::render_stats::RenderStats {
            draw_calls,
            triangles,
            entity_draws,
            skinned_draws,
            terrain_draws,
            terrain_total_chunks: self.terrain_total_chunks,
            transparent_draws,
            billboard_draws,
            particle_draws,
            particle_instances,
            sprite_batches,
            grass_instances: self.grass_instance_count,
            grass_draw_calls,
            shadow_draw_calls,
            shadow_triangles,
            ..Default::default()
        }
    }

    /// Toggle normal direction arrows on/off, returns the new state
    pub fn toggle_normal_arrows(&mut self) -> bool {
        self.debug_state.show_normals = !self.debug_state.show_normals;
        self.debug_state.show_normals
    }

    /// Toggle skeleton overlay on/off, returns the new state
    pub fn toggle_skeleton_overlay(&mut self) -> bool {
        self.debug_state.show_skeleton = !self.debug_state.show_skeleton;
        self.debug_state.show_skeleton
    }

    /// Set the skeleton overlay mesh (line-list geometry for bone visualization)
    pub fn set_skeleton_overlay(&mut self, device: &wgpu::Device, mesh: &crate::primitives::Mesh) {
        self.skeleton_overlay_draws.clear();
        if mesh.indices.is_empty() {
            return;
        }
        let tex_cache = self.texture_cache.as_ref().expect("texture cache");
        let draw = Self::create_draw_call(
            device,
            &self.pipeline,
            mesh,
            true,
            TransformUniforms::new(),
            MaterialUniforms::procedural(),
            tex_cache,
        );
        self.skeleton_overlay_draws.push(draw);
    }

    /// Clear skeleton overlay draw calls
    pub fn clear_skeleton_overlay(&mut self) {
        self.skeleton_overlay_draws.clear();
    }

    /// Show or hide the ground-plane grid at runtime
    pub fn set_show_grid(&mut self, device: &wgpu::Device, show: bool) {
        if show && self.grid_draw.is_none() {
            let grid = create_grid_mesh(40.0, 40, [0.3, 0.3, 0.3, 0.5]);
            let tc = self
                .texture_cache
                .as_ref()
                .expect("texture cache must exist");
            self.grid_draw = Some(Self::create_draw_call(
                device,
                &self.pipeline,
                &grid,
                true,
                TransformUniforms::new(),
                MaterialUniforms::procedural(),
                tc,
            ));
        } else if !show {
            self.grid_draw = None;
        }
    }

    /// Whether the ground-plane grid is currently visible
    pub fn show_grid(&self) -> bool {
        self.grid_draw.is_some()
    }

    /// Enable or disable tone mapping
    pub fn set_tonemapping(&mut self, enabled: bool) {
        self.tonemapping_enabled = enabled;
    }

    /// Enable or disable shadow mapping
    pub fn set_shadows(&mut self, enabled: bool) {
        if let Some(sp) = &mut self.shadow_pass {
            sp.enabled = enabled;
        }
    }

    /// Recreate the shadow pass with a different resolution
    pub fn set_shadow_resolution(&mut self, device: &wgpu::Device, resolution: u32) {
        let was_enabled = self.shadow_pass.as_ref().map_or(false, |sp| sp.enabled);
        let mut shadow_pass = ShadowPass::new(device, resolution);
        shadow_pass.enabled = was_enabled;

        // Recreate light bind group with new shadow pass resources
        let (light_buffer, light_bind_group) =
            Self::create_light_bind(device, &self.pipeline, &self.light_uniforms, &shadow_pass);
        self.light_buffer = light_buffer;
        self.light_bind_group = light_bind_group;
        self.shadow_pass = Some(shadow_pass);
    }

    /// Set the entity to highlight with a selection glow, or None to clear
    pub fn set_selected_entity(&mut self, id: Option<flint_core::EntityId>) {
        self.selected_entity = id;
    }

    /// Toggle shadows on/off, returns the new state
    /// Load an equirectangular panorama image as a skybox
    pub fn load_skybox(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, path: &Path) {
        let skybox_pipeline = match &self.skybox_pipeline {
            Some(p) => p,
            None => {
                tracing::warn!("Skybox pipeline not available");
                return;
            }
        };

        // Load panorama image
        let img = match image::open(path) {
            Ok(img) => img.to_rgba8(),
            Err(e) => {
                tracing::warn!("Failed to load skybox '{}': {:?}", path.display(), e);
                return;
            }
        };
        let (width, height) = img.dimensions();

        // Create GPU texture
        let texture_size = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Skybox Panorama Texture"),
            size: texture_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &img,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * width),
                rows_per_image: Some(height),
            },
            texture_size,
        );

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Skybox Sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Create uniform buffer
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Skybox Uniform Buffer"),
            contents: bytemuck::cast_slice(&[SkyboxUniforms {
                inv_view_proj: identity_matrix(),
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        // Create bind groups
        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &skybox_pipeline.uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
            label: Some("Skybox Uniform Bind Group"),
        });

        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &skybox_pipeline.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
            label: Some("Skybox Texture Bind Group"),
        });

        self.skybox_uniform_buffer = Some(uniform_buffer);
        self.skybox_uniform_bind_group = Some(uniform_bind_group);
        self.skybox_texture_bind_group = Some(texture_bind_group);

        println!("Loaded skybox: {} ({}x{})", path.display(), width, height);
    }

    pub fn toggle_shadows(&mut self) -> bool {
        if let Some(sp) = &mut self.shadow_pass {
            sp.enabled = !sp.enabled;
            sp.enabled
        } else {
            false
        }
    }

    fn default_archetype_visuals() -> HashMap<String, ArchetypeVisual> {
        let mut archetype_visuals = HashMap::new();

        archetype_visuals.insert(
            "room".to_string(),
            ArchetypeVisual {
                color: [0.27, 0.53, 1.0, 0.5],
                wireframe: true,
                default_size: [10.0, 4.0, 10.0],
            },
        );

        archetype_visuals.insert(
            "door".to_string(),
            ArchetypeVisual {
                color: [1.0, 0.53, 0.27, 1.0],
                wireframe: false,
                default_size: [1.0, 2.0, 0.1],
            },
        );

        archetype_visuals.insert(
            "furniture".to_string(),
            ArchetypeVisual {
                color: [0.27, 1.0, 0.53, 1.0],
                wireframe: false,
                default_size: [1.0, 1.0, 1.0],
            },
        );

        archetype_visuals.insert(
            "character".to_string(),
            ArchetypeVisual {
                color: [1.0, 1.0, 0.27, 1.0],
                wireframe: false,
                default_size: [0.5, 1.8, 0.5],
            },
        );

        archetype_visuals.insert(
            "wall".to_string(),
            ArchetypeVisual {
                color: [0.76, 0.70, 0.60, 1.0],
                wireframe: false,
                default_size: [10.0, 4.0, 0.3],
            },
        );

        archetype_visuals.insert(
            "floor".to_string(),
            ArchetypeVisual {
                color: [0.55, 0.55, 0.52, 1.0],
                wireframe: false,
                default_size: [10.0, 0.2, 10.0],
            },
        );

        archetype_visuals.insert(
            "ceiling".to_string(),
            ArchetypeVisual {
                color: [0.65, 0.62, 0.58, 1.0],
                wireframe: false,
                default_size: [10.0, 0.2, 10.0],
            },
        );

        archetype_visuals.insert(
            "pillar".to_string(),
            ArchetypeVisual {
                color: [0.70, 0.65, 0.55, 1.0],
                wireframe: false,
                default_size: [0.5, 4.0, 0.5],
            },
        );

        archetype_visuals
    }

    /// Set visual representation for an archetype
    pub fn set_archetype_visual(&mut self, archetype: &str, visual: ArchetypeVisual) {
        self.archetype_visuals.insert(archetype.to_string(), visual);
    }

    // ── Slim update_from_world: dispatch loop calling extraction helpers ──

    /// Update meshes from the world state
    pub fn update_from_world(&mut self, world: &FlintWorld, device: &wgpu::Device) {
        if self.device_lost {
            return;
        }
        self.entity_draws.clear();
        self.skinned_entity_draws.clear();
        self.transparent_draws.clear();
        self.transparent_skinned_draws.clear();
        self.billboard_draws.clear();
        self.sprite2d_batches.clear();
        self.wireframe_overlay_draws.clear();
        self.normal_arrow_draws.clear();

        // Extract lights from scene entities
        self.extract_lights_from_world(world);

        let need_overlay =
            self.debug_state.wireframe_overlay || self.debug_state.mode == DebugMode::WireframeOnly;
        let need_normals = self.debug_state.show_normals;
        let arrow_length = self.debug_state.normal_arrow_length;

        // Collect sprite2d instances for batching (texture_name, layer, instance data)
        let mut sprite2d_collected: Vec<(String, i32, Sprite2dInstanceGpu)> = Vec::new();

        // Temporarily take texture_cache to avoid borrow conflicts
        let tex_cache = self.texture_cache.take();
        let tex_cache_ref = tex_cache.as_ref().unwrap();

        for entity in world.all_entities() {
            let archetype = entity.archetype.as_deref().unwrap_or("unknown");
            let visual =
                self.archetype_visuals
                    .get(archetype)
                    .cloned()
                    .unwrap_or(ArchetypeVisual {
                        color: [0.5, 0.5, 0.5, 1.0],
                        wireframe: false,
                        default_size: [1.0, 1.0, 1.0],
                    });

            let model_matrix = world
                .get_world_matrix(entity.id)
                .unwrap_or_else(|| Transform::default().to_matrix());
            let world_pos = [model_matrix[3][0], model_matrix[3][1], model_matrix[3][2]];

            // Check if entity has a model component
            let model_asset = world
                .get_components(entity.id)
                .and_then(|components| components.get(comp::MODEL).cloned())
                .and_then(|model| {
                    model
                        .get("asset")
                        .and_then(|v| v.as_str().map(String::from))
                });

            if let Some(asset_name) = &model_asset {
                // Check for skinned meshes first
                if self.extract_skinned_entity(
                    device,
                    tex_cache_ref,
                    world,
                    entity.id,
                    asset_name,
                    model_matrix,
                ) {
                    continue;
                }

                // Check for standard (non-skinned) model
                if self.extract_model_entity(
                    device,
                    tex_cache_ref,
                    world,
                    entity.id,
                    asset_name,
                    model_matrix,
                    need_overlay,
                    need_normals,
                    arrow_length,
                ) {
                    continue;
                }
            }

            // Check for sprite component — render as billboard or 2D sprite
            if let Some(components) = world.get_components(entity.id) {
                if let Some(sprite) = components.get(comp::SPRITE) {
                    self.extract_sprite_entity(
                        device,
                        tex_cache_ref,
                        world,
                        entity.id,
                        components,
                        sprite,
                        world_pos,
                        &mut sprite2d_collected,
                    );
                    continue;
                }
            }

            // Standalone ui_text: entities with ui_text but no sprite
            if let Some(components) = world.get_components(entity.id) {
                if self.extract_ui_text_entity(
                    tex_cache_ref,
                    entity.id,
                    components,
                    world_pos,
                    &mut sprite2d_collected,
                ) {
                    continue;
                }
            }

            // Only draw fallback geometry for entities that explicitly have bounds
            if let Some(components) = world.get_components(entity.id) {
                let has_bounds = components.get(comp::BOUNDS).is_some();
                if !has_bounds {
                    continue;
                }
            }

            // Fall back to procedural bounds geometry
            self.extract_bounds_entity(
                device,
                tex_cache_ref,
                world,
                entity.id,
                &visual,
                model_matrix,
                need_overlay,
                need_normals,
                arrow_length,
            );
        }

        // Sort & batch sprite2d instances
        self.batch_sprite2d_instances(device, tex_cache_ref, sprite2d_collected);

        // Put texture cache back
        self.texture_cache = tex_cache;
    }

    // ── Draw call factory methods ──

    fn create_draw_call(
        device: &wgpu::Device,
        pipeline: &RenderPipeline,
        mesh: &crate::primitives::Mesh,
        is_wireframe: bool,
        transform_uniforms: TransformUniforms,
        material_uniforms: MaterialUniforms,
        texture_cache: &TextureCache,
    ) -> DrawCall {
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Vertex Buffer"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Index Buffer"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let (transform_buffer, transform_bind_group) =
            Self::create_transform_bind(device, pipeline, &transform_uniforms);
        let (material_buffer, material_bind_group) =
            Self::create_material_bind(device, pipeline, &material_uniforms, texture_cache);

        DrawCall {
            vertex_buffer,
            index_buffer,
            index_count: mesh.indices.len() as u32,
            is_wireframe,
            transform_buffer,
            transform_bind_group,
            material_buffer,
            material_bind_group,
            model: transform_uniforms.model,
            model_inv_transpose: transform_uniforms.model_inv_transpose,
            entity_id: None,
            blend_mode: BlendMode::Alpha,
            sort_depth: 0.0,
        }
    }

    /// Create a draw call for a procedural mesh with explicit texture bindings.
    fn create_textured_draw_call(
        device: &wgpu::Device,
        pipeline: &RenderPipeline,
        mesh: &crate::primitives::Mesh,
        transform_uniforms: TransformUniforms,
        material_uniforms: MaterialUniforms,
        base_color_view: &wgpu::TextureView,
        base_color_sampler: &wgpu::Sampler,
        normal_view: &wgpu::TextureView,
        normal_sampler: &wgpu::Sampler,
        mr_view: &wgpu::TextureView,
        mr_sampler: &wgpu::Sampler,
    ) -> DrawCall {
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Textured Vertex Buffer"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Textured Index Buffer"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let (transform_buffer, transform_bind_group) =
            Self::create_transform_bind(device, pipeline, &transform_uniforms);
        let (material_buffer, material_bind_group) = Self::create_material_bind_with_textures(
            device,
            pipeline,
            &material_uniforms,
            base_color_view,
            base_color_sampler,
            normal_view,
            normal_sampler,
            mr_view,
            mr_sampler,
        );

        DrawCall {
            vertex_buffer,
            index_buffer,
            index_count: mesh.indices.len() as u32,
            is_wireframe: false,
            transform_buffer,
            transform_bind_group,
            material_buffer,
            material_bind_group,
            model: transform_uniforms.model,
            model_inv_transpose: transform_uniforms.model_inv_transpose,
            entity_id: None,
            blend_mode: BlendMode::Alpha,
            sort_depth: 0.0,
        }
    }

    /// Create a draw call for an imported mesh that already has GPU buffers.
    fn create_imported_draw_call(
        device: &wgpu::Device,
        pipeline: &RenderPipeline,
        gpu_mesh: &crate::gpu_mesh::GpuMesh,
        transform_uniforms: TransformUniforms,
        material_uniforms: MaterialUniforms,
        base_color_view: &wgpu::TextureView,
        base_color_sampler: &wgpu::Sampler,
        normal_view: &wgpu::TextureView,
        normal_sampler: &wgpu::Sampler,
        mr_view: &wgpu::TextureView,
        mr_sampler: &wgpu::Sampler,
    ) -> DrawCall {
        let (transform_buffer, transform_bind_group) =
            Self::create_transform_bind(device, pipeline, &transform_uniforms);
        let (material_buffer, material_bind_group) = Self::create_material_bind_with_textures(
            device,
            pipeline,
            &material_uniforms,
            base_color_view,
            base_color_sampler,
            normal_view,
            normal_sampler,
            mr_view,
            mr_sampler,
        );

        DrawCall {
            vertex_buffer: gpu_mesh.create_vertex_buffer_copy(device),
            index_buffer: gpu_mesh.create_index_buffer_copy(device),
            index_count: gpu_mesh.index_count,
            is_wireframe: false,
            transform_buffer,
            transform_bind_group,
            material_buffer,
            material_bind_group,
            model: transform_uniforms.model,
            model_inv_transpose: transform_uniforms.model_inv_transpose,
            entity_id: None,
            blend_mode: BlendMode::Alpha,
            sort_depth: 0.0,
        }
    }

    fn create_transform_bind(
        device: &wgpu::Device,
        pipeline: &RenderPipeline,
        uniforms: &TransformUniforms,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Transform Uniform Buffer"),
            contents: bytemuck::cast_slice(&[*uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &pipeline.transform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
            label: Some("Transform Bind Group"),
        });

        (buffer, bind_group)
    }

    fn create_material_bind(
        device: &wgpu::Device,
        pipeline: &RenderPipeline,
        uniforms: &MaterialUniforms,
        texture_cache: &TextureCache,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        Self::create_material_bind_with_textures(
            device,
            pipeline,
            uniforms,
            &texture_cache.default_white.view,
            &texture_cache.default_white.sampler,
            &texture_cache.default_normal.view,
            &texture_cache.default_normal.sampler,
            &texture_cache.default_metallic_roughness.view,
            &texture_cache.default_metallic_roughness.sampler,
        )
    }

    fn create_material_bind_with_textures(
        device: &wgpu::Device,
        pipeline: &RenderPipeline,
        uniforms: &MaterialUniforms,
        base_color_view: &wgpu::TextureView,
        base_color_sampler: &wgpu::Sampler,
        normal_view: &wgpu::TextureView,
        normal_sampler: &wgpu::Sampler,
        mr_view: &wgpu::TextureView,
        mr_sampler: &wgpu::Sampler,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Material Uniform Buffer"),
            contents: bytemuck::cast_slice(&[*uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &pipeline.material_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(base_color_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(base_color_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(normal_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(normal_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(mr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(mr_sampler),
                },
            ],
            label: Some("Material Bind Group"),
        });

        (buffer, bind_group)
    }

    /// Resolve a texture reference, returning the view, sampler, and whether a real texture was found
    fn resolve_texture<'a>(
        cache: &'a TextureCache,
        name: Option<&str>,
        default: &'a crate::texture_cache::GpuTexture,
    ) -> (&'a wgpu::TextureView, &'a wgpu::Sampler, bool) {
        if let Some(name) = name {
            if let Some(gpu_tex) = cache.get(name) {
                return (&gpu_tex.view, &gpu_tex.sampler, true);
            }
        }
        (&default.view, &default.sampler, false)
    }

    fn create_light_bind(
        device: &wgpu::Device,
        pipeline: &RenderPipeline,
        uniforms: &LightUniforms,
        shadow_pass: &ShadowPass,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Light Uniform Buffer"),
            contents: bytemuck::cast_slice(&[*uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &pipeline.light_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&shadow_pass.shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&shadow_pass.shadow_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: shadow_pass.shadow_uniforms_buffer.as_entire_binding(),
                },
            ],
            label: Some("Light Bind Group"),
        });

        (buffer, bind_group)
    }

    // ── Light extraction ──

    /// Extract light entities from the world and update the light uniform buffer
    fn extract_lights_from_world(&mut self, world: &FlintWorld) {
        let mut dir_count = 0u32;
        let mut point_count = 0u32;
        let mut spot_count = 0u32;
        let mut directionals = [DirectionalLight::default(); MAX_DIRECTIONAL_LIGHTS];
        let mut points = [PointLight::default(); MAX_POINT_LIGHTS];
        let mut spots = [SpotLight::default(); MAX_SPOT_LIGHTS];

        for &entity_id in world.entities_with_component(comp::LIGHT) {
            let light_component = world
                .get_components(entity_id)
                .and_then(|components| components.get(comp::LIGHT).cloned());

            if let Some(light) = light_component {
                let light_type = light
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("directional");

                let color = Self::extract_light_vec3(&light, "color").unwrap_or([1.0, 1.0, 1.0]);
                let intensity = light.get("intensity").and_then(toml_f32).unwrap_or(1.0);

                match light_type {
                    "directional" => {
                        if (dir_count as usize) < MAX_DIRECTIONAL_LIGHTS {
                            let direction = Self::extract_light_vec3(&light, "direction")
                                .unwrap_or([0.0, -1.0, 0.0]);
                            let volumetric_intensity = light
                                .get("volumetric_intensity")
                                .and_then(toml_f32)
                                .unwrap_or(0.0);
                            let volumetric_color =
                                Self::extract_light_vec3(&light, "volumetric_color")
                                    .unwrap_or(color);
                            directionals[dir_count as usize] = DirectionalLight {
                                direction,
                                volumetric_intensity,
                                color,
                                intensity,
                                volumetric_color,
                                _pad1: 0.0,
                            };
                            dir_count += 1;
                        }
                    }
                    "point" => {
                        if (point_count as usize) < MAX_POINT_LIGHTS {
                            let light_pos =
                                world.get_world_position(entity_id).unwrap_or(Vec3::ZERO);
                            let radius = light
                                .get("range")
                                .or_else(|| light.get("radius"))
                                .and_then(toml_f32)
                                .unwrap_or(10.0);
                            points[point_count as usize] = PointLight {
                                position: [light_pos.x, light_pos.y, light_pos.z],
                                radius,
                                color,
                                intensity,
                            };
                            point_count += 1;
                        }
                    }
                    "spot" => {
                        if (spot_count as usize) < MAX_SPOT_LIGHTS {
                            let light_pos =
                                world.get_world_position(entity_id).unwrap_or(Vec3::ZERO);
                            let direction = Self::extract_light_vec3(&light, "direction")
                                .unwrap_or([0.0, -1.0, 0.0]);
                            let radius = light
                                .get("range")
                                .or_else(|| light.get("radius"))
                                .and_then(toml_f32)
                                .unwrap_or(10.0);
                            let inner_angle =
                                light.get("inner_angle").and_then(toml_f32).unwrap_or(0.3);
                            let outer_angle =
                                light.get("outer_angle").and_then(toml_f32).unwrap_or(0.5);
                            spots[spot_count as usize] = SpotLight {
                                position: [light_pos.x, light_pos.y, light_pos.z],
                                radius,
                                direction,
                                inner_angle,
                                color,
                                outer_angle,
                                intensity,
                                _pad0: 0.0,
                                _pad1: 0.0,
                                _pad2: 0.0,
                            };
                            spot_count += 1;
                        }
                    }
                    _ => {}
                }
            }
        }

        // If no lights found in scene, use defaults
        if dir_count == 0 && point_count == 0 && spot_count == 0 {
            self.light_uniforms = LightUniforms::default_scene_lights();
        } else {
            self.light_uniforms.directional_lights = directionals;
            self.light_uniforms.point_lights = points;
            self.light_uniforms.spot_lights = spots;
            self.light_uniforms.directional_count = dir_count;
            self.light_uniforms.point_count = point_count;
            self.light_uniforms.spot_count = spot_count;
        }
    }

    fn extract_light_vec3(table: &toml::Value, key: &str) -> Option<[f32; 3]> {
        let arr = table.get(key)?.as_array()?;
        if arr.len() >= 3 {
            let x = arr[0]
                .as_float()
                .or_else(|| arr[0].as_integer().map(|i| i as f64))? as f32;
            let y = arr[1]
                .as_float()
                .or_else(|| arr[1].as_integer().map(|i| i as f64))? as f32;
            let z = arr[2]
                .as_float()
                .or_else(|| arr[2].as_integer().map(|i| i as f64))? as f32;
            Some([x, y, z])
        } else {
            None
        }
    }

    // ── Render entry points ──

    /// Render the scene using a RenderContext (windowed mode)
    pub fn render(
        &mut self,
        context: &RenderContext,
        camera: &Camera,
        view: &wgpu::TextureView,
    ) -> Result<(), wgpu::SurfaceError> {
        if self.device_lost {
            return Ok(());
        }
        self.render_to(
            &context.device,
            &context.queue,
            &context.depth_view,
            camera,
            view,
        );
        Ok(())
    }

    /// Resize post-processing resources (call on window resize).
    pub fn resize_postprocess(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.postprocess_resources = Some(PostProcessResources::new(
            device,
            width,
            height,
            self.postprocess_config.kuwahara_enabled,
        ));
    }

    /// Get the current post-processing configuration.
    pub fn post_process_config(&self) -> &PostProcessConfig {
        &self.postprocess_config
    }

    /// Set the post-processing configuration.
    pub fn set_post_process_config(&mut self, config: PostProcessConfig) {
        self.postprocess_config = config;
    }

    /// Ensure Kuwahara GPU resources exist (call after enabling kuwahara at runtime).
    /// Creates pipelines and textures on demand if they haven't been allocated yet.
    pub fn ensure_kuwahara_resources(&mut self, device: &wgpu::Device, _queue: &wgpu::Queue) {
        if !self.postprocess_config.kuwahara_enabled {
            return;
        }

        // Create only the Kuwahara pipelines (not the entire PostProcessPipeline).
        // Uses catch_unwind because some GPU drivers (Intel Skylake) crash with
        // unrecoverable device loss when compiling these shaders.
        if let Some(pp) = &mut self.postprocess_pipeline {
            if pp.kuwahara.is_none() {
                use std::panic::{catch_unwind, AssertUnwindSafe};
                let device_ptr = device as *const wgpu::Device;
                let result = catch_unwind(AssertUnwindSafe(|| {
                    // SAFETY: device_ptr is valid for the duration of this closure
                    let dev = unsafe { &*device_ptr };
                    crate::postprocess::KuwaharaPipelines::new(dev)
                }));
                match result {
                    Ok(pipelines) => pp.kuwahara = Some(pipelines),
                    Err(_) => {
                        tracing::warn!(
                            "Kuwahara filter unavailable: GPU driver crashed during \
                             shader compilation. Try updating your graphics drivers."
                        );
                        self.postprocess_config.kuwahara_enabled = false;
                        self.device_lost = true;
                        return;
                    }
                }
            }
        }

        // Create only the Kuwahara textures (not the entire PostProcessResources)
        if let Some(resources) = &mut self.postprocess_resources {
            if resources.kuwahara.is_none() {
                let (w, h) = (resources.width, resources.height);
                resources.kuwahara = Some(crate::postprocess::KuwaharaTextures::new(device, w, h));
            }
        }
    }

    // ── Slim render_to: orchestrates shadow → uniforms → main pass → postprocess ──

    /// Render the scene to an arbitrary texture view with explicit device/queue/depth
    pub fn render_to(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        depth_view: &wgpu::TextureView,
        camera: &Camera,
        target_view: &wgpu::TextureView,
    ) {
        if self.device_lost {
            return;
        }
        let view_proj = camera.view_projection_matrix();
        self.camera_frustum = Some(crate::frustum::Frustum::from_view_projection(&view_proj));

        // Count visible terrain chunks for stats
        self.terrain_visible_chunks = if let Some(ref frustum) = self.camera_frustum {
            self.terrain_draws
                .iter()
                .filter(|d| frustum.aabb_visible(d.aabb_min, d.aabb_max))
                .count() as u32
        } else {
            self.terrain_draws.len() as u32
        };

        let camera_pos = camera.position_array();
        let debug_mode_u32 = self.debug_state.mode.as_u32();
        let wireframe_only = self.debug_state.mode == DebugMode::WireframeOnly;

        // Update light uniforms
        queue.write_buffer(
            &self.light_buffer,
            0,
            bytemuck::cast_slice(&[self.light_uniforms]),
        );

        // Shadow pass: render depth from light perspective
        self.render_shadow_pass(device, queue, camera, camera_pos);

        // All scene pipelines target Rgba16Float, so we always render to the HDR
        // buffer and composite to sRGB.
        let has_postprocess =
            self.postprocess_pipeline.is_some() && self.postprocess_resources.is_some();

        // Shader-side tonemapping is always OFF when compositing through the HDR
        // buffer (the composite pass handles ACES + gamma).
        let tonemapping_u32: u32 = if !has_postprocess && self.tonemapping_enabled {
            1
        } else {
            0
        };

        // Update all per-frame uniforms and sort transparent draws
        self.update_per_frame_uniforms(
            queue,
            view_proj,
            camera_pos,
            debug_mode_u32,
            tonemapping_u32,
            camera,
        );

        // Grass compute: dispatch in a separate encoder+submit so we can read back
        // the instance count before the render pass needs it.
        if self.grass_config.as_ref().is_some_and(|c| c.enabled) {
            let mut compute_encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Grass Compute Encoder"),
                });
            let grass_time = self.grass_time;
            self.dispatch_grass_compute(device, queue, &mut compute_encoder, camera, grass_time);
            queue.submit(std::iter::once(compute_encoder.finish()));
            self.read_grass_instance_count(device);
        }

        // Choose render target: HDR buffer or direct to surface
        let scene_target_view = if has_postprocess {
            &self.postprocess_resources.as_ref().unwrap().hdr_view
        } else {
            target_view
        };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scene_target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.1,
                            g: 0.1,
                            b: 0.15,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            self.render_main_pass(&mut render_pass, wireframe_only, queue, camera);
        }

        queue.submit(std::iter::once(encoder.finish()));

        // Composite: always needed to convert HDR → sRGB surface
        if has_postprocess {
            self.render_postprocess(device, queue, depth_view, target_view, camera);
        }
    }
}
