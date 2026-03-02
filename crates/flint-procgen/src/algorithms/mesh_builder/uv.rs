use crate::types::Vertex;
use flint_core::Vec3;

/// Projection axis for planar UV mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionAxis {
    /// Project onto XY plane (u = x, v = y)
    XY,
    /// Project onto XZ plane (u = x, v = z)
    XZ,
    /// Project onto YZ plane (u = y, v = z)
    YZ,
}

/// Apply cylindrical UV mapping around the Y axis.
///
/// - `u = atan2(dz, dx) / TAU + 0.5` (wraps 0..1 around the cylinder)
/// - `v = dy / height` (0 at bottom, 1 at top)
///
/// `center` is the base center of the cylinder, `height` is the total height.
/// `uv_scale` multiplies the final UV coordinates for tiling.
pub fn apply_cylindrical_uv(
    vertices: &mut [Vertex],
    center: Vec3,
    height: f32,
    uv_scale: [f32; 2],
) {
    let inv_height = if height.abs() > 1e-10 {
        1.0 / height
    } else {
        1.0
    };

    for v in vertices.iter_mut() {
        let p = Vec3::from_array(v.position);
        let d = p - center;

        let u = d.z.atan2(d.x) / std::f32::consts::TAU + 0.5;
        let vy = d.y * inv_height;

        v.uv = [u * uv_scale[0], vy * uv_scale[1]];
    }
}

/// Apply planar UV projection along the given axis.
///
/// The two remaining axes become u and v, scaled by `scale`.
pub fn apply_planar_uv(vertices: &mut [Vertex], axis: ProjectionAxis, scale: f32) {
    let inv_scale = if scale.abs() > 1e-10 {
        1.0 / scale
    } else {
        1.0
    };

    for v in vertices.iter_mut() {
        let p = v.position;
        let (u, vy) = match axis {
            ProjectionAxis::XY => (p[0], p[1]),
            ProjectionAxis::XZ => (p[0], p[2]),
            ProjectionAxis::YZ => (p[1], p[2]),
        };
        v.uv = [u * inv_scale, vy * inv_scale];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Vertex;

    fn default_vertex(pos: [f32; 3]) -> Vertex {
        Vertex {
            position: pos,
            normal: [0.0, 1.0, 0.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
        }
    }

    #[test]
    fn cylindrical_uv_wraps_zero_to_one() {
        let mut vertices = vec![
            default_vertex([1.0, 0.0, 0.0]),   // atan2(0, 1) = 0 → u = 0.5
            default_vertex([0.0, 0.0, 1.0]),   // atan2(1, 0) = PI/2 → u = 0.75
            default_vertex([-1.0, 0.0, 0.0]),  // atan2(0, -1) = PI → u = 1.0 (or 0.0)
            default_vertex([0.0, 5.0, 0.0]),   // top center → v = 1.0
        ];

        apply_cylindrical_uv(&mut vertices, Vec3::ZERO, 5.0, [1.0, 1.0]);

        // u should be in [0, 1]
        for v in &vertices {
            assert!(
                v.uv[0] >= -0.01 && v.uv[0] <= 1.01,
                "u should be in [0,1], got {}",
                v.uv[0]
            );
        }

        // Check specific values
        assert!((vertices[0].uv[0] - 0.5).abs() < 1e-4, "atan2(0,1) → u=0.5");
        assert!((vertices[1].uv[0] - 0.75).abs() < 1e-4, "atan2(1,0) → u=0.75");
        assert!((vertices[3].uv[1] - 1.0).abs() < 1e-4, "top → v=1.0");
    }

    #[test]
    fn cylindrical_uv_respects_scale() {
        let mut vertices = vec![default_vertex([1.0, 2.5, 0.0])];

        apply_cylindrical_uv(&mut vertices, Vec3::ZERO, 5.0, [2.0, 3.0]);

        // Without scale: u=0.5, v=0.5
        assert!((vertices[0].uv[0] - 1.0).abs() < 1e-4, "u should be 0.5*2.0=1.0");
        assert!((vertices[0].uv[1] - 1.5).abs() < 1e-4, "v should be 0.5*3.0=1.5");
    }

    #[test]
    fn planar_uv_xy() {
        let mut vertices = vec![
            default_vertex([3.0, 4.0, 99.0]),
            default_vertex([0.0, 0.0, 0.0]),
        ];

        apply_planar_uv(&mut vertices, ProjectionAxis::XY, 1.0);

        assert!((vertices[0].uv[0] - 3.0).abs() < 1e-5);
        assert!((vertices[0].uv[1] - 4.0).abs() < 1e-5);
    }

    #[test]
    fn planar_uv_xz() {
        let mut vertices = vec![default_vertex([3.0, 99.0, 4.0])];

        apply_planar_uv(&mut vertices, ProjectionAxis::XZ, 2.0);

        assert!((vertices[0].uv[0] - 1.5).abs() < 1e-5);
        assert!((vertices[0].uv[1] - 2.0).abs() < 1e-5);
    }

    #[test]
    fn planar_uv_yz() {
        let mut vertices = vec![default_vertex([99.0, 3.0, 4.0])];

        apply_planar_uv(&mut vertices, ProjectionAxis::YZ, 1.0);

        assert!((vertices[0].uv[0] - 3.0).abs() < 1e-5);
        assert!((vertices[0].uv[1] - 4.0).abs() < 1e-5);
    }
}
