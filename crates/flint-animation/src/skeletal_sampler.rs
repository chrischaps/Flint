//! Skeletal keyframe sampling with quaternion slerp for rotation tracks

use crate::clip::Interpolation;
use crate::skeletal_clip::{JointProperty, JointTrack};

/// Sample a joint track at a given time.
///
/// Returns the interpolated value as a Vec<f32> (3 for translation/scale, 4 for rotation).
pub fn sample_joint_track(track: &JointTrack, time: f64) -> Vec<f32> {
    let keyframes = &track.keyframes;
    let is_rotation = track.property == JointProperty::Rotation;
    let components = if is_rotation { 4 } else { 3 };

    if keyframes.is_empty() {
        return if is_rotation {
            vec![0.0, 0.0, 0.0, 1.0] // identity quaternion
        } else {
            vec![0.0; 3]
        };
    }

    // Before first keyframe — clamp
    if time <= keyframes[0].time {
        return keyframes[0].value.clone();
    }

    // After last keyframe — clamp
    let last = &keyframes[keyframes.len() - 1];
    if time >= last.time {
        return last.value.clone();
    }

    // Binary search for the interval
    let idx = match keyframes.binary_search_by(|kf| kf.time.partial_cmp(&time).unwrap()) {
        Ok(i) => return keyframes[i].value.clone(),
        Err(i) => i,
    };

    let prev = &keyframes[idx - 1];
    let next = &keyframes[idx];

    let span = next.time - prev.time;
    if span <= 0.0 {
        return prev.value.clone();
    }
    let t = ((time - prev.time) / span) as f32;

    match track.interpolation {
        Interpolation::Step => prev.value.clone(),
        Interpolation::Linear => {
            if is_rotation {
                quat_slerp(&prev.value, &next.value, t)
            } else {
                lerp_vec(&prev.value, &next.value, t, components)
            }
        }
        Interpolation::CubicSpline => {
            // For cubicspline, glTF packs [in_tangent, value, out_tangent] per keyframe.
            // Our importer stores just the value, so fall back to linear for now.
            if is_rotation {
                quat_slerp(&prev.value, &next.value, t)
            } else {
                lerp_vec(&prev.value, &next.value, t, components)
            }
        }
    }
}

/// Component-wise linear interpolation for a Vec<f32>
fn lerp_vec(a: &[f32], b: &[f32], t: f32, count: usize) -> Vec<f32> {
    (0..count)
        .map(|i| {
            let av = a.get(i).copied().unwrap_or(0.0);
            let bv = b.get(i).copied().unwrap_or(0.0);
            av + (bv - av) * t
        })
        .collect()
}

/// Quaternion spherical linear interpolation (slerp) with shortest-path correction.
///
/// Input quaternions are xyzw format. The result is normalized.
pub fn quat_slerp(a: &[f32], b: &[f32], t: f32) -> Vec<f32> {
    if a.len() < 4 || b.len() < 4 {
        return vec![0.0, 0.0, 0.0, 1.0];
    }

    let (ax, ay, az, aw) = (a[0], a[1], a[2], a[3]);
    let (mut bx, mut by, mut bz, mut bw) = (b[0], b[1], b[2], b[3]);

    // Compute dot product
    let mut dot = ax * bx + ay * by + az * bz + aw * bw;

    // Shortest path: if dot < 0, negate b
    if dot < 0.0 {
        bx = -bx;
        by = -by;
        bz = -bz;
        bw = -bw;
        dot = -dot;
    }

    // If quaternions are very close, use linear interpolation to avoid division by zero
    let (scale_a, scale_b) = if dot > 0.9995 {
        (1.0 - t, t)
    } else {
        let theta = dot.acos();
        let sin_theta = theta.sin();
        (
            ((1.0 - t) * theta).sin() / sin_theta,
            (t * theta).sin() / sin_theta,
        )
    };

    let rx = scale_a * ax + scale_b * bx;
    let ry = scale_a * ay + scale_b * by;
    let rz = scale_a * az + scale_b * bz;
    let rw = scale_a * aw + scale_b * bw;

    // Normalize
    let len = (rx * rx + ry * ry + rz * rz + rw * rw).sqrt();
    if len < 1e-10 {
        return vec![0.0, 0.0, 0.0, 1.0];
    }
    vec![rx / len, ry / len, rz / len, rw / len]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::Interpolation;
    use crate::skeletal_clip::{JointKeyframe, JointProperty, JointTrack};

    #[test]
    fn slerp_identity_at_endpoints() {
        let a = vec![0.0, 0.0, 0.0, 1.0];
        let b = vec![0.0, 0.7071, 0.0, 0.7071]; // 90-degree Y rotation

        let r0 = quat_slerp(&a, &b, 0.0);
        assert!((r0[0] - a[0]).abs() < 1e-4);
        assert!((r0[1] - a[1]).abs() < 1e-4);
        assert!((r0[2] - a[2]).abs() < 1e-4);
        assert!((r0[3] - a[3]).abs() < 1e-4);

        let r1 = quat_slerp(&a, &b, 1.0);
        assert!((r1[0] - b[0]).abs() < 1e-4);
        assert!((r1[1] - b[1]).abs() < 1e-4);
        assert!((r1[2] - b[2]).abs() < 1e-4);
        assert!((r1[3] - b[3]).abs() < 1e-4);
    }

    #[test]
    fn slerp_midpoint_is_normalized() {
        let a = vec![0.0, 0.0, 0.0, 1.0];
        let b = vec![0.0, 1.0, 0.0, 0.0]; // 180-degree Y rotation

        let mid = quat_slerp(&a, &b, 0.5);
        let len = (mid[0] * mid[0] + mid[1] * mid[1] + mid[2] * mid[2] + mid[3] * mid[3]).sqrt();
        assert!(
            (len - 1.0).abs() < 1e-5,
            "slerp midpoint should be normalized, got length {}",
            len
        );
    }

    #[test]
    fn slerp_shortest_path() {
        // When quaternions represent the same rotation but with opposite signs,
        // slerp should take the shortest path (negate and go the short way)
        let a = vec![0.0, 0.0, 0.0, 1.0];
        let neg_a = vec![0.0, 0.0, 0.0, -1.0]; // same rotation, opposite sign

        let r = quat_slerp(&a, &neg_a, 0.5);
        let len = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2] + r[3] * r[3]).sqrt();
        assert!((len - 1.0).abs() < 1e-5);
        // Should be close to identity since they represent the same rotation
        assert!((r[3].abs() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn sample_translation_track_linear() {
        let track = JointTrack {
            joint_index: 0,
            property: JointProperty::Translation,
            interpolation: Interpolation::Linear,
            keyframes: vec![
                JointKeyframe {
                    time: 0.0,
                    value: vec![0.0, 0.0, 0.0],
                },
                JointKeyframe {
                    time: 2.0,
                    value: vec![4.0, 6.0, 8.0],
                },
            ],
        };

        let v = sample_joint_track(&track, 1.0);
        assert_eq!(v.len(), 3);
        assert!((v[0] - 2.0).abs() < 1e-4);
        assert!((v[1] - 3.0).abs() < 1e-4);
        assert!((v[2] - 4.0).abs() < 1e-4);
    }

    #[test]
    fn sample_rotation_track_uses_slerp() {
        let track = JointTrack {
            joint_index: 0,
            property: JointProperty::Rotation,
            interpolation: Interpolation::Linear,
            keyframes: vec![
                JointKeyframe {
                    time: 0.0,
                    value: vec![0.0, 0.0, 0.0, 1.0],
                },
                JointKeyframe {
                    time: 1.0,
                    value: vec![0.0, 0.7071, 0.0, 0.7071],
                },
            ],
        };

        let v = sample_joint_track(&track, 0.0);
        assert_eq!(v.len(), 4);
        assert!((v[3] - 1.0).abs() < 1e-3); // w close to 1 at t=0

        let v_mid = sample_joint_track(&track, 0.5);
        let len = (v_mid[0] * v_mid[0] + v_mid[1] * v_mid[1] + v_mid[2] * v_mid[2] + v_mid[3] * v_mid[3]).sqrt();
        assert!((len - 1.0).abs() < 1e-5, "midpoint should be normalized");
    }

    #[test]
    fn sample_step_interpolation() {
        let track = JointTrack {
            joint_index: 0,
            property: JointProperty::Translation,
            interpolation: Interpolation::Step,
            keyframes: vec![
                JointKeyframe {
                    time: 0.0,
                    value: vec![1.0, 2.0, 3.0],
                },
                JointKeyframe {
                    time: 1.0,
                    value: vec![4.0, 5.0, 6.0],
                },
            ],
        };

        let v = sample_joint_track(&track, 0.5);
        assert_eq!(v, vec![1.0, 2.0, 3.0]); // Step holds first value
    }
}
