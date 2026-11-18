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
use super::{StyleDescriptor, StyleDevice, StyleFrame, StyleParam, StylePass, StyleStage};

pub(super) static STYLE: StyleDescriptor =
    StyleDescriptor::new("tron", "Tron", StyleStage::Display, create)
        .params(&[
            StyleParam::new("glow_r", 0.1, 0.0..=1.0)
                .label("Glow")
                .help("The colour the edges and grid glow in"),
            StyleParam::new("glow_g", 0.9, 0.0..=1.0),
            StyleParam::new("glow_b", 1.0, 0.0..=1.0),
            StyleParam::new("edges", 0.9, 0.0..=2.0)
                .label("Edges")
                .help("How bright silhouettes and luminance edges glow"),
            StyleParam::new("dim", 0.08, 0.0..=1.0)
                .label("Dim")
                .help("How much of the scene shows under the glow"),
            StyleParam::new("sky_dim", 0.05, 0.0..=1.0)
                .label("Sky dim")
                .help("How much of the sky shows"),
            StyleParam::new("grid", 0.5, 0.0..=2.0)
                .label("Grid")
                .help("How bright the world-space grid glows"),
            StyleParam::new("spacing", 2.0, 0.1..=20.0)
                .label("Spacing")
                .help("Metres between grid lines; every fifth is stronger"),
            StyleParam::new("pulse", 2.0, 0.0..=10.0)
                .label("Pulse")
                .help("How fast the grid's glow pulses outward"),
            StyleParam::new("reach", 50.0, 1.0..=500.0)
                .label("Reach")
                .help("Metres over which the grid fades out"),
        ])
        .draws_over_entities()
        .default_order(reality::ORDER);

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    reality::create(
        d,
        "Tron Pass",
        super::kit::wgsl!("prelude_core.wgsl", "../tear_mask.wgsl", "reality.wgsl", "tron.wgsl"),
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
