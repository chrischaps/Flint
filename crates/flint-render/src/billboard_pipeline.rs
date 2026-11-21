//! Billboard sprite render pipeline
//!
//! Renders camera-facing quads for Doom-style sprite entities.
//! Uses a dedicated pipeline separate from PBR to keep things clean.

use crate::gpu_counts::CountedDevice;
use crate::TrustedShader;
use bytemuck::{Pod, Zeroable};

/// Uniform data shared across all billboards in a frame (camera vectors)
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct BillboardUniforms {
    pub view_proj: [[f32; 4]; 4],
    pub camera_right: [f32; 3],
    pub _pad0: f32,
    pub camera_up: [f32; 3],
    pub _pad1: f32,
}

/// Per-sprite instance data
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SpriteInstance {
    pub world_pos: [f32; 3],
    pub width: f32,
    pub height: f32,
    pub frame: u32,
    pub frames_x: u32,
    pub frames_y: u32,
    pub anchor_y: f32,
    pub fullbright: u32,
    pub selection_highlight: u32,
    pub _pad1: f32,
}

/// A billboard draw call with pre-built GPU resources
#[allow(dead_code)]
pub struct BillboardDrawCall {
    pub billboard_buffer: wgpu::Buffer,
    pub sprite_buffer: wgpu::Buffer,
    pub billboard_bind_group: wgpu::BindGroup,
    pub texture_bind_group: wgpu::BindGroup,
    /// A world box the quad lies in whichever way the camera turns it
    /// ([`sprite_bounds`]), for a styled billboard's style scissor (#1011).
    pub world_aabb: ([f32; 3], [f32; 3]),
}

/// A world box that holds a billboard's quad under any camera: the quad's
/// corners are `world_pos + right * x * width + up * (y - anchor_y) * height`
/// with `x` in -0.5..0.5, `y` in 0..1 and `right`, `up` unit and at right
/// angles, so every corner is within the corner reach of `world_pos`.
pub fn sprite_bounds(sprite: &SpriteInstance) -> ([f32; 3], [f32; 3]) {
    let reach_y = sprite.anchor_y.abs().max((1.0 - sprite.anchor_y).abs()) * sprite.height.abs();
    let reach = (0.5 * sprite.width).hypot(reach_y);
    let p = sprite.world_pos;
    (
        [p[0] - reach, p[1] - reach, p[2] - reach],
        [p[0] + reach, p[1] + reach, p[2] + reach],
    )
}

/// The billboard rendering pipeline
pub struct BillboardPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub billboard_bind_group_layout: wgpu::BindGroupLayout,
    pub texture_bind_group_layout: wgpu::BindGroupLayout,
    /// Shared index buffer for unit quad (6 indices: 2 triangles)
    pub quad_index_buffer: wgpu::Buffer,
}

impl BillboardPipeline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, sample_count: u32) -> Self {
        let shader = device.create_trusted_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Billboard Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("billboard_shader.wgsl").into()),
        });

        // Bind group 0: billboard + sprite uniforms
        let billboard_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    // BillboardUniforms
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // SpriteInstance
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
                label: Some("Billboard Bind Group Layout"),
            });

        // Bind group 1: sprite texture + sampler
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
                label: Some("Billboard Texture Bind Group Layout"),
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Billboard Pipeline Layout"),
            bind_group_layouts: &[
                Some(&billboard_bind_group_layout),
                Some(&texture_bind_group_layout),
            ],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline_counted(&wgpu::RenderPipelineDescriptor {
            label: Some("Billboard Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_billboard"),
                buffers: &[], // No vertex buffer — positions generated from vertex_index
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_billboard"),
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
                cull_mode: None, // Billboards are double-sided
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });

        // Shared quad index buffer: 2 triangles = 6 indices
        let quad_indices: [u32; 6] = [0, 1, 2, 2, 1, 3];
        let quad_index_buffer =
            device.create_buffer_init_counted(&wgpu::util::BufferInitDescriptor {
                label: Some("Billboard Quad Index Buffer"),
                contents: bytemuck::cast_slice(&quad_indices),
                usage: wgpu::BufferUsages::INDEX,
            });

        Self {
            pipeline,
            billboard_bind_group_layout,
            texture_bind_group_layout,
            quad_index_buffer,
        }
    }
}
