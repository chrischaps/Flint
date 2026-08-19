/// Camera or light frustum defined by 6 clip planes.
///
/// Each plane `[a, b, c, d]` defines the half-space `ax + by + cz + d >= 0`
/// as visible. Planes are normalized (unit-length normal).
pub struct Frustum {
    pub planes: [[f32; 4]; 6], // left, right, bottom, top, near, far
}

impl Frustum {
    /// Extract 6 frustum planes from a view-projection matrix (Griggs/Hartmann method).
    ///
    /// Works for both perspective and orthographic projections.
    /// The input matrix should be `projection * view` (or `projection * view * model`
    /// if testing AABBs in model-local space).
    pub fn from_view_projection(vp: &[[f32; 4]; 4]) -> Self {
        // Columns of the VP matrix (vp is column-major: vp[col][row])
        let c0 = vp[0];
        let c1 = vp[1];
        let c2 = vp[2];
        let c3 = vp[3];

        let mut planes = [[0.0f32; 4]; 6];

        // Extract planes from VP matrix rows (Griggs/Hartmann method).
        // In column-major layout, row R = [c0[R], c1[R], c2[R], c3[R]].
        // Left  = row3 + row0
        planes[0] = [c0[3] + c0[0], c1[3] + c1[0], c2[3] + c2[0], c3[3] + c3[0]];
        // Right = row3 - row0
        planes[1] = [c0[3] - c0[0], c1[3] - c1[0], c2[3] - c2[0], c3[3] - c3[0]];
        // Bottom = row3 + row1
        planes[2] = [c0[3] + c0[1], c1[3] + c1[1], c2[3] + c2[1], c3[3] + c3[1]];
        // Top = row3 - row1
        planes[3] = [c0[3] - c0[1], c1[3] - c1[1], c2[3] - c2[1], c3[3] - c3[1]];
        // Near = row3 + row2
        planes[4] = [c0[3] + c0[2], c1[3] + c1[2], c2[3] + c2[2], c3[3] + c3[2]];
        // Far = row3 - row2
        planes[5] = [c0[3] - c0[2], c1[3] - c1[2], c2[3] - c2[2], c3[3] - c3[2]];

        // Normalize each plane
        for plane in &mut planes {
            let len = (plane[0] * plane[0] + plane[1] * plane[1] + plane[2] * plane[2]).sqrt();
            if len > 1e-10 {
                plane[0] /= len;
                plane[1] /= len;
                plane[2] /= len;
                plane[3] /= len;
            }
        }

        Self { planes }
    }

    /// Conservative AABB visibility test (p-vertex method).
    ///
    /// Returns `true` if the AABB is potentially visible (inside or intersecting
    /// the frustum). Returns `false` only when the AABB is fully outside at least
    /// one frustum plane. No false negatives — partially visible boxes always pass.
    pub fn aabb_visible(&self, aabb_min: [f32; 3], aabb_max: [f32; 3]) -> bool {
        for plane in &self.planes {
            let (a, b, c, d) = (plane[0], plane[1], plane[2], plane[3]);

            // P-vertex: the AABB corner most in the direction of the plane normal
            let px = if a >= 0.0 { aabb_max[0] } else { aabb_min[0] };
            let py = if b >= 0.0 { aabb_max[1] } else { aabb_min[1] };
            let pz = if c >= 0.0 { aabb_max[2] } else { aabb_min[2] };

            // If the p-vertex is behind this plane, the entire AABB is outside
            if a * px + b * py + c * pz + d < 0.0 {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a simple perspective VP looking down -Z from origin.
    /// FOV 90°, aspect 1:1, near 0.1, far 100.
    /// Uses the same [-1,1] NDC convention as the engine's `Camera::perspective_matrix()`.
    fn test_perspective_vp() -> [[f32; 4]; 4] {
        let fov = std::f32::consts::FRAC_PI_2; // 90 degrees
        let aspect = 1.0;
        let near = 0.1_f32;
        let far = 100.0_f32;

        let f = 1.0 / (fov / 2.0).tan();
        let depth = far - near;
        // Perspective projection (right-handed, [-1,1] depth — matches engine convention)
        let proj = [
            [f / aspect, 0.0, 0.0, 0.0],
            [0.0, f, 0.0, 0.0],
            [0.0, 0.0, -(far + near) / depth, -1.0],
            [0.0, 0.0, -(2.0 * far * near) / depth, 0.0],
        ];

        // View: camera at (0,0,0) looking down -Z (identity — already looking down -Z in RH)
        let view: [[f32; 4]; 4] = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];

        mat4_mul(&proj, &view)
    }

    /// Helper: multiply two 4×4 matrices (column-major, matching flint_core).
    fn mat4_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
        let mut out = [[0.0f32; 4]; 4];
        for col in 0..4 {
            for row in 0..4 {
                out[col][row] = a[0][row] * b[col][0]
                    + a[1][row] * b[col][1]
                    + a[2][row] * b[col][2]
                    + a[3][row] * b[col][3];
            }
        }
        out
    }

    #[test]
    fn aabb_in_front_of_camera_is_visible() {
        let vp = test_perspective_vp();
        let frustum = Frustum::from_view_projection(&vp);
        // Small box at z = -5 (directly in front)
        assert!(frustum.aabb_visible([-1.0, -1.0, -6.0], [1.0, 1.0, -4.0]));
    }

    #[test]
    fn aabb_behind_camera_is_culled() {
        let vp = test_perspective_vp();
        let frustum = Frustum::from_view_projection(&vp);
        // Box behind camera (positive Z in right-handed looking down -Z)
        assert!(!frustum.aabb_visible([-1.0, -1.0, 5.0], [1.0, 1.0, 10.0]));
    }

    #[test]
    fn aabb_far_left_is_culled() {
        let vp = test_perspective_vp();
        let frustum = Frustum::from_view_projection(&vp);
        // Box far to the left, at z=-5 but x = -50..-40
        assert!(!frustum.aabb_visible([-50.0, -1.0, -6.0], [-40.0, 1.0, -4.0]));
    }

    #[test]
    fn aabb_partially_intersecting_is_visible() {
        let vp = test_perspective_vp();
        let frustum = Frustum::from_view_projection(&vp);
        // Large box that straddles the left frustum edge
        assert!(frustum.aabb_visible([-20.0, -1.0, -6.0], [1.0, 1.0, -4.0]));
    }

    #[test]
    fn very_large_aabb_always_visible() {
        let vp = test_perspective_vp();
        let frustum = Frustum::from_view_projection(&vp);
        // Huge box surrounding everything
        assert!(frustum.aabb_visible([-1000.0, -1000.0, -1000.0], [1000.0, 1000.0, 1000.0]));
    }

    #[test]
    fn flat_aabb_on_ground_plane_visible() {
        let vp = test_perspective_vp();
        let frustum = Frustum::from_view_projection(&vp);
        // Zero-height AABB (flat plane) in front of camera
        assert!(frustum.aabb_visible([-2.0, 0.0, -10.0], [2.0, 0.0, -2.0]));
    }

    #[test]
    fn aabb_beyond_far_plane_is_culled() {
        let vp = test_perspective_vp();
        let frustum = Frustum::from_view_projection(&vp);
        // Box beyond the far plane (z = -200 to -150, far = 100)
        assert!(!frustum.aabb_visible([-1.0, -1.0, -200.0], [1.0, 1.0, -150.0]));
    }

    #[test]
    fn offset_aabb_simulates_terrain_transform() {
        // Simulates terrain at position [-192, 0, -192]:
        // local AABB [0,0,0]..[96,25,96] → world AABB [-192,0,-192]..[-96,25,-96]
        // Camera looking down -Z should see this (it's at negative Z)
        let vp = test_perspective_vp();
        let frustum = Frustum::from_view_projection(&vp);
        assert!(frustum.aabb_visible([-192.0, 0.0, -192.0], [-96.0, 25.0, -96.0]));

        // Local AABB without offset would be at [0,0,0]..[96,25,96] — behind camera
        assert!(!frustum.aabb_visible([0.0, 0.0, 0.0], [96.0, 25.0, 96.0]));
    }
}
