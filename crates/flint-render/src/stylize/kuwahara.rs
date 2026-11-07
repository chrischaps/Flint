//! The anisotropic Kuwahara filter (Kyprianidis et al.), the first style on
//! the framework: three passes, structure tensor -> tensor blur -> filter,
//! in HDR before fog, bloom and tonemapping. Ported unchanged from
//! `postprocess.rs` (#262): same shaders, same targets, same clears, so its
//! output is byte-identical. Its settings are `[post_process.styles.kuwahara]`
//! like every other style's (#298); a scene's older `kuwahara_enabled`,
//! `kuwahara_radius`... keys are folded into that table when it loads
//! (`flint_scene::fold_legacy_style_keys`).

use super::kit;
use super::{
    StyleDescriptor, StyleDevice, StyleFrame, StyleHint, StyleInput, StyleParam, StylePass,
    StyleStage,
};
use crate::postprocess::HDR_FORMAT;
use bytemuck::{Pod, Zeroable};

pub(super) static STYLE: StyleDescriptor = StyleDescriptor {
    name: "kuwahara",
    label: "Kuwahara",
    stage: StyleStage::Hdr,
    params: &[
        param("radius", 4.0, 1.0, 8.0),
        param("sharpness", 8.0, 1.0, 16.0),
        param("hardness", 8.0, 1.0, 16.0),
        param("anisotropy", 1.0, 0.0, 2.0),
    ],
    options: &[],
    uses_mask: false,
    hints: &[
        StyleHint::new("radius", "Radius (px)", "The brush: each sector's reach in scene pixels; bigger is more painterly and costs more").integer(),
        StyleHint::new("sharpness", "Sharpness", "How strongly the least varied sector wins; lower blends sectors more softly"),
        StyleHint::new("hardness", "Hardness", "How sharply sectors fall off; lower gives broader, smoother strokes"),
        StyleHint::new("anisotropy", "Anisotropy", "How far strokes stretch along the image's edges; 0 is round"),
    ],
    component: None,
    create: |d| Box::new(Kuwahara::new(d)),
};

const fn param(name: &'static str, default: f32, min: f32, max: f32) -> StyleParam {
    StyleParam {
        name,
        default,
        min,
        max,
    }
}

/// The tensor passes' uniforms.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct TexelUniforms {
    texel_size: [f32; 2],
    _pad: [f32; 2],
}

/// The filter pass's uniforms.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct KuwaharaUniforms {
    texel_size: [f32; 2],
    radius: f32,
    sharpness: f32,
    hardness: f32,
    anisotropy: f32,
    _pad: [f32; 2],
}

struct Kuwahara {
    uniform_bgl: wgpu::BindGroupLayout,
    sampled_bgl: wgpu::BindGroupLayout,
    tensor_pipeline: wgpu::RenderPipeline,
    tensor_uniforms: wgpu::Buffer,
    blur_pipeline: wgpu::RenderPipeline,
    blur_uniforms: wgpu::Buffer,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
}

impl Kuwahara {
    fn new(d: &StyleDevice) -> Self {
        let device = d.device;
        let uniform_bgl = kit::uniform_layout(device, "Kuwahara Uniform BGL");
        let sampled_bgl = kit::sampled_layout(device, "Kuwahara Texture BGL");
        let layouts = [&uniform_bgl, &sampled_bgl];
        // The structure tensor is an intermediate, always HDR
        let tensor_pipeline = kit::fullscreen_pipeline(
            device,
            "Kuwahara Tensor Pipeline",
            &kit::shader(
                device,
                "Kuwahara Tensor Shader",
                include_str!("kuwahara_tensor.wgsl"),
            ),
            &layouts,
            HDR_FORMAT,
        );
        let blur_pipeline = kit::fullscreen_pipeline(
            device,
            "Kuwahara Tensor Blur Pipeline",
            &kit::shader(
                device,
                "Kuwahara Tensor Blur Shader",
                include_str!("kuwahara_tensor_blur.wgsl"),
            ),
            &layouts,
            HDR_FORMAT,
        );
        let pipeline = kit::fullscreen_pipeline(
            device,
            "Kuwahara Pipeline",
            &kit::shader(device, "Kuwahara Shader", include_str!("kuwahara.wgsl")),
            &[&uniform_bgl, &sampled_bgl, &sampled_bgl],
            d.format,
        );
        Self {
            tensor_uniforms: kit::uniform_buffer::<TexelUniforms>(
                device,
                "Kuwahara Tensor Uniform Buffer",
            ),
            blur_uniforms: kit::uniform_buffer::<TexelUniforms>(
                device,
                "Kuwahara Tensor Blur Uniform Buffer",
            ),
            uniforms: kit::uniform_buffer::<KuwaharaUniforms>(device, "Kuwahara Uniform Buffer"),
            uniform_bgl,
            sampled_bgl,
            tensor_pipeline,
            blur_pipeline,
            pipeline,
        }
    }
}

impl StylePass for Kuwahara {
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        let texel_size = frame.texel_size();
        if frame.first_write("kuwahara tensors") {
            let texel = TexelUniforms {
                texel_size,
                _pad: [0.0; 2],
            };
            let bytes = bytemuck::cast_slice(std::slice::from_ref(&texel));
            frame.queue.write_buffer(&self.tensor_uniforms, 0, bytes);
            frame.queue.write_buffer(&self.blur_uniforms, 0, bytes);
        }
        let tensor = frame.target("kuwahara tensor", HDR_FORMAT);
        let tensor_blur = frame.target("kuwahara tensor blur", HDR_FORMAT);

        // Pass 1: the structure tensor of the input
        kit::fullscreen_pass(
            encoder,
            "Kuwahara Tensor Pass",
            &tensor.view,
            &self.tensor_pipeline,
            &[
                &frame.uniform(
                    "Kuwahara Tensor Uniform BG",
                    &self.uniform_bgl,
                    &self.tensor_uniforms,
                ),
                &frame.sampled_input("Kuwahara Tensor Texture BG", &self.sampled_bgl, input),
            ],
        );

        // Pass 2: blur it
        kit::fullscreen_pass(
            encoder,
            "Kuwahara Tensor Blur Pass",
            &tensor_blur.view,
            &self.blur_pipeline,
            &[
                &frame.uniform(
                    "Kuwahara Tensor Blur Uniform BG",
                    &self.uniform_bgl,
                    &self.blur_uniforms,
                ),
                &frame.sampled(
                    "Kuwahara Tensor Blur Texture BG",
                    &self.sampled_bgl,
                    &tensor.view,
                ),
            ],
        );

        // Pass 3: the filter, oriented by the blurred tensor
        let uniforms = KuwaharaUniforms {
            texel_size,
            // Whole pixels, as the old `kuwahara_radius: u32` was
            radius: frame.param("radius").round().max(1.0),
            sharpness: frame.param("sharpness"),
            hardness: frame.param("hardness"),
            anisotropy: frame.param("anisotropy"),
            _pad: [0.0; 2],
        };
        frame
            .queue
            .write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&[uniforms]));
        kit::fullscreen_pass(
            encoder,
            "Kuwahara Pass",
            output,
            &self.pipeline,
            &[
                &frame.uniform("Kuwahara Uniform BG", &self.uniform_bgl, &self.uniforms),
                &frame.sampled_input("Kuwahara HDR BG", &self.sampled_bgl, input),
                &frame.sampled(
                    "Kuwahara Tensor Input BG",
                    &self.sampled_bgl,
                    &tensor_blur.view,
                ),
            ],
        );
    }
}
