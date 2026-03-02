use crate::types::{BoundingBox, MaterialData, MeshData, Vertex};
use crate::{ProcGenError, Result};
use flint_core::spline::SplineSample;
use flint_core::Vec3;
use std::f32::consts::TAU;

/// A single point along an extrusion spine.
#[derive(Debug, Clone)]
pub struct ExtrudePoint {
    pub position: Vec3,
    pub forward: Vec3, // tangent to curve
    pub up: Vec3,      // perpendicular basis vector
    pub radius: f32,
}

/// Parameters for extruding a tube along a curve.
#[derive(Debug, Clone)]
pub struct ExtrudeParams {
    pub spine: Vec<ExtrudePoint>,
    pub radial_segments: u32,
    pub caps: bool,
    pub uv_length_scale: f32,
}

/// Extrude a tube along a curve defined by spine points.
///
/// At each spine point, generates a ring of vertices using the forward/up basis.
/// Adjacent rings are connected with quads. UV v is proportional to cumulative
/// arc length along the spine.
pub fn extrude_along_curve(params: &ExtrudeParams) -> Result<MeshData> {
    let spine = &params.spine;
    if spine.len() < 2 {
        return Err(ProcGenError::InvalidParameter {
            name: "spine".into(),
            reason: "extrusion requires at least 2 spine points".into(),
        });
    }

    let r = params.radial_segments;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    // Compute cumulative arc lengths for UV v-coordinate
    let mut arc_lengths = vec![0.0f32];
    for i in 1..spine.len() {
        let seg_len = (spine[i].position - spine[i - 1].position).length();
        arc_lengths.push(arc_lengths[i - 1] + seg_len);
    }
    let total_length = *arc_lengths.last().unwrap();
    let inv_length = if total_length > 1e-10 {
        params.uv_length_scale / total_length
    } else {
        1.0
    };

    // Generate rings
    for (j, point) in spine.iter().enumerate() {
        let fwd = point.forward.normalized();
        let up = point.up.normalized();
        let right = fwd.cross(&up).normalized();
        // Recompute up to ensure orthogonal basis
        let up = right.cross(&fwd).normalized();

        let v_coord = arc_lengths[j] * inv_length;

        for i in 0..=r {
            let theta = (i as f32 / r as f32) * TAU;
            let cos_t = theta.cos();
            let sin_t = theta.sin();

            // Position on the ring
            let offset = right * (cos_t * point.radius) + up * (sin_t * point.radius);
            let pos = point.position + offset;

            // Normal points outward from center
            let normal = (right * cos_t + up * sin_t).normalized();

            let u = i as f32 / r as f32;

            vertices.push(Vertex {
                position: pos.to_array(),
                normal: normal.to_array(),
                tangent: [0.0, 0.0, 0.0, 1.0],
                uv: [u, v_coord],
            });
        }
    }

    // Connect adjacent rings with quads
    let row_width = r + 1;
    for j in 0..(spine.len() as u32 - 1) {
        for i in 0..r {
            let a = j * row_width + i;
            let b = a + 1;
            let c = (j + 1) * row_width + i;
            let d = c + 1;

            indices.push(a);
            indices.push(c);
            indices.push(b);

            indices.push(b);
            indices.push(c);
            indices.push(d);
        }
    }

    // Caps
    if params.caps {
        // Start cap
        if spine[0].radius > 1e-10 {
            let center_idx = vertices.len() as u32;
            let fwd = spine[0].forward.normalized();
            vertices.push(Vertex {
                position: spine[0].position.to_array(),
                normal: (fwd * -1.0).to_array(),
                tangent: [0.0, 0.0, 0.0, 1.0],
                uv: [0.5, 0.5],
            });

            let first_ring_start = 0u32;
            for i in 0..r {
                indices.push(center_idx);
                indices.push(first_ring_start + i + 1);
                indices.push(first_ring_start + i);
            }
        }

        // End cap
        let last = spine.len() - 1;
        if spine[last].radius > 1e-10 {
            let center_idx = vertices.len() as u32;
            let fwd = spine[last].forward.normalized();
            vertices.push(Vertex {
                position: spine[last].position.to_array(),
                normal: fwd.to_array(),
                tangent: [0.0, 0.0, 0.0, 1.0],
                uv: [0.5, 0.5],
            });

            let last_ring_start = last as u32 * row_width;
            for i in 0..r {
                indices.push(center_idx);
                indices.push(last_ring_start + i);
                indices.push(last_ring_start + i + 1);
            }
        }
    }

    let bbox = BoundingBox::from_positions(
        &vertices.iter().map(|v| v.position).collect::<Vec<_>>(),
    );

    Ok(MeshData {
        vertices,
        indices,
        materials: vec![MaterialData::default()],
        bounding_box: bbox,
    })
}

/// Build a spine from a sequence of positions with linearly interpolated radius.
///
/// Forward vectors are computed via central differences. The up vector uses
/// parallel transport: project the previous up onto the plane perpendicular
/// to the new forward direction, preventing twist accumulation.
pub fn build_spine_from_positions(
    positions: &[Vec3],
    radius_start: f32,
    radius_end: f32,
) -> Vec<ExtrudePoint> {
    if positions.len() < 2 {
        return Vec::new();
    }

    let n = positions.len();
    let mut spine = Vec::with_capacity(n);

    // Compute forward vectors via central differences
    let mut forwards = Vec::with_capacity(n);
    for i in 0..n {
        let fwd = if i == 0 {
            positions[1] - positions[0]
        } else if i == n - 1 {
            positions[n - 1] - positions[n - 2]
        } else {
            positions[i + 1] - positions[i - 1]
        };
        forwards.push(fwd.normalized());
    }

    // Parallel transport for up vectors
    let mut up = initial_up(&forwards[0]);

    for i in 0..n {
        let t = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 };
        let radius = radius_start + (radius_end - radius_start) * t;

        if i > 0 {
            // Project previous up onto plane perpendicular to new forward
            let fwd = forwards[i];
            let proj = fwd * fwd.dot(&up);
            let new_up = up - proj;
            let len = new_up.length();
            if len > 1e-10 {
                up = new_up * (1.0 / len);
            }
            // If degenerate, keep previous up
        }

        spine.push(ExtrudePoint {
            position: positions[i],
            forward: forwards[i],
            up,
            radius,
        });
    }

    spine
}

/// Convert spline samples to extrude points with linearly interpolated radius.
///
/// `SplineSample` already has position, forward, and up basis vectors from
/// Catmull-Rom evaluation, so this is a direct mapping.
pub fn spine_from_spline_samples(
    samples: &[SplineSample],
    radius_start: f32,
    radius_end: f32,
) -> Vec<ExtrudePoint> {
    let n = samples.len();
    samples
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let t = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 };
            let radius = radius_start + (radius_end - radius_start) * t;
            ExtrudePoint {
                position: s.position,
                forward: s.forward,
                up: s.up,
                radius,
            }
        })
        .collect()
}

/// Pick an initial up vector that isn't parallel to the forward direction.
fn initial_up(forward: &Vec3) -> Vec3 {
    let candidate = if forward.y.abs() < 0.9 {
        Vec3::UP
    } else {
        Vec3::RIGHT
    };
    let right = forward.cross(&candidate).normalized();
    right.cross(forward).normalized()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_tube_resembles_cylinder() {
        let positions: Vec<Vec3> = (0..=10)
            .map(|i| Vec3::new(0.0, i as f32 * 0.5, 0.0))
            .collect();
        let spine = build_spine_from_positions(&positions, 1.0, 1.0);

        let mesh = extrude_along_curve(&ExtrudeParams {
            spine,
            radial_segments: 8,
            caps: false,
            uv_length_scale: 1.0,
        })
        .unwrap();

        assert!(mesh.validate().is_ok());
        // Should have (8+1) * 11 = 99 vertices
        assert_eq!(mesh.vertex_count(), 99);
    }

    #[test]
    fn extrude_validates() {
        let spine = build_spine_from_positions(
            &[
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 2.0, 1.0),
                Vec3::new(0.0, 3.0, 0.0),
            ],
            0.5,
            0.2,
        );

        let mesh = extrude_along_curve(&ExtrudeParams {
            spine,
            radial_segments: 12,
            caps: true,
            uv_length_scale: 1.0,
        })
        .unwrap();

        assert!(mesh.validate().is_ok());
    }

    #[test]
    fn bbox_encloses_spine() {
        let positions = vec![
            Vec3::new(-3.0, 0.0, 0.0),
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(3.0, 10.0, 0.0),
        ];
        let spine = build_spine_from_positions(&positions, 0.5, 0.5);

        let mesh = extrude_along_curve(&ExtrudeParams {
            spine,
            radial_segments: 8,
            caps: false,
            uv_length_scale: 1.0,
        })
        .unwrap();

        let bb = &mesh.bounding_box;
        for pos in &positions {
            // Spine positions should be within the bounding box (with radius margin)
            assert!(
                pos.x >= bb.min.x - 0.6 && pos.x <= bb.max.x + 0.6,
                "x={} not in bbox [{}, {}]",
                pos.x,
                bb.min.x,
                bb.max.x
            );
            assert!(
                pos.y >= bb.min.y - 0.6 && pos.y <= bb.max.y + 0.6,
                "y={} not in bbox [{}, {}]",
                pos.y,
                bb.min.y,
                bb.max.y
            );
        }
    }

    #[test]
    fn parallel_transport_no_twist() {
        // Spine along Z axis — up should stay consistent (no twist)
        let positions: Vec<Vec3> = (0..=20)
            .map(|i| Vec3::new(0.0, 0.0, i as f32 * 0.5))
            .collect();

        let spine = build_spine_from_positions(&positions, 0.5, 0.5);

        // All up vectors should be consistent (parallel transport preserves direction
        // for a straight line)
        let first_up = spine[0].up;
        for point in &spine {
            let dot = first_up.x * point.up.x + first_up.y * point.up.y + first_up.z * point.up.z;
            assert!(
                dot > 0.99,
                "up should remain consistent along straight spine, dot={}, up={:?}",
                dot,
                point.up
            );
        }

        // For a curved spine, verify up doesn't flip
        let curved_positions: Vec<Vec3> = (0..=20)
            .map(|i| {
                let t = i as f32 / 20.0;
                let angle = t * std::f32::consts::FRAC_PI_2;
                Vec3::new(angle.sin() * 5.0, angle.cos() * 5.0, 0.0)
            })
            .collect();

        let curved_spine = build_spine_from_positions(&curved_positions, 0.5, 0.5);

        // Adjacent up vectors should not flip (dot product > 0)
        for i in 1..curved_spine.len() {
            let prev_up = curved_spine[i - 1].up;
            let curr_up = curved_spine[i].up;
            let dot = prev_up.x * curr_up.x + prev_up.y * curr_up.y + prev_up.z * curr_up.z;
            assert!(
                dot > 0.5,
                "up should not flip between adjacent points: dot={} at index {}",
                dot,
                i
            );
        }
    }

    #[test]
    fn fewer_than_two_points_errors() {
        let result = extrude_along_curve(&ExtrudeParams {
            spine: vec![ExtrudePoint {
                position: Vec3::ZERO,
                forward: Vec3::UP,
                up: Vec3::FORWARD,
                radius: 1.0,
            }],
            radial_segments: 8,
            caps: false,
            uv_length_scale: 1.0,
        });
        assert!(result.is_err());
    }

    #[test]
    fn empty_positions_returns_empty_spine() {
        let spine = build_spine_from_positions(&[], 1.0, 0.5);
        assert!(spine.is_empty());
    }

    #[test]
    fn spine_from_spline_samples_interpolates_radius() {
        let samples = vec![
            SplineSample {
                position: Vec3::ZERO,
                forward: Vec3::UP,
                right: Vec3::RIGHT,
                up: Vec3::new(0.0, 0.0, -1.0),
                twist: 0.0,
                t: 0.0,
            },
            SplineSample {
                position: Vec3::new(0.0, 5.0, 0.0),
                forward: Vec3::UP,
                right: Vec3::RIGHT,
                up: Vec3::new(0.0, 0.0, -1.0),
                twist: 0.0,
                t: 1.0,
            },
        ];

        let spine = spine_from_spline_samples(&samples, 2.0, 0.5);
        assert_eq!(spine.len(), 2);
        assert!((spine[0].radius - 2.0).abs() < 1e-5);
        assert!((spine[1].radius - 0.5).abs() < 1e-5);
    }
}
