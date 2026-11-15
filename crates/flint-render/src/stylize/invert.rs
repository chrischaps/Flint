//! Invert: the smallest style, and the template for a new one. It turns
//! the finished picture into its negative, `strength` of the way.
//!
//! ```toml
//! [post_process.styles.invert]
//! strength = 1.0
//! ```
//!
//! Everything it needed from outside this file was its name in
//! `register_styles!`.

use super::kit;
use super::{
    StyleDescriptor, StyleDevice, StyleFrame, StyleHint, StyleInput, StyleParam, StylePass,
    StyleStage,
};
use bytemuck::{Pod, Zeroable};

pub(super) static STYLE: StyleDescriptor =
    StyleDescriptor::new("invert", "Invert", StyleStage::Display, create)
        .params(&[StyleParam::new("strength", 1.0, 0.0..=1.0)])
        .hints(&[StyleHint::new(
            "strength",
            "Strength",
            "How far toward the negative: 0 = none, 1 = full",
        )]);

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    let device = d.device;
    let uniform_bgl = kit::uniform_layout(device, "Invert Uniform BGL");
    let input_bgl = kit::sampled_layout(device, "Invert Input BGL");
    Box::new(Invert {
        pipeline: kit::fullscreen_pipeline(
            device,
            "Invert Pipeline",
            &kit::shader(device, "Invert Shader", kit::wgsl!("invert.wgsl")),
            &[&uniform_bgl, &input_bgl],
            d.format,
        ),
        uniforms: kit::uniform_buffer::<InvertUniforms>(device, "Invert Uniforms"),
        uniform_bgl,
        input_bgl,
    })
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct InvertUniforms {
    strength: f32,
    /// 1 when the target is sRGB: invert display values, not linear ones
    srgb: f32,
    _pad: [f32; 2],
}

struct Invert {
    pipeline: std::sync::Arc<wgpu::RenderPipeline>,
    uniforms: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
}

impl StylePass for Invert {
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        let uniforms = InvertUniforms {
            strength: frame.param("strength").clamp(0.0, 1.0),
            srgb: if frame.srgb() { 1.0 } else { 0.0 },
            _pad: [0.0; 2],
        };
        frame
            .queue
            .write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&[uniforms]));
        kit::fullscreen_pass(
            encoder,
            "Invert Pass",
            output,
            &self.pipeline,
            &[
                &frame.uniform("Invert Uniform BG", &self.uniform_bgl, &self.uniforms),
                &frame.sampled_input("Invert Input BG", &self.input_bgl, input),
            ],
        );
    }
}
