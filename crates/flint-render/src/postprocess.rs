//! Post-processing pipeline: HDR buffer, bloom, composite tonemapping
//!
//! Renders the scene to an Rgba16Float intermediate buffer, optionally
//! applies a bloom downsample/upsample chain, then composites to the
//! sRGB surface with exposure, ACES tonemapping, gamma, and vignette.

use crate::camera::Camera;
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
    pub chromatic_aberration: f32,
    pub radial_blur: f32,
    pub desaturate: f32,
    pub ssao_enabled: bool,
    pub ssao_radius: f32,
    pub ssao_intensity: f32,
    pub ssao_bias: f32,
    pub fog_enabled: bool,
    pub fog_color: [f32; 3],
    pub fog_density: f32,
    pub fog_start: f32,
    pub fog_end: f32,
    pub fog_height_enabled: bool,
    pub fog_height_falloff: f32,
    pub fog_height_origin: f32,
    pub dither_enabled: bool,
    pub dither_intensity: f32,
    pub volumetric_enabled: bool,
    pub volumetric_samples: u32,
    pub volumetric_density: f32,
    pub volumetric_max_distance: f32,
    pub volumetric_decay: f32,
    pub kuwahara_enabled: bool,
    pub kuwahara_radius: u32,
    pub kuwahara_sharpness: f32,
    pub kuwahara_hardness: f32,
    pub kuwahara_anisotropy: f32,
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
            chromatic_aberration: 0.0,
            radial_blur: 0.0,
            desaturate: 0.0,
            ssao_enabled: true,
            ssao_radius: 0.5,
            ssao_intensity: 1.0,
            ssao_bias: 0.025,
            fog_enabled: false,
            fog_color: [0.7, 0.75, 0.82],
            fog_density: 0.02,
            fog_start: 5.0,
            fog_end: 100.0,
            fog_height_enabled: false,
            fog_height_falloff: 0.1,
            fog_height_origin: 0.0,
            dither_enabled: false,
            dither_intensity: 0.03,
            volumetric_enabled: false,
            volumetric_samples: 32,
            volumetric_density: 1.0,
            volumetric_max_distance: 100.0,
            volumetric_decay: 0.98,
            kuwahara_enabled: false,
            kuwahara_radius: 4,
            kuwahara_sharpness: 8.0,
            kuwahara_hardness: 8.0,
            kuwahara_anisotropy: 1.0,
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
    pub chromatic_aberration: f32,
    pub radial_blur: f32,
    pub desaturate: f32,
    pub _pad: f32,
    // Fog parameters
    pub fog_color: [f32; 3],
    pub fog_density: f32,
    pub fog_start: f32,
    pub fog_end: f32,
    pub fog_height_falloff: f32,
    pub fog_height_origin: f32,
    pub camera_pos: [f32; 3],
    pub fog_enabled: f32,
    pub near: f32,
    pub far: f32,
    pub fog_height_enabled: f32,
    pub dither_intensity: f32,
    pub inv_view_proj: [[f32; 4]; 4],
}

/// Uniform data for bloom passes (threshold/downsample/upsample).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct BloomUniforms {
    pub texel_size: [f32; 2],
    pub threshold: f32,
    pub soft_threshold: f32,
}

/// Number of hemisphere samples for SSAO kernel.
pub const SSAO_KERNEL_SIZE: usize = 64;

/// Uniform data for the volumetric ray-march pass.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct VolumetricUniforms {
    pub inv_view_proj: [[f32; 4]; 4],
    pub camera_pos: [f32; 3],
    pub num_samples: f32,
    pub density: f32,
    pub max_distance: f32,
    pub decay: f32,
    pub near: f32,
    pub far: f32,
    pub _pad: [f32; 3],
}

/// Uniform data for the volumetric bilateral blur pass.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct VolumetricBlurUniforms {
    pub texel_size: [f32; 2],
    pub depth_threshold: f32,
    pub _pad: f32,
}

/// Uniform data for the SSAO generation pass.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SsaoUniforms {
    pub inv_projection: [[f32; 4]; 4],
    pub projection: [[f32; 4]; 4],
    pub kernel: [[f32; 4]; SSAO_KERNEL_SIZE],
    pub noise_scale: [f32; 2],
    pub radius: f32,
    pub bias: f32,
    pub intensity: f32,
    pub near: f32,
    pub far: f32,
    pub _pad: f32,
}

/// Uniform data for the SSAO blur pass.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SsaoBlurUniforms {
    pub texel_size: [f32; 2],
    pub _pad: [f32; 2],
}

/// Uniform data for the structure tensor pass.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct KuwaharaTensorUniforms {
    pub texel_size: [f32; 2],
    pub _pad: [f32; 2],
}

/// Uniform data for the structure tensor blur pass.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct KuwaharaTensorBlurUniforms {
    pub texel_size: [f32; 2],
    pub _pad: [f32; 2],
}

/// Uniform data for the anisotropic Kuwahara filter pass.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct KuwaharaUniforms {
    pub texel_size: [f32; 2],
    pub radius: f32,
    pub sharpness: f32,
    pub hardness: f32,
    pub anisotropy: f32,
    pub _pad: [f32; 2],
}

/// SSAO texture format (single-channel occlusion).
const SSAO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;

/// GPU resources for the Kuwahara filter (created on demand).
pub struct KuwaharaTextures {
    pub tensor_texture: wgpu::Texture,
    pub tensor_view: wgpu::TextureView,
    pub tensor_blur_texture: wgpu::Texture,
    pub tensor_blur_view: wgpu::TextureView,
    pub output_texture: wgpu::Texture,
    pub output_view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

/// Pipelines and bind group layouts for the Kuwahara filter (created on demand).
pub struct KuwaharaPipelines {
    pub tensor_pipeline: wgpu::RenderPipeline,
    pub tensor_uniform_bgl: wgpu::BindGroupLayout,
    pub tensor_texture_bgl: wgpu::BindGroupLayout,
    pub tensor_uniform_buffer: wgpu::Buffer,
    pub tensor_blur_pipeline: wgpu::RenderPipeline,
    pub tensor_blur_uniform_bgl: wgpu::BindGroupLayout,
    pub tensor_blur_texture_bgl: wgpu::BindGroupLayout,
    pub tensor_blur_uniform_buffer: wgpu::Buffer,
    pub pipeline: wgpu::RenderPipeline,
    pub uniform_bgl: wgpu::BindGroupLayout,
    pub hdr_bgl: wgpu::BindGroupLayout,
    pub tensor_input_bgl: wgpu::BindGroupLayout,
    pub uniform_buffer: wgpu::Buffer,
}

impl KuwaharaTextures {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let tensor_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Kuwahara Tensor Texture"),
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
        let tensor_view = tensor_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let tensor_blur_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Kuwahara Tensor Blur Texture"),
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
        let tensor_blur_view =
            tensor_blur_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let output_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Kuwahara Texture"),
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
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

        Self {
            tensor_texture,
            tensor_view,
            tensor_blur_texture,
            tensor_blur_view,
            output_texture,
            output_view,
            width: width.max(1),
            height: height.max(1),
        }
    }
}

impl KuwaharaPipelines {
    /// Create Kuwahara pipelines. Panics if the GPU driver cannot compile
    /// the shaders — callers should use catch_unwind.
    pub fn new(device: &wgpu::Device) -> Self {
        let kuwahara_tensor_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Kuwahara Tensor Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("kuwahara_tensor_shader.wgsl").into()),
        });

        let tensor_uniform_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Kuwahara Tensor Uniform BGL"),
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

        let tensor_texture_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Kuwahara Tensor Texture BGL"),
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

        let tensor_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Kuwahara Tensor Pipeline Layout"),
                bind_group_layouts: &[&tensor_uniform_bgl, &tensor_texture_bgl],
                push_constant_ranges: &[],
            });

        let tensor_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Kuwahara Tensor Pipeline"),
            layout: Some(&tensor_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &kuwahara_tensor_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &kuwahara_tensor_shader,
                entry_point: Some("fs_main"),
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

        let tensor_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Kuwahara Tensor Uniform Buffer"),
            size: std::mem::size_of::<KuwaharaTensorUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let kuwahara_tensor_blur_shader =
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Kuwahara Tensor Blur Shader"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("kuwahara_tensor_blur_shader.wgsl").into(),
                ),
            });

        let tensor_blur_uniform_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Kuwahara Tensor Blur Uniform BGL"),
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

        let tensor_blur_texture_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Kuwahara Tensor Blur Texture BGL"),
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

        let tensor_blur_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Kuwahara Tensor Blur Pipeline Layout"),
                bind_group_layouts: &[&tensor_blur_uniform_bgl, &tensor_blur_texture_bgl],
                push_constant_ranges: &[],
            });

        let tensor_blur_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Kuwahara Tensor Blur Pipeline"),
            layout: Some(&tensor_blur_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &kuwahara_tensor_blur_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &kuwahara_tensor_blur_shader,
                entry_point: Some("fs_main"),
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

        let tensor_blur_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Kuwahara Tensor Blur Uniform Buffer"),
            size: std::mem::size_of::<KuwaharaTensorBlurUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let kuwahara_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Kuwahara Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("kuwahara_shader.wgsl").into()),
        });

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Kuwahara Uniform BGL"),
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

        let hdr_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Kuwahara HDR BGL"),
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

        let tensor_input_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Kuwahara Tensor Input BGL"),
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

        let kuwahara_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Kuwahara Pipeline Layout"),
                bind_group_layouts: &[&uniform_bgl, &hdr_bgl, &tensor_input_bgl],
                push_constant_ranges: &[],
            });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Kuwahara Pipeline"),
            layout: Some(&kuwahara_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &kuwahara_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &kuwahara_shader,
                entry_point: Some("fs_main"),
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

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Kuwahara Uniform Buffer"),
            size: std::mem::size_of::<KuwaharaUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            tensor_pipeline,
            tensor_uniform_bgl,
            tensor_texture_bgl,
            tensor_uniform_buffer,
            tensor_blur_pipeline,
            tensor_blur_uniform_bgl,
            tensor_blur_texture_bgl,
            tensor_blur_uniform_buffer,
            pipeline,
            uniform_bgl,
            hdr_bgl,
            tensor_input_bgl,
            uniform_buffer,
        }
    }
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
    // SSAO textures (full resolution, R8Unorm)
    pub ssao_texture: wgpu::Texture,
    pub ssao_view: wgpu::TextureView,
    pub ssao_blur_texture: wgpu::Texture,
    pub ssao_blur_view: wgpu::TextureView,
    // Volumetric textures (half resolution, Rgba16Float)
    pub volumetric_texture: wgpu::Texture,
    pub volumetric_view: wgpu::TextureView,
    pub volumetric_blur_texture: wgpu::Texture,
    pub volumetric_blur_view: wgpu::TextureView,
    // Kuwahara textures (full resolution, Rgba16Float) — only allocated when enabled
    pub kuwahara: Option<KuwaharaTextures>,
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
    // SSAO pipelines and resources
    pub ssao_pipeline: wgpu::RenderPipeline,
    pub ssao_uniform_bgl: wgpu::BindGroupLayout,
    pub ssao_depth_bgl: wgpu::BindGroupLayout,
    pub ssao_noise_bgl: wgpu::BindGroupLayout,
    pub ssao_uniform_buffer: wgpu::Buffer,
    pub ssao_noise_texture_view: wgpu::TextureView,
    pub ssao_noise_sampler: wgpu::Sampler,
    pub depth_sampler: wgpu::Sampler,
    pub ssao_blur_pipeline: wgpu::RenderPipeline,
    pub ssao_blur_uniform_bgl: wgpu::BindGroupLayout,
    pub ssao_blur_texture_bgl: wgpu::BindGroupLayout,
    pub ssao_blur_uniform_buffer: wgpu::Buffer,
    // Composite group 3: SSAO texture + depth texture (for fog) + volumetric texture
    pub composite_ssao_bgl: wgpu::BindGroupLayout,
    // 1x1 white R8Unorm fallback when SSAO disabled
    pub white_texture_view: wgpu::TextureView,
    // Cached hemisphere kernel (generated once)
    pub ssao_kernel: [[f32; 4]; SSAO_KERNEL_SIZE],
    // Volumetric (god rays) pipelines and resources
    pub volumetric_pipeline: wgpu::RenderPipeline,
    pub volumetric_uniform_bgl: wgpu::BindGroupLayout,
    pub volumetric_depth_bgl: wgpu::BindGroupLayout,
    pub volumetric_shadow_bgl: wgpu::BindGroupLayout,
    pub volumetric_uniform_buffer: wgpu::Buffer,
    pub volumetric_blur_pipeline: wgpu::RenderPipeline,
    pub volumetric_blur_uniform_bgl: wgpu::BindGroupLayout,
    pub volumetric_blur_texture_bgl: wgpu::BindGroupLayout,
    pub volumetric_blur_uniform_buffer: wgpu::Buffer,
    // 1x1 black HDR fallback when volumetric is disabled
    pub volumetric_black_view: wgpu::TextureView,
    // Kuwahara filter pipelines and resources — only allocated when enabled
    pub kuwahara: Option<KuwaharaPipelines>,
}

impl PostProcessPipeline {
    /// Create all post-processing pipelines and shared resources.
    /// Kuwahara pipelines are only created when `kuwahara_enabled` is true
    /// to avoid exhausting VRAM on constrained GPUs.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        surface_format: wgpu::TextureFormat,
        kuwahara_enabled: bool,
    ) -> Self {
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

        // Group 3: SSAO texture + sampler + depth texture + depth sampler (for fog) + volumetric texture + sampler
        let composite_ssao_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Composite SSAO+Depth+Volumetric BGL"),
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
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 5,
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
                    &composite_ssao_bgl,
                ],
                push_constant_ranges: &[],
            });

        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
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

        let composite_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
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
        let bloom_uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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
        let bloom_texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
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

        let bloom_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
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
        let black_texture_view = black_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // --- SSAO pipelines ---
        let ssao_kernel = generate_ssao_kernel();

        // Non-filtering sampler for Depth32Float (cannot use linear filtering on depth)
        let depth_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("SSAO Depth Sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        // Noise sampler with Repeat addressing (tiles the 4x4 noise across the screen)
        let ssao_noise_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("SSAO Noise Sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });

        // Generate and upload 4x4 noise texture (random rotation vectors)
        let noise_data = generate_ssao_noise();
        let ssao_noise_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("SSAO Noise Texture"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
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
                texture: &ssao_noise_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &noise_data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * 4), // 4 pixels * 4 bytes (RGBA)
                rows_per_image: Some(4),
            },
            wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
        );
        let ssao_noise_texture_view =
            ssao_noise_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // 1x1 white R8Unorm fallback for when SSAO is disabled
        let white_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("SSAO White Texture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SSAO_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &white_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[255u8],
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(1),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let white_texture_view = white_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // SSAO shader
        let ssao_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SSAO Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ssao_shader.wgsl").into()),
        });

        // SSAO Group 0: SsaoUniforms
        let ssao_uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SSAO Uniform BGL"),
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

        // SSAO Group 1: depth texture (non-filterable) + non-filtering sampler
        let ssao_depth_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SSAO Depth BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });

        // SSAO Group 2: noise texture (filterable) + repeat sampler
        let ssao_noise_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SSAO Noise BGL"),
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

        let ssao_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("SSAO Pipeline Layout"),
            bind_group_layouts: &[&ssao_uniform_bgl, &ssao_depth_bgl, &ssao_noise_bgl],
            push_constant_ranges: &[],
        });

        let ssao_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("SSAO Pipeline"),
            layout: Some(&ssao_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &ssao_shader,
                entry_point: Some("vs_ssao"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &ssao_shader,
                entry_point: Some("fs_ssao"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: SSAO_FORMAT,
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

        let ssao_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SSAO Uniform Buffer"),
            size: std::mem::size_of::<SsaoUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // SSAO blur shader
        let ssao_blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("SSAO Blur Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ssao_blur_shader.wgsl").into()),
        });

        // Blur Group 0: SsaoBlurUniforms
        let ssao_blur_uniform_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("SSAO Blur Uniform BGL"),
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

        // Blur Group 1: ssao texture + linear sampler
        let ssao_blur_texture_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("SSAO Blur Texture BGL"),
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

        let ssao_blur_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("SSAO Blur Pipeline Layout"),
                bind_group_layouts: &[&ssao_blur_uniform_bgl, &ssao_blur_texture_bgl],
                push_constant_ranges: &[],
            });

        let ssao_blur_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("SSAO Blur Pipeline"),
            layout: Some(&ssao_blur_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &ssao_blur_shader,
                entry_point: Some("vs_ssao_blur"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &ssao_blur_shader,
                entry_point: Some("fs_ssao_blur"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: SSAO_FORMAT,
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

        let ssao_blur_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("SSAO Blur Uniform Buffer"),
            size: std::mem::size_of::<SsaoBlurUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // --- Volumetric (god rays) pipelines ---
        let volumetric_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Volumetric Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("volumetric_shader.wgsl").into()),
        });

        // Volumetric Group 0: VolumetricUniforms
        let volumetric_uniform_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Volumetric Uniform BGL"),
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
            });

        // Volumetric Group 1: depth texture (non-filterable) + non-filtering sampler
        let volumetric_depth_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Volumetric Depth BGL"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                        count: None,
                    },
                ],
            });

        // Volumetric Group 2: shadow maps + comparison sampler + ShadowUniforms + LightUniforms
        let volumetric_shadow_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Volumetric Shadow BGL"),
                entries: &[
                    // binding 0: shadow_maps (depth texture array)
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Depth,
                            view_dimension: wgpu::TextureViewDimension::D2Array,
                            multisampled: false,
                        },
                        count: None,
                    },
                    // binding 1: shadow comparison sampler
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                        count: None,
                    },
                    // binding 2: ShadowUniforms (cascade matrices + splits)
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    // binding 3: LightUniforms
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
                ],
            });

        let volumetric_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Volumetric Pipeline Layout"),
                bind_group_layouts: &[
                    &volumetric_uniform_bgl,
                    &volumetric_depth_bgl,
                    &volumetric_shadow_bgl,
                ],
                push_constant_ranges: &[],
            });

        let volumetric_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Volumetric Pipeline"),
            layout: Some(&volumetric_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &volumetric_shader,
                entry_point: Some("vs_volumetric"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &volumetric_shader,
                entry_point: Some("fs_volumetric"),
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

        let volumetric_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Volumetric Uniform Buffer"),
            size: std::mem::size_of::<VolumetricUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Volumetric blur shader
        let volumetric_blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Volumetric Blur Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("volumetric_blur_shader.wgsl").into()),
        });

        // Volumetric blur Group 0: VolumetricBlurUniforms
        let volumetric_blur_uniform_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Volumetric Blur Uniform BGL"),
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

        // Volumetric blur Group 1: source texture + sampler + depth texture + depth sampler
        let volumetric_blur_texture_bgl =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Volumetric Blur Texture BGL"),
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
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                        count: None,
                    },
                ],
            });

        let volumetric_blur_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Volumetric Blur Pipeline Layout"),
                bind_group_layouts: &[&volumetric_blur_uniform_bgl, &volumetric_blur_texture_bgl],
                push_constant_ranges: &[],
            });

        let volumetric_blur_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Volumetric Blur Pipeline"),
                layout: Some(&volumetric_blur_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &volumetric_blur_shader,
                    entry_point: Some("vs_volumetric_blur"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &volumetric_blur_shader,
                    entry_point: Some("fs_volumetric_blur"),
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

        let volumetric_blur_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Volumetric Blur Uniform Buffer"),
            size: std::mem::size_of::<VolumetricBlurUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // 1x1 black HDR fallback for when volumetric is disabled
        let volumetric_black_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Volumetric Black Texture"),
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
        let volumetric_black_view =
            volumetric_black_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // --- Kuwahara pipelines (only when enabled) ---
        let kuwahara = if kuwahara_enabled {
            Some(KuwaharaPipelines::new(device))
        } else {
            None
        };

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
            ssao_pipeline,
            ssao_uniform_bgl,
            ssao_depth_bgl,
            ssao_noise_bgl,
            ssao_uniform_buffer,
            ssao_noise_texture_view,
            ssao_noise_sampler,
            depth_sampler,
            ssao_blur_pipeline,
            ssao_blur_uniform_bgl,
            ssao_blur_texture_bgl,
            ssao_blur_uniform_buffer,
            composite_ssao_bgl,
            white_texture_view,
            ssao_kernel,
            volumetric_pipeline,
            volumetric_uniform_bgl,
            volumetric_depth_bgl,
            volumetric_shadow_bgl,
            volumetric_uniform_buffer,
            volumetric_blur_pipeline,
            volumetric_blur_uniform_bgl,
            volumetric_blur_texture_bgl,
            volumetric_blur_uniform_buffer,
            volumetric_black_view,
            kuwahara,
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
                texel_size: [1.0 / resources.width as f32, 1.0 / resources.height as f32],
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

            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
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

            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
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

            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
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

    /// Run the SSAO generation and blur passes.
    /// Reads the depth buffer and writes blurred AO to `resources.ssao_blur_view`.
    pub fn run_ssao(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resources: &PostProcessResources,
        config: &PostProcessConfig,
        depth_view: &wgpu::TextureView,
        camera: &Camera,
    ) {
        // Write SSAO uniforms
        let ssao_uniforms = SsaoUniforms {
            inv_projection: camera.inverse_projection_matrix(),
            projection: camera.projection_matrix(),
            kernel: self.ssao_kernel,
            noise_scale: [resources.width as f32 / 4.0, resources.height as f32 / 4.0],
            radius: config.ssao_radius,
            bias: config.ssao_bias,
            intensity: config.ssao_intensity,
            near: camera.near,
            far: camera.far,
            _pad: 0.0,
        };
        queue.write_buffer(
            &self.ssao_uniform_buffer,
            0,
            bytemuck::cast_slice(&[ssao_uniforms]),
        );

        // Create bind groups
        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SSAO Uniform BG"),
            layout: &self.ssao_uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.ssao_uniform_buffer.as_entire_binding(),
            }],
        });

        let depth_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SSAO Depth BG"),
            layout: &self.ssao_depth_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.depth_sampler),
                },
            ],
        });

        let noise_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SSAO Noise BG"),
            layout: &self.ssao_noise_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.ssao_noise_texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.ssao_noise_sampler),
                },
            ],
        });

        // SSAO generation pass → writes to resources.ssao_view
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("SSAO Generation Encoder"),
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("SSAO Generation Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &resources.ssao_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(&self.ssao_pipeline);
            pass.set_bind_group(0, &uniform_bg, &[]);
            pass.set_bind_group(1, &depth_bg, &[]);
            pass.set_bind_group(2, &noise_bg, &[]);
            pass.draw(0..3, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));

        // Blur pass → reads ssao_view, writes ssao_blur_view
        let blur_uniforms = SsaoBlurUniforms {
            texel_size: [1.0 / resources.width as f32, 1.0 / resources.height as f32],
            _pad: [0.0; 2],
        };
        queue.write_buffer(
            &self.ssao_blur_uniform_buffer,
            0,
            bytemuck::cast_slice(&[blur_uniforms]),
        );

        let blur_uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SSAO Blur Uniform BG"),
            layout: &self.ssao_blur_uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.ssao_blur_uniform_buffer.as_entire_binding(),
            }],
        });

        let blur_texture_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SSAO Blur Texture BG"),
            layout: &self.ssao_blur_texture_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&resources.ssao_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("SSAO Blur Encoder"),
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("SSAO Blur Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &resources.ssao_blur_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(&self.ssao_blur_pipeline);
            pass.set_bind_group(0, &blur_uniform_bg, &[]);
            pass.set_bind_group(1, &blur_texture_bg, &[]);
            pass.draw(0..3, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));
    }

    /// Run the volumetric ray-march and bilateral blur passes.
    /// Reads the depth buffer and shadow maps, writes blurred god rays to
    /// `resources.volumetric_blur_view`.
    pub fn run_volumetric(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resources: &PostProcessResources,
        config: &PostProcessConfig,
        depth_view: &wgpu::TextureView,
        camera: &Camera,
        shadow_view: &wgpu::TextureView,
        shadow_sampler: &wgpu::Sampler,
        shadow_uniforms_buffer: &wgpu::Buffer,
        light_buffer: &wgpu::Buffer,
    ) {
        let half_w = (resources.width / 2).max(1);
        let half_h = (resources.height / 2).max(1);

        // Write volumetric uniforms
        let uniforms = VolumetricUniforms {
            inv_view_proj: camera.inverse_view_projection_matrix(),
            camera_pos: camera.position_array(),
            num_samples: config.volumetric_samples as f32,
            density: config.volumetric_density,
            max_distance: config.volumetric_max_distance,
            decay: config.volumetric_decay,
            near: camera.near,
            far: camera.far,
            _pad: [0.0; 3],
        };
        queue.write_buffer(
            &self.volumetric_uniform_buffer,
            0,
            bytemuck::cast_slice(&[uniforms]),
        );

        // Create bind groups
        let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Volumetric Uniform BG"),
            layout: &self.volumetric_uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.volumetric_uniform_buffer.as_entire_binding(),
            }],
        });

        let depth_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Volumetric Depth BG"),
            layout: &self.volumetric_depth_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.depth_sampler),
                },
            ],
        });

        let shadow_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Volumetric Shadow BG"),
            layout: &self.volumetric_shadow_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(shadow_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: shadow_uniforms_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: light_buffer.as_entire_binding(),
                },
            ],
        });

        // Ray-march pass → writes to resources.volumetric_view (half-res)
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Volumetric Ray-March Encoder"),
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Volumetric Ray-March Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &resources.volumetric_view,
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

            pass.set_pipeline(&self.volumetric_pipeline);
            pass.set_bind_group(0, &uniform_bg, &[]);
            pass.set_bind_group(1, &depth_bg, &[]);
            pass.set_bind_group(2, &shadow_bg, &[]);
            pass.draw(0..3, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));

        // Bilateral blur pass → reads volumetric_view + depth, writes volumetric_blur_view
        let blur_uniforms = VolumetricBlurUniforms {
            texel_size: [1.0 / half_w as f32, 1.0 / half_h as f32],
            depth_threshold: 0.01,
            _pad: 0.0,
        };
        queue.write_buffer(
            &self.volumetric_blur_uniform_buffer,
            0,
            bytemuck::cast_slice(&[blur_uniforms]),
        );

        let blur_uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Volumetric Blur Uniform BG"),
            layout: &self.volumetric_blur_uniform_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.volumetric_blur_uniform_buffer.as_entire_binding(),
            }],
        });

        let blur_texture_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Volumetric Blur Texture BG"),
            layout: &self.volumetric_blur_texture_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&resources.volumetric_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.depth_sampler),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Volumetric Blur Encoder"),
        });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Volumetric Blur Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &resources.volumetric_blur_view,
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

            pass.set_pipeline(&self.volumetric_blur_pipeline);
            pass.set_bind_group(0, &blur_uniform_bg, &[]);
            pass.set_bind_group(1, &blur_texture_bg, &[]);
            pass.draw(0..3, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));
    }

    /// Run the anisotropic Kuwahara filter (3 passes: structure tensor, tensor blur, Kuwahara).
    /// Reads the HDR scene texture and writes the painterly-filtered result to
    /// `resources.kuwahara.output_view`.
    /// Returns early (no-op) if Kuwahara pipelines or textures were not allocated.
    pub fn run_kuwahara(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resources: &PostProcessResources,
        config: &PostProcessConfig,
    ) {
        let kp = match &self.kuwahara {
            Some(kp) => kp,
            None => return,
        };
        let kt = match &resources.kuwahara {
            Some(kt) => kt,
            None => return,
        };

        let texel_size = [1.0 / resources.width as f32, 1.0 / resources.height as f32];

        // --- Pass 1: Structure tensor ---
        {
            let uniforms = KuwaharaTensorUniforms {
                texel_size,
                _pad: [0.0; 2],
            };
            queue.write_buffer(
                &kp.tensor_uniform_buffer,
                0,
                bytemuck::cast_slice(&[uniforms]),
            );

            let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Kuwahara Tensor Uniform BG"),
                layout: &kp.tensor_uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: kp.tensor_uniform_buffer.as_entire_binding(),
                }],
            });

            let texture_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Kuwahara Tensor Texture BG"),
                layout: &kp.tensor_texture_bgl,
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

            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Kuwahara Tensor Encoder"),
            });

            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Kuwahara Tensor Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &kt.tensor_view,
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

                pass.set_pipeline(&kp.tensor_pipeline);
                pass.set_bind_group(0, &uniform_bg, &[]);
                pass.set_bind_group(1, &texture_bg, &[]);
                pass.draw(0..3, 0..1);
            }

            queue.submit(std::iter::once(encoder.finish()));
        }

        // --- Pass 2: Tensor blur ---
        {
            let uniforms = KuwaharaTensorBlurUniforms {
                texel_size,
                _pad: [0.0; 2],
            };
            queue.write_buffer(
                &kp.tensor_blur_uniform_buffer,
                0,
                bytemuck::cast_slice(&[uniforms]),
            );

            let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Kuwahara Tensor Blur Uniform BG"),
                layout: &kp.tensor_blur_uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: kp.tensor_blur_uniform_buffer.as_entire_binding(),
                }],
            });

            let texture_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Kuwahara Tensor Blur Texture BG"),
                layout: &kp.tensor_blur_texture_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&kt.tensor_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                    },
                ],
            });

            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Kuwahara Tensor Blur Encoder"),
            });

            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Kuwahara Tensor Blur Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &kt.tensor_blur_view,
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

                pass.set_pipeline(&kp.tensor_blur_pipeline);
                pass.set_bind_group(0, &uniform_bg, &[]);
                pass.set_bind_group(1, &texture_bg, &[]);
                pass.draw(0..3, 0..1);
            }

            queue.submit(std::iter::once(encoder.finish()));
        }

        // --- Pass 3: Anisotropic Kuwahara filter ---
        {
            let uniforms = KuwaharaUniforms {
                texel_size,
                radius: config.kuwahara_radius as f32,
                sharpness: config.kuwahara_sharpness,
                hardness: config.kuwahara_hardness,
                anisotropy: config.kuwahara_anisotropy,
                _pad: [0.0; 2],
            };
            queue.write_buffer(&kp.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));

            let uniform_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Kuwahara Uniform BG"),
                layout: &kp.uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: kp.uniform_buffer.as_entire_binding(),
                }],
            });

            let hdr_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Kuwahara HDR BG"),
                layout: &kp.hdr_bgl,
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

            let tensor_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Kuwahara Tensor Input BG"),
                layout: &kp.tensor_input_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&kt.tensor_blur_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                    },
                ],
            });

            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Kuwahara Encoder"),
            });

            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Kuwahara Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &kt.output_view,
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

                pass.set_pipeline(&kp.pipeline);
                pass.set_bind_group(0, &uniform_bg, &[]);
                pass.set_bind_group(1, &hdr_bg, &[]);
                pass.set_bind_group(2, &tensor_bg, &[]);
                pass.draw(0..3, 0..1);
            }

            queue.submit(std::iter::once(encoder.finish()));
        }
    }

    /// Run the composite pass: combine HDR scene + bloom + SSAO + volumetric + fog → tonemapped sRGB surface.
    pub fn composite(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resources: &PostProcessResources,
        config: &PostProcessConfig,
        target_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        camera: &Camera,
    ) {
        // SSAO: use blurred AO if enabled, white fallback otherwise
        let ssao_view = if config.enabled && config.ssao_enabled {
            &resources.ssao_blur_view
        } else {
            &self.white_texture_view
        };

        // Volumetric: use blurred god rays if enabled, black fallback otherwise
        let volumetric_view = if config.enabled && config.volumetric_enabled {
            &resources.volumetric_blur_view
        } else {
            &self.volumetric_black_view
        };

        let ssao_depth_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Composite SSAO+Depth+Volumetric BG"),
            layout: &self.composite_ssao_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(ssao_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.depth_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(volumetric_view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                },
            ],
        });
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
            texel_size: [1.0 / resources.width as f32, 1.0 / resources.height as f32],
            // chromatic_aberration/radial_blur predate the effects_on gate and
            // remain ungated; desaturate follows the gated convention.
            chromatic_aberration: config.chromatic_aberration,
            radial_blur: config.radial_blur,
            desaturate: if effects_on { config.desaturate } else { 0.0 },
            _pad: 0.0,
            fog_color: config.fog_color,
            fog_density: config.fog_density,
            fog_start: config.fog_start,
            fog_end: config.fog_end,
            fog_height_falloff: config.fog_height_falloff,
            fog_height_origin: config.fog_height_origin,
            camera_pos: camera.position_array(),
            fog_enabled: if effects_on && config.fog_enabled {
                1.0
            } else {
                0.0
            },
            near: camera.near,
            far: camera.far,
            fog_height_enabled: if config.fog_height_enabled { 1.0 } else { 0.0 },
            dither_intensity: if effects_on && config.dither_enabled {
                config.dither_intensity
            } else {
                0.0
            },
            inv_view_proj: camera.inverse_view_projection_matrix(),
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

        // Kuwahara: use filtered result if enabled and allocated, raw HDR otherwise
        let scene_view = if config.enabled && config.kuwahara_enabled {
            match &resources.kuwahara {
                Some(kt) => &kt.output_view,
                None => &resources.hdr_view,
            }
        } else {
            &resources.hdr_view
        };

        let scene_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Composite Scene BG"),
            layout: &self.composite_scene_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(scene_view),
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

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
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
            pass.set_bind_group(3, &ssao_depth_bg, &[]);
            pass.draw(0..3, 0..1);
        }

        queue.submit(std::iter::once(encoder.finish()));
    }
}

impl PostProcessResources {
    /// Create HDR buffer and bloom mip chain for the given dimensions.
    /// Kuwahara textures are only allocated when `kuwahara_enabled` is true
    /// to avoid exhausting VRAM on constrained GPUs.
    pub fn new(device: &wgpu::Device, width: u32, height: u32, kuwahara_enabled: bool) -> Self {
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

        // SSAO textures (full resolution, R8Unorm)
        let ssao_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("SSAO Texture"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SSAO_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let ssao_view = ssao_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let ssao_blur_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("SSAO Blur Texture"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SSAO_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let ssao_blur_view = ssao_blur_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Volumetric textures (half resolution, Rgba16Float)
        let half_w = (width / 2).max(1);
        let half_h = (height / 2).max(1);

        let volumetric_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Volumetric Texture"),
            size: wgpu::Extent3d {
                width: half_w,
                height: half_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let volumetric_view =
            volumetric_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let volumetric_blur_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Volumetric Blur Texture"),
            size: wgpu::Extent3d {
                width: half_w,
                height: half_h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let volumetric_blur_view =
            volumetric_blur_texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Kuwahara textures (full resolution, Rgba16Float) — only when enabled
        let kuwahara = if kuwahara_enabled {
            Some(KuwaharaTextures::new(device, width, height))
        } else {
            None
        };

        Self {
            hdr_texture,
            hdr_view,
            width,
            height,
            bloom_mips,
            bloom_mip_count,
            ssao_texture,
            ssao_view,
            ssao_blur_texture,
            ssao_blur_view,
            volumetric_texture,
            volumetric_view,
            volumetric_blur_texture,
            volumetric_blur_view,
            kuwahara,
        }
    }
}

/// Deterministic xorshift32 RNG for kernel/noise generation (no rand crate dependency).
fn xorshift32(state: &mut u32) -> u32 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    *state = x;
    x
}

/// Generate a random f32 in [0, 1) from xorshift32.
fn xorshift_f32(state: &mut u32) -> f32 {
    (xorshift32(state) & 0x00FF_FFFF) as f32 / 16_777_216.0
}

/// Generate 64-sample hemisphere kernel for SSAO.
/// Samples are in tangent space (z-up hemisphere) with positions weighted
/// toward the center for better quality at close range.
fn generate_ssao_kernel() -> [[f32; 4]; SSAO_KERNEL_SIZE] {
    let mut kernel = [[0.0f32; 4]; SSAO_KERNEL_SIZE];
    let mut rng_state: u32 = 0xDEAD_BEEF;

    for (i, sample) in kernel.iter_mut().enumerate() {
        // Random direction in tangent-space hemisphere
        let x = xorshift_f32(&mut rng_state) * 2.0 - 1.0;
        let y = xorshift_f32(&mut rng_state) * 2.0 - 1.0;
        let z = xorshift_f32(&mut rng_state); // z in [0, 1) — hemisphere

        // Normalize
        let len = (x * x + y * y + z * z).sqrt().max(0.0001);
        let (nx, ny, nz) = (x / len, y / len, z / len);

        // Scale: accelerating interpolation (more samples closer to the surface)
        let scale = i as f32 / SSAO_KERNEL_SIZE as f32;
        let scale = 0.1 + scale * scale * 0.9; // lerp(0.1, 1.0, scale^2)
        let r = xorshift_f32(&mut rng_state) * scale;

        sample[0] = nx * r;
        sample[1] = ny * r;
        sample[2] = nz * r;
        sample[3] = 0.0; // padding for vec4 alignment
    }

    kernel
}

/// Generate 4x4 noise texture data (64 bytes RGBA8).
/// Each pixel is a random rotation vector in tangent space (XY), with Z=0 and A=255.
fn generate_ssao_noise() -> [u8; 64] {
    let mut data = [0u8; 64];
    let mut rng_state: u32 = 0xCAFE_BABE;

    for i in 0..16 {
        // Random rotation vector in tangent plane
        let x = xorshift_f32(&mut rng_state) * 2.0 - 1.0;
        let y = xorshift_f32(&mut rng_state) * 2.0 - 1.0;
        // Encode [-1,1] → [0,255]
        data[i * 4] = ((x * 0.5 + 0.5) * 255.0) as u8;
        data[i * 4 + 1] = ((y * 0.5 + 0.5) * 255.0) as u8;
        data[i * 4 + 2] = 0; // Z = 0 (tangent plane rotation only)
        data[i * 4 + 3] = 255; // Alpha = 1.0
    }

    data
}
