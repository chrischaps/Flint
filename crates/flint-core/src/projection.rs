//! Screen-to-world picking on a camera's view-projection matrix (ADR 0075).
//!
//! Matrices are column-major (`m[col][row]`) in wgpu clip space: NDC x and
//! y run -1..1 (left to right, bottom to top) and depth runs 0..1 (near to
//! far). Screen points have their origin at the top-left with y down, in any
//! unit, as long as the viewport size is given in the same unit. Logical
//! points with a logical size and physical pixels with a physical size give
//! the same answer, so HiDPI scaling cancels out.
//!
//! The work is done in `f64`. A perspective matrix with a far plane thousands
//! of times its near plane loses most of an `f32` inverse's precision, and a
//! ground hit a long way from the camera would drift off the cursor.

/// Screen point to NDC (y flipped: screen y runs down, NDC y runs up).
pub fn screen_to_ndc(x: f32, y: f32, width: f32, height: f32) -> [f32; 2] {
    [x / width * 2.0 - 1.0, 1.0 - y / height * 2.0]
}

/// The world-space ray through an NDC point, as `(origin, direction)`.
///
/// `origin` lies on the near plane and `direction` is unit length, pointing
/// away from the camera. The same code serves perspective cameras (rays fan
/// out from the eye) and orthographic ones (parallel rays, origins spread
/// across the near plane). Returns `None` when the matrix is singular.
pub fn unproject_ray(view_proj: &[[f32; 4]; 4], ndc: [f32; 2]) -> Option<([f32; 3], [f32; 3])> {
    let inv = mat4_inverse_f64(view_proj)?;
    let near = transform_point(&inv, [ndc[0] as f64, ndc[1] as f64, 0.0])?;
    let far = transform_point(&inv, [ndc[0] as f64, ndc[1] as f64, 1.0])?;
    let d = [far[0] - near[0], far[1] - near[1], far[2] - near[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if len.is_nan() || len <= 1e-12 {
        return None;
    }
    Some((
        [near[0] as f32, near[1] as f32, near[2] as f32],
        [
            (d[0] / len) as f32,
            (d[1] / len) as f32,
            (d[2] / len) as f32,
        ],
    ))
}

/// Where a ray meets the horizontal plane `y = plane_y`, as `[x, z]`.
///
/// `None` when the ray runs parallel to the plane or the plane is behind the
/// ray's origin.
pub fn ray_hit_plane_y(origin: [f32; 3], dir: [f32; 3], plane_y: f32) -> Option<[f32; 2]> {
    if dir[1].abs() < 1e-6 {
        return None;
    }
    let t = (plane_y - origin[1]) / dir[1];
    if t < 0.0 {
        return None;
    }
    Some([origin[0] + dir[0] * t, origin[2] + dir[2] * t])
}

/// The point on the plane `y = plane_y` under a screen point, as `[x, z]`.
///
/// `(x, y)` is in the same unit as `width`/`height`, origin top-left. `None`
/// when the view doesn't reach the plane there: above the horizon, an
/// orthographic camera looking level, or a plane behind the camera.
pub fn screen_to_ground(
    view_proj: &[[f32; 4]; 4],
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    plane_y: f32,
) -> Option<[f32; 2]> {
    if width.is_nan() || height.is_nan() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let (origin, dir) = unproject_ray(view_proj, screen_to_ndc(x, y, width, height))?;
    ray_hit_plane_y(origin, dir, plane_y)
}

/// `m · (p, 1)` with the perspective divide. `None` when w is zero.
fn transform_point(m: &[[f64; 4]; 4], p: [f64; 3]) -> Option<[f64; 3]> {
    let mut out = [0.0f64; 4];
    for (row, o) in out.iter_mut().enumerate() {
        *o = m[0][row] * p[0] + m[1][row] * p[1] + m[2][row] * p[2] + m[3][row];
    }
    if out[3].abs() < 1e-300 {
        return None;
    }
    Some([out[0] / out[3], out[1] / out[3], out[2] / out[3]])
}

/// Inverse of a column-major 4x4, in `f64`, by Gauss-Jordan elimination with
/// partial pivoting. `None` when the matrix is singular.
pub(crate) fn mat4_inverse_f64(m: &[[f32; 4]; 4]) -> Option<[[f64; 4]; 4]> {
    // Row-major working copy [a | I].
    let mut a = [[0.0f64; 8]; 4];
    for (r, row) in a.iter_mut().enumerate() {
        for c in 0..4 {
            row[c] = m[c][r] as f64;
        }
        row[4 + r] = 1.0;
    }
    let scale = a
        .iter()
        .flat_map(|row| row[..4].iter())
        .fold(0.0f64, |acc, v| acc.max(v.abs()));
    if scale == 0.0 || !scale.is_finite() {
        return None;
    }
    for col in 0..4 {
        let pivot = (col..4)
            .max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))
            .unwrap_or(col);
        if a[pivot][col].abs() < scale * 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        let p = a[col][col];
        for v in a[col].iter_mut() {
            *v /= p;
        }
        let pivot_row = a[col];
        for (r, row) in a.iter_mut().enumerate() {
            let f = row[col];
            if r != col && f != 0.0 {
                for (v, p) in row.iter_mut().zip(pivot_row) {
                    *v -= f * p;
                }
            }
        }
    }
    // Back to column-major.
    let mut inv = [[0.0f64; 4]; 4];
    for (r, row) in a.iter().enumerate() {
        for c in 0..4 {
            inv[c][r] = row[4 + c];
        }
    }
    Some(inv)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> [[f32; 4]; 4] {
        [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ]
    }

    #[test]
    fn screen_to_ndc_corners_and_centre() {
        assert_eq!(screen_to_ndc(0.0, 0.0, 800.0, 600.0), [-1.0, 1.0]);
        assert_eq!(screen_to_ndc(800.0, 600.0, 800.0, 600.0), [1.0, -1.0]);
        assert_eq!(screen_to_ndc(400.0, 300.0, 800.0, 600.0), [0.0, 0.0]);
    }

    #[test]
    #[allow(clippy::needless_range_loop)] // matrix indices read best as indices
    fn inverse_round_trips() {
        let m = [
            [2.0, 0.5, 0.0, 0.0],
            [0.0, 3.0, 1.0, 0.0],
            [1.0, 0.0, 4.0, -1.0],
            [5.0, -2.0, 7.0, 1.0],
        ];
        let inv = mat4_inverse_f64(&m).unwrap();
        for i in 0..4 {
            for j in 0..4 {
                let mut s = 0.0;
                for k in 0..4 {
                    s += m[k][j] as f64 * inv[i][k];
                }
                let want = if i == j { 1.0 } else { 0.0 };
                assert!((s - want).abs() < 1e-12, "({i},{j}) = {s}");
            }
        }
    }

    #[test]
    fn singular_matrix_has_no_ray() {
        let mut m = identity();
        m[2] = [0.0; 4];
        assert!(unproject_ray(&m, [0.0, 0.0]).is_none());
        assert!(unproject_ray(&[[0.0; 4]; 4], [0.0, 0.0]).is_none());
        assert!(screen_to_ground(&m, 1.0, 1.0, 2.0, 2.0, 0.0).is_none());
    }

    #[test]
    fn zero_viewport_has_no_hit() {
        assert!(screen_to_ground(&identity(), 0.0, 0.0, 0.0, 600.0, 0.0).is_none());
    }

    #[test]
    fn plane_hits_and_misses() {
        // Straight down from y = 10 onto y = 2.
        assert_eq!(
            ray_hit_plane_y([3.0, 10.0, -4.0], [0.0, -1.0, 0.0], 2.0),
            Some([3.0, -4.0])
        );
        // Level ray: parallel, no hit.
        assert_eq!(
            ray_hit_plane_y([0.0, 10.0, 0.0], [1.0, 0.0, 0.0], 0.0),
            None
        );
        // Looking up at a plane below: it's behind the ray.
        assert_eq!(
            ray_hit_plane_y([0.0, 10.0, 0.0], [0.0, 1.0, 0.0], 0.0),
            None
        );
    }
}
