//! Render pipeline for skinned meshes with bone matrix storage buffer

use crate::primitives::SkinnedVertex;

/// Maximum bone count per skeleton (determines storage buffer size)
pub const MAX_BONES: usize = 256;

/// The skinned mesh render pipeline
pub struct SkinnedPipeline {
    pub pipeline: wgpu::RenderPipeline,
    /// Skinned wireframe lines (LineList, LessEqual + negative bias so the
    /// edges sit on top of the animated surface).
    pub wire_line_pipeline: wgpu::RenderPipeline,
    pub transparent_alpha_pipeline: wgpu::RenderPipeline,
    /// Back-face pass of the alpha pipeline (drawn first).
    pub transparent_alpha_back_pipeline: wgpu::RenderPipeline,
    pub transparent_additive_pipeline: wgpu::RenderPipeline,
    pub transparent_multiply_pipeline: wgpu::RenderPipeline,
    /// Living props' echoes that may only add light (`[living] echo_blend =
    /// "lighten"`): front faces, no depth write, a max blend.
    pub echo_lighten_pipeline: wgpu::RenderPipeline,
    pub bone_bind_group_layout: wgpu::BindGroupLayout,
    /// What shallow wooze's skinned pipelines build on (#215).
    pub(crate) shader: wgpu::ShaderModule,
    pub(crate) pipeline_layout: wgpu::PipelineLayout,
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
        sample_count: u32,
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
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
            multiview: None,
            cache: None,
        });

        // The skinned wireframe's vertex and fragment stages (#602: what is
        // left of the old outline shader)
        let wire_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Skinned Wire Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("skinned_wire_shader.wgsl").into()),
        });

        // Alpha-blended meshes draw in two passes, back faces first (cull front) then
        // front faces (cull back). Depth write stays off, but with no sort inside one
        // mesh a single double-sided pass blends its own back faces over its front
        // faces in index order, which shows as triangular patches on a translucent
        // sphere. Two passes make every convex translucent object order-independent.
        let make_transparent = |label: &str,
                                cull_mode: Option<wgpu::Face>,
                                fragment: &str,
                                blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_skinned"),
                    buffers: &[SkinnedVertex::desc()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(blend),
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
        let alpha = wgpu::BlendState::ALPHA_BLENDING;
        let transparent_alpha_back_pipeline = make_transparent(
            "Skinned Transparent Alpha Pipeline (back faces)",
            Some(wgpu::Face::Front),
            "fs_skinned",
            alpha,
        );
        let transparent_alpha_pipeline = make_transparent(
            "Skinned Transparent Alpha Pipeline (front faces)",
            Some(wgpu::Face::Back),
            "fs_skinned",
            alpha,
        );
        let echo_lighten_pipeline = make_transparent(
            "Skinned Echo Lighten Pipeline",
            Some(wgpu::Face::Back),
            "fs_skinned_lighten",
            crate::pipeline::LIGHTEN_BLEND,
        );

        // Transparent additive pipeline for skinned meshes
        let transparent_additive_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Skinned Transparent Additive Pipeline"),
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
                        blend: Some(crate::pipeline::ADDITIVE_BLEND),
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

        // Transparent multiply pipeline for skinned meshes
        let transparent_multiply_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Skinned Transparent Multiply Pipeline"),
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
                        blend: Some(crate::pipeline::MULTIPLY_BLEND),
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

        // Wireframe line pipeline for skinned meshes — same skinning vertex
        // path as the scene's, drawn as edge lines with a depth bias so
        // they follow the animated pose and sit on top of the solid surface.
        let wire_line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Skinned Wireframe Line Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &wire_shader,
                entry_point: Some("vs_skinned_wire"),
                buffers: &[SkinnedVertex::desc()],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &wire_shader,
                entry_point: Some("fs_wire"),
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
                bias: crate::pipeline::line_depth_bias(),
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
            wire_line_pipeline,
            transparent_alpha_pipeline,
            transparent_alpha_back_pipeline,
            transparent_additive_pipeline,
            transparent_multiply_pipeline,
            echo_lighten_pipeline,
            bone_bind_group_layout,
            shader,
            pipeline_layout,
        }
    }
}
