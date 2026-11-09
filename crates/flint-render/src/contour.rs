//! The contour pass (#214): ink lines, found in screen space, that can boil.
//!
//! One full-screen draw between the Kuwahara filter and the composite, reading
//! depth and whatever the scene colour is by then. So its cost doesn't grow with
//! the scene, and nothing is allocated, compiled or run until a scene turns it on.

use bytemuck::{Pod, Zeroable};

use crate::postprocess::{fullscreen_pass, PostProcessConfig, PostProcessResources, HDR_FORMAT};

/// Uniform data for the contour pass. Matches `contour_shader.wgsl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ContourUniforms {
    pub texel_size: [f32; 2],
    pub width: f32,
    pub threshold: f32,
    pub color: [f32; 3],
    pub strength: f32,
    pub near: f32,
    pub far: f32,
    pub luma: f32,
    pub boil: f32,
    pub seed: u32,
    pub fade: f32,
    pub _pad: [u32; 2],
}

impl ContourUniforms {
    /// This frame's uniforms for a target of the given size. `width` is authored
    /// at 1080p and scaled, so a line is the same share of the frame at any size.
    pub fn new(config: &PostProcessConfig, width: u32, height: u32, near: f32, far: f32) -> Self {
        Self {
            texel_size: [1.0 / width.max(1) as f32, 1.0 / height.max(1) as f32],
            // Depth is read by the nearest texel, so a reach under a pixel sees nothing.
            width: (config.contour_width * height as f32 / 1080.0).max(0.75),
            threshold: config.contour_threshold.max(1e-4),
            color: config.contour_color,
            strength: config.contour_strength,
            near,
            far,
            luma: config.contour_luma.max(0.0),
            boil: config.contour_boil.max(0.0) * config.boil.amount,
            seed: config.boil.seed,
            fade: config.contour_fade.max(0.0),
            _pad: [0; 2],
        }
    }
}

/// The pass's output, the size of the HDR buffer. Made when a scene turns it on.
pub struct ContourTextures {
    pub output_view: wgpu::TextureView,
    _output: wgpu::Texture,
}

impl ContourTextures {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let output = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Contour Texture"),
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
        let output_view = output.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            output_view,
            _output: output,
        }
    }
}

/// The pass's pipeline. Made when a scene turns it on.
pub struct ContourPipeline {
    pipeline: wgpu::RenderPipeline,
    uniform_bgl: wgpu::BindGroupLayout,
    scene_bgl: wgpu::BindGroupLayout,
    depth_bgl: wgpu::BindGroupLayout,
    uniform_buffer: wgpu::Buffer,
}

impl ContourPipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Contour Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("contour_shader.wgsl").into()),
        });
        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Contour Uniform BGL"),
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
        let sampled = |label, filterable| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(if filterable {
                            wgpu::SamplerBindingType::Filtering
                        } else {
                            wgpu::SamplerBindingType::NonFiltering
                        }),
                        count: None,
                    },
                ],
            })
        };
        let scene_bgl = sampled("Contour Scene BGL", true);
        let depth_bgl = sampled("Contour Depth BGL", false);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Contour Pipeline Layout"),
            bind_group_layouts: &[&uniform_bgl, &scene_bgl, &depth_bgl],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Contour Pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Contour Uniform Buffer"),
            size: std::mem::size_of::<ContourUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            uniform_bgl,
            scene_bgl,
            depth_bgl,
            uniform_buffer,
        }
    }

    /// Ink `scene` into the pass's own texture. Its `id` says which view that
    /// is, for the bind group cache; `depth_key` who vouches for `depth_view`,
    /// so a bind group over it can be kept as long as the view lives.
    #[allow(clippy::too_many_arguments)]
    pub fn run(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        resources: &PostProcessResources,
        config: &PostProcessConfig,
        scene: &crate::stylize::StyleInput<'_>,
        samplers: (&wgpu::Sampler, &wgpu::Sampler),
        depth_view: &wgpu::TextureView,
        depth_key: crate::bind_cache::DepthKey,
        clip: (f32, f32),
    ) {
        let Some(textures) = &resources.contour else {
            return;
        };
        let (linear_sampler, depth_sampler) = samplers;
        let uniforms =
            ContourUniforms::new(config, resources.width, resources.height, clip.0, clip.1);
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

        let uniform_bg = resources.binds.get("contour uniform", 0, || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Contour Uniform BG"),
                layout: &self.uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform_buffer.as_entire_binding(),
                }],
            })
        });
        let sampled = |label: &'static str,
                       layout: &wgpu::BindGroupLayout,
                       view: &wgpu::TextureView,
                       sampler: &wgpu::Sampler| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            })
        };
        let scene_bg = resources.binds.get("contour scene", scene.id, || {
            sampled(
                "Contour Scene BG",
                &self.scene_bgl,
                scene.view,
                linear_sampler,
            )
        });
        let depth_bg = resources.binds.get_over(depth_key, "contour depth", 0, || {
            sampled(
                "Contour Depth BG",
                &self.depth_bgl,
                depth_view,
                depth_sampler,
            )
        });

        fullscreen_pass(
            encoder,
            "Contour Pass",
            &textures.output_view,
            Some(wgpu::Color::BLACK),
            &self.pipeline,
            &[&uniform_bg, &scene_bg, &depth_bg],
        );
    }
}
