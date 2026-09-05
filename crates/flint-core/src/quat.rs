//! Quaternion and rigid-transform helpers shared by physics, animation and scripting.
//!
//! Quaternions are `[x, y, z, w]` arrays, matching the `transform.rotation_quat`
//! scene field and glTF. All helpers agree with [`Transform::to_matrix`]: Euler
//! angles are degrees composed ZYX (`Rz * Ry * Rx`), so a quaternion built here
//! rotates exactly like the same Euler triple rendered.

use crate::types::{Transform, Vec3};

/// Hamilton product `a * b` (apply `b` first, then `a`).
pub fn quat_mul(a: &[f32; 4], b: &[f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// Unit-length copy of `q`; identity when `q` is degenerate.
pub fn quat_normalize(q: &[f32; 4]) -> [f32; 4] {
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if len < 1e-10 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    [q[0] / len, q[1] / len, q[2] / len, q[3] / len]
}

/// Inverse rotation of a unit quaternion.
pub fn quat_conjugate(q: &[f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// Quaternion for a rotation of `radians` about `axis` (normalised internally).
pub fn quat_from_axis_angle(axis: [f32; 3], radians: f32) -> [f32; 4] {
    let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    if len < 1e-10 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let (s, c) = (radians * 0.5).sin_cos();
    [axis[0] / len * s, axis[1] / len * s, axis[2] / len * s, c]
}

/// Euler degrees (pitch x, yaw y, roll z) to a quaternion, ZYX order: the same
/// rotation [`Transform::to_matrix`] builds from `transform.rotation`.
pub fn euler_deg_to_quat(x_deg: f32, y_deg: f32, z_deg: f32) -> [f32; 4] {
    let qx = quat_from_axis_angle([1.0, 0.0, 0.0], x_deg.to_radians());
    let qy = quat_from_axis_angle([0.0, 1.0, 0.0], y_deg.to_radians());
    let qz = quat_from_axis_angle([0.0, 0.0, 1.0], z_deg.to_radians());
    quat_normalize(&quat_mul(&quat_mul(&qz, &qy), &qx))
}

/// Rotate a vector by a unit quaternion.
pub fn quat_rotate_vec3(q: &[f32; 4], v: [f32; 3]) -> [f32; 3] {
    let [qx, qy, qz, qw] = *q;
    // t = 2 * cross(q.xyz, v)
    let tx = 2.0 * (qy * v[2] - qz * v[1]);
    let ty = 2.0 * (qz * v[0] - qx * v[2]);
    let tz = 2.0 * (qx * v[1] - qy * v[0]);
    // rotated = v + w * t + cross(q.xyz, t)
    [
        v[0] + qw * tx + (qy * tz - qz * ty),
        v[1] + qw * ty + (qz * tx - qx * tz),
        v[2] + qw * tz + (qx * ty - qy * tx),
    ]
}

/// Rotation part of a column-major 4x4 matrix as a quaternion, with the basis
/// orthonormalised first so a scaled or slightly drifted matrix still yields a
/// clean rotation.
pub fn mat4_to_quat(m: &[[f32; 4]; 4]) -> [f32; 4] {
    let (r, _) = orthonormal_basis(m);
    // r[col][row]
    let (m00, m11, m22) = (r[0][0], r[1][1], r[2][2]);
    let trace = m00 + m11 + m22;
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [
            (r[1][2] - r[2][1]) / s,
            (r[2][0] - r[0][2]) / s,
            (r[0][1] - r[1][0]) / s,
            0.25 * s,
        ]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        [
            0.25 * s,
            (r[0][1] + r[1][0]) / s,
            (r[2][0] + r[0][2]) / s,
            (r[1][2] - r[2][1]) / s,
        ]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        [
            (r[0][1] + r[1][0]) / s,
            0.25 * s,
            (r[1][2] + r[2][1]) / s,
            (r[2][0] - r[0][2]) / s,
        ]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        [
            (r[2][0] + r[0][2]) / s,
            (r[1][2] + r[2][1]) / s,
            0.25 * s,
            (r[0][1] - r[1][0]) / s,
        ]
    };
    quat_normalize(&q)
}

/// Split a column-major 4x4 into its rigid part: translation and rotation.
/// Scale and shear are discarded (see [`mat4_scale`] to inspect them).
pub fn mat4_to_rigid(m: &[[f32; 4]; 4]) -> (Vec3, [f32; 4]) {
    (Vec3::new(m[3][0], m[3][1], m[3][2]), mat4_to_quat(m))
}

/// Lengths of the three basis columns: `[1, 1, 1]` for a rigid transform.
pub fn mat4_scale(m: &[[f32; 4]; 4]) -> [f32; 3] {
    let (_, s) = orthonormal_basis(m);
    s
}

/// Express a world-space pose relative to `parent_world`, assuming the parent
/// is rigid (its scale is normalised away). Returns `(local_pos, local_quat)`.
pub fn rigid_inverse_apply(
    parent_world: &[[f32; 4]; 4],
    world_pos: Vec3,
    world_quat: [f32; 4],
) -> (Vec3, [f32; 4]) {
    let (pt, pq) = mat4_to_rigid(parent_world);
    let inv = quat_conjugate(&pq);
    let d = [world_pos.x - pt.x, world_pos.y - pt.y, world_pos.z - pt.z];
    let lp = quat_rotate_vec3(&inv, d);
    let lq = quat_normalize(&quat_mul(&inv, &world_quat));
    (Vec3::new(lp[0], lp[1], lp[2]), lq)
}

/// Shortest-arc rotation taking direction `a` onto direction `b` (both are
/// normalised internally). Identity when they already agree; a half turn about
/// any perpendicular axis when they are opposite.
pub fn quat_from_two_vectors(a: [f32; 3], b: [f32; 3]) -> [f32; 4] {
    let norm = |v: [f32; 3]| {
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if len < 1e-10 {
            None
        } else {
            Some([v[0] / len, v[1] / len, v[2] / len])
        }
    };
    let (Some(a), Some(b)) = (norm(a), norm(b)) else {
        return [0.0, 0.0, 0.0, 1.0];
    };
    let d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    if d > 1.0 - 1e-6 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    if d < -1.0 + 1e-6 {
        // Opposite: pick any axis perpendicular to `a`.
        let helper = if a[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let axis = [
            a[1] * helper[2] - a[2] * helper[1],
            a[2] * helper[0] - a[0] * helper[2],
            a[0] * helper[1] - a[1] * helper[0],
        ];
        return quat_from_axis_angle(axis, std::f32::consts::PI);
    }
    let axis = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    quat_normalize(&[axis[0], axis[1], axis[2], 1.0 + d])
}

/// Normalised linear interpolation from `a` (t = 0) to `b` (t = 1) along the
/// short way round. Good enough for blending nearby rotations (IK weights).
pub fn quat_nlerp(a: &[f32; 4], b: &[f32; 4], t: f32) -> [f32; 4] {
    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let sign = if dot < 0.0 { -1.0 } else { 1.0 };
    let t = t.clamp(0.0, 1.0);
    quat_normalize(&[
        a[0] + (sign * b[0] - a[0]) * t,
        a[1] + (sign * b[1] - a[1]) * t,
        a[2] + (sign * b[2] - a[2]) * t,
        a[3] + (sign * b[3] - a[3]) * t,
    ])
}

/// Normalise the three basis columns of a column-major matrix.
/// Returns the orthonormal basis (as `[col][row]`) and the column lengths.
fn orthonormal_basis(m: &[[f32; 4]; 4]) -> ([[f32; 3]; 3], [f32; 3]) {
    let mut basis = [[0.0f32; 3]; 3];
    let mut scale = [1.0f32; 3];
    for c in 0..3 {
        let col = [m[c][0], m[c][1], m[c][2]];
        let len = (col[0] * col[0] + col[1] * col[1] + col[2] * col[2]).sqrt();
        scale[c] = len;
        basis[c] = if len > 1e-10 {
            [col[0] / len, col[1] / len, col[2] / len]
        } else {
            let mut unit = [0.0; 3];
            unit[c] = 1.0;
            unit
        };
    }
    (basis, scale)
}

impl Transform {
    /// The rotation this transform renders with, as a quaternion:
    /// `rotation_quat` when present, otherwise the Euler angles converted.
    pub fn effective_quat(&self) -> [f32; 4] {
        match self.rotation_quat {
            Some(q) => quat_normalize(&q),
            None => euler_deg_to_quat(self.rotation.x, self.rotation.y, self.rotation.z),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::mat4_mul;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn quat_close(a: &[f32; 4], b: &[f32; 4]) -> bool {
        // q and -q are the same rotation
        let same = (0..4).all(|i| close(a[i], b[i]));
        let flipped = (0..4).all(|i| close(a[i], -b[i]));
        same || flipped
    }

    #[test]
    fn euler_quat_matches_transform_matrix() {
        let t = Transform::default().with_rotation(Vec3::new(30.0, -70.0, 15.0));
        let euler_mat = t.to_matrix();
        let q = euler_deg_to_quat(30.0, -70.0, 15.0);
        let quat_mat = t.with_rotation_quat(q).to_matrix();
        for c in 0..4 {
            for r in 0..4 {
                assert!(
                    close(euler_mat[c][r], quat_mat[c][r]),
                    "mismatch at [{c}][{r}]: {} vs {}",
                    euler_mat[c][r],
                    quat_mat[c][r]
                );
            }
        }
    }

    #[test]
    fn mat4_to_quat_round_trips() {
        let q = euler_deg_to_quat(12.0, 140.0, -80.0);
        let m = Transform::default()
            .with_rotation_quat(q)
            .with_scale(Vec3::new(2.0, 0.5, 3.0))
            .to_matrix();
        assert!(quat_close(&mat4_to_quat(&m), &q));
        let s = mat4_scale(&m);
        assert!(close(s[0], 2.0) && close(s[1], 0.5) && close(s[2], 3.0));
    }

    #[test]
    fn rotate_vec3_matches_matrix() {
        let q = euler_deg_to_quat(0.0, 90.0, 0.0);
        let v = quat_rotate_vec3(&q, [1.0, 0.0, 0.0]);
        assert!(close(v[0], 0.0) && close(v[1], 0.0) && close(v[2], -1.0));
    }

    #[test]
    fn rigid_inverse_recovers_local_pose() {
        let parent = Transform::default()
            .with_position(Vec3::new(10.0, 2.0, -3.0))
            .with_rotation(Vec3::new(0.0, 45.0, 20.0));
        let local = Transform::default()
            .with_position(Vec3::new(0.0, 5.0, 1.0))
            .with_rotation_quat(euler_deg_to_quat(33.0, 0.0, -10.0));
        let world = mat4_mul(&parent.to_matrix(), &local.to_matrix());
        let (wp, wq) = mat4_to_rigid(&world);
        let (lp, lq) = rigid_inverse_apply(&parent.to_matrix(), wp, wq);
        assert!(
            close(lp.x, 0.0) && close(lp.y, 5.0) && close(lp.z, 1.0),
            "{lp:?}"
        );
        assert!(quat_close(&lq, &local.rotation_quat.unwrap()));
    }

    #[test]
    fn from_two_vectors_maps_a_onto_b() {
        let q = quat_from_two_vectors([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let v = quat_rotate_vec3(&q, [1.0, 0.0, 0.0]);
        assert!(close(v[0], 0.0) && close(v[1], 0.0) && close(v[2], 1.0), "{v:?}");
        // Opposite vectors: still a valid half turn.
        let q = quat_from_two_vectors([0.0, 1.0, 0.0], [0.0, -2.0, 0.0]);
        let v = quat_rotate_vec3(&q, [0.0, 1.0, 0.0]);
        assert!(close(v[1], -1.0), "{v:?}");
        // Same direction: identity.
        let q = quat_from_two_vectors([0.0, 0.0, 3.0], [0.0, 0.0, 1.0]);
        assert!(quat_close(&q, &[0.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn nlerp_endpoints_and_short_way() {
        let a = [0.0, 0.0, 0.0, 1.0];
        let b = euler_deg_to_quat(0.0, 90.0, 0.0);
        assert!(quat_close(&quat_nlerp(&a, &b, 0.0), &a));
        assert!(quat_close(&quat_nlerp(&a, &b, 1.0), &b));
        // -b is the same rotation; the midpoint must not swing the long way.
        let neg_b = [-b[0], -b[1], -b[2], -b[3]];
        let mid = quat_nlerp(&a, &neg_b, 0.5);
        let v = quat_rotate_vec3(&mid, [1.0, 0.0, 0.0]);
        assert!(v[2] < 0.0 && v[0] > 0.0, "{v:?}");
    }

    #[test]
    fn effective_quat_prefers_quaternion() {
        let q = [0.0, 0.70710677, 0.0, 0.70710677];
        let t = Transform::default()
            .with_rotation(Vec3::new(90.0, 0.0, 0.0))
            .with_rotation_quat(q);
        assert!(quat_close(&t.effective_quat(), &q));
        let e = Transform::default().with_rotation(Vec3::new(0.0, 90.0, 0.0));
        assert!(quat_close(&e.effective_quat(), &q));
    }
}
