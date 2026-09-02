//! GPU-instanced 2D sprite render pipeline
//!
//! Renders flat XY-plane quads via instanced draw calls using a storage buffer.
//! Follows the particle pipeline pattern: no vertex buffers, quads generated
//! from `vertex_index` in the shader. One draw call per texture batch.
//!
//! Key differences from billboard pipeline:
//! - Sprites are axis-aligned on the XY plane (not camera-facing)
//! - No `camera_right`/`camera_up` needed — just `view_proj`
//! - Storage buffer instancing for efficient batched rendering
//! - Layer-based Z-offset for draw ordering (depth write disabled)

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// GPU instance data for a single 2D sprite — 80 bytes, 16-byte aligned (5 x vec4).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Sprite2dInstanceGpu {
    /// xy = world position, z = anchor_x (0-1), w = layer z-offset
    pub pos_layer: [f32; 4],
    /// x = width, y = height, z = anchor_y (0-1), w = unused
    pub size: [f32; 4],
    /// UV rect: [u_min, v_min, u_max, v_max]
    pub uv_rect: [f32; 4],
    /// RGBA tint color multiplier
    pub tint: [f32; 4],
    /// x = flip_x (0/1), y = flip_y (0/1), z = unused, w = unused
    pub flags: [f32; 4],
}

/// Camera uniforms for the 2D sprite pipeline — just view_proj, no billboard vectors.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Sprite2dUniforms {
    pub view_proj: [[f32; 4]; 4],
}

/// A single 2D sprite batch draw call (one per texture group).
pub struct Sprite2dBatch {
    pub instance_buffer: wgpu::Buffer,
    pub instance_count: u32,
    pub texture_bind_group: wgpu::BindGroup,
    pub instance_bind_group: wgpu::BindGroup,
}

/// The 2D sprite rendering pipeline.
pub struct Sprite2dPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub uniform_bind_group_layout: wgpu::BindGroupLayout,
    pub instance_bind_group_layout: wgpu::BindGroupLayout,
    pub texture_bind_group_layout: wgpu::BindGroupLayout,
    pub quad_index_buffer: wgpu::Buffer,
    pub uniform_buffer: wgpu::Buffer,
    pub uniform_bind_group: wgpu::BindGroup,
}

impl Sprite2dPipeline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, sample_count: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Sprite2D Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sprite2d_shader.wgsl").into()),
        });

        // Group 0: Sprite2dUniforms (view_proj)
        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
                label: Some("Sprite2D Uniform Bind Group Layout"),
            });

        // Group 1: Instance storage buffer (read-only)
        let instance_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
                label: Some("Sprite2D Instance Bind Group Layout"),
            });

        // Group 2: Texture + sampler
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
                label: Some("Sprite2D Texture Bind Group Layout"),
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Sprite2D Pipeline Layout"),
            bind_group_layouts: &[
                &uniform_bind_group_layout,
                &instance_bind_group_layout,
                &texture_bind_group_layout,
            ],
            push_constant_ranges: &[],
        });

        // Alpha blending, depth test enabled but depth write DISABLED
        // (2D sprites rely on layer-based draw order, not depth buffer writes)
        let depth_stencil = wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Sprite2D Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sprite2d"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_sprite2d"),
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
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(depth_stencil),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });

        // Shared quad index buffer (same as particle pipeline)
        let quad_indices: [u32; 6] = [0, 1, 2, 2, 1, 3];
        let quad_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sprite2D Quad Index Buffer"),
            contents: bytemuck::cast_slice(&quad_indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // Persistent uniform buffer for view_proj
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sprite2D Uniform Buffer"),
            contents: bytemuck::cast_slice(&[Sprite2dUniforms {
                view_proj: [[0.0; 4]; 4],
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
            label: Some("Sprite2D Uniform Bind Group"),
        });

        Self {
            pipeline,
            uniform_bind_group_layout,
            instance_bind_group_layout,
            texture_bind_group_layout,
            quad_index_buffer,
            uniform_buffer,
            uniform_bind_group,
        }
    }
}
