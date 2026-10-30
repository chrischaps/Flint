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

/// A closed 2D cross-section swept along a spine by
/// [`extrude_profile_along_curve`].
///
/// Points live in the spine's local plane (x along the frame's right vector,
/// y along up) and are scaled by each spine point's radius, so a unit-sized
/// profile tapers with the spine. The loop runs counter-clockwise and its
/// last entry repeats the first position with `u = 1`, giving the texture a
/// seam instead of a wrap-around smear.
#[derive(Debug, Clone)]
pub struct CrossSection {
    pub points: Vec<[f32; 2]>,
    /// Outward unit normal at each point, in the same plane.
    pub normals: Vec<[f32; 2]>,
    /// Texture u at each point: arc length around the loop, 0 to 1.
    pub u: Vec<f32>,
    /// `connect[k]` joins point `k` to `k + 1`. False between the two copies
    /// of a crease point, which share a position but not a normal.
    pub connect: Vec<bool>,
}

impl CrossSection {
    /// A unit circle of `segments` sides with analytic normals.
    pub fn circle(segments: u32) -> Self {
        let mut points = Vec::with_capacity(segments as usize + 1);
        let mut u = Vec::with_capacity(segments as usize + 1);
        for i in 0..=segments {
            let theta = (i as f32 / segments as f32) * TAU;
            points.push([theta.cos(), theta.sin()]);
            u.push(i as f32 / segments as f32);
        }
        Self {
            normals: points.clone(),
            connect: vec![true; segments as usize],
            points,
            u,
        }
    }

    /// A closed counter-clockwise polygon, shaded smooth: each point's normal
    /// averages its two edges' normals. Do not repeat the first point.
    pub fn smooth(points: &[[f32; 2]]) -> Self {
        let n = points.len();
        let edge_normals = polygon_edge_normals(points);
        let lengths = cumulative_lengths(points);
        let mut out = Self::empty();
        for (k, &u) in lengths.iter().enumerate() {
            let i = k % n;
            let prev = edge_normals[(i + n - 1) % n];
            let next = edge_normals[i];
            out.points.push(points[i]);
            out.normals
                .push(normalize2([prev[0] + next[0], prev[1] + next[1]]));
            out.u.push(u);
        }
        out.connect = vec![true; n];
        out
    }

    /// A closed counter-clockwise polygon with a hard crease at every point:
    /// each edge gets its own two vertices and a flat normal, for faceted
    /// shapes like crystals or cut stone. Do not repeat the first point.
    pub fn faceted(points: &[[f32; 2]]) -> Self {
        let n = points.len();
        let edge_normals = polygon_edge_normals(points);
        let lengths = cumulative_lengths(points);
        let mut out = Self::empty();
        for (i, &normal) in edge_normals.iter().enumerate() {
            for k in [i, i + 1] {
                out.points.push(points[k % n]);
                out.normals.push(normal);
                out.u.push(lengths[k]);
            }
            out.connect.push(true);
            if i + 1 < n {
                out.connect.push(false);
            }
        }
        out
    }

    /// A star of `tips` points alternating between radius 1 and `inner`,
    /// faceted.
    pub fn star(tips: u32, inner: f32) -> Self {
        let points: Vec<[f32; 2]> = (0..tips * 2)
            .map(|i| {
                let theta = (i as f32 / (tips * 2) as f32) * TAU;
                let r = if i % 2 == 0 { 1.0 } else { inner };
                [theta.cos() * r, theta.sin() * r]
            })
            .collect();
        Self::faceted(&points)
    }

    fn empty() -> Self {
        Self {
            points: Vec::new(),
            normals: Vec::new(),
            u: Vec::new(),
            connect: Vec::new(),
        }
    }
}

/// Outward normal of each edge `i -> i+1` of a counter-clockwise polygon.
fn polygon_edge_normals(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let n = points.len();
    (0..n)
        .map(|i| {
            let a = points[i];
            let b = points[(i + 1) % n];
            normalize2([b[1] - a[1], a[0] - b[0]])
        })
        .collect()
}

/// Arc length to each point of the closed loop (n + 1 entries, the last
/// back at the start), as a fraction of the perimeter.
fn cumulative_lengths(points: &[[f32; 2]]) -> Vec<f32> {
    let n = points.len();
    let mut acc = vec![0.0f32];
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        let len = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        acc.push(acc[i] + len);
    }
    let total = acc[n].max(1e-10);
    acc.iter().map(|l| l / total).collect()
}

fn normalize2(v: [f32; 2]) -> [f32; 2] {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len > 1e-10 {
        [v[0] / len, v[1] / len]
    } else {
        [1.0, 0.0]
    }
}

/// Extrude a tube along a curve defined by spine points.
///
/// At each spine point, generates a ring of vertices using the forward/up basis.
/// Adjacent rings are connected with quads. UV v is proportional to cumulative
/// arc length along the spine.
pub fn extrude_along_curve(params: &ExtrudeParams) -> Result<MeshData> {
    extrude_profile_along_curve(params, &CrossSection::circle(params.radial_segments))
}

/// Sweep an arbitrary [`CrossSection`] along a spine.
///
/// `params.radial_segments` is ignored; the profile sets the ring. Caps fan
/// from the spine point to the end rings, so they are correct for any profile
/// that is star-shaped around its origin (every convex profile is).
pub fn extrude_profile_along_curve(
    params: &ExtrudeParams,
    profile: &CrossSection,
) -> Result<MeshData> {
    let spine = &params.spine;
    if spine.len() < 2 {
        return Err(ProcGenError::InvalidParameter {
            name: "spine".into(),
            reason: "extrusion requires at least 2 spine points".into(),
        });
    }
    if profile.points.len() < 3 {
        return Err(ProcGenError::InvalidParameter {
            name: "profile".into(),
            reason: "a cross-section needs at least 3 points".into(),
        });
    }

    let row_width = profile.points.len() as u32;
    let spans: Vec<u32> = (0..profile.connect.len() as u32)
        .filter(|&k| profile.connect[k as usize])
        .collect();
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

        for k in 0..row_width as usize {
            let [x, y] = profile.points[k];
            let [nx, ny] = profile.normals[k];

            // Position on the ring
            let offset = right * (x * point.radius) + up * (y * point.radius);
            let pos = point.position + offset;

            // Normal points outward from the profile
            let normal = (right * nx + up * ny).normalized();

            vertices.push(Vertex {
                position: pos.to_array(),
                normal: normal.to_array(),
                tangent: [0.0, 0.0, 0.0, 1.0],
                uv: [profile.u[k], v_coord],
            });
        }
    }

    // Connect adjacent rings with quads
    for j in 0..(spine.len() as u32 - 1) {
        for &i in &spans {
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

            // The profile runs counter-clockwise about +forward, so it reads
            // counter-clockwise from behind the start: center, i, i+1.
            let first_ring_start = 0u32;
            for &i in &spans {
                indices.push(center_idx);
                indices.push(first_ring_start + i);
                indices.push(first_ring_start + i + 1);
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
            for &i in &spans {
                indices.push(center_idx);
                indices.push(last_ring_start + i + 1);
                indices.push(last_ring_start + i);
            }
        }
    }

    let bbox =
        BoundingBox::from_positions(&vertices.iter().map(|v| v.position).collect::<Vec<_>>());

    Ok(MeshData {
        vertices,
        indices,
        materials: vec![MaterialData::default()],
        submeshes: vec![],
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
        let t = if n > 1 {
            i as f32 / (n - 1) as f32
        } else {
            0.0
        };
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
            let t = if n > 1 {
                i as f32 / (n - 1) as f32
            } else {
                0.0
            };
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

    #[test]
    fn straight_extrusion_is_identical_to_cylinder() {
        use crate::algorithms::mesh_builder::primitives::{cylinder, CylinderParams};

        let (segments, rings) = (16u32, 4u32);
        let cyl = cylinder(&CylinderParams {
            radius: 1.0,
            height: 2.0,
            radial_segments: segments,
            height_segments: rings,
            caps: false,
        });

        // Frame the spine so `right` is +X and `up` is +Z, the plane the
        // cylinder sweeps its angle in.
        let positions: Vec<Vec3> = (0..=rings)
            .map(|j| Vec3::new(0.0, 2.0 * j as f32 / rings as f32, 0.0))
            .collect();
        let mut spine = build_spine_from_positions(&positions, 1.0, 1.0);
        for p in &mut spine {
            p.up = Vec3::new(0.0, 0.0, 1.0);
        }
        let tube = extrude_along_curve(&ExtrudeParams {
            spine,
            radial_segments: segments,
            caps: false,
            uv_length_scale: 1.0,
        })
        .unwrap();

        assert_eq!(tube.indices, cyl.indices);
        assert_eq!(tube.vertex_count(), cyl.vertex_count());
        for (a, b) in tube.vertices.iter().zip(&cyl.vertices) {
            for k in 0..3 {
                assert!((a.position[k] - b.position[k]).abs() < 1e-5);
                assert!((a.normal[k] - b.normal[k]).abs() < 1e-5);
            }
            for k in 0..2 {
                assert!((a.uv[k] - b.uv[k]).abs() < 1e-5);
            }
        }
    }

    fn straight_spine() -> Vec<ExtrudePoint> {
        let positions: Vec<Vec3> = (0..=6).map(|j| Vec3::new(0.0, j as f32, 0.0)).collect();
        build_spine_from_positions(&positions, 1.0, 0.3)
    }

    #[test]
    fn circle_profile_matches_default_extrusion() {
        let params = ExtrudeParams {
            spine: straight_spine(),
            radial_segments: 10,
            caps: true,
            uv_length_scale: 1.0,
        };
        let a = extrude_along_curve(&params).unwrap();
        let b = extrude_profile_along_curve(&params, &CrossSection::circle(10)).unwrap();
        assert_eq!(a.indices, b.indices);
        assert_eq!(a.vertices, b.vertices);
    }

    #[test]
    fn smooth_regular_polygon_matches_circle() {
        let n = 12;
        let points: Vec<[f32; 2]> = (0..n)
            .map(|i| {
                let t = i as f32 / n as f32 * TAU;
                [t.cos(), t.sin()]
            })
            .collect();
        let smooth = CrossSection::smooth(&points);
        let circle = CrossSection::circle(n);
        assert_eq!(smooth.points.len(), circle.points.len());
        for k in 0..smooth.points.len() {
            for c in 0..2 {
                assert!((smooth.points[k][c] - circle.points[k][c]).abs() < 1e-5);
                assert!((smooth.normals[k][c] - circle.normals[k][c]).abs() < 1e-5);
            }
            assert!((smooth.u[k] - circle.u[k]).abs() < 1e-5);
        }
    }

    #[test]
    fn star_profile_is_faceted_and_valid() {
        let tips = 5;
        let star = CrossSection::star(tips, 0.45);
        let edges = (tips * 2) as usize;
        assert_eq!(star.points.len(), edges * 2);
        assert_eq!(star.connect.iter().filter(|c| **c).count(), edges);

        let spine = straight_spine();
        let rings = spine.len();
        let mesh = extrude_profile_along_curve(
            &ExtrudeParams {
                spine,
                radial_segments: 0,
                caps: true,
                uv_length_scale: 1.0,
            },
            &star,
        )
        .unwrap();
        assert!(mesh.validate().is_ok());
        // Body quads only across real edges, plus one fan triangle per edge per cap.
        assert_eq!(mesh.triangle_count(), (rings - 1) * edges * 2 + edges * 2);

        // Each body triangle is flat-shaded: its vertex normals agree with its
        // face normal.
        let body_tris = (rings - 1) * edges * 2;
        for tri in mesh.indices.chunks_exact(3).take(body_tris) {
            let p = |i: u32| Vec3::from_array(mesh.vertices[i as usize].position);
            let face = (p(tri[1]) - p(tri[0]))
                .cross(&(p(tri[2]) - p(tri[0])))
                .normalized();
            for &i in tri {
                let n = Vec3::from_array(mesh.vertices[i as usize].normal);
                // Taper tilts the true face a little off the extruded normal.
                assert!(face.dot(&n) > 0.95, "facet normal off: {}", face.dot(&n));
            }
        }
        for v in &mesh.vertices {
            assert!((0.0..=1.0).contains(&v.uv[0]));
        }
    }

    #[test]
    fn profile_needs_three_points() {
        let err = extrude_profile_along_curve(
            &ExtrudeParams {
                spine: straight_spine(),
                radial_segments: 0,
                caps: false,
                uv_length_scale: 1.0,
            },
            &CrossSection::smooth(&[[1.0, 0.0]]),
        );
        assert!(err.is_err());
    }

    #[test]
    fn caps_and_body_wind_outward() {
        let positions = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.3, 1.0, 0.0),
            Vec3::new(0.0, 2.0, 0.6),
        ];
        let spine = build_spine_from_positions(&positions, 0.6, 0.4);
        for profile in [CrossSection::circle(9), CrossSection::star(4, 0.5)] {
            let mesh = extrude_profile_along_curve(
                &ExtrudeParams {
                    spine: spine.clone(),
                    radial_segments: 0,
                    caps: true,
                    uv_length_scale: 1.0,
                },
                &profile,
            )
            .unwrap();
            for tri in mesh.indices.chunks_exact(3) {
                let p = |i: u32| Vec3::from_array(mesh.vertices[i as usize].position);
                let face = (p(tri[1]) - p(tri[0])).cross(&(p(tri[2]) - p(tri[0])));
                // Cap vertices share the ring, so compare against the cap
                // centre's normal when the triangle is a cap fan.
                let n = Vec3::from_array(mesh.vertices[tri[0] as usize].normal);
                assert!(face.dot(&n) > 0.0, "triangle {tri:?} winds inward");
            }
        }
    }
}
