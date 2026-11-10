//! The anisotropic Kuwahara filter (Kyprianidis et al.), the first style on
//! the framework: three passes, structure tensor -> tensor blur -> filter,
//! in HDR before fog, bloom and tonemapping. Ported unchanged from
//! `postprocess.rs` (#262): same shaders, same targets, same clears, so its
//! output is byte-identical. Its settings are `[post_process.styles.kuwahara]`
//! like every other style's (#298); a scene's older `kuwahara_enabled`,
//! `kuwahara_radius`... keys are folded into that table when it loads
//! (`flint_scene::fold_legacy_style_keys`). In a scene with a `[wooze]`
//! field the paint also boils (#214): `boil` moves the kernel and turns the
//! strokes a little on each drawing of the field (the flat
//! `[post_process] kuwahara_boil` before #599, folded in the same way).

use super::kit;
use super::{
    StyleDescriptor, StyleDevice, StyleFrame, StyleHint, StyleInput, StyleParam, StylePass,
    StyleStage,
};
use crate::postprocess::HDR_FORMAT;
use bytemuck::{Pod, Zeroable};

pub(super) static STYLE: StyleDescriptor = StyleDescriptor::new(
    "kuwahara",
    "Kuwahara",
    StyleStage::Hdr,
    create,
)
.params(&[
    StyleParam::new("radius", 4.0, 1.0..=8.0),
    StyleParam::new("sharpness", 8.0, 1.0..=16.0),
    StyleParam::new("hardness", 8.0, 1.0..=16.0),
    StyleParam::new("anisotropy", 1.0, 0.0..=2.0),
    StyleParam::new("boil", 0.0, 0.0..=1.0),
])
.hints(&[
    StyleHint::new(
        "radius",
        "Radius (px)",
        "The brush: each sector's reach in scene pixels; bigger is more painterly and costs more",
    )
    .integer(),
    StyleHint::new(
        "sharpness",
        "Sharpness",
        "How strongly the least varied sector wins; lower blends sectors more softly",
    ),
    StyleHint::new(
        "hardness",
        "Hardness",
        "How sharply sectors fall off; lower gives broader, smoother strokes",
    ),
    StyleHint::new(
        "anisotropy",
        "Anisotropy",
        "How far strokes stretch along the image's edges; 0 is round",
    ),
    StyleHint::new(
        "boil",
        "Boil",
        "How far each drawing of the wooze field moves the paint, as a share of the radius; needs a [wooze] table",
    ),
]);

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    Box::new(Kuwahara::new(d))
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
    /// Which drawing this is (#214): changes on the wooze field's beat.
    boil_seed: u32,
    /// How far the paint moves this frame, as a share of the radius. 0 is off.
    boil: f32,
}

#[cfg(test)]
mod tests {
    /// The boil's seed and amount (#214) ride the two floats that padded the
    /// filter's uniforms, so its buffer is the size it always was.
    #[test]
    fn boil_rides_the_old_padding() {
        assert_eq!(std::mem::size_of::<super::KuwaharaUniforms>(), 32);
    }
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
            // The shading boil (#214): `boil`, scaled by this frame's
            // drawing from the wooze field (0 with no field)
            boil_seed: frame.config.boil.seed,
            boil: frame.param("boil").max(0.0) * frame.config.boil.amount,
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
