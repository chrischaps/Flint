//! Palette quantize (#324): the whole frame in a palette's colours, no
//! glyphs. CGA's four colours, a phosphor tube's greens, cream paper and
//! its inks, or a game's own house palette.
//!
//! ```toml
//! [post_process.styles.palette]
//! palette = "cga"   # built-in, palettes/<name>.palette.toml, or a path
//! dither = 0.5      # 0 = flat bands, 1 = an ordered 4x4 dither between inks
//! pixel = 4         # block size in points: 1 = per pixel, more = chunky
//! ```
//!
//! It reads the same [`Palette`](crate::palette::Palette) files the ASCII
//! style does and picks with the same matcher, `pal_pick` in
//! `palette.wgsl` ([`PALETTE_WGSL`](crate::palette::PALETTE_WGSL)), so a palette means one thing
//! everywhere. One difference is deliberate: ASCII's glyph density carries
//! a cell's brightness, so its ground is never an ink, but here there is
//! nothing else to carry it, so the ground joins the inks
//! ([`Palette::table`](crate::palette::Palette::table)): CGA's black, or
//! paper's cream as the lightest rung.

use super::kit;
use super::{
    StyleDescriptor, StyleDevice, StyleFrame, StyleHint, StyleInput, StyleOption, StyleParam,
    StylePass, StyleStage,
};
use crate::palette::{oklab, Palette, PaletteCache, BUILTIN_PALETTES, MAX_COLORS};
use bytemuck::{Pod, Zeroable};
use std::sync::{Arc, Mutex};

/// The palette the style draws in when the scene names none.
pub const DEFAULT_PALETTE: &str = "cga";

/// Slots in the table: every ink and the ground.
pub const TABLE_SLOTS: usize = MAX_COLORS + 1;

const PARAMS: &[StyleParam] = &[
    StyleParam::new("dither", 0.0, 0.0..=1.0),
    StyleParam::new("pixel", 1.0, 1.0..=32.0),
    StyleParam::new("chroma", 1.0, 0.0..=4.0),
];

const OPTIONS: &[StyleOption] = &[
    // The scene's own palettes too, as ASCII offers them (#322)
    StyleOption::new("palette", DEFAULT_PALETTE, BUILTIN_PALETTES)
        .discovered(super::ascii::palette_choices),
];

const HINTS: &[StyleHint] = &[
    StyleHint::new(
        "palette",
        "Palette",
        "A built-in, or palettes/<name>.palette.toml beside the scene",
    ),
    StyleHint::new(
        "dither",
        "Dither",
        "Ordered 4x4 dither between the two nearest colours (a ladder: half a rung either way), fixed to the blocks: 0 = flat bands",
    ),
    StyleHint::new(
        "pixel",
        "Pixel size",
        "Block size in points: 1 = every pixel its own colour, more = a chunky low-resolution screen",
    )
    .integer(),
    StyleHint::new(
        "chroma",
        "Chroma",
        "For a nearest palette: scales each block's saturation (OKLab a/b) before matching, so a muted scene reaches the palette's colours; 1 = as it is",
    ),
];

pub(super) static STYLE: StyleDescriptor =
    StyleDescriptor::new("palette", "Palette", StyleStage::Display, create)
        .params(PARAMS)
        .options(OPTIONS)
        .hints(HINTS);

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    let device = d.device;
    let uniform_bgl = kit::uniform_layout(device, "Palette Uniform BGL");
    let input_bgl = kit::sampled_layout(device, "Palette Input BGL");
    Box::new(PaletteStyle {
        pipeline: kit::fullscreen_pipeline(
            device,
            "Palette Pipeline",
            &kit::shader(device, "Palette Shader", SHADER),
            &[&uniform_bgl, &input_bgl],
            d.format,
        ),
        uniforms: kit::uniform_buffer::<PaletteUniforms>(device, "Palette Uniforms"),
        uniform_bgl,
        input_bgl,
        palettes: Mutex::new(PaletteCache::default()),
    })
}

/// The style's shader: its own code, then the shared matcher.
pub const SHADER: &str = concat!(
    include_str!("palette.wgsl"),
    include_str!("../palette.wgsl")
);

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct PaletteUniforms {
    len: u32,
    matching: u32,
    /// 1 when the target is sRGB: samples come back linear
    srgb: u32,
    /// Block size in pixels
    block: u32,
    dither: f32,
    /// OKLab a/b scale before a nearest match, as ASCII's `chroma`
    chroma: f32,
    _pad: [f32; 2],
    /// The table's first [`MAX_COLORS`] slots, then its last: one
    /// `array<vec4<f32>, 33>` in the shader (bytemuck has no 33-arrays)
    inks: [[f32; 4]; MAX_COLORS],
    last: [f32; 4],
    /// The same table in OKLab, for `palette_ink_lab`
    lab: [[f32; 4]; MAX_COLORS],
    lab_last: [f32; 4],
}

/// `colors` as 33 uniform slots: the first 32, then the last.
fn slots(colors: &[[f32; 3]]) -> ([[f32; 4]; MAX_COLORS], [f32; 4]) {
    let mut all = [[0.0; 4]; TABLE_SLOTS];
    for (slot, c) in all.iter_mut().zip(colors) {
        *slot = [c[0], c[1], c[2], 0.0];
    }
    let mut first = [[0.0; 4]; MAX_COLORS];
    first.copy_from_slice(&all[..MAX_COLORS]);
    (first, all[MAX_COLORS])
}

impl PaletteUniforms {
    fn new(palette: &Palette, srgb: bool, block: u32, dither: f32, chroma: f32) -> Self {
        let table = palette.table();
        let lab: Vec<[f32; 3]> = table.iter().map(|c| oklab(*c)).collect();
        let (inks, last) = slots(&table);
        let (lab, lab_last) = slots(&lab);
        Self {
            len: table.len() as u32,
            matching: palette.matching().index(),
            srgb: srgb as u32,
            block: block.max(1),
            dither: dither.clamp(0.0, 1.0),
            chroma: chroma.clamp(0.0, 4.0),
            _pad: [0.0; 2],
            inks,
            last,
            lab,
            lab_last,
        }
    }
}

/// The block size in pixels for `pixel` points at `pixels_per_point`.
pub fn block_pixels(pixel: f32, pixels_per_point: f32) -> u32 {
    (pixel.clamp(1.0, 32.0) * pixels_per_point.max(0.25))
        .round()
        .max(1.0) as u32
}

struct PaletteStyle {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    input_bgl: wgpu::BindGroupLayout,
    /// The palettes, found from the frame's project and scene folder
    palettes: Mutex<PaletteCache>,
}

impl PaletteStyle {
    /// The palette the config names; the default when it can't be loaded
    /// (the cache has warned, once).
    fn palette(&self, frame: &StyleFrame<'_>) -> Arc<Palette> {
        let mut guard = self.palettes.lock().unwrap_or_else(|e| e.into_inner());
        if guard.scope() != Some(frame.assets) {
            *guard = PaletteCache::new(frame.assets.clone());
        }
        // An edited palette file shows while the viewer is open (#323),
        // checked as often as the ASCII style checks its own
        if frame.frame.is_multiple_of(15) {
            guard.poll();
        }
        let name = match frame.option("palette") {
            "" => DEFAULT_PALETTE,
            n => n,
        };
        guard
            .get(name)
            .unwrap_or_else(|_| Palette::builtin(DEFAULT_PALETTE).expect("built-in"))
    }
}

impl StylePass for PaletteStyle {
    fn encode(
        &self,
        frame: &StyleFrame<'_>,
        encoder: &mut wgpu::CommandEncoder,
        input: &StyleInput<'_>,
        output: &wgpu::TextureView,
    ) {
        let palette = self.palette(frame);
        let uniforms = PaletteUniforms::new(
            &palette,
            frame.srgb(),
            block_pixels(frame.param("pixel"), frame.pixels_per_point),
            frame.param("dither"),
            frame.param("chroma"),
        );
        frame
            .queue
            .write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        kit::fullscreen_pass(
            encoder,
            "Palette Pass",
            output,
            &self.pipeline,
            &[
                &frame.uniform("Palette Uniform BG", &self.uniform_bgl, &self.uniforms),
                &frame.sampled_input("Palette Input BG", &self.input_bgl, input),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::PALETTE_WGSL;

    #[test]
    fn the_shader_validates_and_its_uniforms_match_rust() {
        let module = naga::front::wgsl::parse_str(SHADER).expect("palette shader parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("palette shader validates");
        let (_, u) = module
            .global_variables
            .iter()
            .find(|(_, v)| v.name.as_deref() == Some("u"))
            .expect("uniform u");
        assert_eq!(
            module.types[u.ty].inner.size(module.to_ctx()) as usize,
            std::mem::size_of::<PaletteUniforms>()
        );
        assert!(include_str!("palette.wgsl").contains(&format!("array<vec4<f32>, {TABLE_SLOTS}>")));
    }

    #[test]
    fn a_ladder_takes_its_ground_at_its_lightness() {
        let paper = Palette::builtin("paper").unwrap();
        let table = paper.table();
        assert_eq!(*table.last().unwrap(), paper.background(), "cream on top");
        let phosphor = Palette::builtin("phosphor").unwrap();
        assert_eq!(
            phosphor.table()[0],
            phosphor.background(),
            "near-black below"
        );
        let cga = Palette::builtin("cga").unwrap();
        assert_eq!(cga.table()[0], [0.0; 3]);
        // Black quantizes to CGA's black ground, white to its white ink
        assert_eq!(cga.quantize([0.02; 3], 1.0, 0.0, [0, 0]), [0.0; 3]);
        assert_eq!(cga.quantize([1.0; 3], 1.0, 0.0, [0, 0]), [1.0; 3]);
    }

    #[test]
    fn ascii_and_palette_compile_one_matcher() {
        // Not a copy: both shaders hold the same file
        assert!(SHADER.contains(PALETTE_WGSL));
        assert!(super::super::ascii::GLYPH_SHADER.contains(PALETTE_WGSL));
    }

    #[test]
    fn the_block_is_points_times_the_scale() {
        assert_eq!(block_pixels(1.0, 1.0), 1);
        assert_eq!(block_pixels(4.0, 1.0), 4);
        assert_eq!(block_pixels(4.0, 1.5), 6);
        assert_eq!(block_pixels(0.0, 1.0), 1);
    }

    #[test]
    fn every_offered_palette_is_built_in() {
        assert!(Palette::builtin(DEFAULT_PALETTE).is_some());
        for p in BUILTIN_PALETTES {
            assert!(Palette::table(&Palette::builtin(p).unwrap()).len() <= TABLE_SLOTS);
        }
    }
}
