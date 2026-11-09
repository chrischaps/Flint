//! The form's poses: one smooth variant of the spine per shape seed.
//!
//! A pose is drawn straight from a seeded curve along the spine, not regrown, so
//! its vocabulary is a choice: `bend_deg` (one slow lean of the whole form),
//! `ripple` and `ripple_waves` (a wave up it), `swell` (the section breathing
//! wider and narrower along it), `turn_deg` (the section turning about the spine,
//! which reads on a flat form) and `stretch` (the length). Each is the full size
//! of the pose at `[living] drift.amplitude = 1`; the drift amplitude and the
//! wooze field's `amplitude` scale all of them at once.
//!
//! A segment's rotation is absolute, in the prop's space: the tilt at its height
//! along the curve, after a turn about its own rest direction.

use std::f32::consts::TAU;

use flint_core::quat::{quat_from_axis_angle, quat_mul, quat_normalize};

use super::body::Spine;
use super::FormParams;
use crate::living::{LivingPose, LivingSkeleton, SegmentPose};
use crate::SeededRng;

/// The pose `shape_seed` gives a lineage's skeleton.
pub(super) fn full_pose(
    p: &FormParams,
    spine: &Spine,
    skeleton: &LivingSkeleton,
    amplitude: f32,
    shape_seed: u64,
) -> LivingPose {
    let mut rng = SeededRng::new(skeleton.lineage_seed).fork(&format!("pose{shape_seed}"));
    let bend_rad = p.bend_deg.to_radians() * amplitude;
    let bend: [f32; 2] = std::array::from_fn(|_| rng.next_range(-1.0, 1.0));
    let ripple: [f32; 2] = std::array::from_fn(|_| rng.next_range(-1.0, 1.0) * p.ripple);
    let ripple_phase: [f32; 2] = std::array::from_fn(|_| rng.next_range(0.0, TAU));
    let ripple_waves = p.ripple_waves * rng.next_range(0.75, 1.25);
    let swell_phase = rng.next_range(0.0, TAU);
    let swell_waves = rng.next_range(0.8, 1.6);
    let turn: [f32; 2] = std::array::from_fn(|_| rng.next_range(-1.0, 1.0));
    let stretch = rng.next_range(-1.0, 1.0);

    let n = skeleton.segments.len();
    let segments = skeleton
        .segments
        .iter()
        .enumerate()
        .map(|(i, rest)| {
            let u = (i as f32 + 0.5) / n as f32;
            let frame = spine.frame(u);
            let tilt = |k: usize| {
                bend_rad
                    * (bend[k] * u
                        + 0.5 * ripple[k] * (TAU * ripple_waves * u + ripple_phase[k]).sin())
            };
            let d = rest.direction();
            // The lean, as a small rotation vector across the rest direction.
            let w = frame.normal * tilt(0) + frame.binormal * tilt(1);
            let w = w - d * w.dot(&d);
            let lean = if w.length() < 1e-9 {
                SegmentPose::IDENTITY.rotation
            } else {
                quat_from_axis_angle(d.cross(&w).normalized().to_array(), w.length())
            };
            let about = p.turn_deg.to_radians()
                * amplitude
                * (turn[0] * u + 0.5 * turn[1] * (TAU * u).sin());
            let twist = quat_from_axis_angle(d.to_array(), about);
            SegmentPose {
                rotation: quat_normalize(&quat_mul(&lean, &twist)),
                length_scale: 1.0 + p.stretch * amplitude * stretch,
                radius_scale: 1.0
                    + p.swell * amplitude * (TAU * swell_waves * u + swell_phase).sin(),
            }
        })
        .collect();
    LivingPose {
        shape_seed,
        segments,
    }
}
