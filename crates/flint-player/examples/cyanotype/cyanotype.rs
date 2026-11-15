//! The cyanotype style: written in this game, not in the engine (#419).
//!
//! Everything here is public engine API: the descriptor is a `static` built
//! with `StyleDescriptor::new` and its `const` setters (so a field the
//! engine adds later gets its default there and this file still builds,
//! #366), and the pass is drawn with `stylize::kit` and `StyleFrame`, as the
//! built-in styles are. Two passes: the light the paper received into a
//! target of the style's own, then the print read from it (cyanotype.wgsl).
//!
//! ```toml
//! [post_process.styles.cyanotype]
//! process = "photogram"   # or "print"
//! exposure = 1.2
//! bleed = 0.4
//! ```

use bytemuck::{Pod, Zeroable};
use flint_render::stylize::{
    kit, StyleDescriptor, StyleDevice, StyleFrame, StyleHint, StyleInput, StyleOption, StyleParam,
    StylePass, StyleStage,
};

/// The descriptor the game's extension hands the player.
pub static STYLE: StyleDescriptor =
    StyleDescriptor::new("cyanotype", "Cyanotype", StyleStage::Display, create)
        .params(PARAMS)
        .options(OPTIONS)
        .hints(HINTS);

const PARAMS: &[StyleParam] = &[
    StyleParam::new("exposure", 1.2, 0.25..=3.0),
    StyleParam::new("bleed", 0.4, 0.0..=1.0),
    StyleParam::new("coating", 0.07, 0.0..=0.2),
    StyleParam::new("grain", 0.5, 0.0..=1.0),
    StyleParam::new("strength", 1.0, 0.0..=1.0),
];

const OPTIONS: &[StyleOption] = &[StyleOption::new(
    "process",
    "photogram",
    &["photogram", "print"],
)];

const HINTS: &[StyleHint] = &[
    StyleHint::new(
        "process",
        "Process",
        "photogram: light turns the paper blue and what stands in it stays white; \
         print: from a negative, light stays white and shadow turns blue",
    ),
    StyleHint::new(
        "exposure",
        "Exposure",
        "Time in the sun: longer turns the same light a deeper blue",
    ),
    StyleHint::new(
        "bleed",
        "Bleed",
        "Light creeping under the edges, up to 9 points across",
    ),
    StyleHint::new(
        "coating",
        "Coating margin",
        "Where the brush stopped: bare paper at each edge, as a share of the height",
    ),
    StyleHint::new(
        "grain",
        "Paper grain",
        "Fibres and tooth showing through the blue",
    ),
    StyleHint::new("strength", "Strength", "0 = the picture, 1 = the print"),
];

/// The dose: sharp luminance in r, the spread under edges in g. Half floats
/// so a soft gradient survives to the print.
const DOSE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Uniforms {
    texel: [f32; 2],
    aspect: f32,
    srgb: f32,
    exposure: f32,
    bleed: f32,
    coating: f32,
    grain: f32,
    strength: f32,
    photogram: f32,
    ppp: f32,
    _pad: f32,
}

struct Cyanotype {
    dose: wgpu::RenderPipeline,
    print: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    sampled_bgl: wgpu::BindGroupLayout,
}

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    let device = d.device;
    let uniform_bgl = kit::uniform_layout(device, "Cyanotype Uniform BGL");
    let sampled_bgl = kit::sampled_layout(device, "Cyanotype Sampled BGL");
    let module = kit::shader(device, "Cyanotype Shader", include_str!("cyanotype.wgsl"));
    Box::new(Cyanotype {
        // The dose reads the input; the print reads the input and the dose
        dose: kit::fullscreen_pipeline_entry(
            device,
            "Cyanotype Dose Pipeline",
            &module,
            "fs_dose",
            &[&uniform_bgl, &sampled_bgl],
            DOSE_FORMAT,
        ),
        print: kit::fullscreen_pipeline_entry(
            device,
            "Cyanotype Print Pipeline",
            &module,
            "fs_print",
            &[&uniform_bgl, &sampled_bgl, &sampled_bgl],
            d.format,
        ),
        uniforms: kit::uniform_buffer::<Uniforms>(device, "Cyanotype Uniforms"),
        uniform_bgl,
        sampled_bgl,
    })
}

impl StylePass for Cyanotype {
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        let uniforms = Uniforms {
            texel: frame.texel_size(),
            aspect: frame.width as f32 / frame.height.max(1) as f32,
            srgb: if frame.srgb() { 1.0 } else { 0.0 },
            exposure: frame.param("exposure"),
            bleed: frame.param("bleed"),
            coating: frame.param("coating"),
            grain: frame.param("grain"),
            strength: frame.param("strength"),
            photogram: if frame.option("process") == "print" {
                0.0
            } else {
                1.0
            },
            ppp: frame.pixels_per_point.max(0.5),
            _pad: 0.0,
        };
        frame
            .queue
            .write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&[uniforms]));

        // The style's own target, at the output's size, kept by the
        // framework (rebuilt on resize along with its bind groups)
        let dose = frame.target("cyanotype.dose", DOSE_FORMAT);
        let params = frame.uniform("Cyanotype Uniform BG", &self.uniform_bgl, &self.uniforms);
        let input_bg = frame.sampled_input("Cyanotype Input BG", &self.sampled_bgl, input);
        kit::fullscreen_pass(
            encoder,
            "Cyanotype Dose",
            &dose.view,
            &self.dose,
            &[&params, &input_bg],
        );
        let dose_bg = frame.sampled("Cyanotype Dose BG", &self.sampled_bgl, &dose.view);
        kit::fullscreen_pass(
            encoder,
            "Cyanotype Print",
            output,
            &self.print,
            &[&params, &input_bg, &dose_bg],
        );
    }
}
