//! Tron (#746; it was render mode 4): a wire world. The scene dims to a
//! ghost, its silhouettes and luminance edges glow, and a world-space grid
//! rebuilt from depth rides the surfaces. A reality style
//! ([`super::reality`]).
//!
//! ```toml
//! [post_process.styles.tron]
//! spacing = 2.0
//! blend_source = "tear"
//! ```

use super::reality::{self, row};
use super::{StyleDescriptor, StyleDevice, StyleFrame, StyleHint, StyleParam, StylePass, StyleStage};

pub(super) static STYLE: StyleDescriptor =
    StyleDescriptor::new("tron", "Tron", StyleStage::Display, create)
        .params(&[
            StyleParam::new("glow_r", 0.1, 0.0..=1.0),
            StyleParam::new("glow_g", 0.9, 0.0..=1.0),
            StyleParam::new("glow_b", 1.0, 0.0..=1.0),
            StyleParam::new("edges", 0.9, 0.0..=2.0),
            StyleParam::new("dim", 0.08, 0.0..=1.0),
            StyleParam::new("sky_dim", 0.05, 0.0..=1.0),
            StyleParam::new("grid", 0.5, 0.0..=2.0),
            StyleParam::new("spacing", 2.0, 0.1..=20.0),
            StyleParam::new("pulse", 2.0, 0.0..=10.0),
            StyleParam::new("reach", 50.0, 1.0..=500.0),
        ])
        .hints(&[
            StyleHint::new("glow", "Glow", "The colour the edges and grid glow in"),
            StyleHint::new("edges", "Edges", "How bright silhouettes and luminance edges glow"),
            StyleHint::new("dim", "Dim", "How much of the scene shows under the glow"),
            StyleHint::new("sky_dim", "Sky dim", "How much of the sky shows"),
            StyleHint::new("grid", "Grid", "How bright the world-space grid glows"),
            StyleHint::new(
                "spacing",
                "Spacing",
                "Metres between grid lines; every fifth is stronger",
            ),
            StyleHint::new("pulse", "Pulse", "How fast the grid's glow pulses outward"),
            StyleHint::new("reach", "Reach", "Metres over which the grid fades out"),
        ])
        .draws_over_entities()
        .default_order(reality::ORDER);

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    reality::create(
        d,
        "Tron Pass",
        super::kit::wgsl!("../tear_mask.wgsl", "reality.wgsl", "tron.wgsl"),
        fill,
    )
}

fn fill(frame: &StyleFrame<'_>) -> [[f32; 4]; 4] {
    [
        row(frame.color("glow"), frame.param("edges")),
        [
            frame.param("dim"),
            frame.param("sky_dim"),
            frame.param("grid"),
            frame.param("spacing"),
        ],
        [frame.param("pulse"), frame.param("reach"), 0.0, 0.0],
        [0.0; 4],
    ]
}
