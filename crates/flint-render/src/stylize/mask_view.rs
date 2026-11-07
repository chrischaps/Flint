//! Mask view (#263): the entity style mask, made visible. Each style id
//! gets its own hue over the scene's shading, unstyled pixels fade to grey,
//! and a white line marks where two ids meet. `flint render --debug-mode
//! style` (and the F4 debug-shading combo's "Style Mask") switch it on;
//! a scene can too:
//!
//! ```toml
//! [post_process.styles.mask_view]
//! dim = 0.35
//! ```
//!
//! It is also the first style that reads the mask, so it doubles as the
//! example for one that does: `uses_mask: true`, and
//! [`StyleFrame::mask_bind`] for the bind group.

use super::kit;
use super::{
    StyleDescriptor, StyleFrame, StyleHint, StyleInput, StyleParam, StylePass, StyleStage,
};
use bytemuck::{Pod, Zeroable};

pub(super) static STYLE: StyleDescriptor = StyleDescriptor {
    name: "mask_view",
    label: "Mask view",
    stage: StyleStage::Display,
    params: &[
        StyleParam {
            name: "shading",
            default: 0.8,
            min: 0.0,
            max: 1.0,
        },
        StyleParam {
            name: "dim",
            default: 0.35,
            min: 0.0,
            max: 1.0,
        },
        StyleParam {
            name: "outline",
            default: 1.0,
            min: 0.0,
            max: 4.0,
        },
    ],
    options: &[],
    uses_mask: true,
    needs: &[],
    hints: &[
        StyleHint::new(
            "shading",
            "Shading",
            "How much of the scene's light shows under each style's hue",
        ),
        StyleHint::new(
            "dim",
            "Unstyled dim",
            "Brightness of the grey unstyled pixels fade to",
        ),
        StyleHint::new(
            "outline",
            "Outline (px)",
            "Width of the white line where two style ids meet",
        ),
    ],
    component: None,
    create: |d| {
        let device = d.device;
        let uniform_bgl = kit::uniform_layout(device, "Mask View Uniform BGL");
        let input_bgl = kit::sampled_layout(device, "Mask View Input BGL");
        let mask_bgl = kit::mask_layout(device, "Mask View Mask BGL");
        Box::new(MaskView {
            pipeline: kit::fullscreen_pipeline(
                device,
                "Mask View Pipeline",
                &kit::shader(device, "Mask View Shader", include_str!("mask_view.wgsl")),
                &[&uniform_bgl, &input_bgl, &mask_bgl],
                d.format,
            ),
            uniforms: kit::uniform_buffer::<MaskViewUniforms>(device, "Mask View Uniforms"),
            uniform_bgl,
            input_bgl,
            mask_bgl,
        })
    },
};

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct MaskViewUniforms {
    shading: f32,
    dim: f32,
    outline: f32,
    srgb: f32,
}

struct MaskView {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    mask_bgl: wgpu::BindGroupLayout,
}

impl StylePass for MaskView {
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        // The framework draws the mask whenever a `uses_mask` style is on
        let Some(mask) = frame.mask_bind("Mask View Mask BG", &self.mask_bgl) else {
            return;
        };
        let uniforms = MaskViewUniforms {
            shading: frame.param("shading").clamp(0.0, 1.0),
            dim: frame.param("dim").clamp(0.0, 1.0),
            outline: frame.param("outline").clamp(0.0, 4.0),
            srgb: if frame.srgb() { 1.0 } else { 0.0 },
        };
        frame
            .queue
            .write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&[uniforms]));
        kit::fullscreen_pass(
            encoder,
            "Mask View Pass",
            output,
            &self.pipeline,
            &[
                &frame.uniform("Mask View Uniform BG", &self.uniform_bgl, &self.uniforms),
                &frame.sampled_input("Mask View Input BG", &self.input_bgl, input),
                &mask,
            ],
        );
    }
}
