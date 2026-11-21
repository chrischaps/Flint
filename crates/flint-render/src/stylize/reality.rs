//! The reality styles (#746): `matrix`, `blood`, `drunk`, `tron` and
//! `underwater`, the looks that were render modes 1-5 of the composite.
//!
//! Each was one game's look (Float's reality tears and its submerged eye)
//! wired into every game's composite pass, its tuning four positional
//! numbers whose meaning changed per mode. Each is now a display style
//! with named, ranged params, so F4, `--style`, `--sweep`, `--list-styles`
//! and `set_post_style_param` reach them as they reach every other style,
//! and a game that never asks for them never builds them.
//!
//! They share this pass: one full-screen draw of a style's WGSL (after
//! `tear_mask.wgsl`'s noise and `reality.wgsl`'s prelude) over the input,
//! the scene's depth and the camera, with a style's numbers in four vec4s.
//! The composite drew them between the tone map and the vignette; a
//! display style runs after the vignette, so each draws its own look
//! through the frame's vignette factor (`vignetted`), and reads the
//! picture under it with the vignette undone (`unvignetted`), which keeps
//! the look where the composite put it. They tear like any style: a table's
//! `blend_source = "tear"` shows one only inside `[post_process] tear`.

use super::kit;
use super::{StyleDevice, StyleFrame, StyleInput, StylePass};
use crate::gpu_counts::{CountedDevice, CountedQueue};
use bytemuck::{Pod, Zeroable};

/// Where the reality styles stand in the display chain (their
/// `default_order`): before watercolor, the first display style registered
/// (index 1), as the composite drew them before every display style. Ties
/// keep registration order (matrix, blood, drunk, tron, underwater).
pub(super) const ORDER: f32 = 0.5;

/// A reality style's uniform, laid out as `Reality` in reality.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct RealityUniforms {
    pub inv_view_proj: [[f32; 4]; 4],
    pub camera_pos: [f32; 3],
    /// The post time.
    pub time: f32,
    pub texel_size: [f32; 2],
    pub near: f32,
    pub far: f32,
    /// 1 on an sRGB target (the sample is linear), the frame's vignette
    /// intensity and smoothness as the composite drew it, width / height.
    pub srgb: f32,
    pub vignette: f32,
    pub vignette_smoothness: f32,
    pub aspect: f32,
    /// The style's own numbers.
    pub p: [[f32; 4]; 4],
}

/// The style's numbers for this frame, from its params and the frame.
pub(super) type Fill = fn(&StyleFrame<'_>) -> [[f32; 4]; 4];

pub(super) struct RealityPass {
    pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    uniform_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    depth_bgl: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    depth_sampler: wgpu::Sampler,
    label: &'static str,
    fill: Fill,
}

/// Build a reality style's pass from its WGSL file (`kit::wgsl!` names it,
/// after the shared noise and prelude).
pub(super) fn create(
    d: &StyleDevice,
    label: &'static str,
    source: kit::Wgsl,
    fill: Fill,
) -> Box<dyn StylePass> {
    let device = d.device;
    let uniform_bgl = kit::uniform_layout(device, "Reality Uniform BGL");
    let input_bgl = kit::sampled_layout(device, "Reality Input BGL");
    let depth_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Reality Depth BGL"),
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
    let pipeline = kit::fullscreen_pipeline(
        device,
        label,
        &kit::shader(device, label, source),
        &[&uniform_bgl, &input_bgl, &depth_bgl],
        d.format,
    );
    let depth_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("Reality Depth Sampler"),
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        ..Default::default()
    });
    Box::new(RealityPass {
        pipeline,
        uniforms: kit::uniform_buffer::<RealityUniforms>(device, "Reality Uniforms"),
        uniform_bgl,
        input_bgl,
        depth_bgl,
        depth_sampler,
        label,
        fill,
    })
}

/// The frame's part of the uniform, with the style's numbers.
pub(super) fn uniforms(frame: &StyleFrame<'_>, p: [[f32; 4]; 4]) -> RealityUniforms {
    let config = frame.config;
    RealityUniforms {
        inv_view_proj: frame.camera.inv_view_proj,
        camera_pos: frame.camera.position,
        time: frame.time,
        texel_size: frame.texel_size(),
        near: frame.camera.near,
        far: frame.camera.far,
        srgb: if frame.srgb() { 1.0 } else { 0.0 },
        // As the composite's uniform has it
        vignette: if config.enabled && config.vignette_enabled {
            config.vignette_intensity
        } else {
            0.0
        },
        vignette_smoothness: config.vignette_smoothness,
        aspect: frame.width.max(1) as f32 / frame.height.max(1) as f32,
        p,
    }
}

impl StylePass for RealityPass {
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        let u = uniforms(frame, (self.fill)(frame));
        frame
            .queue
            .write_buffer_counted(&self.uniforms, 0, bytemuck::bytes_of(&u));
        let depth = frame.depth_bind("Reality Depth BG", 0, || {
            frame
                .device
                .create_bind_group_counted(&wgpu::BindGroupDescriptor {
                    label: Some("Reality Depth BG"),
                    layout: &self.depth_bgl,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(frame.depth),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.depth_sampler),
                        },
                    ],
                })
        });
        kit::fullscreen_pass(
            encoder,
            self.label,
            output,
            &self.pipeline,
            &[
                &frame.uniform("Reality Uniform BG", &self.uniform_bgl, &self.uniforms),
                &frame.sampled_input("Reality Input BG", &self.input_bgl, input),
                &depth,
            ],
        );
    }
}

/// A colour param's three channels and a fourth number, as one vec4.
pub(super) fn row(rgb: [f32; 3], w: f32) -> [f32; 4] {
    [rgb[0], rgb[1], rgb[2], w]
}

/// The style's whole-frame blend, for the two that draw it themselves
/// (`drunk`, `underwater`: their sway grows with it, which a mix of the
/// input back can't do).
pub(super) fn own_blend(frame: &StyleFrame<'_>) -> f32 {
    frame.config.styles.blend(frame.descriptor.name)
}

/// `draws_blend` for the styles that draw their own: always.
pub(super) fn always(
    _: &crate::postprocess::PostProcessConfig,
    _: Option<&crate::style_mask::StyleVariant>,
) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// mat4, camera + time, texel + near/far, the flags, four vec4s.
    #[test]
    fn uniforms_match_wgsl_layout() {
        assert_eq!(std::mem::size_of::<RealityUniforms>(), 176);
        let source = format!(
            "{}{}{}fn _unused() {{}}",
            include_str!("prelude_core.wgsl"),
            include_str!("../tear_mask.wgsl"),
            include_str!("reality.wgsl")
        );
        let module = naga::front::wgsl::parse_str(&source).unwrap();
        let span = module
            .types
            .iter()
            .find_map(|(_, t)| match &t.inner {
                naga::TypeInner::Struct { span, .. } if t.name.as_deref() == Some("Reality") => {
                    Some(*span)
                }
                _ => None,
            })
            .expect("Reality in the prelude");
        assert_eq!(span as usize, std::mem::size_of::<RealityUniforms>());
    }

    /// Every reality style's shader parses and validates with the prelude.
    #[test]
    fn every_reality_shader_validates() {
        for (name, body) in [
            ("matrix", include_str!("matrix.wgsl")),
            ("blood", include_str!("blood.wgsl")),
            ("drunk", include_str!("drunk.wgsl")),
            ("tron", include_str!("tron.wgsl")),
            ("underwater", include_str!("underwater.wgsl")),
        ] {
            let source = format!(
                "{}{}{}{}",
                include_str!("prelude_core.wgsl"),
                include_str!("../tear_mask.wgsl"),
                include_str!("reality.wgsl"),
                body
            );
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&source)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
    }
}
