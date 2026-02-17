//! Scene renderer - converts FlintWorld entities to GPU meshes

use crate::billboard_pipeline::{BillboardDrawCall, BillboardPipeline, BillboardUniforms, SpriteInstance};
use crate::camera::{Camera, mat4_inverse, mat4_mul};
use crate::context::RenderContext;
use crate::debug::{DebugMode, DebugState};
use crate::gpu_mesh::MeshCache;
use crate::particle_pipeline::{ParticleDrawCall, ParticleDrawData, ParticlePipeline, ParticleUniforms};
use crate::pipeline::{
    DirectionalLight, LightUniforms, MaterialUniforms, PointLight, RenderPipeline, SpotLight,
    TransformUniforms, MAX_DIRECTIONAL_LIGHTS, MAX_POINT_LIGHTS, MAX_SPOT_LIGHTS,
};
use crate::postprocess::{PostProcessConfig, PostProcessPipeline, PostProcessResources, HDR_FORMAT};
use crate::shadow::{ShadowDrawUniforms, ShadowPass, CASCADE_COUNT, DEFAULT_SHADOW_RESOLUTION};
use crate::skybox_pipeline::{SkyboxPipeline, SkyboxUniforms};
use crate::skinned_pipeline::SkinnedPipeline;
use crate::texture_cache::TextureCache;
use crate::primitives::{
    create_box_mesh, create_grid_mesh, create_wireframe_box_mesh, generate_normal_arrows,
    triangles_to_wireframe_indices, Mesh,
};
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
    billboard_draws: Vec<BillboardDrawCall>,
    debug_state: DebugState,
    wireframe_overlay_draws: Vec<DrawCall>,
    normal_arrow_draws: Vec<DrawCall>,
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
    // Particles
    particle_pipeline: Option<ParticlePipeline>,
    particle_draws: Vec<ParticleDrawCall>,
    // Post-processing
    postprocess_pipeline: Option<PostProcessPipeline>,
    postprocess_resources: Option<PostProcessResources>,
    postprocess_config: PostProcessConfig,
    #[allow(dead_code)]
    surface_format: wgpu::TextureFormat,
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

        let shadow_pass = ShadowPass::new(
            &context.device,
            DEFAULT_SHADOW_RESOLUTION,
        );

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
        let particle_pipeline = ParticlePipeline::new(&context.device, scene_format);
        let skybox_pipeline = SkyboxPipeline::new(&context.device, scene_format);

        // Create post-processing pipeline and resources
        let postprocess_pipeline = PostProcessPipeline::new(&context.device, surface_format);
        let postprocess_resources = PostProcessResources::new(
            &context.device,
            context.config.width,
            context.config.height,
        );
        let postprocess_config = PostProcessConfig::default();

        Self {
            pipeline,
            skinned_pipeline: Some(skinned_pipeline),
            billboard_pipeline: Some(billboard_pipeline),
            archetype_visuals,
            mesh_cache: MeshCache::new(),
            grid_draw,
            entity_draws: Vec::new(),
            skinned_entity_draws: Vec::new(),
            billboard_draws: Vec::new(),
            debug_state: DebugState::default(),
            wireframe_overlay_draws: Vec::new(),
            normal_arrow_draws: Vec::new(),
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
            particle_pipeline: Some(particle_pipeline),
            particle_draws: Vec::new(),
            postprocess_pipeline: Some(postprocess_pipeline),
            postprocess_resources: Some(postprocess_resources),
            postprocess_config,
            surface_format,
        }
    }

    /// Upload particle instance data from the simulation and create draw calls.
    /// Called each frame after ParticleSystem::update().
    pub fn update_particles(
        &mut self,
        device: &wgpu::Device,
        draw_data: Vec<ParticleDrawData>,
    ) {
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

        let shadow_pass = ShadowPass::new(
            device,
            DEFAULT_SHADOW_RESOLUTION,
        );

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

        let billboard_pipeline = BillboardPipeline::new(device, scene_format);
        let skybox_pipeline = SkyboxPipeline::new(device, scene_format);

        // Create post-processing pipeline and resources for headless
        let postprocess_pipeline = PostProcessPipeline::new(device, surface_format);
        let postprocess_resources = PostProcessResources::new(device, width, height);
        let postprocess_config = PostProcessConfig::default();

        Self {
            pipeline,
            skinned_pipeline: Some(skinned_pipeline),
            billboard_pipeline: Some(billboard_pipeline),
            archetype_visuals,
            mesh_cache: MeshCache::new(),
            grid_draw,
            entity_draws: Vec::new(),
            skinned_entity_draws: Vec::new(),
            billboard_draws: Vec::new(),
            debug_state: DebugState::default(),
            wireframe_overlay_draws: Vec::new(),
            normal_arrow_draws: Vec::new(),
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
            particle_pipeline: None, // No particles in headless mode
            particle_draws: Vec::new(),
            postprocess_pipeline: Some(postprocess_pipeline),
            postprocess_resources: Some(postprocess_resources),
            postprocess_config,
            surface_format,
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

        // Upload textures referenced by materials
        if let Some(cache) = &mut self.texture_cache {
            for texture in &import_result.textures {
                cache.upload(device, queue, &texture.name, texture);
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

        // Also upload textures
        if let Some(cache) = &mut self.texture_cache {
            for texture in &import_result.textures {
                cache.upload(device, queue, &texture.name, texture);
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
        if let Some(skinned_meshes) = self.mesh_cache.get_skinned_mut(asset_name) {
            for mesh in skinned_meshes.iter_mut() {
                queue.write_buffer(
                    &mesh.bone_buffer,
                    0,
                    bytemuck::cast_slice(matrices),
                );
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

    /// Toggle normal direction arrows on/off, returns the new state
    pub fn toggle_normal_arrows(&mut self) -> bool {
        self.debug_state.show_normals = !self.debug_state.show_normals;
        self.debug_state.show_normals
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
        let mut shadow_pass = ShadowPass::new(
            device,
            resolution,
        );
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
                eprintln!("Skybox pipeline not available");
                return;
            }
        };

        // Load panorama image
        let img = match image::open(path) {
            Ok(img) => img.to_rgba8(),
            Err(e) => {
                eprintln!("Failed to load skybox '{}': {:?}", path.display(), e);
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

    /// Update meshes from the world state
    pub fn update_from_world(&mut self, world: &FlintWorld, device: &wgpu::Device) {
        self.entity_draws.clear();
        self.skinned_entity_draws.clear();
        self.billboard_draws.clear();
        self.wireframe_overlay_draws.clear();
        self.normal_arrow_draws.clear();

        // Extract lights from scene entities
        self.extract_lights_from_world(world);

        let need_overlay = self.debug_state.wireframe_overlay
            || self.debug_state.mode == DebugMode::WireframeOnly;
        let need_normals = self.debug_state.show_normals;
        let arrow_length = self.debug_state.normal_arrow_length;

        // Temporarily take texture_cache to avoid borrow conflicts
        let tex_cache = self.texture_cache.take();
        let tex_cache_ref = tex_cache.as_ref().unwrap();

        for entity in world.all_entities() {
            let archetype = entity.archetype.as_deref().unwrap_or("unknown");
            let visual = self
                .archetype_visuals
                .get(archetype)
                .cloned()
                .unwrap_or(ArchetypeVisual {
                    color: [0.5, 0.5, 0.5, 1.0],
                    wireframe: false,
                    default_size: [1.0, 1.0, 1.0],
                });

            let model_matrix = world.get_world_matrix(entity.id)
                .unwrap_or_else(|| Transform::default().to_matrix());
            let world_pos = [model_matrix[3][0], model_matrix[3][1], model_matrix[3][2]];

            // Check if entity has a model component
            let model_asset = world
                .get_components(entity.id)
                .and_then(|components| components.get("model").cloned())
                .and_then(|model| {
                    model
                        .get("asset")
                        .and_then(|v| v.as_str().map(String::from))
                });

            if let Some(asset_name) = &model_asset {
                // Check for skinned meshes first
                if let Some(skinned_meshes) = self.mesh_cache.get_skinned(asset_name) {
                    let inv_transpose = mat4_inv_transpose(&model_matrix);

                    for gpu_mesh in skinned_meshes {
                        let transform_uniforms = TransformUniforms {
                            view_proj: [[0.0; 4]; 4],
                            model: model_matrix,
                            model_inv_transpose: inv_transpose,
                            camera_pos: [0.0; 3],
                            _pad: 0.0,
                        };

                        let (bc_view, bc_sampler, has_bc) =
                            Self::resolve_texture(tex_cache_ref, gpu_mesh.material.base_color_texture.as_deref(), &tex_cache_ref.default_white);
                        let (nm_view, nm_sampler, has_nm) =
                            Self::resolve_texture(tex_cache_ref, gpu_mesh.material.normal_texture.as_deref(), &tex_cache_ref.default_normal);
                        let (mr_view, mr_sampler, has_mr) =
                            Self::resolve_texture(tex_cache_ref, gpu_mesh.material.metallic_roughness_texture.as_deref(), &tex_cache_ref.default_metallic_roughness);

                        let mut material_uniforms = MaterialUniforms::from_pbr(
                            gpu_mesh.material.base_color,
                            gpu_mesh.material.metallic,
                            gpu_mesh.material.roughness,
                        );
                        material_uniforms.has_base_color_tex = if has_bc { 1 } else { 0 };
                        material_uniforms.has_normal_map = if has_nm { 1 } else { 0 };
                        material_uniforms.has_metallic_roughness_tex = if has_mr { 1 } else { 0 };
                        if gpu_mesh.material.use_vertex_color {
                            material_uniforms.use_vertex_color = 1;
                        }

                        let (transform_buffer, transform_bind_group) =
                            Self::create_transform_bind(device, &self.pipeline, &transform_uniforms);
                        let (material_buffer, material_bind_group) =
                            Self::create_material_bind_with_textures(
                                device,
                                &self.pipeline,
                                &material_uniforms,
                                bc_view, bc_sampler,
                                nm_view, nm_sampler,
                                mr_view, mr_sampler,
                            );

                        let bone_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                            layout: &self.skinned_pipeline.as_ref().unwrap().bone_bind_group_layout,
                            entries: &[wgpu::BindGroupEntry {
                                binding: 0,
                                resource: gpu_mesh.bone_buffer.as_entire_binding(),
                            }],
                            label: Some("Skinned Draw Bone Bind Group"),
                        });

                        self.skinned_entity_draws.push(SkinnedDrawCall {
                            vertex_buffer: gpu_mesh.create_vertex_buffer_copy(device),
                            index_buffer: gpu_mesh.create_index_buffer_copy(device),
                            index_count: gpu_mesh.index_count,
                            transform_buffer,
                            transform_bind_group,
                            material_buffer,
                            material_bind_group,
                            bone_bind_group,
                            model: model_matrix,
                            model_inv_transpose: inv_transpose,
                            entity_id: Some(entity.id),
                        });
                    }
                    continue;
                }

                if let Some(gpu_meshes) = self.mesh_cache.get(asset_name) {
                    let inv_transpose = mat4_inv_transpose(&model_matrix);

                    for gpu_mesh in gpu_meshes {
                        let transform_uniforms = TransformUniforms {
                            view_proj: [[0.0; 4]; 4],
                            model: model_matrix,
                            model_inv_transpose: inv_transpose,
                            camera_pos: [0.0; 3],
                            _pad: 0.0,
                        };

                        // Resolve textures for this material
                        let (bc_view, bc_sampler, has_bc) =
                            Self::resolve_texture(tex_cache_ref, gpu_mesh.material.base_color_texture.as_deref(), &tex_cache_ref.default_white);
                        let (nm_view, nm_sampler, has_nm) =
                            Self::resolve_texture(tex_cache_ref, gpu_mesh.material.normal_texture.as_deref(), &tex_cache_ref.default_normal);
                        let (mr_view, mr_sampler, has_mr) =
                            Self::resolve_texture(tex_cache_ref, gpu_mesh.material.metallic_roughness_texture.as_deref(), &tex_cache_ref.default_metallic_roughness);

                        let base_color = if let Some(override_color) = world
                            .get_components(entity.id)
                            .and_then(|c| c.get("material"))
                            .and_then(|m| {
                                let r = m.get("base_color_r")?.as_float()? as f32;
                                let g = m.get("base_color_g")?.as_float()? as f32;
                                let b = m.get("base_color_b")?.as_float()? as f32;
                                let a = m.get("base_color_a").and_then(|v| v.as_float()).unwrap_or(1.0) as f32;
                                Some([r, g, b, a])
                            }) {
                            override_color
                        } else {
                            gpu_mesh.material.base_color
                        };

                        let mut material_uniforms = MaterialUniforms::from_pbr(
                            base_color,
                            gpu_mesh.material.metallic,
                            gpu_mesh.material.roughness,
                        );
                        material_uniforms.has_base_color_tex = if has_bc { 1 } else { 0 };
                        material_uniforms.has_normal_map = if has_nm { 1 } else { 0 };
                        material_uniforms.has_metallic_roughness_tex = if has_mr { 1 } else { 0 };
                        if gpu_mesh.material.use_vertex_color {
                            material_uniforms.use_vertex_color = 1;
                        }

                        let mut draw = Self::create_imported_draw_call(
                            device,
                            &self.pipeline,
                            gpu_mesh,
                            transform_uniforms,
                            material_uniforms,
                            bc_view, bc_sampler,
                            nm_view, nm_sampler,
                            mr_view, mr_sampler,
                        );
                        draw.entity_id = Some(entity.id);
                        self.entity_draws.push(draw);

                        // Generate wireframe overlay for imported meshes
                        if need_overlay {
                            let tri_indices = gpu_mesh.triangle_indices();
                            let wire_indices = triangles_to_wireframe_indices(&tri_indices);
                            if !wire_indices.is_empty() {
                                let vertices = gpu_mesh.vertices();
                                let black_verts: Vec<_> = vertices
                                    .iter()
                                    .map(|v| crate::primitives::Vertex {
                                        color: [0.0, 0.0, 0.0, 1.0],
                                        ..*v
                                    })
                                    .collect();
                                let wire_mesh = Mesh {
                                    vertices: black_verts,
                                    indices: wire_indices,
                                };
                                let wire_transform = TransformUniforms {
                                    view_proj: [[0.0; 4]; 4],
                                    model: model_matrix,
                                    model_inv_transpose: inv_transpose,
                                    camera_pos: [0.0; 3],
                                    _pad: 0.0,
                                };
                                let overlay = Self::create_draw_call(
                                    device,
                                    &self.pipeline,
                                    &wire_mesh,
                                    true,
                                    wire_transform,
                                    MaterialUniforms::procedural(),
                                    tex_cache_ref,
                                );
                                self.wireframe_overlay_draws.push(overlay);
                            }
                        }

                        // Generate normal arrows for imported meshes
                        if need_normals {
                            let tri_indices = gpu_mesh.triangle_indices();
                            let vertices = gpu_mesh.vertices();
                            let arrows = generate_normal_arrows(&vertices, &tri_indices, arrow_length);
                            if !arrows.indices.is_empty() {
                                let arrow_transform = TransformUniforms {
                                    view_proj: [[0.0; 4]; 4],
                                    model: model_matrix,
                                    model_inv_transpose: inv_transpose,
                                    camera_pos: [0.0; 3],
                                    _pad: 0.0,
                                };
                                let arrow_draw = Self::create_draw_call(
                                    device,
                                    &self.pipeline,
                                    &arrows,
                                    true,
                                    arrow_transform,
                                    MaterialUniforms::procedural(),
                                    tex_cache_ref,
                                );
                                self.normal_arrow_draws.push(arrow_draw);
                            }
                        }
                    }
                    continue;
                }
            }

            // Check for sprite component — render as billboard instead of geometry
            if let Some(components) = world.get_components(entity.id) {
                if let Some(sprite) = components.get("sprite") {
                    let visible = sprite.get("visible")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    if visible {
                        if let Some(bp) = &self.billboard_pipeline {
                            let tex_name = sprite.get("texture")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let width = sprite.get("width")
                                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                                .unwrap_or(1.0) as f32;
                            let height = sprite.get("height")
                                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                                .unwrap_or(1.0) as f32;
                            let frame = sprite.get("frame")
                                .and_then(|v| v.as_integer())
                                .unwrap_or(0) as u32;
                            let frames_x = sprite.get("frames_x")
                                .and_then(|v| v.as_integer())
                                .unwrap_or(1) as u32;
                            let frames_y = sprite.get("frames_y")
                                .and_then(|v| v.as_integer())
                                .unwrap_or(1) as u32;
                            let anchor_y = sprite.get("anchor_y")
                                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                                .unwrap_or(0.0) as f32;
                            let fullbright = sprite.get("fullbright")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(true);

                            let sprite_instance = SpriteInstance {
                                world_pos,
                                width,
                                height,
                                frame,
                                frames_x,
                                frames_y,
                                anchor_y,
                                fullbright: if fullbright { 1 } else { 0 },
                                selection_highlight: 0,
                                _pad1: 0.0,
                            };

                            // Billboard uniforms will be filled during render (need camera)
                            let billboard_uniforms = BillboardUniforms {
                                view_proj: [[0.0; 4]; 4],
                                camera_right: [1.0, 0.0, 0.0],
                                _pad0: 0.0,
                                camera_up: [0.0, 1.0, 0.0],
                                _pad1: 0.0,
                            };

                            let billboard_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some("Billboard Uniform Buffer"),
                                contents: bytemuck::cast_slice(&[billboard_uniforms]),
                                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                            });

                            let sprite_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some("Sprite Instance Buffer"),
                                contents: bytemuck::cast_slice(&[sprite_instance]),
                                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                            });

                            let billboard_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                                layout: &bp.billboard_bind_group_layout,
                                entries: &[
                                    wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: billboard_buffer.as_entire_binding(),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: sprite_buffer.as_entire_binding(),
                                    },
                                ],
                                label: Some("Billboard Bind Group"),
                            });

                            // Resolve sprite texture
                            let (tex_view, tex_sampler, _has_tex) = Self::resolve_texture(
                                tex_cache_ref,
                                if tex_name.is_empty() { None } else { Some(tex_name) },
                                &tex_cache_ref.default_white,
                            );

                            let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                                layout: &bp.texture_bind_group_layout,
                                entries: &[
                                    wgpu::BindGroupEntry {
                                        binding: 0,
                                        resource: wgpu::BindingResource::TextureView(tex_view),
                                    },
                                    wgpu::BindGroupEntry {
                                        binding: 1,
                                        resource: wgpu::BindingResource::Sampler(tex_sampler),
                                    },
                                ],
                                label: Some("Billboard Texture Bind Group"),
                            });

                            self.billboard_draws.push(BillboardDrawCall {
                                billboard_buffer,
                                sprite_buffer,
                                billboard_bind_group,
                                texture_bind_group,
                                entity_id: Some(entity.id),
                            });
                        }
                    }
                    continue; // Don't render sprites as geometry
                }
            }

            // Only draw fallback geometry for entities that explicitly have bounds or material.
            // Entities without a model, bounds, or material are non-visual (lights, scripts,
            // particle emitters, splines, etc.) and should not get a default cube.
            if let Some(components) = world.get_components(entity.id) {
                let has_bounds = components.get("bounds").is_some();
                let has_material = components.get("material").is_some();
                if !has_bounds && !has_material {
                    continue;
                }
            }

            // Fall back to procedural shapes
            let (size, bounds_center) = if let Some(components) = world.get_components(entity.id) {
                if let Some(bounds) = components.get("bounds") {
                    extract_bounds_info(bounds)
                        .unwrap_or((visual.default_size, [0.0, 0.0, 0.0]))
                } else {
                    (visual.default_size, [0.0, 0.0, 0.0])
                }
            } else {
                (visual.default_size, [0.0, 0.0, 0.0])
            };

            let mesh = if visual.wireframe {
                create_wireframe_box_mesh(size[0], size[1], size[2], visual.color)
            } else {
                create_box_mesh(size[0], size[1], size[2], visual.color)
            };

            let mut model = model_matrix;
            // Apply bounds_center in local space so rotation pivots around entity position
            let rx = model[0][0] * bounds_center[0] + model[1][0] * bounds_center[1] + model[2][0] * bounds_center[2];
            let ry = model[0][1] * bounds_center[0] + model[1][1] * bounds_center[1] + model[2][1] * bounds_center[2];
            let rz = model[0][2] * bounds_center[0] + model[1][2] * bounds_center[1] + model[2][2] * bounds_center[2];
            model[3][0] += rx;
            model[3][1] += ry;
            model[3][2] += rz;

            let inv_transpose = mat4_inv_transpose(&model);

            let transform_uniforms = TransformUniforms {
                view_proj: [[0.0; 4]; 4],
                model,
                model_inv_transpose: inv_transpose,
                camera_pos: [0.0; 3],
                _pad: 0.0,
            };

            // Check for material.texture to use file-based textures on procedural geometry
            let material_component = world
                .get_components(entity.id)
                .and_then(|components| components.get("material").cloned());

            let material_texture = material_component
                .as_ref()
                .and_then(|m| m.get("texture").and_then(|v| v.as_str().map(String::from)));

            if !visual.wireframe {
                if let Some(tex_name) = &material_texture {
                    let (bc_view, bc_sampler, has_bc) =
                        Self::resolve_texture(tex_cache_ref, Some(tex_name.as_str()), &tex_cache_ref.default_white);

                    if has_bc {
                        let metallic = material_component
                            .as_ref()
                            .and_then(|m| m.get("metallic"))
                            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                            .unwrap_or(0.0) as f32;
                        let roughness = material_component
                            .as_ref()
                            .and_then(|m| m.get("roughness"))
                            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                            .unwrap_or(0.7) as f32;

                        let mut material_uniforms = MaterialUniforms::from_pbr(
                            [1.0, 1.0, 1.0, 1.0],
                            metallic,
                            roughness,
                        );
                        material_uniforms.has_base_color_tex = 1;

                        let mut draw = Self::create_textured_draw_call(
                            device,
                            &self.pipeline,
                            &mesh,
                            transform_uniforms,
                            material_uniforms,
                            bc_view, bc_sampler,
                            &tex_cache_ref.default_normal.view, &tex_cache_ref.default_normal.sampler,
                            &tex_cache_ref.default_metallic_roughness.view, &tex_cache_ref.default_metallic_roughness.sampler,
                        );
                        draw.entity_id = Some(entity.id);
                        self.entity_draws.push(draw);
                    } else {
                        let mut draw = Self::create_draw_call(
                            device,
                            &self.pipeline,
                            &mesh,
                            false,
                            transform_uniforms,
                            MaterialUniforms::procedural(),
                            tex_cache_ref,
                        );
                        draw.entity_id = Some(entity.id);
                        self.entity_draws.push(draw);
                    }
                } else {
                    // Use material.color for PBR base color if present
                    let mat_color = material_component
                        .as_ref()
                        .and_then(|m| m.get("color"))
                        .and_then(|v| extract_color(v));

                    let mat_uniforms = if let Some(color) = mat_color {
                        let metallic = material_component
                            .as_ref()
                            .and_then(|m| m.get("metallic"))
                            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                            .unwrap_or(0.0) as f32;
                        let roughness = material_component
                            .as_ref()
                            .and_then(|m| m.get("roughness"))
                            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                            .unwrap_or(0.7) as f32;
                        MaterialUniforms::from_pbr(color, metallic, roughness)
                    } else {
                        MaterialUniforms::procedural()
                    };

                    let mut draw = Self::create_draw_call(
                        device,
                        &self.pipeline,
                        &mesh,
                        false,
                        transform_uniforms,
                        mat_uniforms,
                        tex_cache_ref,
                    );
                    draw.entity_id = Some(entity.id);
                    self.entity_draws.push(draw);
                }
            } else {
                let mut draw = Self::create_draw_call(
                    device,
                    &self.pipeline,
                    &mesh,
                    true,
                    transform_uniforms,
                    MaterialUniforms::procedural(),
                    tex_cache_ref,
                );
                draw.entity_id = Some(entity.id);
                self.entity_draws.push(draw);
            }

            // Generate wireframe overlay for procedural solid shapes
            if need_overlay && !visual.wireframe {
                let wire_indices = triangles_to_wireframe_indices(&mesh.indices);
                if !wire_indices.is_empty() {
                    let black_verts: Vec<_> = mesh
                        .vertices
                        .iter()
                        .map(|v| crate::primitives::Vertex {
                            color: [0.0, 0.0, 0.0, 1.0],
                            ..*v
                        })
                        .collect();
                    let wire_mesh = Mesh {
                        vertices: black_verts,
                        indices: wire_indices,
                    };
                    let wire_transform = TransformUniforms {
                        view_proj: [[0.0; 4]; 4],
                        model,
                        model_inv_transpose: inv_transpose,
                        camera_pos: [0.0; 3],
                        _pad: 0.0,
                    };
                    let overlay = Self::create_draw_call(
                        device,
                        &self.pipeline,
                        &wire_mesh,
                        true,
                        wire_transform,
                        MaterialUniforms::procedural(),
                        tex_cache_ref,
                    );
                    self.wireframe_overlay_draws.push(overlay);
                }
            }

            // Generate normal arrows for procedural solid shapes
            if need_normals && !visual.wireframe {
                let arrows = generate_normal_arrows(&mesh.vertices, &mesh.indices, arrow_length);
                if !arrows.indices.is_empty() {
                    let arrow_transform = TransformUniforms {
                        view_proj: [[0.0; 4]; 4],
                        model,
                        model_inv_transpose: inv_transpose,
                        camera_pos: [0.0; 3],
                        _pad: 0.0,
                    };
                    let arrow_draw = Self::create_draw_call(
                        device,
                        &self.pipeline,
                        &arrows,
                        true,
                        arrow_transform,
                        MaterialUniforms::procedural(),
                        tex_cache_ref,
                    );
                    self.normal_arrow_draws.push(arrow_draw);
                }
            }
        }

        // Put texture cache back
        self.texture_cache = tex_cache;
    }

    fn create_draw_call(
        device: &wgpu::Device,
        pipeline: &RenderPipeline,
        mesh: &Mesh,
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
        }
    }

    /// Create a draw call for a procedural mesh with explicit texture bindings.
    fn create_textured_draw_call(
        device: &wgpu::Device,
        pipeline: &RenderPipeline,
        mesh: &Mesh,
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
        let (material_buffer, material_bind_group) =
            Self::create_material_bind_with_textures(
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
        let (material_buffer, material_bind_group) =
            Self::create_material_bind_with_textures(
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

    /// Extract light entities from the world and update the light uniform buffer
    fn extract_lights_from_world(&mut self, world: &FlintWorld) {
        let mut dir_count = 0u32;
        let mut point_count = 0u32;
        let mut spot_count = 0u32;
        let mut directionals = [DirectionalLight::default(); MAX_DIRECTIONAL_LIGHTS];
        let mut points = [PointLight::default(); MAX_POINT_LIGHTS];
        let mut spots = [SpotLight::default(); MAX_SPOT_LIGHTS];

        for entity in world.all_entities() {
            let light_component = world
                .get_components(entity.id)
                .and_then(|components| components.get("light").cloned());

            if let Some(light) = light_component {
                let light_type = light
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("directional");

                let color = Self::extract_light_vec3(&light, "color").unwrap_or([1.0, 1.0, 1.0]);
                let intensity = light
                    .get("intensity")
                    .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                    .unwrap_or(1.0) as f32;

                match light_type {
                    "directional" => {
                        if (dir_count as usize) < MAX_DIRECTIONAL_LIGHTS {
                            let direction = Self::extract_light_vec3(&light, "direction")
                                .unwrap_or([0.0, -1.0, 0.0]);
                            directionals[dir_count as usize] = DirectionalLight {
                                direction,
                                _pad0: 0.0,
                                color,
                                intensity,
                            };
                            dir_count += 1;
                        }
                    }
                    "point" => {
                        if (point_count as usize) < MAX_POINT_LIGHTS {
                            let light_pos = world.get_world_position(entity.id)
                                .unwrap_or(Vec3::ZERO);
                            let radius = light
                                .get("range")
                                .or_else(|| light.get("radius"))
                                .and_then(|v| {
                                    v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
                                })
                                .unwrap_or(10.0) as f32;
                            points[point_count as usize] = PointLight {
                                position: [
                                    light_pos.x,
                                    light_pos.y,
                                    light_pos.z,
                                ],
                                radius,
                                color,
                                intensity,
                            };
                            point_count += 1;
                        }
                    }
                    "spot" => {
                        if (spot_count as usize) < MAX_SPOT_LIGHTS {
                            let light_pos = world.get_world_position(entity.id)
                                .unwrap_or(Vec3::ZERO);
                            let direction = Self::extract_light_vec3(&light, "direction")
                                .unwrap_or([0.0, -1.0, 0.0]);
                            let radius = light
                                .get("range")
                                .or_else(|| light.get("radius"))
                                .and_then(|v| {
                                    v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
                                })
                                .unwrap_or(10.0) as f32;
                            let inner_angle = light
                                .get("inner_angle")
                                .and_then(|v| {
                                    v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
                                })
                                .unwrap_or(0.3) as f32;
                            let outer_angle = light
                                .get("outer_angle")
                                .and_then(|v| {
                                    v.as_float().or_else(|| v.as_integer().map(|i| i as f64))
                                })
                                .unwrap_or(0.5) as f32;
                            spots[spot_count as usize] = SpotLight {
                                position: [
                                    light_pos.x,
                                    light_pos.y,
                                    light_pos.z,
                                ],
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

    /// Render the scene using a RenderContext (windowed mode)
    pub fn render(
        &mut self,
        context: &RenderContext,
        camera: &Camera,
        view: &wgpu::TextureView,
    ) -> Result<(), wgpu::SurfaceError> {
        self.render_to(&context.device, &context.queue, &context.depth_view, camera, view);
        Ok(())
    }

    /// Resize post-processing resources (call on window resize).
    pub fn resize_postprocess(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.postprocess_resources = Some(PostProcessResources::new(device, width, height));
    }

    /// Get the current post-processing configuration.
    pub fn post_process_config(&self) -> &PostProcessConfig {
        &self.postprocess_config
    }

    /// Set the post-processing configuration.
    pub fn set_post_process_config(&mut self, config: PostProcessConfig) {
        self.postprocess_config = config;
    }

    /// Render the scene to an arbitrary texture view with explicit device/queue/depth
    pub fn render_to(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        depth_view: &wgpu::TextureView,
        camera: &Camera,
        target_view: &wgpu::TextureView,
    ) {
        let view_proj = camera.view_projection_matrix();
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
        if let Some(shadow_pass) = &mut self.shadow_pass {
            if shadow_pass.enabled && self.light_uniforms.directional_count > 0 {
                // Use the first directional light for shadows
                let light = &self.light_uniforms.directional_lights[0];

                // Update cascade matrices
                let camera_inv = camera.inverse_view_projection_matrix();
                shadow_pass.update_cascades(
                    light.direction,
                    camera_pos,
                    camera_inv,
                    0.1,
                    200.0,
                );

                // Write shadow uniforms
                queue.write_buffer(
                    &shadow_pass.shadow_uniforms_buffer,
                    0,
                    bytemuck::cast_slice(&[*shadow_pass.shadow_uniforms()]),
                );

                // Render each cascade
                for cascade in 0..CASCADE_COUNT {
                    let cascade_vp = shadow_pass.shadow_uniforms().cascade_view_proj[cascade];

                    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some(&format!("Shadow Cascade {} Encoder", cascade)),
                    });

                    {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some(&format!("Shadow Cascade {} Pass", cascade)),
                            color_attachments: &[],
                            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                                view: &shadow_pass.cascade_views[cascade],
                                depth_ops: Some(wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(1.0),
                                    store: wgpu::StoreOp::Store,
                                }),
                                stencil_ops: None,
                            }),
                            timestamp_writes: None,
                            occlusion_query_set: None,
                        });

                        pass.set_pipeline(&shadow_pass.shadow_pipeline);

                        // Render each solid entity
                        for draw in &self.entity_draws {
                            if draw.is_wireframe {
                                continue;
                            }

                            let shadow_uniforms = ShadowDrawUniforms {
                                light_view_proj: cascade_vp,
                                model: draw.model,
                            };

                            let shadow_buffer = device.create_buffer_init(
                                &wgpu::util::BufferInitDescriptor {
                                    label: Some("Shadow Draw Uniform"),
                                    contents: bytemuck::cast_slice(&[shadow_uniforms]),
                                    usage: wgpu::BufferUsages::UNIFORM,
                                },
                            );

                            let shadow_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                                layout: &shadow_pass.shadow_bind_group_layout,
                                entries: &[wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: shadow_buffer.as_entire_binding(),
                                }],
                                label: Some("Shadow Draw Bind Group"),
                            });

                            pass.set_bind_group(0, &shadow_bind, &[]);
                            pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                            pass.set_index_buffer(
                                draw.index_buffer.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            pass.draw_indexed(0..draw.index_count, 0, 0..1);
                        }

                        // Render skinned entities into shadow map
                        pass.set_pipeline(&shadow_pass.skinned_shadow_pipeline);

                        for draw in &self.skinned_entity_draws {
                            let shadow_uniforms = ShadowDrawUniforms {
                                light_view_proj: cascade_vp,
                                model: draw.model,
                            };

                            let shadow_buffer = device.create_buffer_init(
                                &wgpu::util::BufferInitDescriptor {
                                    label: Some("Skinned Shadow Draw Uniform"),
                                    contents: bytemuck::cast_slice(&[shadow_uniforms]),
                                    usage: wgpu::BufferUsages::UNIFORM,
                                },
                            );

                            let shadow_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                                layout: &shadow_pass.shadow_bind_group_layout,
                                entries: &[wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: shadow_buffer.as_entire_binding(),
                                }],
                                label: Some("Skinned Shadow Draw Bind Group"),
                            });

                            pass.set_bind_group(0, &shadow_bind, &[]);
                            pass.set_bind_group(1, &draw.bone_bind_group, &[]);
                            pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                            pass.set_index_buffer(
                                draw.index_buffer.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            pass.draw_indexed(0..draw.index_count, 0, 0..1);
                        }
                    }

                    queue.submit(std::iter::once(encoder.finish()));
                }
            }
        }

        // Update grid transform
        if let Some(grid) = &self.grid_draw {
            let uniforms = TransformUniforms {
                view_proj,
                model: identity_matrix(),
                model_inv_transpose: identity_matrix(),
                camera_pos,
                _pad: 0.0,
            };
            queue.write_buffer(&grid.transform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
        }

        // All scene pipelines target Rgba16Float, so we always render to the HDR
        // buffer and composite to sRGB.  The `enabled` flag only controls whether
        // bloom / vignette are applied during the composite pass.
        let has_postprocess = self.postprocess_pipeline.is_some()
            && self.postprocess_resources.is_some();

        // Shader-side tonemapping is always OFF when compositing through the HDR
        // buffer (the composite pass handles ACES + gamma).  Only fall back to
        // shader tonemapping when there is no post-process pipeline at all.
        let tonemapping_u32: u32 = if !has_postprocess && self.tonemapping_enabled {
            1
        } else {
            0
        };

        // Update entity transforms and write debug_mode + tonemapping into each material buffer
        for draw in &self.entity_draws {
            let uniforms = TransformUniforms {
                view_proj,
                model: draw.model,
                model_inv_transpose: draw.model_inv_transpose,
                camera_pos,
                _pad: 0.0,
            };
            queue.write_buffer(&draw.transform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

            // Write debug_mode at byte offset 28 in the material buffer
            // Layout: base_color(16) + metallic(4) + roughness(4) + use_vertex_color(4) = 28
            queue.write_buffer(
                &draw.material_buffer,
                28,
                bytemuck::cast_slice(&[debug_mode_u32]),
            );

            // Write enable_tonemapping at byte offset 32
            // Layout: ... + debug_mode(4) = 32
            queue.write_buffer(
                &draw.material_buffer,
                32,
                bytemuck::cast_slice(&[tonemapping_u32]),
            );
        }

        // Update skinned entity transforms
        for draw in &self.skinned_entity_draws {
            let uniforms = TransformUniforms {
                view_proj,
                model: draw.model,
                model_inv_transpose: draw.model_inv_transpose,
                camera_pos,
                _pad: 0.0,
            };
            queue.write_buffer(&draw.transform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

            queue.write_buffer(
                &draw.material_buffer,
                28,
                bytemuck::cast_slice(&[debug_mode_u32]),
            );
            queue.write_buffer(
                &draw.material_buffer,
                32,
                bytemuck::cast_slice(&[tonemapping_u32]),
            );
        }

        // Update wireframe overlay transforms
        for draw in &self.wireframe_overlay_draws {
            let uniforms = TransformUniforms {
                view_proj,
                model: draw.model,
                model_inv_transpose: draw.model_inv_transpose,
                camera_pos,
                _pad: 0.0,
            };
            queue.write_buffer(&draw.transform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
        }

        // Update billboard uniforms with camera vectors
        {
            // Extract camera right and up from the view matrix
            let cam_right = camera.right_vector();
            let cam_up = camera.up_vector();
            let billboard_uniforms = BillboardUniforms {
                view_proj,
                camera_right: cam_right,
                _pad0: 0.0,
                camera_up: cam_up,
                _pad1: 0.0,
            };
            for draw in &self.billboard_draws {
                queue.write_buffer(
                    &draw.billboard_buffer,
                    0,
                    bytemuck::cast_slice(&[billboard_uniforms]),
                );
            }
        }

        // Update particle uniforms with camera vectors
        if let Some(pp) = &self.particle_pipeline {
            if !self.particle_draws.is_empty() {
                let cam_right = camera.right_vector();
                let cam_up = camera.up_vector();
                let particle_uniforms = ParticleUniforms {
                    view_proj,
                    camera_right: cam_right,
                    _pad0: 0.0,
                    camera_up: cam_up,
                    _pad1: 0.0,
                };
                queue.write_buffer(
                    &pp.uniform_buffer,
                    0,
                    bytemuck::cast_slice(&[particle_uniforms]),
                );
            }
        }

        // Update normal arrow transforms
        for draw in &self.normal_arrow_draws {
            let uniforms = TransformUniforms {
                view_proj,
                model: draw.model,
                model_inv_transpose: draw.model_inv_transpose,
                camera_pos,
                _pad: 0.0,
            };
            queue.write_buffer(&draw.transform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
        }

        // All scene pipelines target Rgba16Float, so always render to HDR buffer
        // when the post-process resources exist.
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

            // Bind lights once for the entire pass (group 2 is shared)
            render_pass.set_bind_group(2, &self.light_bind_group, &[]);

            // Render skybox (before everything else, at the far plane)
            if let (Some(sp), Some(ub), Some(ubg), Some(tbg)) = (
                &self.skybox_pipeline,
                &self.skybox_uniform_buffer,
                &self.skybox_uniform_bind_group,
                &self.skybox_texture_bind_group,
            ) {
                // Build view matrix with translation stripped (rotation only)
                let view = camera.view_matrix();
                let view_rot_only = [
                    view[0],
                    view[1],
                    view[2],
                    [0.0, 0.0, 0.0, 1.0], // zero out translation column
                ];
                let proj = camera.projection_matrix();
                let vp = mat4_mul(&proj, &view_rot_only);
                let inv_vp = mat4_inverse(&vp);

                queue.write_buffer(
                    ub,
                    0,
                    bytemuck::cast_slice(&[SkyboxUniforms { inv_view_proj: inv_vp }]),
                );

                render_pass.set_pipeline(&sp.pipeline);
                render_pass.set_bind_group(0, ubg, &[]);
                render_pass.set_bind_group(1, tbg, &[]);
                render_pass.draw(0..3, 0..1);
            }

            // Render grid
            if let Some(grid) = &self.grid_draw {
                render_pass.set_pipeline(&self.pipeline.line_pipeline);
                render_pass.set_bind_group(0, &grid.transform_bind_group, &[]);
                render_pass.set_bind_group(1, &grid.material_bind_group, &[]);
                render_pass.set_vertex_buffer(0, grid.vertex_buffer.slice(..));
                render_pass.set_index_buffer(
                    grid.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                render_pass.draw_indexed(0..grid.index_count, 0, 0..1);
            }

            // In WireframeOnly mode: depth prepass masks outline interior,
            // then outline, then wireframe lines on top.
            if wireframe_only {
                if let Some(sel_id) = self.selected_entity {
                    // Step 1: Depth prepass — write front-face depth (no color)
                    // so the outline interior is blocked by the depth buffer.
                    render_pass.set_pipeline(&self.pipeline.depth_prepass_pipeline);
                    for draw in &self.entity_draws {
                        if draw.entity_id == Some(sel_id) && !draw.is_wireframe {
                            render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                            render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                            render_pass.set_index_buffer(
                                draw.index_buffer.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                        }
                    }

                    if let Some(sp) = &self.skinned_pipeline {
                        render_pass.set_pipeline(&sp.depth_prepass_pipeline);
                        for draw in &self.skinned_entity_draws {
                            if draw.entity_id == Some(sel_id) {
                                render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                                render_pass.set_bind_group(3, &draw.bone_bind_group, &[]);
                                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                                render_pass.set_index_buffer(
                                    draw.index_buffer.slice(..),
                                    wgpu::IndexFormat::Uint32,
                                );
                                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                            }
                        }
                    }

                    // Step 2: Outline — back faces pushed out, only visible at silhouette
                    render_pass.set_pipeline(&self.pipeline.outline_pipeline);
                    for draw in &self.entity_draws {
                        if draw.entity_id == Some(sel_id) && !draw.is_wireframe {
                            render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                            render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                            render_pass.set_index_buffer(
                                draw.index_buffer.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                        }
                    }

                    if let Some(sp) = &self.skinned_pipeline {
                        render_pass.set_pipeline(&sp.outline_pipeline);
                        for draw in &self.skinned_entity_draws {
                            if draw.entity_id == Some(sel_id) {
                                render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                                render_pass.set_bind_group(3, &draw.bone_bind_group, &[]);
                                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                                render_pass.set_index_buffer(
                                    draw.index_buffer.slice(..),
                                    wgpu::IndexFormat::Uint32,
                                );
                                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                            }
                        }
                    }
                }

                // Step 3: Wireframe lines — use overlay pipeline (LessEqual + bias)
                // so lines draw on top of the depth prepass.
                render_pass.set_pipeline(&self.pipeline.overlay_line_pipeline);
                for draw in &self.wireframe_overlay_draws {
                    render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                    render_pass.set_bind_group(1, &draw.material_bind_group, &[]);
                    render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                    render_pass.set_index_buffer(
                        draw.index_buffer.slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }

                // Also render entities that are already wireframe (rooms etc.)
                for draw in &self.entity_draws {
                    if draw.is_wireframe {
                        render_pass.set_pipeline(&self.pipeline.overlay_line_pipeline);
                        render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                        render_pass.set_bind_group(1, &draw.material_bind_group, &[]);
                        render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                        render_pass.set_index_buffer(
                            draw.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                    }
                }
            } else {
                // Outline pass for selected standard entities (before normal rendering)
                if let Some(sel_id) = self.selected_entity {
                    render_pass.set_pipeline(&self.pipeline.outline_pipeline);
                    for draw in &self.entity_draws {
                        if draw.entity_id == Some(sel_id) && !draw.is_wireframe {
                            render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                            render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                            render_pass.set_index_buffer(
                                draw.index_buffer.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                        }
                    }
                }

                // Normal entity rendering
                for draw in &self.entity_draws {
                    if draw.is_wireframe {
                        render_pass.set_pipeline(&self.pipeline.line_pipeline);
                    } else {
                        render_pass.set_pipeline(&self.pipeline.pipeline);
                    }
                    render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                    render_pass.set_bind_group(1, &draw.material_bind_group, &[]);
                    render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                    render_pass.set_index_buffer(
                        draw.index_buffer.slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }

                // Outline pass for selected skinned entities
                if let Some(sel_id) = self.selected_entity {
                    if let Some(sp) = &self.skinned_pipeline {
                        render_pass.set_pipeline(&sp.outline_pipeline);
                        for draw in &self.skinned_entity_draws {
                            if draw.entity_id == Some(sel_id) {
                                render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                                render_pass.set_bind_group(3, &draw.bone_bind_group, &[]);
                                render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                                render_pass.set_index_buffer(
                                    draw.index_buffer.slice(..),
                                    wgpu::IndexFormat::Uint32,
                                );
                                render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                            }
                        }
                    }
                }

                // Render skinned entities
                if let Some(sp) = &self.skinned_pipeline {
                    if !self.skinned_entity_draws.is_empty() {
                        render_pass.set_pipeline(&sp.pipeline);
                        for draw in &self.skinned_entity_draws {
                            render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                            render_pass.set_bind_group(1, &draw.material_bind_group, &[]);
                            // Light bind group 2 is already set
                            render_pass.set_bind_group(3, &draw.bone_bind_group, &[]);
                            render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                            render_pass.set_index_buffer(
                                draw.index_buffer.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                        }
                    }
                }

                // Outline pass for selected billboards
                if let Some(sel_id) = self.selected_entity {
                    if let Some(bp) = &self.billboard_pipeline {
                        render_pass.set_pipeline(&bp.outline_pipeline);
                        for draw in &self.billboard_draws {
                            if draw.entity_id == Some(sel_id) {
                                render_pass.set_bind_group(0, &draw.billboard_bind_group, &[]);
                                render_pass.set_bind_group(1, &draw.texture_bind_group, &[]);
                                render_pass.set_index_buffer(
                                    bp.quad_index_buffer.slice(..),
                                    wgpu::IndexFormat::Uint32,
                                );
                                render_pass.draw_indexed(0..6, 0, 0..1);
                            }
                        }
                    }
                }

                // Billboard sprites (after geometry, uses depth test)
                if let Some(bp) = &self.billboard_pipeline {
                    if !self.billboard_draws.is_empty() {
                        render_pass.set_pipeline(&bp.pipeline);
                        for draw in &self.billboard_draws {
                            render_pass.set_bind_group(0, &draw.billboard_bind_group, &[]);
                            render_pass.set_bind_group(1, &draw.texture_bind_group, &[]);
                            render_pass.set_index_buffer(
                                bp.quad_index_buffer.slice(..),
                                wgpu::IndexFormat::Uint32,
                            );
                            render_pass.draw_indexed(0..6, 0, 0..1);
                        }
                    }
                }

                // Particle systems (after billboards, before wireframe overlay)
                // Alpha-blended particles first, then additive
                if let Some(pp) = &self.particle_pipeline {
                    if !self.particle_draws.is_empty() {
                        render_pass.set_index_buffer(
                            pp.quad_index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        render_pass.set_bind_group(0, &pp.uniform_bind_group, &[]);

                        // Alpha pass
                        render_pass.set_pipeline(&pp.alpha_pipeline);
                        for draw in &self.particle_draws {
                            if draw.additive {
                                continue;
                            }
                            render_pass.set_bind_group(1, &draw.instance_bind_group, &[]);
                            render_pass.set_bind_group(2, &draw.texture_bind_group, &[]);
                            render_pass.draw_indexed(0..6, 0, 0..draw.instance_count);
                        }

                        // Additive pass
                        render_pass.set_pipeline(&pp.additive_pipeline);
                        for draw in &self.particle_draws {
                            if !draw.additive {
                                continue;
                            }
                            render_pass.set_bind_group(1, &draw.instance_bind_group, &[]);
                            render_pass.set_bind_group(2, &draw.texture_bind_group, &[]);
                            render_pass.draw_indexed(0..6, 0, 0..draw.instance_count);
                        }
                    }
                }

                // Wireframe overlay pass (on top of solid geometry)
                if self.debug_state.wireframe_overlay {
                    render_pass.set_pipeline(&self.pipeline.overlay_line_pipeline);
                    for draw in &self.wireframe_overlay_draws {
                        render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                        render_pass.set_bind_group(1, &draw.material_bind_group, &[]);
                        render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                        render_pass.set_index_buffer(
                            draw.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                    }
                }
            }

            // Normal arrows pass (always uses line pipeline)
            if self.debug_state.show_normals {
                render_pass.set_pipeline(&self.pipeline.line_pipeline);
                for draw in &self.normal_arrow_draws {
                    render_pass.set_bind_group(0, &draw.transform_bind_group, &[]);
                    render_pass.set_bind_group(1, &draw.material_bind_group, &[]);
                    render_pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                    render_pass.set_index_buffer(
                        draw.index_buffer.slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    render_pass.draw_indexed(0..draw.index_count, 0, 0..1);
                }
            }
        }

        queue.submit(std::iter::once(encoder.finish()));

        // Composite: always needed to convert HDR → sRGB surface
        if has_postprocess {
            let pp = self.postprocess_pipeline.as_ref().unwrap();
            let resources = self.postprocess_resources.as_ref().unwrap();

            // Run bloom if post-processing effects are enabled
            if self.postprocess_config.enabled
                && self.postprocess_config.bloom_enabled
                && resources.bloom_mip_count > 0
            {
                pp.run_bloom(device, queue, resources, &self.postprocess_config);
            }

            // Composite: HDR + bloom → tonemapped sRGB surface
            // (always runs — this is what converts Rgba16Float → surface format)
            pp.composite(device, queue, resources, &self.postprocess_config, target_view);
        }
    }
}

fn identity_matrix() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Compute the inverse-transpose of a 4x4 model matrix (for correct normal transformation).
/// Only the upper 3x3 matters for normals; we embed it in a 4x4 for GPU upload.
fn mat4_inv_transpose(m: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    // Extract upper-left 3x3
    let a = m[0][0]; let b = m[1][0]; let c = m[2][0];
    let d = m[0][1]; let e = m[1][1]; let f = m[2][1];
    let g = m[0][2]; let h = m[1][2]; let i = m[2][2];

    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);

    if det.abs() < 1e-10 {
        return identity_matrix();
    }

    let inv_det = 1.0 / det;

    // Cofactor matrix: cof(i,j) / det gives the inverse-transpose entries.
    // In column-major storage m[col][row], column c needs [cof(0,c), cof(1,c), cof(2,c)] / det.
    //
    // Row 0 cofactors:
    let cof00 = (e * i - f * h) * inv_det;
    let cof01 = (f * g - d * i) * inv_det;
    let cof02 = (d * h - e * g) * inv_det;
    // Row 1 cofactors:
    let cof10 = (c * h - b * i) * inv_det;
    let cof11 = (a * i - c * g) * inv_det;
    let cof12 = (b * g - a * h) * inv_det;
    // Row 2 cofactors:
    let cof20 = (b * f - c * e) * inv_det;
    let cof21 = (c * d - a * f) * inv_det;
    let cof22 = (a * e - b * d) * inv_det;

    // Column-major: column j = [cof(0,j), cof(1,j), cof(2,j)]
    [
        [cof00, cof10, cof20, 0.0],
        [cof01, cof11, cof21, 0.0],
        [cof02, cof12, cof22, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Extract both the size and center offset from bounds.
fn extract_bounds_info(bounds: &toml::Value) -> Option<([f32; 3], [f32; 3])> {
    let min = bounds.get("min")?;
    let max = bounds.get("max")?;

    let min_arr = extract_vec3(min)?;
    let max_arr = extract_vec3(max)?;

    let size = [
        max_arr[0] - min_arr[0],
        max_arr[1] - min_arr[1],
        max_arr[2] - min_arr[2],
    ];

    let center = [
        (min_arr[0] + max_arr[0]) / 2.0,
        (min_arr[1] + max_arr[1]) / 2.0,
        (min_arr[2] + max_arr[2]) / 2.0,
    ];

    Some((size, center))
}

fn extract_vec3(value: &toml::Value) -> Option<[f32; 3]> {
    if let Some(arr) = value.as_array() {
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
            return Some([x, y, z]);
        }
    }
    None
}

/// Extract an RGBA color array from a TOML value like `[0.7, 0.35, 0.2, 1.0]`
fn extract_color(value: &toml::Value) -> Option<[f32; 4]> {
    let arr = value.as_array()?;
    if arr.len() < 3 {
        return None;
    }
    let r = arr[0]
        .as_float()
        .or_else(|| arr[0].as_integer().map(|i| i as f64))? as f32;
    let g = arr[1]
        .as_float()
        .or_else(|| arr[1].as_integer().map(|i| i as f64))? as f32;
    let b = arr[2]
        .as_float()
        .or_else(|| arr[2].as_integer().map(|i| i as f64))? as f32;
    let a = if arr.len() >= 4 {
        arr[3]
            .as_float()
            .or_else(|| arr[3].as_integer().map(|i| i as f64))
            .unwrap_or(1.0) as f32
    } else {
        1.0
    };
    Some([r, g, b, a])
}
