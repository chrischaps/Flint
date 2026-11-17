//! Drunk (#746; it was render mode 3): the frame sways and sees double. Its
//! `blend` is how drunk: the sway and the ghost grow with it (the style
//! draws its own blend). Float's tear also leaned the camera's FOV, blurred
//! and fringed the frame and muffled the sound from its script; those are
//! the game's, and stay there. A reality style ([`super::reality`]).
//!
//! ```toml
//! [post_process.styles.drunk]
//! blend = 0.6
//! ```

use super::reality::{self, own_blend};
use super::{StyleDescriptor, StyleDevice, StyleFrame, StyleParam, StylePass, StyleStage};

pub(super) static STYLE: StyleDescriptor =
    StyleDescriptor::new("drunk", "Drunk", StyleStage::Display, create)
        .params(&[
            StyleParam::new("sway", 1.0, 0.0..=4.0).label("Sway").help("How far the frame wanders"),
            StyleParam::new("ghost", 1.0, 0.0..=2.5)
                .label("Ghost")
                .help("How far the double drifts and how strongly it shows"),
            StyleParam::new("speed", 1.0, 0.0..=4.0)
                .label("Speed")
                .help("How fast it all moves; 0 holds it still"),
        ])
        .draws_over_entities()
        .default_order(reality::ORDER)
        .draws_blend(reality::always);

fn create(d: &StyleDevice) -> Box<dyn StylePass> {
    reality::create(
        d,
        "Drunk Pass",
        super::kit::wgsl!("prelude_core.wgsl", "../tear_mask.wgsl", "reality.wgsl", "drunk.wgsl"),
        fill,
    )
}

fn fill(frame: &StyleFrame<'_>) -> [[f32; 4]; 4] {
    [
        [
            frame.param("sway"),
            frame.param("ghost"),
            frame.param("speed"),
            own_blend(frame),
        ],
        [0.0; 4],
        [0.0; 4],
        [0.0; 4],
    ]
}
