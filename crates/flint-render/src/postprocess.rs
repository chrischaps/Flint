//! Post-processing pipeline: HDR buffer, bloom, composite tonemapping
//!
//! Renders the scene to an Rgba16Float intermediate buffer, optionally
//! applies a bloom downsample/upsample chain, then composites to the
//! sRGB surface with exposure, ACES tonemapping, gamma, and vignette.

use crate::camera::Camera;
use bytemuck::{Pod, Zeroable};

/// Maximum number of bloom mip levels in the downsample/upsample chain.
pub const MAX_BLOOM_MIPS: usize = 5;

/// Bytes between the bloom steps' uniform slots: the uniform offset
/// alignment.
const BLOOM_SLOT_STRIDE: u64 = crate::bind_cache::SLOT_STRIDE;

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
    pub dof_strength: f32,
    pub dof_focus_distance: f32,
    pub dof_focus_range: f32,
    pub ssao_enabled: bool,
    pub ssao_radius: f32,
    pub ssao_intensity: f32,
    pub ssao_bias: f32,
    /// Hemisphere samples per pixel, 1..=64 (clamped at upload). 64 is the
    /// full kernel (the pre-lever behavior); the shader strides through the
    /// scale-ordered kernel so lower counts keep full radius coverage. SSAO
    /// is the renderer's heaviest per-pixel pass — 16 is the quality/cost
    /// sweet spot when the blur pass can hide the extra noise.
    pub ssao_samples: u32,
    pub fog_enabled: bool,
    pub fog_color: [f32; 3],
    pub fog_density: f32,
    pub fog_start: f32,
    pub fog_end: f32,
    pub fog_height_enabled: bool,
    pub fog_height_falloff: f32,
    pub fog_height_origin: f32,
    /// Height fog's density at `fog_height_origin` (ADR 0101). It falls by
    /// e every `1 / fog_height_falloff` metres above the origin, and its
    /// optical depth along each view ray adds to the distance fog's. 0 (the
    /// default) adds nothing, so a scene that only turned height fog on
    /// looks as it did when height fog did nothing.
    pub fog_height_density: f32,
    /// Aerial tint (ADR 0101): far things take `fog_tint_color` (linear),
    /// up to `fog_tint_strength` (0 = off, the default), ramping in over
    /// `fog_tint_start..fog_tint_end` metres from the camera. It lies under
    /// the fog, so the farthest things still run into `fog_color`. Rides
    /// `fog_enabled`, and leaves the sky alone.
    pub fog_tint_color: [f32; 3],
    pub fog_tint_strength: f32,
    pub fog_tint_start: f32,
    pub fog_tint_end: f32,
    /// Ground fog (ADR 0106): fog on floors of its own, read from the map
    /// set with `SceneRenderer::set_ground_fog`, so a mist can fill hollows
    /// at many heights. Its density per metre at a texel's floor where the
    /// texel's amount is 1; 0 (the default) is off, as is a renderer with
    /// no map. Unlit and in `ground_fog_color` (linear), it lies under the
    /// distance fog and the aerial tint, and rides `enabled`, not
    /// `fog_enabled`.
    pub ground_fog_density: f32,
    pub ground_fog_color: [f32; 3],
    /// It falls by e every `1 / ground_fog_falloff` metres over the floor.
    pub ground_fog_falloff: f32,
    /// Steps each view ray marches through it, 1..=64 (clamped at upload).
    pub ground_fog_steps: u32,
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
    /// Stylized render mode: 0 = none, 1 = Matrix code, 2 = blood ocean
    /// grade, 3 = drunk, 4 = Tron wireframe (reality tears), 5 = underwater
    /// (submerged-eye moments; masks by a per-pixel waterline, not the
    /// tear mask).
    pub render_mode: u32,
    /// Blend strength for the active render mode (0 = normal view).
    pub mode_mix: f32,
    /// Animated film grain intensity (0 = off). Hash noise on pixel coords,
    /// time-quantized to 24 Hz off the shared post `time`; luma-weighted so
    /// highlights stay clean (ADR 0050).
    pub film_grain: f32,
    /// Color grade, ASC-CDL-shaped, applied right after ACES tonemapping:
    /// pow(max(color * gain + lift, 0), 1/gamma). Neutral = lift 0 /
    /// gamma 1 / gain 1; neutrality is guarded by a CPU-computed enable
    /// flag, never by pow(x, 1.0) float identity (ADR 0050).
    pub grade_lift: [f32; 3],
    pub grade_gamma: [f32; 3],
    pub grade_gain: [f32; 3],
    /// Per-mode tuning. Tears (1-4): x = bleed-mask scale, y = mask style
    /// (0 fbm / 1 iris), z = per-mode rate, w = spare. Underwater (5):
    /// x = signed eye depth in meters (+ = submerged; waterline Y =
    /// camera Y + x), y = sea energy 0..1, z = daylight 0..1, w = biolum 0..1.
    pub mode_params: [f32; 4],
    /// FXAA pass on the final composite output (default off — headless
    /// pixel-diff gates run single-path without it; ADR 0050, the cheap
    /// interim for tech-debt #13 until the MSAA workstream).
    pub fxaa_enabled: bool,
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
            dof_strength: 0.0,
            dof_focus_distance: 10.0,
            dof_focus_range: 5.0,
            ssao_enabled: true,
            ssao_radius: 0.5,
            ssao_intensity: 1.0,
            ssao_bias: 0.025,
            ssao_samples: 64,
            fog_enabled: false,
            fog_color: [0.7, 0.75, 0.82],
            fog_density: 0.02,
            fog_start: 5.0,
            fog_end: 100.0,
            fog_height_enabled: false,
            fog_height_falloff: 0.1,
            fog_height_origin: 0.0,
            fog_height_density: 0.0,
            fog_tint_color: [0.42, 0.50, 0.65],
            fog_tint_strength: 0.0,
            fog_tint_start: 100.0,
            fog_tint_end: 1000.0,
            ground_fog_density: 0.0,
            ground_fog_color: [0.8, 0.82, 0.86],
            ground_fog_falloff: 0.5,
            ground_fog_steps: 24,
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
            render_mode: 0,
            mode_mix: 0.0,
            mode_params: [0.0; 4],
            film_grain: 0.0,
            grade_lift: [0.0; 3],
            grade_gamma: [1.0; 3],
            grade_gain: [1.0; 3],
            fxaa_enabled: false,
        }
    }
}

impl PostProcessConfig {
    /// True when any grade component deviates from neutral. The shader
    /// gates the grade block on this (uploaded as grade_enabled) so the
    /// neutral path is the original code verbatim — pow(x, 1.0) is not
    /// trusted to be bit-exact across drivers.
    pub fn grade_active(&self) -> bool {
        self.grade_lift != [0.0; 3] || self.grade_gamma != [1.0; 3] || self.grade_gain != [1.0; 3]
    }
}

/// Convert a `PostProcessDef` (scene TOML) to a `PostProcessConfig` (renderer).
///
/// Centralises the field-by-field copy so that new fields are never forgotten.
/// Lives here (not in the player) so every front end that renders a scene —
/// player, CLI render, viewer — applies the authored `[post_process]` block
/// through one function.
pub fn post_process_config_from_def(pp_def: &flint_scene::PostProcessDef) -> PostProcessConfig {
    let mut config = PostProcessConfig::default();
    config.bloom_enabled = pp_def.bloom_enabled;
    config.bloom_intensity = pp_def.bloom_intensity;
    config.bloom_threshold = pp_def.bloom_threshold;
    config.vignette_enabled = pp_def.vignette_enabled;
    config.vignette_intensity = pp_def.vignette_intensity;
    config.vignette_smoothness = pp_def.vignette_smoothness;
    config.exposure = pp_def.exposure;
    config.ssao_enabled = pp_def.ssao_enabled;
    config.ssao_radius = pp_def.ssao_radius;
    config.ssao_intensity = pp_def.ssao_intensity;
    config.ssao_samples = pp_def.ssao_samples;
    config.fog_enabled = pp_def.fog_enabled;
    config.fog_color = pp_def.fog_color;
    config.fog_density = pp_def.fog_density;
    config.fog_start = pp_def.fog_start;
    config.fog_end = pp_def.fog_end;
    config.fog_height_enabled = pp_def.fog_height_enabled;
    config.fog_height_falloff = pp_def.fog_height_falloff;
    config.fog_height_origin = pp_def.fog_height_origin;
    config.fog_height_density = pp_def.fog_height_density;
    config.fog_tint_color = pp_def.fog_tint_color;
    config.fog_tint_strength = pp_def.fog_tint_strength;
    config.fog_tint_start = pp_def.fog_tint_start;
    config.fog_tint_end = pp_def.fog_tint_end;
    config.chromatic_aberration = pp_def.chromatic_aberration;
    config.radial_blur = pp_def.radial_blur;
    config.desaturate = pp_def.desaturate;
    config.dof_strength = pp_def.dof_strength;
    config.dof_focus_distance = pp_def.dof_focus_distance;
    config.dof_focus_range = pp_def.dof_focus_range;
    config.dither_enabled = pp_def.dither_enabled;
    config.dither_intensity = pp_def.dither_intensity;
    config.volumetric_enabled = pp_def.volumetric_enabled;
    config.volumetric_samples = pp_def.volumetric_samples;
    config.volumetric_density = pp_def.volumetric_density;
    config.volumetric_max_distance = pp_def.volumetric_max_distance;
    config.volumetric_decay = pp_def.volumetric_decay;
    config.kuwahara_enabled = pp_def.kuwahara_enabled;
    config.kuwahara_radius = pp_def.kuwahara_radius;
    config.kuwahara_sharpness = pp_def.kuwahara_sharpness;
    config.kuwahara_hardness = pp_def.kuwahara_hardness;
    config.kuwahara_anisotropy = pp_def.kuwahara_anisotropy;
    config.film_grain = pp_def.film_grain;
    config.grade_lift = pp_def.grade_lift;
    config.grade_gamma = pp_def.grade_gamma;
    config.grade_gain = pp_def.grade_gain;
    config.fxaa_enabled = pp_def.fxaa;
    config
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
    /// Film grain intensity (rides the former _pad slot; 0 = off).
    pub grain_intensity: f32,
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
    // Depth of field (trailing scalar row — appended after the mat4 so every
    // pre-existing offset is untouched; WGSL mirror must match field-for-field)
    pub dof_strength: f32,
    pub dof_focus_distance: f32,
    pub dof_focus_range: f32,
    pub _pad2: f32,
    // Reality-tear render mode (must mirror composite_shader.wgsl)
    pub render_mode: u32,
    pub mode_mix: f32,
    pub mode_time: f32,
    pub _pad_mode: f32,
    pub mode_params: [f32; 4],
    // Color grade (ADR 0050) — three appended 16-byte rows; grade_enabled is
    // CPU-computed (see PostProcessConfig::grade_active) so the neutral path
    // never relies on pow(x, 1.0) float identity.
    pub grade_lift: [f32; 3],
    pub grade_enabled: f32,
    pub grade_gamma: [f32; 3],
    pub _pad3: f32,
    pub grade_gain: [f32; 3],
    pub _pad4: f32,
    // Aerial tint and height fog density (ADR 0101) — two appended rows.
    pub fog_tint_color: [f32; 3],
    pub fog_tint_strength: f32,
    pub fog_tint_start: f32,
    pub fog_tint_end: f32,
    pub fog_height_density: f32,
    pub _pad5: f32,
    // Ground fog (ADR 0106) — four appended rows. The box the pass marches
    // runs from ground_fog_floor_base up to ground_fog_ceiling, over
    // origin..origin + size in X and Z.
    pub ground_fog_color: [f32; 3],
    pub ground_fog_density: f32,
    pub ground_fog_origin: [f32; 2],
    pub ground_fog_size: [f32; 2],
    pub ground_fog_floor_base: f32,
    pub ground_fog_ceiling: f32,
    pub ground_fog_falloff: f32,
    pub ground_fog_steps: f32,
    pub ground_fog_texel: f32,
    pub ground_fog_max_lod: f32,
    pub ground_fog_enabled: f32,
    pub _pad6: f32,
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
    /// Hemisphere sample count (rides the former padding slot — layout
    /// unchanged). The shader treats out-of-range as the full 64.
    pub sample_count: f32,
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

/// Intermediate target for the FXAA pass (ADR 0050) — only allocated when
/// `fxaa_enabled` (Kuwahara lazy pattern). The composite pass renders into
/// this instead of the swapchain; the FXAA pass then samples it and writes
/// the real target. Same format as the composite pipeline's baked fragment
/// target so that pipeline is reused unchanged.
pub struct FxaaResources {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
}

impl FxaaResources {
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("FXAA Intermediate Texture"),
            size: wgpu::Extent3d {
                width: width.max(1),
                height: height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { texture, view }
    }
}

/// Uniforms for the FXAA pass.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct FxaaUniforms {
    pub texel_size: [f32; 2],
    pub _pad: [f32; 2],
}

/// FXAA pipeline — only created when enabled (Kuwahara lazy pattern).
pub struct FxaaPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub uniform_bgl: wgpu::BindGroupLayout,
    pub texture_bgl: wgpu::BindGroupLayout,
    pub uniform_buffer: wgpu::Buffer,
}

impl FxaaPipeline {
    pub fn new(device: &wgpu::Device, surface_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("FXAA Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("fxaa_shader.wgsl").into()),
        });

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("FXAA Uniform BGL"),
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

        let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("FXAA Texture BGL"),
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("FXAA Pipeline Layout"),
            bind_group_layouts: &[&uniform_bgl, &texture_bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("FXAA Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_fxaa"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_fxaa"),
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

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("FXAA Uniform Buffer"),
            size: std::mem::size_of::<FxaaUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            uniform_bgl,
            texture_bgl,
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
    // FXAA intermediate (full resolution, surface format) — only allocated when enabled
    pub fxaa: Option<FxaaResources>,
    /// The chain's bind groups, kept until a resize replaces it (#139).
    pub binds: crate::bind_cache::BindCache,
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
    // 1x1 clear map bound when no ground fog is set (ADR 0106)
    pub ground_fog_empty_view: wgpu::TextureView,
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
    // FXAA pipeline — only created when enabled (ADR 0050)
    pub fxaa: Option<FxaaPipeline>,
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
                    // The ground fog's map (ADR 0106), read through binding 5
                    wgpu::BindGroupLayoutEntry {
                        binding: 6,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
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

        // A slot per bloom step (`run_bloom`)
        let bloom_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Bloom Uniform Buffer"),
            size: 2 * MAX_BLOOM_MIPS as u64 * BLOOM_SLOT_STRIDE,
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

        // 1x1 clear ground fog map (zero-initialised) for when none is set
        let ground_fog_empty_view = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("Ground Fog Empty Map"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: crate::ground_fog::FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default());

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
            ground_fog_empty_view,
            kuwahara,
            // FXAA is created on demand via ensure_fxaa_resources (ADR 0050)
            fxaa: None,
        }
    }

    /// Run the FXAA pass: sample the intermediate composite output and write
    /// the real target. Caller guarantees both `self.fxaa` and
    /// `resources.fxaa` exist (see SceneRenderer::ensure_fxaa_resources).
    pub fn run_fxaa(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        resources: &PostProcessResources,
        target_view: &wgpu::TextureView,
        timer: Option<&mut crate::gpu_timer::GpuTimer>,
    ) {
        let (Some(fxaa), Some(fxaa_res)) = (&self.fxaa, &resources.fxaa) else {
            return;
        };

        if resources.binds.first_write("fxaa") {
            let uniforms = FxaaUniforms {
                texel_size: [1.0 / resources.width as f32, 1.0 / resources.height as f32],
                _pad: [0.0; 2],
            };
            queue.write_buffer(&fxaa.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
        }

        let uniform_bg = resources.binds.get("fxaa uniform", 0, || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("FXAA Uniform BG"),
                layout: &fxaa.uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: fxaa.uniform_buffer.as_entire_binding(),
                }],
            })
        });
        let texture_bg = resources.binds.get("fxaa texture", 0, || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("FXAA Texture BG"),
                layout: &fxaa.texture_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&fxaa_res.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                    },
                ],
            })
        });

        let timed = timer.map(|t| {
            let scope = t.begin(encoder, "FXAA");
            (t, scope)
        });
        fullscreen_pass(
            encoder,
            "FXAA Pass",
            target_view,
            Some(wgpu::Color::BLACK),
            &fxaa.pipeline,
            &[&uniform_bg, &texture_bg],
        );
        if let Some((timer, scope)) = timed {
            timer.end(encoder, scope);
        }
    }

    /// Run the bloom downsample/upsample chain.
    /// Reads from the HDR scene texture and writes to bloom mip chain.
    ///
    /// Each step reads its own slot of the bloom uniform buffer (#139):
    /// the steps used to share one, rewritten and submitted step by step.
    /// Slot 0 is the threshold, `i` the downsample into mip `i`, and
    /// `mips + i` the upsample into mip `i`.
    pub fn run_bloom(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        resources: &PostProcessResources,
        config: &PostProcessConfig,
    ) {
        let mips = resources.bloom_mip_count;
        if mips == 0 {
            return;
        }
        let stride = BLOOM_SLOT_STRIDE as usize;
        let mut slots = vec![0u8; 2 * mips * stride];
        let mut put = |slot: usize, u: BloomUniforms| {
            slots[slot * stride..slot * stride + std::mem::size_of::<BloomUniforms>()]
                .copy_from_slice(bytemuck::bytes_of(&u));
        };
        put(
            0,
            BloomUniforms {
                texel_size: [1.0 / resources.width as f32, 1.0 / resources.height as f32],
                threshold: config.bloom_threshold,
                soft_threshold: config.bloom_soft_threshold,
            },
        );
        let texel = |mip: &BloomMip| BloomUniforms {
            texel_size: [1.0 / mip.width as f32, 1.0 / mip.height as f32],
            threshold: 0.0,
            soft_threshold: 0.0,
        };
        for i in 1..mips {
            put(i, texel(&resources.bloom_mips[i - 1]));
        }
        for i in 0..mips - 1 {
            put(mips + i, texel(&resources.bloom_mips[i + 1]));
        }
        queue.write_buffer(&self.bloom_uniform_buffer, 0, &slots);

        let uniform_bg = |slot: usize| {
            resources.binds.get("bloom uniform", slot as u64, || {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Bloom Uniform BG"),
                    layout: &self.bloom_uniform_bgl,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.bloom_uniform_buffer,
                            offset: slot as u64 * BLOOM_SLOT_STRIDE,
                            size: wgpu::BufferSize::new(
                                std::mem::size_of::<BloomUniforms>() as u64
                            ),
                        }),
                    }],
                })
            })
        };
        // The source of each step: the HDR scene (variant 0) or a mip (1 + i)
        let source_bg = |variant: usize, view: &wgpu::TextureView| {
            resources.binds.get("bloom source", variant as u64, || {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Bloom Source BG"),
                    layout: &self.bloom_texture_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                        },
                    ],
                })
            })
        };

        // Step 1: Threshold — extract bright pixels from HDR into mip[0]
        fullscreen_pass(
            encoder,
            "Bloom Threshold Pass",
            &resources.bloom_mips[0].view,
            Some(wgpu::Color::BLACK),
            &self.bloom_threshold_pipeline,
            &[&uniform_bg(0), &source_bg(0, &resources.hdr_view)],
        );

        // Step 2: Progressive downsample mip[0] → mip[1] → ... → mip[N-1]
        for i in 1..mips {
            fullscreen_pass(
                encoder,
                "Bloom Downsample Pass",
                &resources.bloom_mips[i].view,
                Some(wgpu::Color::BLACK),
                &self.bloom_downsample_pipeline,
                &[
                    &uniform_bg(i),
                    &source_bg(i, &resources.bloom_mips[i - 1].view),
                ],
            );
        }

        // Step 3: Progressive upsample mip[N-1] → mip[N-2] → ... → mip[0]
        // Additive blending accumulates the bloom result
        for i in (0..mips - 1).rev() {
            fullscreen_pass(
                encoder,
                "Bloom Upsample Pass",
                &resources.bloom_mips[i].view,
                None,
                &self.bloom_upsample_pipeline,
                &[
                    &uniform_bg(mips + i),
                    &source_bg(i + 2, &resources.bloom_mips[i + 1].view),
                ],
            );
        }
    }

    /// Run the SSAO generation and blur passes.
    /// Reads the depth buffer and writes blurred AO to `resources.ssao_blur_view`.
    /// `depth_owned`: `depth_view` lives as long as `resources` (the scaled
    /// scene's depth), so the bind group over it can be kept.
    #[allow(clippy::too_many_arguments)]
    pub fn run_ssao(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        resources: &PostProcessResources,
        config: &PostProcessConfig,
        depth_view: &wgpu::TextureView,
        depth_owned: bool,
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
            sample_count: config.ssao_samples.clamp(1, SSAO_KERNEL_SIZE as u32) as f32,
        };
        queue.write_buffer(
            &self.ssao_uniform_buffer,
            0,
            bytemuck::cast_slice(&[ssao_uniforms]),
        );

        let uniform_bg = resources.binds.get("ssao uniform", 0, || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("SSAO Uniform BG"),
                layout: &self.ssao_uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.ssao_uniform_buffer.as_entire_binding(),
                }],
            })
        });
        let depth_bg = resources.binds.get_if(depth_owned, "ssao depth", 0, || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
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
            })
        });
        let noise_bg = resources.binds.get("ssao noise", 0, || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
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
            })
        });

        // SSAO generation pass → writes to resources.ssao_view
        fullscreen_pass(
            encoder,
            "SSAO Generation Pass",
            &resources.ssao_view,
            Some(wgpu::Color::WHITE),
            &self.ssao_pipeline,
            &[&uniform_bg, &depth_bg, &noise_bg],
        );

        // Blur pass → reads ssao_view, writes ssao_blur_view
        if resources.binds.first_write("ssao blur") {
            let blur_uniforms = SsaoBlurUniforms {
                texel_size: [1.0 / resources.width as f32, 1.0 / resources.height as f32],
                _pad: [0.0; 2],
            };
            queue.write_buffer(
                &self.ssao_blur_uniform_buffer,
                0,
                bytemuck::cast_slice(&[blur_uniforms]),
            );
        }
        let blur_uniform_bg = resources.binds.get("ssao blur uniform", 0, || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("SSAO Blur Uniform BG"),
                layout: &self.ssao_blur_uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.ssao_blur_uniform_buffer.as_entire_binding(),
                }],
            })
        });
        let blur_texture_bg = resources.binds.get("ssao blur texture", 0, || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
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
            })
        });
        fullscreen_pass(
            encoder,
            "SSAO Blur Pass",
            &resources.ssao_blur_view,
            Some(wgpu::Color::WHITE),
            &self.ssao_blur_pipeline,
            &[&blur_uniform_bg, &blur_texture_bg],
        );
    }

    /// Run the volumetric ray-march and bilateral blur passes.
    /// Reads the depth buffer and shadow maps, writes blurred god rays to
    /// `resources.volumetric_blur_view`. `shadow_id` is the shadow pass's
    /// [`gpu_id`](crate::bind_cache::gpu_id), `depth_owned` as in
    /// [`run_ssao`](Self::run_ssao).
    #[allow(clippy::too_many_arguments)]
    pub fn run_volumetric(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        resources: &PostProcessResources,
        config: &PostProcessConfig,
        depth_view: &wgpu::TextureView,
        depth_owned: bool,
        camera: &Camera,
        shadow: VolumetricShadow<'_>,
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

        let uniform_bg = resources.binds.get("volumetric uniform", 0, || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Volumetric Uniform BG"),
                layout: &self.volumetric_uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.volumetric_uniform_buffer.as_entire_binding(),
                }],
            })
        });
        let depth_bg = resources
            .binds
            .get_if(depth_owned, "volumetric depth", 0, || {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
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
                })
            });
        let shadow_bg = resources.binds.get("volumetric shadow", shadow.id, || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Volumetric Shadow BG"),
                layout: &self.volumetric_shadow_bgl,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(shadow.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(shadow.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: shadow.uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: shadow.lights.as_entire_binding(),
                    },
                ],
            })
        });

        // Ray-march pass → writes to resources.volumetric_view (half-res)
        fullscreen_pass(
            encoder,
            "Volumetric Ray-March Pass",
            &resources.volumetric_view,
            Some(wgpu::Color::BLACK),
            &self.volumetric_pipeline,
            &[&uniform_bg, &depth_bg, &shadow_bg],
        );

        // Bilateral blur pass → reads volumetric_view + depth, writes volumetric_blur_view
        if resources.binds.first_write("volumetric blur") {
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
        }
        let blur_uniform_bg = resources.binds.get("volumetric blur uniform", 0, || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Volumetric Blur Uniform BG"),
                layout: &self.volumetric_blur_uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.volumetric_blur_uniform_buffer.as_entire_binding(),
                }],
            })
        });
        let blur_texture_bg =
            resources
                .binds
                .get_if(depth_owned, "volumetric blur texture", 0, || {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Volumetric Blur Texture BG"),
                        layout: &self.volumetric_blur_texture_bgl,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(
                                    &resources.volumetric_view,
                                ),
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
                    })
                });
        fullscreen_pass(
            encoder,
            "Volumetric Blur Pass",
            &resources.volumetric_blur_view,
            Some(wgpu::Color::BLACK),
            &self.volumetric_blur_pipeline,
            &[&blur_uniform_bg, &blur_texture_bg],
        );
    }

    /// Run the anisotropic Kuwahara filter (3 passes: structure tensor, tensor blur, Kuwahara).
    /// Reads the HDR scene texture and writes the painterly-filtered result to
    /// `resources.kuwahara.output_view`.
    /// Returns early (no-op) if Kuwahara pipelines or textures were not allocated.
    pub fn run_kuwahara(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
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
        let sampled =
            |label: &'static str, layout: &wgpu::BindGroupLayout, view: &wgpu::TextureView| {
                resources.binds.get(label, 0, || {
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
                                resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
                            },
                        ],
                    })
                })
            };
        let uniform =
            |label: &'static str, layout: &wgpu::BindGroupLayout, buffer: &wgpu::Buffer| {
                resources.binds.get(label, 0, || {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some(label),
                        layout,
                        entries: &[wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        }],
                    })
                })
            };

        if resources.binds.first_write("kuwahara tensors") {
            let tensor = KuwaharaTensorUniforms {
                texel_size,
                _pad: [0.0; 2],
            };
            queue.write_buffer(
                &kp.tensor_uniform_buffer,
                0,
                bytemuck::cast_slice(&[tensor]),
            );
            let blur = KuwaharaTensorBlurUniforms {
                texel_size,
                _pad: [0.0; 2],
            };
            queue.write_buffer(
                &kp.tensor_blur_uniform_buffer,
                0,
                bytemuck::cast_slice(&[blur]),
            );
        }

        // --- Pass 1: Structure tensor ---
        fullscreen_pass(
            encoder,
            "Kuwahara Tensor Pass",
            &kt.tensor_view,
            Some(wgpu::Color::BLACK),
            &kp.tensor_pipeline,
            &[
                &uniform(
                    "Kuwahara Tensor Uniform BG",
                    &kp.tensor_uniform_bgl,
                    &kp.tensor_uniform_buffer,
                ),
                &sampled(
                    "Kuwahara Tensor Texture BG",
                    &kp.tensor_texture_bgl,
                    &resources.hdr_view,
                ),
            ],
        );

        // --- Pass 2: Tensor blur ---
        fullscreen_pass(
            encoder,
            "Kuwahara Tensor Blur Pass",
            &kt.tensor_blur_view,
            Some(wgpu::Color::BLACK),
            &kp.tensor_blur_pipeline,
            &[
                &uniform(
                    "Kuwahara Tensor Blur Uniform BG",
                    &kp.tensor_blur_uniform_bgl,
                    &kp.tensor_blur_uniform_buffer,
                ),
                &sampled(
                    "Kuwahara Tensor Blur Texture BG",
                    &kp.tensor_blur_texture_bgl,
                    &kt.tensor_view,
                ),
            ],
        );

        // --- Pass 3: Anisotropic Kuwahara filter ---
        let uniforms = KuwaharaUniforms {
            texel_size,
            radius: config.kuwahara_radius as f32,
            sharpness: config.kuwahara_sharpness,
            hardness: config.kuwahara_hardness,
            anisotropy: config.kuwahara_anisotropy,
            _pad: [0.0; 2],
        };
        queue.write_buffer(&kp.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
        fullscreen_pass(
            encoder,
            "Kuwahara Pass",
            &kt.output_view,
            Some(wgpu::Color::BLACK),
            &kp.pipeline,
            &[
                &uniform("Kuwahara Uniform BG", &kp.uniform_bgl, &kp.uniform_buffer),
                &sampled("Kuwahara HDR BG", &kp.hdr_bgl, &resources.hdr_view),
                &sampled(
                    "Kuwahara Tensor Input BG",
                    &kp.tensor_input_bgl,
                    &kt.tensor_blur_view,
                ),
            ],
        );
    }

    /// Run the composite pass: combine HDR scene + bloom + SSAO + volumetric + fog → tonemapped sRGB surface.
    #[allow(clippy::too_many_arguments)]
    pub fn composite(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        resources: &PostProcessResources,
        config: &PostProcessConfig,
        target_view: &wgpu::TextureView,
        depth_view: &wgpu::TextureView,
        depth_owned: bool,
        camera: &Camera,
        time: f32,
        ground_fog: Option<&crate::ground_fog::GroundFogGpu>,
        timer: Option<&mut crate::gpu_timer::GpuTimer>,
    ) {
        // SSAO: use blurred AO if enabled, white fallback otherwise
        let ssao_on = config.enabled && config.ssao_enabled;
        let ssao_view = if ssao_on {
            &resources.ssao_blur_view
        } else {
            &self.white_texture_view
        };

        // Volumetric: use blurred god rays if enabled, black fallback otherwise
        let volumetric_on = config.enabled && config.volumetric_enabled;
        let volumetric_view = if volumetric_on {
            &resources.volumetric_blur_view
        } else {
            &self.volumetric_black_view
        };

        // The variant: which views it binds, and whose fog map
        let variant = u64::from(ssao_on)
            | u64::from(volumetric_on) << 1
            | ground_fog.map_or(0, |g| g.id) << 2;
        let ssao_depth_bg =
            resources
                .binds
                .get_if(depth_owned, "composite ssao+depth", variant, || {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
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
                            wgpu::BindGroupEntry {
                                binding: 6,
                                resource: wgpu::BindingResource::TextureView(
                                    ground_fog.map_or(&self.ground_fog_empty_view, |g| &g.view),
                                ),
                            },
                        ],
                    })
                });
        let effects_on = config.enabled;
        let ground_fog = ground_fog.filter(|_| effects_on && config.ground_fog_density > 0.0);
        let falloff = config.ground_fog_falloff.max(1e-3);
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
            grain_intensity: if effects_on { config.film_grain } else { 0.0 },
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
            dof_strength: if effects_on { config.dof_strength } else { 0.0 },
            dof_focus_distance: config.dof_focus_distance,
            dof_focus_range: config.dof_focus_range.max(0.001),
            _pad2: 0.0,
            render_mode: if effects_on { config.render_mode } else { 0 },
            mode_mix: if effects_on { config.mode_mix } else { 0.0 },
            mode_time: time,
            _pad_mode: 0.0,
            mode_params: config.mode_params,
            grade_lift: config.grade_lift,
            grade_enabled: if effects_on && config.grade_active() {
                1.0
            } else {
                0.0
            },
            grade_gamma: config.grade_gamma,
            _pad3: 0.0,
            grade_gain: config.grade_gain,
            _pad4: 0.0,
            fog_tint_color: config.fog_tint_color,
            fog_tint_strength: config.fog_tint_strength.max(0.0),
            fog_tint_start: config.fog_tint_start,
            // smoothstep needs start < end
            fog_tint_end: config.fog_tint_end.max(config.fog_tint_start + 1e-3),
            fog_height_density: config.fog_height_density.max(0.0),
            _pad5: 0.0,
            ground_fog_color: config.ground_fog_color,
            ground_fog_density: config.ground_fog_density.max(0.0),
            ground_fog_origin: ground_fog.map_or([0.0; 2], |g| g.origin),
            ground_fog_size: ground_fog.map_or([1.0; 2], |g| g.size),
            ground_fog_floor_base: ground_fog.map_or(0.0, |g| g.floor_base),
            // Over the highest floor the fog has thinned to e^-7 by here
            ground_fog_ceiling: ground_fog.map_or(0.0, |g| g.floor_top + 7.0 / falloff),
            ground_fog_falloff: falloff,
            ground_fog_steps: config.ground_fog_steps.clamp(1, 64) as f32,
            ground_fog_texel: ground_fog.map_or(1.0, |g| g.texel),
            ground_fog_max_lod: ground_fog.map_or(0.0, |g| (g.mip_count - 1) as f32),
            ground_fog_enabled: if ground_fog.is_some() { 1.0 } else { 0.0 },
            _pad6: 0.0,
        };

        queue.write_buffer(
            &self.composite_uniform_buffer,
            0,
            bytemuck::cast_slice(&[uniforms]),
        );

        let uniform_bg = resources.binds.get("composite uniform", 0, || {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Composite Uniform BG"),
                layout: &self.composite_uniform_bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.composite_uniform_buffer.as_entire_binding(),
                }],
            })
        });

        // Kuwahara: use filtered result if enabled and allocated, raw HDR otherwise
        let kuwahara_view = (config.enabled && config.kuwahara_enabled)
            .then_some(resources.kuwahara.as_ref())
            .flatten()
            .map(|kt| &kt.output_view);
        let scene_view = kuwahara_view.unwrap_or(&resources.hdr_view);
        let scene_bg = resources.binds.get(
            "composite scene",
            u64::from(kuwahara_view.is_some()),
            || {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
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
                })
            },
        );

        // Use bloom mip[0] if available and bloom enabled, otherwise use black texture
        let bloom_on = config.bloom_enabled && !resources.bloom_mips.is_empty();
        let bloom_view = if bloom_on {
            &resources.bloom_mips[0].view
        } else {
            &self.black_texture_view
        };
        let bloom_bg = resources
            .binds
            .get("composite bloom", u64::from(bloom_on), || {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
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
                })
            });

        let timed = timer.map(|t| {
            let scope = t.begin(encoder, "Composite");
            (t, scope)
        });
        fullscreen_pass(
            encoder,
            "Composite Pass",
            target_view,
            Some(wgpu::Color::BLACK),
            &self.composite_pipeline,
            &[&uniform_bg, &scene_bg, &bloom_bg, &ssao_depth_bg],
        );
        if let Some((timer, scope)) = timed {
            timer.end(encoder, scope);
        }
    }
}

/// What the volumetric pass reads of the shadow pass and the lights.
pub struct VolumetricShadow<'a> {
    /// The shadow pass's [`gpu_id`](crate::bind_cache::gpu_id).
    pub id: u64,
    pub view: &'a wgpu::TextureView,
    pub sampler: &'a wgpu::Sampler,
    pub uniforms: &'a wgpu::Buffer,
    pub lights: &'a wgpu::Buffer,
}

/// Record one full-screen triangle into `target`, cleared to `clear` first
/// or blended over what's there, with `groups` bound in order.
fn fullscreen_pass(
    encoder: &mut wgpu::CommandEncoder,
    label: &str,
    target: &wgpu::TextureView,
    clear: Option<wgpu::Color>,
    pipeline: &wgpu::RenderPipeline,
    groups: &[&wgpu::BindGroup],
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            ops: wgpu::Operations {
                load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(pipeline);
    for (i, group) in groups.iter().enumerate() {
        pass.set_bind_group(i as u32, *group, &[]);
    }
    pass.draw(0..3, 0..1);
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
            // FXAA intermediate is allocated on demand (ADR 0050)
            fxaa: None,
            binds: Default::default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The Rust uniform struct must byte-match the WGSL PostProcessUniforms
    /// in composite_shader.wgsl. Layout: 12 scalars + texel_size vec2 +
    /// fog block (through dither_intensity) = 112 bytes, inv_view_proj mat4
    /// at 112..176, DoF row (strength/focus/range/pad) at 176..192, mode
    /// scalars (render_mode/mode_mix/mode_time/pad) at 192..208, mode_params
    /// vec4 at 208..224, grade rows (lift+enabled / gamma+pad / gain+pad)
    /// at 224..272 (ADR 0050; grain_intensity reused the former _pad @44),
    /// fog rows (tint color+strength / tint start+end+height density+pad)
    /// at 272..304 (ADR 0101), ground fog rows (color+density / origin+size
    /// / floor base, ceiling, falloff, steps / texel, max lod, enabled, pad)
    /// at 304..368 (ADR 0106).
    #[test]
    fn composite_uniforms_match_wgsl_layout() {
        assert_eq!(std::mem::size_of::<PostProcessUniforms>(), 368);
        assert_eq!(std::mem::size_of::<PostProcessUniforms>() % 16, 0);
    }

    /// Grade neutrality is a CPU decision (grade_enabled), never float-math
    /// identity: defaults must read as inactive.
    #[test]
    fn grade_defaults_are_inactive() {
        let config = PostProcessConfig::default();
        assert!(!config.grade_active());
        assert_eq!(config.film_grain, 0.0);
        let mut warm = config.clone();
        warm.grade_gain = [1.05, 1.0, 0.95];
        assert!(warm.grade_active());
    }

    /// `height_fog_depth` in composite_shader.wgsl, line for line.
    fn height_fog_depth(c: &PostProcessConfig, eye: [f32; 3], p: [f32; 3]) -> f32 {
        let dist = (0..3).map(|i| (p[i] - eye[i]).powi(2)).sum::<f32>().sqrt();
        let k = c.fog_height_falloff.max(1e-5);
        let at_eye = c.fog_height_density * (-k * (eye[1] - c.fog_height_origin)).min(30.0).exp();
        let x = (k * (p[1] - eye[1])).max(-30.0);
        let share = if x.abs() > 1e-3 {
            (1.0 - (-x).exp()) / x
        } else {
            1.0 - 0.5 * x
        };
        at_eye * dist * share
    }

    /// The closed form is the density integrated along the ray (ADR 0101):
    /// level, climbing, falling and close to level.
    #[test]
    fn height_fog_is_its_density_along_the_ray() {
        let c = PostProcessConfig {
            fog_height_density: 0.02,
            fog_height_falloff: 0.15,
            fog_height_origin: 1.0,
            ..PostProcessConfig::default()
        };
        let eye = [0.0, 6.0, 0.0];
        let ends: [[f32; 3]; 4] = [
            [300.0, 6.0, 0.0],
            [80.0, 40.0, 30.0],
            [50.0, -3.0, -20.0],
            [200.0, 6.004, 0.0],
        ];
        for p in ends {
            let steps = 20_000;
            let dist = (0..3).map(|i| (p[i] - eye[i]).powi(2)).sum::<f32>().sqrt();
            let density: f32 = (0..steps)
                .map(|s| {
                    let t = (s as f32 + 0.5) / steps as f32;
                    let y = eye[1] + (p[1] - eye[1]) * t;
                    (-c.fog_height_falloff * (y - c.fog_height_origin)).exp()
                })
                .sum::<f32>()
                / steps as f32;
            let numeric = c.fog_height_density * density * dist;
            let closed = height_fog_depth(&c, eye, p);
            assert!(
                (closed - numeric).abs() < 1e-3 * numeric,
                "{p:?}: {closed} vs {numeric}"
            );
        }
        // Thicker toward the origin: the same ray, low and high.
        let low = height_fog_depth(&c, [0.0, 1.5, 0.0], [100.0, 1.5, 0.0]);
        let high = height_fog_depth(&c, [0.0, 30.0, 0.0], [100.0, 30.0, 0.0]);
        assert!(low > 10.0 * high, "{low} vs {high}");
        // Off by default: a scene that only turned height fog on adds
        // nothing, and there's no tint.
        let off = PostProcessConfig {
            fog_height_falloff: 0.15,
            ..PostProcessConfig::default()
        };
        assert_eq!(height_fog_depth(&off, eye, [300.0, 0.0, 0.0]), 0.0);
        assert_eq!(off.fog_tint_strength, 0.0);
    }
}
