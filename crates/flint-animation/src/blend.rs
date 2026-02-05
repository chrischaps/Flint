//! Animation blending utilities for skeletal animation
//!
//! Provides crossfade blending between two full pose arrays and
//! additive blending that layers a delta clip onto a base pose.

use crate::skeleton::JointPose;

/// Linearly blend two full pose arrays.
///
/// Translation and scale use component-wise lerp.
/// Rotation uses quaternion slerp for correct interpolation.
/// `weight` of 0.0 = fully `a`, 1.0 = fully `b`.
pub fn blend_poses(a: &[JointPose], b: &[JointPose], weight: f32, out: &mut [JointPose]) {
    let count = a.len().min(b.len()).min(out.len());
    let w = weight.clamp(0.0, 1.0);
    let iw = 1.0 - w;

    for i in 0..count {
        // Translation: lerp
        out[i].translation = [
            a[i].translation[0] * iw + b[i].translation[0] * w,
            a[i].translation[1] * iw + b[i].translation[1] * w,
            a[i].translation[2] * iw + b[i].translation[2] * w,
        ];

        // Scale: lerp
        out[i].scale = [
            a[i].scale[0] * iw + b[i].scale[0] * w,
            a[i].scale[1] * iw + b[i].scale[1] * w,
            a[i].scale[2] * iw + b[i].scale[2] * w,
        ];

        // Rotation: slerp
        out[i].rotation = quat_slerp(&a[i].rotation, &b[i].rotation, w);
    }
}

/// Additive blend: layer a partial clip's delta onto a base pose.
///
/// For each joint: `out = base + (additive - reference) * weight`
/// Translation/scale are additive differences; rotation uses quaternion multiplication.
pub fn additive_blend(
    base: &[JointPose],
    additive: &[JointPose],
    reference: &[JointPose],
    weight: f32,
    out: &mut [JointPose],
) {
    let count = base.len().min(additive.len()).min(reference.len()).min(out.len());
    let w = weight.clamp(0.0, 1.0);

    for i in 0..count {
        // Translation: base + (additive - reference) * weight
        let dt = [
            (additive[i].translation[0] - reference[i].translation[0]) * w,
            (additive[i].translation[1] - reference[i].translation[1]) * w,
            (additive[i].translation[2] - reference[i].translation[2]) * w,
        ];
        out[i].translation = [
            base[i].translation[0] + dt[0],
            base[i].translation[1] + dt[1],
            base[i].translation[2] + dt[2],
        ];

        // Scale: base * lerp(1, additive/reference, weight)
        let ds = [
            1.0 + (additive[i].scale[0] / reference[i].scale[0].max(1e-10) - 1.0) * w,
            1.0 + (additive[i].scale[1] / reference[i].scale[1].max(1e-10) - 1.0) * w,
            1.0 + (additive[i].scale[2] / reference[i].scale[2].max(1e-10) - 1.0) * w,
        ];
        out[i].scale = [
            base[i].scale[0] * ds[0],
            base[i].scale[1] * ds[1],
            base[i].scale[2] * ds[2],
        ];

        // Rotation: base * slerp(identity, inv(reference) * additive, weight)
        let ref_inv = quat_conjugate(&reference[i].rotation);
        let delta_rot = quat_mul(&ref_inv, &additive[i].rotation);
        let identity = [0.0, 0.0, 0.0, 1.0];
        let weighted_delta = quat_slerp(&identity, &delta_rot, w);
        out[i].rotation = quat_normalize(&quat_mul(&base[i].rotation, &weighted_delta));
    }
}

/// Quaternion slerp with shortest-path correction
fn quat_slerp(a: &[f32; 4], b: &[f32; 4], t: f32) -> [f32; 4] {
    let mut dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];

    // Ensure shortest path
    let mut b_adj = *b;
    if dot < 0.0 {
        b_adj = [-b[0], -b[1], -b[2], -b[3]];
        dot = -dot;
    }

    // If very close, use lerp to avoid division by zero
    if dot > 0.9995 {
        let result = [
            a[0] + t * (b_adj[0] - a[0]),
            a[1] + t * (b_adj[1] - a[1]),
            a[2] + t * (b_adj[2] - a[2]),
            a[3] + t * (b_adj[3] - a[3]),
        ];
        return quat_normalize(&result);
    }

    let theta = dot.acos();
    let sin_theta = theta.sin();
    let wa = ((1.0 - t) * theta).sin() / sin_theta;
    let wb = (t * theta).sin() / sin_theta;

    [
        a[0] * wa + b_adj[0] * wb,
        a[1] * wa + b_adj[1] * wb,
        a[2] * wa + b_adj[2] * wb,
        a[3] * wa + b_adj[3] * wb,
    ]
}

fn quat_normalize(q: &[f32; 4]) -> [f32; 4] {
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if len < 1e-10 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    [q[0] / len, q[1] / len, q[2] / len, q[3] / len]
}

fn quat_conjugate(q: &[f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

fn quat_mul(a: &[f32; 4], b: &[f32; 4]) -> [f32; 4] {
    // Hamilton product: (x,y,z,w)
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity_pose() -> JointPose {
        JointPose {
            translation: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }
    }

    #[test]
    fn blend_weight_zero_returns_a() {
        let a = [JointPose {
            translation: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }];
        let b = [JointPose {
            translation: [10.0, 20.0, 30.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [2.0, 2.0, 2.0],
        }];
        let mut out = [identity_pose()];
        blend_poses(&a, &b, 0.0, &mut out);
        assert!((out[0].translation[0] - 1.0).abs() < 1e-5);
        assert!((out[0].translation[1] - 2.0).abs() < 1e-5);
        assert!((out[0].scale[0] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn blend_weight_one_returns_b() {
        let a = [JointPose {
            translation: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }];
        let b = [JointPose {
            translation: [10.0, 20.0, 30.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [2.0, 2.0, 2.0],
        }];
        let mut out = [identity_pose()];
        blend_poses(&a, &b, 1.0, &mut out);
        assert!((out[0].translation[0] - 10.0).abs() < 1e-5);
        assert!((out[0].translation[1] - 20.0).abs() < 1e-5);
        assert!((out[0].scale[0] - 2.0).abs() < 1e-5);
    }

    #[test]
    fn blend_midpoint_interpolates() {
        let a = [JointPose {
            translation: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }];
        let b = [JointPose {
            translation: [10.0, 20.0, 30.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [3.0, 3.0, 3.0],
        }];
        let mut out = [identity_pose()];
        blend_poses(&a, &b, 0.5, &mut out);
        assert!((out[0].translation[0] - 5.0).abs() < 1e-5);
        assert!((out[0].translation[1] - 10.0).abs() < 1e-5);
        assert!((out[0].scale[0] - 2.0).abs() < 1e-5);
    }

    #[test]
    fn additive_zero_weight_returns_base() {
        let base = [JointPose {
            translation: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }];
        let additive = [JointPose {
            translation: [5.0, 5.0, 5.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [2.0, 2.0, 2.0],
        }];
        let reference = [identity_pose()];
        let mut out = [identity_pose()];
        additive_blend(&base, &additive, &reference, 0.0, &mut out);
        assert!((out[0].translation[0] - 1.0).abs() < 1e-5);
        assert!((out[0].translation[1] - 2.0).abs() < 1e-5);
    }

    #[test]
    fn additive_full_weight_adds_delta() {
        let base = [JointPose {
            translation: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }];
        let reference = [identity_pose()];
        let additive = [JointPose {
            translation: [5.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }];
        let mut out = [identity_pose()];
        additive_blend(&base, &additive, &reference, 1.0, &mut out);
        // delta = additive - reference = (5,0,0) - (0,0,0) = (5,0,0)
        // out = base + delta = (1+5, 2+0, 3+0) = (6,2,3)
        assert!((out[0].translation[0] - 6.0).abs() < 1e-5);
        assert!((out[0].translation[1] - 2.0).abs() < 1e-5);
        assert!((out[0].translation[2] - 3.0).abs() < 1e-5);
    }

    #[test]
    fn blend_rotation_slerp_midpoint() {
        // 90 degrees around Y axis
        let angle = std::f32::consts::FRAC_PI_2;
        let a = [JointPose {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0], // identity
            scale: [1.0; 3],
        }];
        let b = [JointPose {
            translation: [0.0; 3],
            rotation: [0.0, (angle / 2.0).sin(), 0.0, (angle / 2.0).cos()], // 90 deg Y
            scale: [1.0; 3],
        }];
        let mut out = [identity_pose()];
        blend_poses(&a, &b, 0.5, &mut out);

        // Should be 45 degrees around Y
        let half_angle = angle / 4.0; // 45 / 2 = 22.5 deg
        assert!((out[0].rotation[1] - half_angle.sin()).abs() < 1e-4);
        assert!((out[0].rotation[3] - half_angle.cos()).abs() < 1e-4);
    }
}
