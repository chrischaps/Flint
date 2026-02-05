//! Render pipeline for skinned meshes with bone matrix storage buffer

use crate::primitives::SkinnedVertex;

/// Maximum bone count per skeleton (determines storage buffer size)
pub const MAX_BONES: usize = 256;

/// The skinned mesh render pipeline
pub struct SkinnedPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub bone_bind_group_layout: wgpu::BindGroupLayout,
}

impl SkinnedPipeline {
    /// Create the skinned mesh pipeline.
    ///
    /// Uses bind groups 0-2 from the standard pipeline (transform, material, lights)
    /// and adds bind group 3 for bone matrices (storage buffer).
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        transform_bind_group_layout: &wgpu::BindGroupLayout,
        material_bind_group_layout: &wgpu::BindGroupLayout,
        light_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Skinned PBR Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("skinned_shader.wgsl").into()),
        });

        // Bind group 3: Bone matrices storage buffer (vertex shader only)
        let bone_bind_group_layout =
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
                label: Some("Bone Bind Group Layout"),
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Skinned PBR Pipeline Layout"),
            bind_group_layouts: &[
                transform_bind_group_layout,
                material_bind_group_layout,
                light_bind_group_layout,
                &bone_bind_group_layout,
            ],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Skinned PBR Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_skinned"),
                buffers: &[SkinnedVertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_skinned"),
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
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            bone_bind_group_layout,
        }
    }
}
