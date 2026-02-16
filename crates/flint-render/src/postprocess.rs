//! Post-processing pipeline: HDR buffer, bloom, composite tonemapping
//!
//! Renders the scene to an Rgba16Float intermediate buffer, optionally
//! applies a bloom downsample/upsample chain, then composites to the
//! sRGB surface with exposure, ACES tonemapping, gamma, and vignette.

use bytemuck::{Pod, Zeroable};

/// Maximum number of bloom mip levels in the downsample/upsample chain.
pub const MAX_BLOOM_MIPS: usize = 5;

/// HDR texture format used for the intermediate scene buffer and bloom chain.
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// Post-processing configuration (runtime-adjustable parameters).
#[derive(Debug, Clone)]
pub struct PostProcessConfig {
    pub enabled: bool,
    pub exposure: f32,
    pub bloom_enabled: bool,
    pub bloom_intensity: f32,
    pub bloom_threshold: f32,
    pub bloom_soft_threshold: f32,
    pub vignette_enabled: bool,
    pub vignette_intensity: f32,
    pub vignette_smoothness: f32,
}

impl Default for PostProcessConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            exposure: 1.0,
            bloom_enabled: true,
            bloom_intensity: 0.04,
            bloom_threshold: 1.0,
            bloom_soft_threshold: 0.5,
            vignette_enabled: false,
            vignette_intensity: 0.3,
            vignette_smoothness: 2.0,
        }
    }
}

/// Uniform data for the composite fullscreen pass.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct PostProcessUniforms {
    pub exposure: f32,
    pub bloom_intensity: f32,
    pub bloom_threshold: f32,
    pub bloom_soft_threshold: f32,
    pub vignette_intensity: f32,
    pub vignette_smoothness: f32,
    pub texel_size: [f32; 2],
}

/// Uniform data for bloom passes (threshold/downsample/upsample).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct BloomUniforms {
    pub texel_size: [f32; 2],
    pub threshold: f32,
    pub soft_threshold: f32,
}

/// GPU resources for the HDR buffer and bloom mip chain.
/// Recreated on resize.
pub struct PostProcessResources {
    pub hdr_texture: wgpu::Texture,
    pub hdr_view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    // Bloom mip chain (each level is half the size of the previous)
    pub bloom_mips: Vec<BloomMip>,
    pub bloom_mip_count: usize,
}

/// A single level in the bloom mip chain.
pub struct BloomMip {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

/// All render pipelines and bind group layouts for post-processing.
pub struct PostProcessPipeline {
    // Composite (HDR + bloom → tonemapped sRGB surface)
    pub composite_pipeline: wgpu::RenderPipeline,
    pub composite_uniform_bgl: wgpu::BindGroupLayout,
    pub composite_scene_bgl: wgpu::BindGroupLayout,
    pub composite_bloom_bgl: wgpu::BindGroupLayout,
    pub composite_uniform_buffer: wgpu::Buffer,
    // The sampler shared across composite and bloom passes
    pub linear_sampler: wgpu::Sampler,
    // Bloom pipelines
    pub bloom_threshold_pipeline: wgpu::RenderPipeline,
    pub bloom_downsample_pipeline: wgpu::RenderPipeline,
    pub bloom_upsample_pipeline: wgpu::RenderPipeline,
    pub bloom_uniform_bgl: wgpu::BindGroupLayout,
    pub bloom_texture_bgl: wgpu::BindGroupLayout,
    pub bloom_uniform_buffer: wgpu::Buffer,
    // A 1x1 black texture used when bloom is disabled
    pub black_texture_view: wgpu::TextureView,
}

impl PostProcessPipeline {
    /// Create all post-processing pipelines and shared resources.
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let linear_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("PostProcess Linear Sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        // --- Composite pipeline ---
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Composite Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite_shader.wgsl").into()),
        });

        // Group 0: PostProcessUniforms
        let composite_uniform_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Composite Uniform BGL"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        // Group 1: HDR scene texture + sampler
        let composite_scene_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Composite Scene BGL"),
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
            });

        // Group 2: Bloom texture + sampler
        let composite_bloom_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Composite Bloom BGL"),
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
            });

        let composite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Composite Pipeline Layout"),
                bind_group_layouts: &[
                    &composite_uniform_bgl,
                    &composite_scene_bgl,
                    &composite_bloom_bgl,
                ],
                push_constant_ranges: &[],
            });

        let composite_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Composite Pipeline"),
                layout: Some(&composite_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &composite_shader,
                    entry_point: Some("vs_composite"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &composite_shader,
                    entry_point: Some("fs_composite"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: surface_format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        let composite_uniform_buffer =
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("PostProcess Uniform Buffer"),
                size: std::mem::size_of::<PostProcessUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

        // --- Bloom pipelines ---
        let bloom_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Bloom Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("bloom_shader.wgsl").into()),
        });

        // Bloom group 0: BloomUniforms
        let bloom_uniform_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Bloom Uniform BGL"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        // Bloom group 1: source texture + sampler
        let bloom_texture_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Bloom Texture BGL"),
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
            });

        let bloom_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Bloom Pipeline Layout"),
                bind_group_layouts: &[&bloom_uniform_bgl, &bloom_texture_bgl],
                push_constant_ranges: &[],
            });

        let bloom_threshold_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Bloom Threshold Pipeline"),
                layout: Some(&bloom_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &bloom_shader,
                    entry_point: Some("vs_bloom"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &bloom_shader,
                    entry_point: Some("fs_bloom_threshold"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        let bloom_downsample_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Bloom Downsample Pipeline"),
                layout: Some(&bloom_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &bloom_shader,
                    entry_point: Some("vs_bloom"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &bloom_shader,
                    entry_point: Some("fs_downsample"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        // Upsample uses additive blending: src + dst
        let bloom_upsample_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Bloom Upsample Pipeline"),
                layout: Some(&bloom_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &bloom_shader,
                    entry_point: Some("vs_bloom"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &bloom_shader,
                    entry_point: Some("fs_upsample"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
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
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });

        let bloom_uniform_buffer =
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Bloom Uniform Buffer"),
                size: std::mem::size_of::<BloomUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });

        // 1x1 black texture for when bloom is disabled
        let black_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Black Texture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let black_texture_view =
            black_texture.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            composite_pipeline,
            composite_uniform_bgl,
            composite_scene_bgl,
            composite_bloom_bgl,
            composite_uniform_buffer,
            linear_sampler,
            bloom_threshold_pipeline,
            bloom_downsample_pipeline,
            bloom_upsample_pipeline,
            bloom_uniform_bgl,
            bloom_texture_bgl,
            bloom_uniform_buffer,
            black_texture_view,
        }
    }

    /// Run the bloom downsample/upsample chain.
    /// Reads from the HDR scene texture and writes to bloom mip chain.
    pub fn run_bloom(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resources: &PostProcessResources,
        config: &PostProcessConfig,
    ) {
        if resources.bloom_mip_count == 0 {
            return;
        }

        // Step 1: Threshold — extract bright pixels from HDR into mip[0]
        {
            let bloom_uniforms = BloomUniforms {
                texel_size: [
                    1.0 / resources.width as f32,
                    1.0 / resources.height as f32,
                ],
                threshold: config.bloom_threshold,
                soft_threshold: config.bloom_soft_threshold,
            };
            queue.write_buffer(
                &self.bloom_uniform_buffer,
                0,
                bytemuck::cast_slice(&[bloom_uniforms]),
            );

            let hdr_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Bloom Threshold Source BG"),
                layout: &self.bloom_texture_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&resources.hdr_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                    },
                ],
            });

            let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Bloom Threshold Uniform BG"),
                layout: &self.bloom_uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.bloom_uniform_buffer.as_entire_binding(),
                }],
            });

            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Bloom Threshold Encoder"),
                });

            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Bloom Threshold Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &resources.bloom_mips[0].view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

                pass.set_pipeline(&self.bloom_threshold_pipeline);
                pass.set_bind_group(0, &uniform_bg, &[]);
                pass.set_bind_group(1, &hdr_bind_group, &[]);
                pass.draw(0..3, 0..1);
            }

            queue.submit(std::iter::once(encoder.finish()));
        }

        // Step 2: Progressive downsample mip[0] → mip[1] → ... → mip[N-1]
        for i in 1..resources.bloom_mip_count {
            let src = &resources.bloom_mips[i - 1];
            let dst = &resources.bloom_mips[i];

            let bloom_uniforms = BloomUniforms {
                texel_size: [1.0 / src.width as f32, 1.0 / src.height as f32],
                threshold: 0.0,
                soft_threshold: 0.0,
            };
            queue.write_buffer(
                &self.bloom_uniform_buffer,
                0,
                bytemuck::cast_slice(&[bloom_uniforms]),
            );

            let src_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Bloom Downsample Source BG"),
                layout: &self.bloom_texture_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&src.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                    },
                ],
            });

            let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Bloom Downsample Uniform BG"),
                layout: &self.bloom_uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.bloom_uniform_buffer.as_entire_binding(),
                }],
            });

            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some(&format!("Bloom Downsample {} Encoder", i)),
                });

            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(&format!("Bloom Downsample {} Pass", i)),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &dst.view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

                pass.set_pipeline(&self.bloom_downsample_pipeline);
                pass.set_bind_group(0, &uniform_bg, &[]);
                pass.set_bind_group(1, &src_bg, &[]);
                pass.draw(0..3, 0..1);
            }

            queue.submit(std::iter::once(encoder.finish()));
        }

        // Step 3: Progressive upsample mip[N-1] → mip[N-2] → ... → mip[0]
        // Additive blending accumulates the bloom result
        for i in (0..resources.bloom_mip_count - 1).rev() {
            let src = &resources.bloom_mips[i + 1];
            let dst = &resources.bloom_mips[i];

            let bloom_uniforms = BloomUniforms {
                texel_size: [1.0 / src.width as f32, 1.0 / src.height as f32],
                threshold: 0.0,
                soft_threshold: 0.0,
            };
            queue.write_buffer(
                &self.bloom_uniform_buffer,
                0,
                bytemuck::cast_slice(&[bloom_uniforms]),
            );

            let src_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Bloom Upsample Source BG"),
                layout: &self.bloom_texture_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&src.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                    },
                ],
            });

            let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Bloom Upsample Uniform BG"),
                layout: &self.bloom_uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.bloom_uniform_buffer.as_entire_binding(),
                }],
            });

            let mut encoder =
                device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some(&format!("Bloom Upsample {} Encoder", i)),
                });

            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(&format!("Bloom Upsample {} Pass", i)),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &dst.view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });

                pass.set_pipeline(&self.bloom_upsample_pipeline);
                pass.set_bind_group(0, &uniform_bg, &[]);
                pass.set_bind_group(1, &src_bg, &[]);
                pass.draw(0..3, 0..1);
            }

            queue.submit(std::iter::once(encoder.finish()));
        }
    }

    /// Run the composite pass: combine HDR scene + bloom → tonemapped sRGB surface.
    pub fn composite(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resources: &PostProcessResources,
        config: &PostProcessConfig,
        target_view: &wgpu::TextureView,
    ) {
        let effects_on = config.enabled;
        let uniforms = PostProcessUniforms {
            exposure: config.exposure,
            bloom_intensity: if effects_on && config.bloom_enabled {
                config.bloom_intensity
            } else {
                0.0
            },
            bloom_threshold: config.bloom_threshold,
            bloom_soft_threshold: config.bloom_soft_threshold,
            vignette_intensity: if effects_on && config.vignette_enabled {
                config.vignette_intensity
            } else {
                0.0
            },
            vignette_smoothness: config.vignette_smoothness,
            texel_size: [
                1.0 / resources.width as f32,
                1.0 / resources.height as f32,
            ],
        };

        queue.write_buffer(
            &self.composite_uniform_buffer,
            0,
            bytemuck::cast_slice(&[uniforms]),
        );

        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Composite Uniform BG"),
            layout: &self.composite_uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.composite_uniform_buffer.as_entire_binding(),
            }],
        });

        let scene_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Composite Scene BG"),
            layout: &self.composite_scene_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&resources.hdr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                },
            ],
        });

        // Use bloom mip[0] if available and bloom enabled, otherwise use black texture
        let bloom_view = if config.bloom_enabled && !resources.bloom_mips.is_empty() {
            &resources.bloom_mips[0].view
        } else {
            &self.black_texture_view
        };

        let bloom_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Composite Bloom BG"),
            layout: &self.composite_bloom_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(bloom_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                },
            ],
        });

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Composite Encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Composite Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(&self.composite_pipeline);
            pass.set_bind_group(0, &uniform_bg, &[]);
            pass.set_bind_group(1, &scene_bg, &[]);
            pass.set_bind_group(2, &bloom_bg, &[]);
            pass.draw(0..3, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));
    }
}

impl PostProcessResources {
    /// Create HDR buffer and bloom mip chain for the given dimensions.
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let hdr_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("HDR Scene Texture"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let hdr_view = hdr_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Compute bloom mip count: floor(log2(min(w, h))).min(MAX_BLOOM_MIPS)
        // Minimum mip size is 8x8
        let min_dim = width.min(height).max(1);
        let max_mips = (min_dim as f32).log2().floor() as usize;
        // Subtract 3 so the smallest mip is at least 8x8 (2^3 = 8)
        let bloom_mip_count = max_mips.saturating_sub(3).min(MAX_BLOOM_MIPS);

        let mut bloom_mips = Vec::with_capacity(bloom_mip_count);
        let mut mip_w = (width / 2).max(1);
        let mut mip_h = (height / 2).max(1);

        for i in 0..bloom_mip_count {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(&format!("Bloom Mip {}", i)),
                size: wgpu::Extent3d {
                    width: mip_w,
                    height: mip_h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: HDR_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

            bloom_mips.push(BloomMip {
                texture,
                view,
                width: mip_w,
                height: mip_h,
            });

            mip_w = (mip_w / 2).max(1);
            mip_h = (mip_h / 2).max(1);
        }

        Self {
            hdr_texture,
            hdr_view,
            width,
            height,
            bloom_mips,
            bloom_mip_count,
        }
    }
}
