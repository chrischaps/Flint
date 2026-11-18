//! Blood (#746; it was render mode 2's grade): the world sinks into a
//! crimson key that keeps its luminance and spares what is already red. The
//! sea's half of the old mode is the `ocean` component's `tint`, which a
//! game drives beside it. A reality style ([`super::reality`]).
//!
//! ```toml
//! [post_process.styles.blood]
//! strength = 0.85
//! blend_source = "tear"
//! ```

use super::reality::{self, row};
use super::{StyleDescriptor, StyleDevice, StyleFrame, StyleParam, StylePass, StyleStage};

pub(super) static STYLE: StyleDescriptor =
    StyleDescriptor::new("blood", "Blood", StyleStage::Display, create)
        .params(&[
            StyleParam::new("strength", 0.85, 0.0..=1.0)
                .label("Strength")
                .help("How far the world goes to the tint"),
            StyleParam::new("tint_r", 1.45, 0.0..=2.0)
                .label("Tint")
                .help("The key: each channel is the pixel's luminance times this"),
            StyleParam::new("tint_g", 0.2, 0.0..=2.0),
            StyleParam::new("tint_b", 0.24, 0.0..=2.0),
            StyleParam::new("shadow_r", 0.02, 0.0..=1.0).label("Shadow").help("What black goes to"),
            StyleParam::new("shadow_g", 0.0, 0.0..=1.0),
            StyleParam::new("shadow_b", 0.004, 0.0..=1.0),
            StyleParam::new("knee", 0.18, 0.01..=1.0)
                .label("Knee")
                .help("The luminance from which the tint is full; below it the shadow shows through"),
            StyleParam::new("spare_red", 3.0, 0.0..=10.0)
                .label("Spare red")
                .help("How strongly pixels already red are left alone (a tinted sea's pools and foam); 0 tints them too"),
        ])
        .draws_over_entities()
        .default_order(reality::ORDER);

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    reality::create(
        d,
        "Blood Pass",
        super::kit::wgsl!("prelude_core.wgsl", "../tear_mask.wgsl", "reality.wgsl", "blood.wgsl"),
        fill,
    )
}

fn fill(frame: &StyleFrame<'_>) -> [[f32; 4]; 4] {
    [
        row(frame.color("tint"), frame.param("strength")),
        row(frame.color("shadow"), frame.param("knee")),
        [frame.param("spare_red"), 0.0, 0.0, 0.0],
        [0.0; 4],
    ]
}
