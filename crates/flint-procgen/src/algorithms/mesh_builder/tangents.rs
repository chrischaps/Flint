use crate::types::Vertex;
use flint_core::Vec3;

/// Compute MikkTSpace-style tangents from UV-space edge derivatives.
///
/// For each triangle, derives tangent and bitangent from the UV-space edges,
/// accumulates per-vertex, then Gram-Schmidt orthogonalizes against the normal.
/// Handedness (`tangent.w`) is `sign(dot(cross(N, T), B))`.
///
/// Normals must be computed before calling this function.
pub fn compute_tangents(vertices: &mut [Vertex], indices: &[u32]) {
    let n = vertices.len();
    let mut tan1 = vec![Vec3::ZERO; n]; // tangent accumulator
    let mut tan2 = vec![Vec3::ZERO; n]; // bitangent accumulator

    for tri in indices.chunks_exact(3) {
        let i0 = tri[0] as usize;
        let i1 = tri[1] as usize;
        let i2 = tri[2] as usize;

        let p0 = Vec3::from_array(vertices[i0].position);
        let p1 = Vec3::from_array(vertices[i1].position);
        let p2 = Vec3::from_array(vertices[i2].position);

        let uv0 = vertices[i0].uv;
        let uv1 = vertices[i1].uv;
        let uv2 = vertices[i2].uv;

        let dp1 = p1 - p0;
        let dp2 = p2 - p0;

        let duv1 = [uv1[0] - uv0[0], uv1[1] - uv0[1]];
        let duv2 = [uv2[0] - uv0[0], uv2[1] - uv0[1]];

        let det = duv1[0] * duv2[1] - duv1[1] * duv2[0];

        // Skip degenerate triangles (zero UV area)
        if det.abs() < 1e-10 {
            continue;
        }

        let inv_det = 1.0 / det;

        let tangent = (dp1 * (duv2[1] * inv_det)) + (dp2 * -(duv1[1] * inv_det));
        let bitangent = (dp2 * (duv1[0] * inv_det)) + (dp1 * -(duv2[0] * inv_det));

        for &idx in &[i0, i1, i2] {
            tan1[idx] = tan1[idx] + tangent;
            tan2[idx] = tan2[idx] + bitangent;
        }
    }

    // Gram-Schmidt orthogonalize and compute handedness
    for (i, v) in vertices.iter_mut().enumerate() {
        let n = Vec3::from_array(v.normal);
        let t = tan1[i];
        let b = tan2[i];

        // Orthogonalize: T' = normalize(T - N * dot(N, T))
        let t_proj = t - n * n.dot(&t);
        let len = t_proj.length();

        if len < 1e-10 {
            // Fallback: pick an arbitrary tangent perpendicular to normal
            let fallback = if n.x.abs() < 0.9 {
                Vec3::RIGHT
            } else {
                Vec3::UP
            };
            let perp = n.cross(&fallback).normalized();
            v.tangent = [perp.x, perp.y, perp.z, 1.0];
            continue;
        }

        let t_ortho = t_proj * (1.0 / len);

        // Handedness: w = sign(dot(cross(N, T), B))
        let w = if n.cross(&t_ortho).dot(&b) < 0.0 {
            -1.0
        } else {
            1.0
        };

        v.tangent = [t_ortho.x, t_ortho.y, t_ortho.z, w];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algorithms::mesh_builder::normals::compute_normals;
    use crate::types::Vertex;

    fn make_vertex(pos: [f32; 3], uv: [f32; 2]) -> Vertex {
        Vertex {
            position: pos,
            normal: [0.0, 0.0, 0.0],
            tangent: [0.0, 0.0, 0.0, 0.0],
            uv,
        }
    }

    #[test]
    fn tangents_perpendicular_to_normals() {
        let mut vertices = vec![
            make_vertex([0.0, 0.0, 0.0], [0.0, 0.0]),
            make_vertex([1.0, 0.0, 0.0], [1.0, 0.0]),
            make_vertex([1.0, 0.0, 1.0], [1.0, 1.0]),
            make_vertex([0.0, 0.0, 1.0], [0.0, 1.0]),
        ];
        let indices = vec![0, 1, 2, 0, 2, 3];

        compute_normals(&mut vertices, &indices);
        compute_tangents(&mut vertices, &indices);

        for v in &vertices {
            let n = Vec3::from_array(v.normal);
            let t = Vec3::new(v.tangent[0], v.tangent[1], v.tangent[2]);
            let d = n.dot(&t).abs();
            assert!(
                d < 1e-4,
                "tangent should be perpendicular to normal, dot={}",
                d
            );
        }
    }

    #[test]
    fn tangent_xyz_unit_length() {
        let mut vertices = vec![
            make_vertex([0.0, 0.0, 0.0], [0.0, 0.0]),
            make_vertex([1.0, 0.0, 0.0], [1.0, 0.0]),
            make_vertex([0.0, 1.0, 0.0], [0.0, 1.0]),
        ];
        let indices = vec![0, 1, 2];

        compute_normals(&mut vertices, &indices);
        compute_tangents(&mut vertices, &indices);

        for v in &vertices {
            let t = Vec3::new(v.tangent[0], v.tangent[1], v.tangent[2]);
            let len = t.length();
            assert!(
                (len - 1.0).abs() < 1e-4,
                "tangent xyz should be unit length, got {}",
                len
            );
        }
    }

    #[test]
    fn handedness_is_plus_or_minus_one() {
        let mut vertices = vec![
            make_vertex([0.0, 0.0, 0.0], [0.0, 0.0]),
            make_vertex([1.0, 0.0, 0.0], [1.0, 0.0]),
            make_vertex([0.0, 0.0, 1.0], [0.0, 1.0]),
        ];
        let indices = vec![0, 1, 2];

        compute_normals(&mut vertices, &indices);
        compute_tangents(&mut vertices, &indices);

        for v in &vertices {
            let w = v.tangent[3];
            assert!(
                (w - 1.0).abs() < 1e-5 || (w + 1.0).abs() < 1e-5,
                "handedness should be +/-1, got {}",
                w
            );
        }
    }

    #[test]
    fn flat_quad_tangent_aligns_with_u_axis() {
        // Quad on XZ plane, UV u-axis along +X
        let mut vertices = vec![
            make_vertex([0.0, 0.0, 0.0], [0.0, 0.0]),
            make_vertex([1.0, 0.0, 0.0], [1.0, 0.0]),
            make_vertex([1.0, 0.0, 1.0], [1.0, 1.0]),
            make_vertex([0.0, 0.0, 1.0], [0.0, 1.0]),
        ];
        let indices = vec![0, 1, 2, 0, 2, 3];

        compute_normals(&mut vertices, &indices);
        compute_tangents(&mut vertices, &indices);

        for v in &vertices {
            let t = Vec3::new(v.tangent[0], v.tangent[1], v.tangent[2]);
            // Tangent should align with +X (UV u-direction)
            assert!(
                t.x.abs() > 0.9,
                "tangent should align with u-axis (+X), got {:?}",
                [t.x, t.y, t.z]
            );
        }
    }
}
