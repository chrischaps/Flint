//! Underwater (#746; it was render mode 5): the eye under the sea. A
//! waterline across the frame where the plane `depth` metres above the
//! eye meets the near plane, banded murk, a Snell window, light shafts and
//! bioluminescent specks below it, a white foam lip on it. A game sets
//! `depth` (the signed eye depth) each frame from its sea; the ocean's
//! `tint` with `tint_preset = "underside"` (`OceanTint::underside`, #1035)
//! turns the surface above into the ceiling it is from below. Its `blend` is how far under the look
//! is (the style draws its own blend, since the sway grows with it). A
//! reality style ([`super::reality`]).
//!
//! ```toml
//! [post_process.styles.underwater]
//! depth = 0.4
//! daylight = 1.0
//! ```

use super::reality::{self, own_blend};
use super::{StyleDescriptor, StyleDevice, StyleFrame, StyleParam, StylePass, StyleStage};

pub(super) static STYLE: StyleDescriptor =
    StyleDescriptor::new("underwater", "Underwater", StyleStage::Display, create)
        .params(&[
            StyleParam::new("depth", 0.4, -2.0..=10.0)
                .label("Eye depth")
                .help("Metres the eye is under the water; the waterline is the plane this far above it (below 0 the eye is out)"),
            StyleParam::new("churn", 0.3, 0.0..=1.0)
                .label("Churn")
                .help("The sea's energy: murk that pulls the bands in, a wobblier line"),
            StyleParam::new("daylight", 1.0, 0.0..=1.0)
                .label("Daylight")
                .help("Light from above: murk, window and shafts"),
            StyleParam::new("glow", 0.0, 0.0..=1.0)
                .label("Glow")
                .help("Bioluminescent specks in the water"),
            StyleParam::new("sway", 1.0, 0.0..=4.0)
                .label("Sway")
                .help("The refraction's wander once submerged"),
        ])
        .draws_over_entities()
        .default_order(reality::ORDER)
        .draws_blend(reality::always);

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    reality::create(
        d,
        "Underwater Pass",
        super::kit::wgsl!("prelude_core.wgsl", "../tear_mask.wgsl", "reality.wgsl", "underwater.wgsl"),
        fill,
    )
}

fn fill(frame: &StyleFrame<'_>) -> [[f32; 4]; 4] {
    [
        [
            frame.param("depth"),
            frame.param("churn"),
            frame.param("daylight"),
            frame.param("glow"),
        ],
        [own_blend(frame), frame.param("sway"), 0.0, 0.0],
        [0.0; 4],
        [0.0; 4],
    ]
}
