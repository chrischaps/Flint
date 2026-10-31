//! Render pipeline setup

use crate::primitives::Vertex;

/// Depth bias for the line-list overlays (wireframe, skeleton), pulling
/// them in front of the surface they trace. WebGPU forbids any depth bias
/// on line topologies and rejects the pipeline, so the web draws them
/// unbiased.
pub(crate) fn line_depth_bias() -> wgpu::DepthBiasState {
    if cfg!(target_arch = "wasm32") {
        wgpu::DepthBiasState::default()
    } else {
        wgpu::DepthBiasState {
            constant: -2,
            slope_scale: -1.0,
            clamp: 0.0,
        }
    }
}
use bytemuck::{Pod, Zeroable};

/// Blend mode for transparent materials
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    /// Standard alpha blending (default)
    Alpha,
    /// Additive blending (glow, energy, fire)
    Additive,
    /// Multiplicative blending (tinted glass, color filters)
    Multiply,
}

/// Transform uniform buffer data (bind group 0)
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct TransformUniforms {
    pub view_proj: [[f32; 4]; 4],
    pub model: [[f32; 4]; 4],
    pub model_inv_transpose: [[f32; 4]; 4],
    pub camera_pos: [f32; 3],
    pub _pad: f32,
}

impl TransformUniforms {
    pub fn new() -> Self {
        let identity = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        Self {
            view_proj: identity,
            model: identity,
            model_inv_transpose: identity,
            camera_pos: [0.0, 0.0, 0.0],
            _pad: 0.0,
        }
    }
}

impl Default for TransformUniforms {
    fn default() -> Self {
        Self::new()
    }
}

/// Material uniform buffer data (bind group 1)
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct MaterialUniforms {
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub use_vertex_color: u32,
    pub debug_mode: u32,
    pub enable_tonemapping: u32,
    pub has_base_color_tex: u32,
    pub has_normal_map: u32,
    pub has_metallic_roughness_tex: u32,
    pub selection_highlight: u32,
    pub opacity: f32,
    pub alpha_cutoff: f32,
    pub texture_scale: f32,
    /// Emission (ADR 0076). `rgb` is the material's own light, linear and
    /// already times its strength. `w` scales per-vertex emission, where a
    /// vertex's alpha says how much of it is surface (see
    /// `ImportedMaterial::vertex_emission`).
    pub emissive: [f32; 4],
    /// 1 when the material has an emissive map (glTF `emissiveTexture`),
    /// bound at 7 and 8. Its rgb multiplies `emissive.rgb`.
    pub has_emissive_tex: u32,
    /// 1 when vertex alpha is the surface share, not opacity (ADR 0090).
    /// Separate from `emissive.w` so a strength of 0 leaves the light parts
    /// dark rather than turning them back into lit, see-through surface.
    pub vertex_emission: u32,
    /// How much of each per-vertex light part is also lit surface, 0..1
    /// (ADR 0096: the entity's `material.glow_surface`). Its light is
    /// unchanged, so with `emissive_scale` a light part's surface and light
    /// are set apart. 0 is the mesh as uploaded.
    pub glow_surface: f32,
    pub _pad_emissive: u32,
    /// Multiplies every vertex colour, `rgb` only, as surface and as
    /// per-vertex light (ADR 0102: the entity's `material.vertex_tint_*`).
    /// White is the mesh as uploaded. Unused without `use_vertex_color`.
    pub vertex_tint: [f32; 4],
}

impl MaterialUniforms {
    /// Default material for procedural shapes (uses vertex color)
    pub fn procedural() -> Self {
        Self {
            base_color: [1.0, 1.0, 1.0, 1.0],
            metallic: 0.0,
            roughness: 0.5,
            use_vertex_color: 1,
            debug_mode: 0,
            enable_tonemapping: 1,
            has_base_color_tex: 0,
            has_normal_map: 0,
            has_metallic_roughness_tex: 0,
            selection_highlight: 0,
            opacity: 1.0,
            alpha_cutoff: 0.5,
            texture_scale: 1.0,
            emissive: [0.0; 4],
            has_emissive_tex: 0,
            vertex_emission: 0,
            glow_surface: 0.0,
            _pad_emissive: 0,
            vertex_tint: [1.0; 4],
        }
    }

    /// Material from imported PBR parameters
    pub fn from_pbr(base_color: [f32; 4], metallic: f32, roughness: f32) -> Self {
        Self {
            base_color,
            metallic,
            roughness,
            use_vertex_color: 0,
            debug_mode: 0,
            enable_tonemapping: 1,
            has_base_color_tex: 0,
            has_normal_map: 0,
            has_metallic_roughness_tex: 0,
            selection_highlight: 0,
            opacity: 1.0,
            alpha_cutoff: 0.5,
            texture_scale: 1.0,
            emissive: [0.0; 4],
            has_emissive_tex: 0,
            vertex_emission: 0,
            glow_surface: 0.0,
            _pad_emissive: 0,
            vertex_tint: [1.0; 4],
        }
    }

    /// Sets the emission from an imported material.
    pub fn set_emission(&mut self, material: &flint_import::ImportedMaterial) {
        let [r, g, b] = material.emissive;
        let s = material.emissive_strength;
        let per_vertex = material.vertex_emission && material.use_vertex_color;
        self.emissive = [r * s, g * s, b * s, if per_vertex { s } else { 0.0 }];
        self.vertex_emission = per_vertex as u32;
    }

    /// Multiplies all of the emission, the material's own and per-vertex,
    /// by `scale` (ADR 0090: the entity's `material.emissive_scale`). The
    /// surface share of each vertex is unchanged, so 0 leaves pure-light
    /// vertices black and lit-surface vertices lit.
    pub fn scale_emission(&mut self, scale: f32) {
        let scale = scale.max(0.0);
        for c in &mut self.emissive {
            *c *= scale;
        }
    }
}

impl Default for MaterialUniforms {
    fn default() -> Self {
        Self::procedural()
    }
}

/// A directional light (sun, moon, etc.)
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct DirectionalLight {
    pub direction: [f32; 3],
    pub volumetric_intensity: f32, // 0.0 = no volumetric (was _pad0)
    pub color: [f32; 3],
    pub intensity: f32,
    pub volumetric_color: [f32; 3], // god ray tint, defaults to light color
    /// Apparent angular size of the source in radians (authored in degrees
    /// on the light table); 0 = punctual → hard legacy shadows. Drives PCSS
    /// penumbra via ShadowUniforms.pcss.w — the WGSL mirrors keep this slot
    /// named `_pad1` because no shader reads it directly (ADR 0056).
    pub angular_size: f32, // was _pad1 — layout unchanged
}

impl Default for DirectionalLight {
    fn default() -> Self {
        Self {
            direction: [0.0, -1.0, 0.0],
            volumetric_intensity: 0.0,
            color: [1.0, 1.0, 1.0],
            intensity: 0.0,
            volumetric_color: [1.0, 1.0, 1.0],
            angular_size: 0.0,
        }
    }
}

/// A point light with radius-based attenuation
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct PointLight {
    pub position: [f32; 3],
    pub radius: f32,
    pub color: [f32; 3],
    pub intensity: f32,
    /// Physical source radius in world units (representative-point area
    /// light, ADR 0056); 0 = punctual → verbatim legacy shading. Grew the
    /// struct 32 → 48 B — mirrored in all six WGSL light structs.
    pub source_radius: f32,
    pub _pad0: f32,
    pub _pad1: f32,
    pub _pad2: f32,
}

impl Default for PointLight {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            radius: 10.0,
            color: [1.0, 1.0, 1.0],
            intensity: 0.0,
            source_radius: 0.0,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        }
    }
}

/// A spot light with inner/outer cone angles
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SpotLight {
    pub position: [f32; 3],
    pub radius: f32,
    pub direction: [f32; 3],
    pub inner_angle: f32,
    pub color: [f32; 3],
    pub outer_angle: f32,
    pub intensity: f32,
    /// Physical source radius in world units (ADR 0056); 0 = punctual.
    /// Rides the former _pad0 slot — layout unchanged; only shader.wgsl's
    /// mirror renames the field (sole reader).
    pub source_radius: f32, // was _pad0
    pub _pad1: f32,
    pub _pad2: f32,
}

impl Default for SpotLight {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            radius: 10.0,
            direction: [0.0, -1.0, 0.0],
            inner_angle: 0.3,
            color: [1.0, 1.0, 1.0],
            outer_angle: 0.5,
            intensity: 0.0,
            source_radius: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        }
    }
}

/// Maximum counts for each light type in the uniform buffer
pub const MAX_DIRECTIONAL_LIGHTS: usize = 4;
pub const MAX_POINT_LIGHTS: usize = 16;
pub const MAX_SPOT_LIGHTS: usize = 8;

/// Combined light uniform buffer (bind group 2)
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct LightUniforms {
    pub directional_lights: [DirectionalLight; MAX_DIRECTIONAL_LIGHTS],
    pub point_lights: [PointLight; MAX_POINT_LIGHTS],
    pub spot_lights: [SpotLight; MAX_SPOT_LIGHTS],
    pub directional_count: u32,
    pub point_count: u32,
    pub spot_count: u32,
    pub _pad: u32,
    /// rgb = hemisphere sky color; w encodes the diffuse-wrap knob as
    /// (1 + wrap) so legacy writes of 1.0 decode to wrap = 0 (ADR 0044).
    pub ambient_sky: [f32; 4],
    /// rgb = hemisphere ground color; w encodes the Oren-Nayar diffuse
    /// blend the same way: (1 + oren), legacy 1.0 = off (ADR 0048).
    pub ambient_ground: [f32; 4],
    /// rgb = sheen tint (linear), w = sheen strength; all-zero = off.
    /// New field (not an encoding trick): `Pod::zeroed()` and every
    /// pre-existing writer leave it disabled. Mirrored in SIX WGSL structs —
    /// shader / skinned / terrain / grass_render / volumetric / ocean —
    /// see the `light_uniforms_layout` test (ADR 0048).
    pub sheen_color_strength: [f32; 4],
}

impl LightUniforms {
    /// Built-in hemisphere ambient colors (linear RGB), used whenever a scene
    /// does not author `[environment] ambient_sky / ambient_ground`.
    pub const DEFAULT_AMBIENT_SKY: [f32; 3] = [0.12, 0.13, 0.18];
    pub const DEFAULT_AMBIENT_GROUND: [f32; 3] = [0.06, 0.05, 0.04];

    /// Default lighting — matches the original two hardcoded directionals
    pub fn default_scene_lights() -> Self {
        let mut lights = Self::zeroed();

        // Key light (warm sun from upper-right)
        lights.directional_lights[0] = DirectionalLight {
            direction: [0.5, 1.0, 0.3],
            volumetric_intensity: 0.0,
            color: [1.0, 0.98, 0.95],
            intensity: 3.0,
            volumetric_color: [1.0, 0.98, 0.95],
            angular_size: 0.0,
        };

        // Fill light (cool, from lower-left-behind)
        lights.directional_lights[1] = DirectionalLight {
            direction: [-0.4, -0.3, -0.6],
            volumetric_intensity: 0.0,
            color: [0.6, 0.7, 0.9],
            intensity: 0.8,
            volumetric_color: [0.6, 0.7, 0.9],
            angular_size: 0.0,
        };

        lights.directional_count = 2;
        lights.point_count = 0;
        lights.spot_count = 0;
        lights._pad = 0;
        let [sr, sg, sb] = Self::DEFAULT_AMBIENT_SKY;
        let [gr, gg, gb] = Self::DEFAULT_AMBIENT_GROUND;
        lights.ambient_sky = [sr, sg, sb, 1.0];
        lights.ambient_ground = [gr, gg, gb, 1.0];

        lights
    }
}

/// The main render pipeline
pub struct RenderPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub line_pipeline: wgpu::RenderPipeline,
    pub overlay_line_pipeline: wgpu::RenderPipeline,
    pub skeleton_line_pipeline: wgpu::RenderPipeline,
    pub outline_pipeline: wgpu::RenderPipeline,
    pub depth_prepass_pipeline: wgpu::RenderPipeline,
    pub transparent_alpha_pipeline: wgpu::RenderPipeline,
    /// Back-face pass of the alpha pipeline (drawn first).
    pub transparent_alpha_back_pipeline: wgpu::RenderPipeline,
    pub transparent_additive_pipeline: wgpu::RenderPipeline,
    pub transparent_multiply_pipeline: wgpu::RenderPipeline,
    pub transform_bind_group_layout: wgpu::BindGroupLayout,
    pub material_bind_group_layout: wgpu::BindGroupLayout,
    pub light_bind_group_layout: wgpu::BindGroupLayout,
    /// The sampler every PBR material's textures share, bound in the light
    /// group (binding 4) rather than in each material's: linear, with linear
    /// mips, repeating, as `TextureCache` made every texture's own.
    pub material_sampler: wgpu::Sampler,
}

impl RenderPipeline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, sample_count: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("PBR Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        // Bind group 0: Transform uniforms (vertex + fragment)
        let transform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
                label: Some("Transform Bind Group Layout"),
            });

        // Bind group 1: Material uniforms + textures (fragment only). No
        // samplers: they sample with the light group's `material_sampler`,
        // so a material bind group spends no sampler descriptors. wgpu 23's
        // DX12 backend copies every bind group's samplers into a 2048-slot
        // shader-visible heap with no sharing, and four a material capped a
        // scene at about 512 distinct looks (Capitol #525, #577). Bindings
        // keep their numbers (textures at 1, 3, 5, 7).
        let material_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    // binding 0: MaterialUniforms
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // binding 1: base_color_texture
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // binding 3: normal_map_texture
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // binding 5: metallic_roughness_texture
                    wgpu::BindGroupLayoutEntry {
                        binding: 5,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // binding 7: emissive_texture
                    wgpu::BindGroupLayoutEntry {
                        binding: 7,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
                label: Some("Material Bind Group Layout"),
            });

        // Bind group 2: Light uniforms + shadow resources (fragment only)
        let light_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    // binding 0: LightUniforms
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // binding 1: shadow_maps (depth texture array)
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Depth,
                            view_dimension: wgpu::TextureViewDimension::D2Array,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // binding 2: shadow comparison sampler
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                        count: None,
                    },
                    // binding 3: ShadowUniforms (cascade matrices + splits)
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // binding 4: material_sampler, the one sampler every
                    // material's textures share (see bind group 1)
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
                label: Some("Light Bind Group Layout"),
            });

        let material_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Material Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            // Ground seen edge-on (roads, floors) keeps its detail; needs
            // all three filters linear, as they are
            anisotropy_clamp: 8,
            ..Default::default()
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("PBR Pipeline Layout"),
            bind_group_layouts: &[
                &transform_bind_group_layout,
                &material_bind_group_layout,
                &light_bind_group_layout,
            ],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("PBR Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Vertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });

        // Line pipeline for wireframes and grid
        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Line Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Vertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });

        // Overlay line pipeline — renders lines on top of solid geometry with depth bias
        let overlay_line_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Overlay Line Pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Vertex::desc()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::LineList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: wgpu::StencilState::default(),
                    bias: line_depth_bias(),
                }),
                multisample: wgpu::MultisampleState {
                    count: sample_count,
                    ..Default::default()
                },
                multiview: None,
                cache: None,
            });

        // Overlay line pipeline — renders lines on top of solid geometry with depth bias
        let skeleton_line_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Skeleton Line Pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Vertex::desc()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::LineList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::Always,
                    stencil: wgpu::StencilState::default(),
                    bias: line_depth_bias(),
                }),
                multisample: wgpu::MultisampleState {
                    count: sample_count,
                    ..Default::default()
                },
                multiview: None,
                cache: None,
            });

        // Outline pipeline — inverted hull (front-face culled, solid orange)
        let outline_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Outline Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("outline_shader.wgsl").into()),
        });

        let outline_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Outline Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &outline_shader,
                entry_point: Some("vs_outline"),
                buffers: &[Vertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &outline_shader,
                entry_point: Some("fs_outline"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Front), // Inverted hull: cull front faces
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });

        // Depth prepass pipeline — writes depth only (no color) for selected entity
        // front faces, used in wireframe mode to mask the outline interior.
        let depth_prepass_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Depth Prepass Pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &outline_shader,
                    entry_point: Some("vs_depth_prepass"),
                    buffers: &[Vertex::desc()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &outline_shader,
                    entry_point: Some("fs_outline"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::empty(),
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: sample_count,
                    ..Default::default()
                },
                multiview: None,
                cache: None,
            });

        // Alpha-blended meshes draw in two passes, back faces first (cull front) then
        // front faces (cull back). Depth write stays off, but with no sort inside one
        // mesh a single double-sided pass blends its own back faces over its front
        // faces in index order, which shows as triangular patches on a translucent
        // sphere. Two passes make every convex translucent object order-independent.
        let make_transparent_alpha = |label: &str, cull_mode: Option<wgpu::Face>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Vertex::desc()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: sample_count,
                    ..Default::default()
                },
                multiview: None,
                cache: None,
            })
        };
        let transparent_alpha_back_pipeline =
            make_transparent_alpha("Transparent Alpha Pipeline (back faces)", Some(wgpu::Face::Front));
        let transparent_alpha_pipeline =
            make_transparent_alpha("Transparent Alpha Pipeline (front faces)", Some(wgpu::Face::Back));

        // Transparent additive pipeline: src*SrcAlpha + dst*One
        let transparent_additive_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Transparent Additive Pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Vertex::desc()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::SrcAlpha,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: sample_count,
                    ..Default::default()
                },
                multiview: None,
                cache: None,
            });

        // Transparent multiply pipeline: src*DstColor + dst*OneMinusSrcAlpha
        let transparent_multiply_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Transparent Multiply Pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Vertex::desc()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::Dst,
                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                operation: wgpu::BlendOperation::Add,
                            },
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: false,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: sample_count,
                    ..Default::default()
                },
                multiview: None,
                cache: None,
            });

        Self {
            pipeline,
            line_pipeline,
            overlay_line_pipeline,
            skeleton_line_pipeline,
            outline_pipeline,
            depth_prepass_pipeline,
            transparent_alpha_pipeline,
            transparent_alpha_back_pipeline,
            transparent_additive_pipeline,
            transparent_multiply_pipeline,
            transform_bind_group_layout,
            material_bind_group_layout,
            light_bind_group_layout,
            material_sampler,
        }
    }
}
