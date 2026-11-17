//! Matrix (#746; it was render mode 1): the frame as falling code. The
//! screen is cut into glyph cells, each a procedural dot-matrix glyph lit
//! by the picture's luminance there, with a rain head falling down each
//! column. A reality style ([`super::reality`]).
//!
//! ```toml
//! [post_process.styles.matrix]
//! rate = 6.0               # glyph rerolls a second
//! blend_source = "tear"    # only where [post_process] tear opens the frame
//! ```

use super::reality::{self, row};
use super::{StyleDescriptor, StyleDevice, StyleFrame, StyleParam, StylePass, StyleStage};

pub(super) static STYLE: StyleDescriptor =
    StyleDescriptor::new("matrix", "Matrix", StyleStage::Display, create)
        .params(&[
            StyleParam::new("cell_w", 8.0, 2.0..=32.0)
                .label("Cell width")
                .help("A glyph cell's width, in points"),
            StyleParam::new("cell_h", 12.0, 2.0..=48.0)
                .label("Cell height")
                .help("A glyph cell's height, in points"),
            StyleParam::new("rate", 6.0, 0.0..=30.0)
                .label("Rate")
                .help("Glyph rerolls a second; 0 reads as 6, the code's flicker"),
            StyleParam::new("fall", 1.0, 0.0..=4.0)
                .label("Fall")
                .help("How fast the rain heads fall; 0 holds them"),
            StyleParam::new("ink_r", 0.15, 0.0..=1.0).label("Ink").help("A lit glyph's colour"),
            StyleParam::new("ink_g", 1.0, 0.0..=1.0),
            StyleParam::new("ink_b", 0.3, 0.0..=1.0),
            StyleParam::new("ground_r", 0.0, 0.0..=1.0)
                .label("Ground")
                .help("An unlit cell's colour"),
            StyleParam::new("ground_g", 0.025, 0.0..=1.0),
            StyleParam::new("ground_b", 0.005, 0.0..=1.0),
            StyleParam::new("head_r", 0.7, 0.0..=1.0)
                .label("Head")
                .help("The falling rain head's colour"),
            StyleParam::new("head_g", 1.0, 0.0..=1.0),
            StyleParam::new("head_b", 0.8, 0.0..=1.0),
            StyleParam::new("glow", 0.6, 0.0..=2.0)
                .label("Glow")
                .help("How bright the rain head burns over its glyphs"),
        ])
        // Like the composite's mode it was, it draws over every pixel, a
        // styled prop's too: an ASCII prop in a tear glyphs the code
        .draws_over_entities()
        .default_order(reality::ORDER);

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    reality::create(
        d,
        "Matrix Pass",
        super::kit::wgsl!("prelude_core.wgsl", "../tear_mask.wgsl", "reality.wgsl", "matrix.wgsl"),
        fill,
    )
}

fn fill(frame: &StyleFrame<'_>) -> [[f32; 4]; 4] {
    let ppp = frame.pixels_per_point.max(0.25);
    [
        [
            frame.param("cell_w") * ppp,
            frame.param("cell_h") * ppp,
            frame.param("rate"),
            frame.param("fall"),
        ],
        row(frame.color("ink"), frame.param("glow")),
        row(frame.color("ground"), 0.0),
        row(frame.color("head"), 0.0),
    ]
}
