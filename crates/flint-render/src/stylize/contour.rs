//! The contour style (#214, a registered style since #599): ink lines,
//! found in screen space, that can boil.
//!
//! One full-screen draw at the HDR stage, reading depth and whatever the
//! scene colour is by then, so it inks over the Kuwahara paint and never
//! smears it: registered after every other built-in, it is the last HDR
//! style unless a table's `order` moves it. Its cost doesn't grow with the
//! scene, and nothing is allocated, compiled or run until a scene turns it
//! on. Settings are `[post_process.styles.contour]` (`width`, `threshold`,
//! `strength`, `luma`, `color`, `fade`, `boil`); a scene's older
//! `contour_enabled`, `contour_width`... keys fold into that table when it
//! loads (`flint_scene::fold_legacy_style_keys`).
//!
//! Lines come from breaks in depth (the second difference of 1 / distance,
//! so flat ground draws none) and in luminance. In a scene with a `[wooze]`
//! field, `boil` re-decides on each drawing of the field where a line runs,
//! how wide it is and how hard the pen pressed; at 0 none of that runs.

use super::kit;
use super::{
    StyleDescriptor, StyleDevice, StyleFrame, StyleHint, StyleInput, StyleParam, StylePass,
    StyleStage,
};
use crate::postprocess::{PostProcessConfig, HDR_FORMAT};
use bytemuck::{Pod, Zeroable};

pub(super) static STYLE: StyleDescriptor = StyleDescriptor::new(
    "contour",
    "Contour",
    StyleStage::Hdr,
    create,
)
.params(&[
    StyleParam::new("width", 1.25, 0.25..=5.0),
    StyleParam::new("threshold", 0.08, 0.01..=0.5),
    StyleParam::new("strength", 0.9, 0.0..=1.0),
    StyleParam::new("luma", 0.5, 0.0..=2.0),
    StyleParam::new("color_r", 0.015, 0.0..=1.0),
    StyleParam::new("color_g", 0.012, 0.0..=1.0),
    StyleParam::new("color_b", 0.02, 0.0..=1.0),
    StyleParam::new("fade", 30.0, 0.0..=200.0),
    StyleParam::new("boil", 0.0, 0.0..=2.0),
])
.hints(&[
    StyleHint::new(
        "width",
        "Width (px)",
        "Half a line's width, in pixels at 1080p; scaled with the frame",
    ),
    StyleHint::new(
        "threshold",
        "Threshold",
        "How strong an edge must be before it inks; lower draws more",
    ),
    StyleHint::new("strength", "Strength", "How dark the ink goes"),
    StyleHint::new(
        "luma",
        "Luma",
        "How much edges in luminance count against edges in depth; 0 is silhouettes only",
    ),
    StyleHint::new(
        "color",
        "Ink",
        "The ink's colour, as the HDR scene holds it (linear)",
    ),
    StyleHint::new(
        "fade",
        "Fade (m)",
        "Metres at which a line has faded by half, so distance doesn't go solid; 0 never fades",
    ),
    StyleHint::new(
        "boil",
        "Boil",
        "How far each drawing of the wooze field moves the lines; needs a [wooze] table",
    ),
]);

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    Box::new(Contour::new(d))
}

/// Uniform data for the contour pass. Matches `contour.wgsl`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub(crate) struct ContourUniforms {
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
    /// This frame's uniforms for a target of the given size. `width` is
    /// authored at 1080p and scaled, so a line is the same share of the
    /// frame at any size.
    pub fn new(config: &PostProcessConfig, width: u32, height: u32, near: f32, far: f32) -> Self {
        let param = |name| STYLE.param(config, name);
        Self {
            texel_size: [1.0 / width.max(1) as f32, 1.0 / height.max(1) as f32],
            // Depth is read by the nearest texel, so a reach under a pixel sees nothing.
            width: (param("width") * height as f32 / 1080.0).max(0.75),
            threshold: param("threshold").max(1e-4),
            color: STYLE.color(config, "color"),
            strength: param("strength"),
            near,
            far,
            luma: param("luma").max(0.0),
            // Scaled by this frame's drawing from the wooze field (0 with no field)
            boil: param("boil").max(0.0) * config.boil.amount,
            seed: config.boil.seed,
            fade: param("fade").max(0.0),
            _pad: [0; 2],
        }
    }
}

struct Contour {
    pipeline: wgpu::RenderPipeline,
    uniform_bgl: wgpu::BindGroupLayout,
    scene_bgl: wgpu::BindGroupLayout,
    depth_bgl: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    /// Depth is read by the nearest texel. One kept bind group holds it,
    /// bound once a frame, so the DX12 sampler heap is safe.
    depth_sampler: wgpu::Sampler,
}

impl Contour {
    fn new(d: &StyleDevice) -> Self {
        let device = d.device;
        let uniform_bgl = kit::uniform_layout(device, "Contour Uniform BGL");
        let scene_bgl = kit::sampled_layout(device, "Contour Scene BGL");
        let depth_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Contour Depth BGL"),
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
            "Contour Pipeline",
            &kit::shader(device, "Contour Shader", kit::wgsl!("contour.wgsl")),
            &[&uniform_bgl, &scene_bgl, &depth_bgl],
            HDR_FORMAT,
        );
        let depth_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Contour Depth Sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        Self {
            uniforms: kit::uniform_buffer::<ContourUniforms>(device, "Contour Uniform Buffer"),
            pipeline,
            uniform_bgl,
            scene_bgl,
            depth_bgl,
            depth_sampler,
        }
    }
}

impl StylePass for Contour {
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        let uniforms = ContourUniforms::new(
            frame.config,
            frame.width,
            frame.height,
            frame.camera.near,
            frame.camera.far,
        );
        frame
            .queue
            .write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&[uniforms]));
        let depth = frame.depth_bind("Contour Depth BG", 0, || {
            frame.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Contour Depth BG"),
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
            "Contour Pass",
            output,
            &self.pipeline,
            &[
                &frame.uniform("Contour Uniform BG", &self.uniform_bgl, &self.uniforms),
                &frame.sampled_input("Contour Scene BG", &self.scene_bgl, input),
                &depth,
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// texel/width/threshold, colour/strength, near/far/luma/boil, seed/fade/pad.
    #[test]
    fn uniforms_match_wgsl_layout() {
        assert_eq!(std::mem::size_of::<ContourUniforms>(), 64);
    }

    /// A width is authored at 1080p, and the field's jitter scales the boil.
    #[test]
    fn uniforms_scale_with_the_frame_and_the_field() {
        let mut config = PostProcessConfig::default();
        crate::post_style::set_post_style_param(&mut config, "contour", "width", 2.0.into())
            .unwrap();
        crate::post_style::set_post_style_param(&mut config, "contour", "boil", 0.5.into())
            .unwrap();
        config.boil.amount = 1.5;
        config.boil.seed = 9;
        let u = ContourUniforms::new(&config, 1920, 540, 0.1, 100.0);
        assert_eq!(u.width, 1.0);
        assert_eq!(u.boil, 0.75);
        assert_eq!(u.seed, 9);
        assert_eq!(u.color, [0.015, 0.012, 0.02]);
        config.boil.amount = 0.0;
        let still = ContourUniforms::new(&config, 1920, 540, 0.1, 100.0);
        assert_eq!(still.boil, 0.0);
    }

    /// Off unless a table turns it on, as `contour_enabled` was.
    #[test]
    fn off_by_default() {
        assert!(!STYLE.is_enabled(&PostProcessConfig::default()));
    }
}
