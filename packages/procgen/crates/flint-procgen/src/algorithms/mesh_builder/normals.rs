use crate::types::Vertex;
use flint_core::Vec3;

/// Compute area-weighted smooth normals for a triangle mesh.
///
/// Each triangle contributes its (unnormalized) cross product to its three vertices.
/// Because `|cross(e0, e1)| = 2 * triangle_area`, larger faces contribute more.
/// After accumulation every vertex normal is normalized to unit length.
/// Degenerate vertices (zero accumulated normal) fall back to `[0, 1, 0]`.
pub fn compute_normals(vertices: &mut [Vertex], indices: &[u32]) {
    // Zero out existing normals
    for v in vertices.iter_mut() {
        v.normal = [0.0, 0.0, 0.0];
    }

    // Accumulate face-area-weighted normals
    for tri in indices.chunks_exact(3) {
        let i0 = tri[0] as usize;
        let i1 = tri[1] as usize;
        let i2 = tri[2] as usize;

        let p0 = Vec3::from_array(vertices[i0].position);
        let p1 = Vec3::from_array(vertices[i1].position);
        let p2 = Vec3::from_array(vertices[i2].position);

        let e0 = p1 - p0;
        let e1 = p2 - p0;
        let face_normal = e0.cross(&e1); // unnormalized = 2 * area * unit_normal

        for &idx in &[i0, i1, i2] {
            vertices[idx].normal[0] += face_normal.x;
            vertices[idx].normal[1] += face_normal.y;
            vertices[idx].normal[2] += face_normal.z;
        }
    }

    // Normalize
    for v in vertices.iter_mut() {
        let n = Vec3::from_array(v.normal);
        let len = n.length();
        if len > 1e-10 {
            let unit = n * (1.0 / len);
            v.normal = unit.to_array();
        } else {
            v.normal = [0.0, 1.0, 0.0]; // degenerate fallback
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Vertex;

    fn default_vertex(pos: [f32; 3]) -> Vertex {
        Vertex {
            position: pos,
            normal: [0.0, 0.0, 0.0],
            tangent: [0.0, 0.0, 0.0, 0.0],
            uv: [0.0, 0.0],
        }
    }

    #[test]
    fn flat_quad_normals_equal_face_normal() {
        // Two triangles forming a flat quad on the XZ plane (CCW from +Y)
        let mut vertices = vec![
            default_vertex([0.0, 0.0, 0.0]),
            default_vertex([1.0, 0.0, 0.0]),
            default_vertex([1.0, 0.0, 1.0]),
            default_vertex([0.0, 0.0, 1.0]),
        ];
        // Winding: 0,3,2 and 0,2,1 → CCW from above → normal +Y
        let indices = vec![0, 3, 2, 0, 2, 1];

        compute_normals(&mut vertices, &indices);

        for v in &vertices {
            assert!(
                (v.normal[0]).abs() < 1e-5,
                "normal.x should be ~0, got {}",
                v.normal[0]
            );
            assert!(
                (v.normal[1] - 1.0).abs() < 1e-5,
                "normal.y should be ~1, got {}",
                v.normal[1]
            );
            assert!(
                (v.normal[2]).abs() < 1e-5,
                "normal.z should be ~0, got {}",
                v.normal[2]
            );
        }
    }

    #[test]
    fn normals_are_unit_length() {
        let mut vertices = vec![
            default_vertex([0.0, 0.0, 0.0]),
            default_vertex([1.0, 0.0, 0.0]),
            default_vertex([0.0, 1.0, 0.0]),
        ];
        let indices = vec![0, 1, 2];

        compute_normals(&mut vertices, &indices);

        for v in &vertices {
            let len = Vec3::from_array(v.normal).length();
            assert!(
                (len - 1.0).abs() < 1e-5,
                "normal length should be 1.0, got {}",
                len
            );
        }
    }

    #[test]
    fn degenerate_triangle_fallback() {
        // Collinear vertices produce a degenerate triangle
        let mut vertices = vec![
            default_vertex([0.0, 0.0, 0.0]),
            default_vertex([1.0, 0.0, 0.0]),
            default_vertex([2.0, 0.0, 0.0]),
        ];
        let indices = vec![0, 1, 2];

        compute_normals(&mut vertices, &indices);

        for v in &vertices {
            assert_eq!(
                v.normal,
                [0.0, 1.0, 0.0],
                "degenerate should fall back to UP"
            );
        }
    }

    #[test]
    fn area_weighting_favors_large_triangles() {
        // Shared vertex between a small and large triangle on different planes.
        // The large triangle's normal should dominate.
        let mut vertices = vec![
            // Small triangle in XY plane (area = 0.005)
            default_vertex([0.0, 0.0, 0.0]), // 0 (shared)
            default_vertex([0.1, 0.0, 0.0]), // 1
            default_vertex([0.0, 0.1, 0.0]), // 2
            // Large triangle in XZ plane (area = 50)
            default_vertex([10.0, 0.0, 0.0]), // 3
            default_vertex([0.0, 0.0, 10.0]), // 4
        ];
        // Small tri: 0,1,2 → normal +Z
        // Large tri: 0,3,4 → normal +Y
        let indices = vec![0, 1, 2, 0, 3, 4];

        compute_normals(&mut vertices, &indices);

        let shared = Vec3::from_array(vertices[0].normal);
        // Y component (from large tri) should dominate over Z component (from small tri)
        assert!(
            shared.y.abs() > shared.z.abs() * 10.0,
            "large triangle should dominate: y={}, z={}",
            shared.y,
            shared.z
        );
    }
}
