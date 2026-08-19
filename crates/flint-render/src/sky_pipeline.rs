//! Procedural sky pipeline — gradient, sun disc, stars, FBM clouds.
//!
//! Drawn as a fullscreen triangle at the far plane (same trick as the
//! texture skybox, which it replaces when a `sky` component is present).
//! All parameters live in one uniform block interpolated per frame by a
//! game-side time-of-day controller — sunrise blends to noon to starfield
//! continuously.

use bytemuck::{Pod, Zeroable};
use flint_core::toml_util::{toml_color, toml_f32};

/// Uniform block — layout must match `SkyUniforms` in sky_shader.wgsl.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct SkyUniformsGpu {
    pub inv_view_proj: [[f32; 4]; 4],
    pub sun_dir: [f32; 3],
    pub sun_disc_size: f32,
    pub sun_color: [f32; 4],
    pub zenith_color: [f32; 4],
    pub horizon_color: [f32; 4],
    pub haze_color: [f32; 4],
    pub cloud_tint: [f32; 4],
    pub sun_glow: f32,
    pub star_opacity: f32,
    pub cloud_coverage: f32,
    pub cloud_density: f32,
    pub cloud_scale: f32,
    pub cloud_drift_x: f32,
    pub cloud_drift_y: f32,
    pub time: f32,
}

/// CPU-side mirror of the `sky` component (minus sun, which is copied from
/// the scene's first directional light so disc and lighting always agree).
#[derive(Debug, Clone, PartialEq)]
pub struct SkyParams {
    pub zenith_color: [f32; 4],
    pub horizon_color: [f32; 4],
    pub haze_color: [f32; 4],
    pub cloud_tint: [f32; 4],
    pub sun_disc_size: f32,
    pub sun_glow: f32,
    pub star_opacity: f32,
    pub cloud_coverage: f32,
    pub cloud_density: f32,
    pub cloud_scale: f32,
    pub cloud_drift_x: f32,
    pub cloud_drift_y: f32,
    /// Optional hemisphere ambient override (rgb + intensity in alpha).
    pub ambient_sky: Option<[f32; 4]>,
    pub ambient_ground: Option<[f32; 4]>,
}

impl Default for SkyParams {
    fn default() -> Self {
        Self {
            zenith_color: [0.12, 0.34, 0.72, 1.0],
            horizon_color: [0.55, 0.76, 0.92, 1.0],
            haze_color: [0.85, 0.92, 0.98, 0.6],
            cloud_tint: [1.0, 1.0, 1.0, 0.85],
            sun_disc_size: 0.035,
            sun_glow: 1.0,
            star_opacity: 0.0,
            cloud_coverage: 0.35,
            cloud_density: 0.25,
            cloud_scale: 0.9,
            cloud_drift_x: 0.004,
            cloud_drift_y: 0.0015,
            ambient_sky: None,
            ambient_ground: None,
        }
    }
}

impl SkyParams {
    pub fn from_component(value: &toml::Value) -> Self {
        let d = Self::default();
        let f = |name: &str, dv: f32| value.get(name).and_then(toml_f32).unwrap_or(dv);
        let c = |name: &str, dv: [f32; 4]| value.get(name).and_then(toml_color).unwrap_or(dv);
        Self {
            zenith_color: c("zenith_color", d.zenith_color),
            horizon_color: c("horizon_color", d.horizon_color),
            haze_color: c("haze_color", d.haze_color),
            cloud_tint: c("cloud_tint", d.cloud_tint),
            sun_disc_size: f("sun_disc_size", d.sun_disc_size).clamp(0.001, 0.5),
            sun_glow: f("sun_glow", d.sun_glow).max(0.0),
            star_opacity: f("star_opacity", d.star_opacity).clamp(0.0, 1.0),
            cloud_coverage: f("cloud_coverage", d.cloud_coverage).clamp(0.0, 1.0),
            cloud_density: f("cloud_density", d.cloud_density).clamp(0.0, 1.0),
            cloud_scale: f("cloud_scale", d.cloud_scale).max(0.01),
            cloud_drift_x: f("cloud_drift_x", d.cloud_drift_x),
            cloud_drift_y: f("cloud_drift_y", d.cloud_drift_y),
            ambient_sky: value.get("ambient_sky").and_then(toml_color),
            ambient_ground: value.get("ambient_ground").and_then(toml_color),
        }
    }
}

/// The procedural sky pipeline.
pub struct SkyPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub uniform_bind_group_layout: wgpu::BindGroupLayout,
}

impl SkyPipeline {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Sky Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sky_shader.wgsl").into()),
        });

        let uniform_bind_group_layout =
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
                label: Some("Sky Uniform Bind Group Layout"),
            });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Sky Pipeline Layout"),
            bind_group_layouts: &[&uniform_bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Sky Render Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_sky"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_sky"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            uniform_bind_group_layout,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_struct_matches_wgsl_layout() {
        // mat4(64) + sun_dir+size(16) + 5 colors(80) + 8 f32(32) = 192
        assert_eq!(std::mem::size_of::<SkyUniformsGpu>(), 192);
        assert_eq!(192 % 16, 0);
    }
}
