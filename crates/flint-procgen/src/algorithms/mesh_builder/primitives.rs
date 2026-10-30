use crate::types::{BoundingBox, MaterialData, MeshData, Vertex};
use std::f32::consts::TAU;

/// Parameters for generating a tapered cylinder (trunk primitive).
#[derive(Debug, Clone)]
pub struct TaperedCylinderParams {
    pub radius_bottom: f32,
    pub radius_top: f32,
    pub height: f32,
    pub radial_segments: u32,
    pub height_segments: u32,
    pub caps: bool,
}

/// Parameters for generating a cylinder.
#[derive(Debug, Clone)]
pub struct CylinderParams {
    pub radius: f32,
    pub height: f32,
    pub radial_segments: u32,
    pub height_segments: u32,
    pub caps: bool,
}

/// Parameters for generating a UV sphere.
#[derive(Debug, Clone)]
pub struct SphereParams {
    pub radius: f32,
    pub segments: u32, // longitude
    pub rings: u32,    // latitude
}

/// Parameters for generating an ellipsoid (sphere with per-axis radii).
#[derive(Debug, Clone)]
pub struct EllipsoidParams {
    pub radius_x: f32,
    pub radius_y: f32,
    pub radius_z: f32,
    pub segments: u32, // longitude
    pub rings: u32,    // latitude
}

/// Parameters for generating a cone.
#[derive(Debug, Clone)]
pub struct ConeParams {
    pub radius: f32,
    pub height: f32,
    pub radial_segments: u32,
    pub height_segments: u32,
    pub cap: bool,
}

/// Generate a tapered cylinder with linearly interpolated radius.
///
/// Body vertices: `(radial_segments + 1) * (height_segments + 1)`
/// Body triangles: `radial_segments * height_segments * 2`
/// Each cap adds `radial_segments + 1` vertices and `radial_segments` triangles.
///
/// The UV seam is handled by duplicating the first column with u=1.0.
/// Normals are analytically computed from the cone slope.
pub fn tapered_cylinder(params: &TaperedCylinderParams) -> MeshData {
    let r = params.radial_segments;
    let h = params.height_segments;

    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    // Slope for normal Y-component: the normal tilts outward based on taper
    let slope = (params.radius_bottom - params.radius_top) / params.height;

    // Body vertices: rings along height
    for j in 0..=h {
        let t = j as f32 / h as f32;
        let y = t * params.height;
        let radius = params.radius_bottom + (params.radius_top - params.radius_bottom) * t;

        for i in 0..=r {
            let theta = (i as f32 / r as f32) * TAU;
            let cos_t = theta.cos();
            let sin_t = theta.sin();

            let x = radius * cos_t;
            let z = radius * sin_t;

            // Normal: outward radial + slope component
            let nx = cos_t;
            let nz = sin_t;
            let ny = slope;
            let n_len = (nx * nx + ny * ny + nz * nz).sqrt();
            let (nx, ny, nz) = if n_len > 1e-10 {
                (nx / n_len, ny / n_len, nz / n_len)
            } else {
                (0.0, 1.0, 0.0)
            };

            let u = i as f32 / r as f32;
            let v = t;

            vertices.push(Vertex {
                position: [x, y, z],
                normal: [nx, ny, nz],
                tangent: [0.0, 0.0, 0.0, 1.0], // computed later if needed
                uv: [u, v],
            });
        }
    }

    // Body indices
    for j in 0..h {
        for i in 0..r {
            let row_width = r + 1;
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

    // Bottom cap
    if params.caps && params.radius_bottom > 1e-10 {
        let center_idx = vertices.len() as u32;
        vertices.push(Vertex {
            position: [0.0, 0.0, 0.0],
            normal: [0.0, -1.0, 0.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [0.5, 0.5],
        });

        let first_ring = vertices.len() as u32;
        for i in 0..=r {
            let theta = (i as f32 / r as f32) * TAU;
            let cos_t = theta.cos();
            let sin_t = theta.sin();
            let x = params.radius_bottom * cos_t;
            let z = params.radius_bottom * sin_t;
            vertices.push(Vertex {
                position: [x, 0.0, z],
                normal: [0.0, -1.0, 0.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv: [cos_t * 0.5 + 0.5, sin_t * 0.5 + 0.5],
            });
        }

        for i in 0..r {
            // Counter-clockwise seen from below (-Y), like the body seen
            // from outside, so back-face culling keeps the cap.
            indices.push(center_idx);
            indices.push(first_ring + i);
            indices.push(first_ring + i + 1);
        }
    }

    // Top cap
    if params.caps && params.radius_top > 1e-10 {
        let center_idx = vertices.len() as u32;
        vertices.push(Vertex {
            position: [0.0, params.height, 0.0],
            normal: [0.0, 1.0, 0.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [0.5, 0.5],
        });

        let first_ring = vertices.len() as u32;
        for i in 0..=r {
            let theta = (i as f32 / r as f32) * TAU;
            let cos_t = theta.cos();
            let sin_t = theta.sin();
            let x = params.radius_top * cos_t;
            let z = params.radius_top * sin_t;
            vertices.push(Vertex {
                position: [x, params.height, z],
                normal: [0.0, 1.0, 0.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv: [cos_t * 0.5 + 0.5, sin_t * 0.5 + 0.5],
            });
        }

        for i in 0..r {
            // Counter-clockwise seen from above (+Y). The ring's angle runs
            // from +X toward +Z, which is clockwise from above, hence i+1, i.
            indices.push(center_idx);
            indices.push(first_ring + i + 1);
            indices.push(first_ring + i);
        }
    }

    let bbox =
        BoundingBox::from_positions(&vertices.iter().map(|v| v.position).collect::<Vec<_>>());

    MeshData {
        vertices,
        indices,
        materials: vec![MaterialData::default()],
        submeshes: vec![],
        bounding_box: bbox,
    }
}

/// Generate a cylinder (equal radii).
pub fn cylinder(params: &CylinderParams) -> MeshData {
    tapered_cylinder(&TaperedCylinderParams {
        radius_bottom: params.radius,
        radius_top: params.radius,
        height: params.height,
        radial_segments: params.radial_segments,
        height_segments: params.height_segments,
        caps: params.caps,
    })
}

/// Generate a UV sphere.
///
/// Vertices: `(segments + 1) * (rings + 1)`
/// Triangles: `segments * rings * 2`
pub fn sphere(params: &SphereParams) -> MeshData {
    let s = params.segments;
    let ri = params.rings;

    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    for j in 0..=ri {
        let phi = std::f32::consts::PI * j as f32 / ri as f32;
        let sin_phi = phi.sin();
        let cos_phi = phi.cos();

        for i in 0..=s {
            let theta = TAU * i as f32 / s as f32;
            let sin_theta = theta.sin();
            let cos_theta = theta.cos();

            let x = sin_phi * cos_theta;
            let y = cos_phi;
            let z = sin_phi * sin_theta;

            let u = i as f32 / s as f32;
            let v = j as f32 / ri as f32;

            vertices.push(Vertex {
                position: [x * params.radius, y * params.radius, z * params.radius],
                normal: [x, y, z], // normalized position = normal for unit sphere
                tangent: [0.0, 0.0, 0.0, 1.0],
                uv: [u, v],
            });
        }
    }

    let row_width = s + 1;
    for j in 0..ri {
        for i in 0..s {
            let a = j * row_width + i;
            let b = a + 1;
            let c = (j + 1) * row_width + i;
            let d = c + 1;

            // Rows run top-down (row j + 1 is below row j), the opposite of
            // the cylinder, so the quad winds a-b-c to face outward.
            indices.push(a);
            indices.push(b);
            indices.push(c);

            indices.push(b);
            indices.push(d);
            indices.push(c);
        }
    }

    let bbox =
        BoundingBox::from_positions(&vertices.iter().map(|v| v.position).collect::<Vec<_>>());

    MeshData {
        vertices,
        indices,
        materials: vec![MaterialData::default()],
        submeshes: vec![],
        bounding_box: bbox,
    }
}

/// Generate an ellipsoid (UV sphere with per-axis radii).
///
/// Vertices: `(segments + 1) * (rings + 1)`
/// Triangles: `segments * rings * 2`
pub fn ellipsoid(params: &EllipsoidParams) -> MeshData {
    let s = params.segments;
    let ri = params.rings;

    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    for j in 0..=ri {
        let phi = std::f32::consts::PI * j as f32 / ri as f32;
        let sin_phi = phi.sin();
        let cos_phi = phi.cos();

        for i in 0..=s {
            let theta = TAU * i as f32 / s as f32;
            let sin_theta = theta.sin();
            let cos_theta = theta.cos();

            // Unit sphere direction
            let ux = sin_phi * cos_theta;
            let uy = cos_phi;
            let uz = sin_phi * sin_theta;

            // Position scaled by per-axis radii
            let px = ux * params.radius_x;
            let py = uy * params.radius_y;
            let pz = uz * params.radius_z;

            // Normal: gradient of the implicit ellipsoid surface x²/a² + y²/b² + z²/c² = 1
            // ∇ = (2x/a², 2y/b², 2z/c²), normalized
            let nx = ux / params.radius_x;
            let ny = uy / params.radius_y;
            let nz = uz / params.radius_z;
            let n_len = (nx * nx + ny * ny + nz * nz).sqrt();
            let (nx, ny, nz) = if n_len > 1e-10 {
                (nx / n_len, ny / n_len, nz / n_len)
            } else {
                (0.0, 1.0, 0.0)
            };

            let u = i as f32 / s as f32;
            let v = j as f32 / ri as f32;

            vertices.push(Vertex {
                position: [px, py, pz],
                normal: [nx, ny, nz],
                tangent: [0.0, 0.0, 0.0, 1.0],
                uv: [u, v],
            });
        }
    }

    let row_width = s + 1;
    for j in 0..ri {
        for i in 0..s {
            let a = j * row_width + i;
            let b = a + 1;
            let c = (j + 1) * row_width + i;
            let d = c + 1;

            // Rows run top-down (row j + 1 is below row j), the opposite of
            // the cylinder, so the quad winds a-b-c to face outward.
            indices.push(a);
            indices.push(b);
            indices.push(c);

            indices.push(b);
            indices.push(d);
            indices.push(c);
        }
    }

    let bbox =
        BoundingBox::from_positions(&vertices.iter().map(|v| v.position).collect::<Vec<_>>());

    MeshData {
        vertices,
        indices,
        materials: vec![MaterialData::default()],
        submeshes: vec![],
        bounding_box: bbox,
    }
}

/// Generate a cone (tapered cylinder with radius_top = 0).
///
/// Apex vertices share position but get per-face slant normals
/// (handled naturally by the tapered cylinder's per-vertex normals).
pub fn cone(params: &ConeParams) -> MeshData {
    tapered_cylinder(&TaperedCylinderParams {
        radius_bottom: params.radius,
        radius_top: 0.0,
        height: params.height,
        radial_segments: params.radial_segments,
        height_segments: params.height_segments,
        caps: params.cap,
    })
}

/// Generate an axis-aligned box centred on the origin.
///
/// Each face has its own four vertices (24 total, 12 triangles), so normals
/// stay flat and every face gets the full `[0, 1]` UV square.
pub fn cuboid(width: f32, height: f32, depth: f32) -> MeshData {
    let h = [width * 0.5, height * 0.5, depth * 0.5];
    // (normal, u axis, v axis) per face, with u x v = normal so the quads
    // wind counter-clockwise seen from outside.
    const FACES: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];

    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (n, u, v) in FACES {
        let base = vertices.len() as u32;
        for (su, sv) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            let mut position = [0.0; 3];
            for k in 0..3 {
                position[k] = (n[k] + u[k] * su + v[k] * sv) * h[k];
            }
            vertices.push(Vertex {
                position,
                normal: n,
                tangent: [u[0], u[1], u[2], 1.0],
                uv: [(su + 1.0) * 0.5, (sv + 1.0) * 0.5],
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    let bbox =
        BoundingBox::from_positions(&vertices.iter().map(|v| v.position).collect::<Vec<_>>());

    MeshData {
        vertices,
        indices,
        materials: vec![MaterialData::default()],
        submeshes: vec![],
        bounding_box: bbox,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::Vec3;

    #[test]
    fn tapered_cylinder_vertex_count() {
        let r = 8;
        let h = 4;
        let mesh = tapered_cylinder(&TaperedCylinderParams {
            radius_bottom: 1.0,
            radius_top: 0.5,
            height: 2.0,
            radial_segments: r,
            height_segments: h,
            caps: false,
        });

        let expected_verts = (r + 1) * (h + 1);
        assert_eq!(
            mesh.vertex_count(),
            expected_verts as usize,
            "body verts: (R+1)*(H+1)"
        );

        let expected_tris = r * h * 2;
        assert_eq!(
            mesh.triangle_count(),
            expected_tris as usize,
            "body tris: R*H*2"
        );
    }

    #[test]
    fn tapered_cylinder_with_caps_vertex_count() {
        let r = 8;
        let h = 4;
        let mesh = tapered_cylinder(&TaperedCylinderParams {
            radius_bottom: 1.0,
            radius_top: 0.5,
            height: 2.0,
            radial_segments: r,
            height_segments: h,
            caps: true,
        });

        let body_verts = (r + 1) * (h + 1);
        let cap_verts = (r + 1 + 1) * 2; // center + ring per cap
        assert_eq!(mesh.vertex_count(), (body_verts + cap_verts) as usize);

        let body_tris = r * h * 2;
        let cap_tris = r * 2;
        assert_eq!(mesh.triangle_count(), (body_tris + cap_tris) as usize);
    }

    #[test]
    fn tapered_cylinder_validates() {
        let mesh = tapered_cylinder(&TaperedCylinderParams {
            radius_bottom: 1.0,
            radius_top: 0.3,
            height: 3.0,
            radial_segments: 12,
            height_segments: 6,
            caps: true,
        });
        assert!(mesh.validate().is_ok());
    }

    #[test]
    fn tapered_cylinder_bounding_box() {
        let mesh = tapered_cylinder(&TaperedCylinderParams {
            radius_bottom: 2.0,
            radius_top: 1.0,
            height: 5.0,
            radial_segments: 32,
            height_segments: 1,
            caps: false,
        });

        let bb = &mesh.bounding_box;
        assert!(bb.min.y >= -0.01, "min y should be ~0");
        assert!((bb.max.y - 5.0).abs() < 0.01, "max y should be ~5");
        // X/Z extent should be close to radius_bottom
        assert!(
            (bb.max.x - 2.0).abs() < 0.1,
            "max x should be ~radius_bottom"
        );
    }

    #[test]
    fn tapered_cylinder_normals_unit_length() {
        let mesh = tapered_cylinder(&TaperedCylinderParams {
            radius_bottom: 1.0,
            radius_top: 0.5,
            height: 2.0,
            radial_segments: 8,
            height_segments: 4,
            caps: false,
        });

        for v in &mesh.vertices {
            let len = Vec3::from_array(v.normal).length();
            assert!(
                (len - 1.0).abs() < 1e-4,
                "normal should be unit length, got {}",
                len
            );
        }
    }

    #[test]
    fn tapered_cylinder_uvs_in_range() {
        let mesh = tapered_cylinder(&TaperedCylinderParams {
            radius_bottom: 1.0,
            radius_top: 0.5,
            height: 2.0,
            radial_segments: 16,
            height_segments: 8,
            caps: false,
        });

        for v in &mesh.vertices {
            assert!(
                v.uv[0] >= -1e-5 && v.uv[0] <= 1.0 + 1e-5,
                "u should be in [0,1], got {}",
                v.uv[0]
            );
            assert!(
                v.uv[1] >= -1e-5 && v.uv[1] <= 1.0 + 1e-5,
                "v should be in [0,1], got {}",
                v.uv[1]
            );
        }
    }

    #[test]
    fn cylinder_delegates_correctly() {
        let mesh = cylinder(&CylinderParams {
            radius: 1.5,
            height: 3.0,
            radial_segments: 8,
            height_segments: 4,
            caps: true,
        });
        assert!(mesh.validate().is_ok());

        // All body vertex radii should be ~1.5
        let body_verts = (8 + 1) * (4 + 1);
        for v in &mesh.vertices[..body_verts as usize] {
            let r = (v.position[0] * v.position[0] + v.position[2] * v.position[2]).sqrt();
            assert!((r - 1.5).abs() < 0.01, "radius should be 1.5, got {}", r);
        }
    }

    #[test]
    fn sphere_vertex_count() {
        let s = 16;
        let ri = 12;
        let mesh = sphere(&SphereParams {
            radius: 1.0,
            segments: s,
            rings: ri,
        });

        let expected_verts = (s + 1) * (ri + 1);
        assert_eq!(mesh.vertex_count(), expected_verts as usize);

        let expected_tris = s * ri * 2;
        assert_eq!(mesh.triangle_count(), expected_tris as usize);
    }

    #[test]
    fn sphere_validates() {
        let mesh = sphere(&SphereParams {
            radius: 2.0,
            segments: 16,
            rings: 12,
        });
        assert!(mesh.validate().is_ok());
    }

    #[test]
    fn sphere_normals_unit_length() {
        let mesh = sphere(&SphereParams {
            radius: 3.0,
            segments: 8,
            rings: 6,
        });

        for v in &mesh.vertices {
            let len = Vec3::from_array(v.normal).length();
            assert!(
                (len - 1.0).abs() < 1e-4,
                "sphere normal should be unit length, got {}",
                len
            );
        }
    }

    #[test]
    fn sphere_bounding_box() {
        let r = 2.5;
        let mesh = sphere(&SphereParams {
            radius: r,
            segments: 32,
            rings: 16,
        });

        let bb = &mesh.bounding_box;
        assert!((bb.max.y - r).abs() < 0.1, "max y should be ~radius");
        assert!((bb.min.y + r).abs() < 0.1, "min y should be ~-radius");
    }

    #[test]
    fn cone_validates() {
        let mesh = cone(&ConeParams {
            radius: 1.0,
            height: 2.0,
            radial_segments: 12,
            height_segments: 4,
            cap: true,
        });
        assert!(mesh.validate().is_ok());
    }

    #[test]
    fn ellipsoid_vertex_count() {
        let s = 16;
        let ri = 12;
        let mesh = ellipsoid(&EllipsoidParams {
            radius_x: 2.0,
            radius_y: 1.0,
            radius_z: 1.5,
            segments: s,
            rings: ri,
        });

        let expected_verts = (s + 1) * (ri + 1);
        assert_eq!(mesh.vertex_count(), expected_verts as usize);

        let expected_tris = s * ri * 2;
        assert_eq!(mesh.triangle_count(), expected_tris as usize);
    }

    #[test]
    fn ellipsoid_validates() {
        let mesh = ellipsoid(&EllipsoidParams {
            radius_x: 2.0,
            radius_y: 1.5,
            radius_z: 3.0,
            segments: 16,
            rings: 12,
        });
        assert!(mesh.validate().is_ok());
    }

    #[test]
    fn ellipsoid_bounding_box() {
        let mesh = ellipsoid(&EllipsoidParams {
            radius_x: 2.0,
            radius_y: 1.0,
            radius_z: 3.0,
            segments: 32,
            rings: 16,
        });

        let bb = &mesh.bounding_box;
        assert!((bb.max.x - 2.0).abs() < 0.1, "max x should be ~radius_x");
        assert!((bb.max.y - 1.0).abs() < 0.1, "max y should be ~radius_y");
        assert!((bb.max.z - 3.0).abs() < 0.1, "max z should be ~radius_z");
    }

    #[test]
    fn ellipsoid_normals_unit_length() {
        let mesh = ellipsoid(&EllipsoidParams {
            radius_x: 2.0,
            radius_y: 1.0,
            radius_z: 1.5,
            segments: 8,
            rings: 6,
        });

        for v in &mesh.vertices {
            let len = Vec3::from_array(v.normal).length();
            assert!(
                (len - 1.0).abs() < 1e-4,
                "ellipsoid normal should be unit length, got {}",
                len
            );
        }
    }

    #[test]
    fn ellipsoid_equal_radii_matches_sphere() {
        let r = 1.5;
        let s = 12;
        let ri = 8;
        let sphere_mesh = sphere(&SphereParams {
            radius: r,
            segments: s,
            rings: ri,
        });
        let ellipsoid_mesh = ellipsoid(&EllipsoidParams {
            radius_x: r,
            radius_y: r,
            radius_z: r,
            segments: s,
            rings: ri,
        });

        assert_eq!(sphere_mesh.vertex_count(), ellipsoid_mesh.vertex_count());
        assert_eq!(
            sphere_mesh.triangle_count(),
            ellipsoid_mesh.triangle_count()
        );

        // Positions should match
        for (sv, ev) in sphere_mesh
            .vertices
            .iter()
            .zip(ellipsoid_mesh.vertices.iter())
        {
            for k in 0..3 {
                assert!(
                    (sv.position[k] - ev.position[k]).abs() < 1e-5,
                    "position mismatch at axis {}",
                    k
                );
            }
        }
    }

    #[test]
    fn cone_apex_at_height() {
        let mesh = cone(&ConeParams {
            radius: 1.0,
            height: 3.0,
            radial_segments: 8,
            height_segments: 4,
            cap: false,
        });

        // Top ring vertices should be at the apex (radius = 0)
        let top_ring_start = (8 + 1) * 4; // row_width * height_segments
        for i in 0..=8 {
            let v = &mesh.vertices[(top_ring_start + i) as usize];
            let r = (v.position[0] * v.position[0] + v.position[2] * v.position[2]).sqrt();
            assert!(r < 1e-5, "apex radius should be ~0, got {}", r);
            assert!((v.position[1] - 3.0).abs() < 1e-5, "apex y should be 3.0");
        }
    }

    /// Every triangle has three distinct indices, non-zero area unless it
    /// touches a pole, and winds so its face normal agrees with the vertex
    /// normals (counter-clockwise seen from outside).
    fn assert_outward_and_non_degenerate(mesh: &MeshData) {
        for tri in mesh.indices.chunks_exact(3) {
            assert!(
                tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2],
                "degenerate indices {tri:?}"
            );
            let p = |i: u32| Vec3::from_array(mesh.vertices[i as usize].position);
            let face = (p(tri[1]) - p(tri[0])).cross(&(p(tri[2]) - p(tri[0])));
            if face.length() < 1e-7 {
                continue; // zero-area pole triangle of a UV sphere/cone apex
            }
            let n: Vec3 = tri
                .iter()
                .map(|&i| Vec3::from_array(mesh.vertices[i as usize].normal))
                .fold(Vec3::new(0.0, 0.0, 0.0), |a, b| a + b);
            assert!(face.dot(&n) > 0.0, "triangle {tri:?} winds inward");
        }
    }

    #[test]
    fn cylinder_acceptance_counts() {
        // The counts named in issue #19.
        let mesh = tapered_cylinder(&TaperedCylinderParams {
            radius_bottom: 1.0,
            radius_top: 0.5,
            height: 2.0,
            radial_segments: 16,
            height_segments: 4,
            caps: false,
        });
        assert_eq!(mesh.vertex_count(), 85);
        assert_eq!(mesh.indices.len(), 384);
    }

    #[test]
    fn primitives_wind_outward_without_degenerates() {
        assert_outward_and_non_degenerate(&tapered_cylinder(&TaperedCylinderParams {
            radius_bottom: 1.0,
            radius_top: 0.5,
            height: 2.0,
            radial_segments: 16,
            height_segments: 4,
            caps: true,
        }));
        assert_outward_and_non_degenerate(&sphere(&SphereParams {
            radius: 1.0,
            segments: 16,
            rings: 12,
        }));
        assert_outward_and_non_degenerate(&cone(&ConeParams {
            radius: 1.0,
            height: 2.0,
            radial_segments: 12,
            height_segments: 3,
            cap: true,
        }));
        assert_outward_and_non_degenerate(&ellipsoid(&EllipsoidParams {
            radius_x: 1.0,
            radius_y: 0.5,
            radius_z: 2.0,
            segments: 12,
            rings: 8,
        }));
        assert_outward_and_non_degenerate(&cuboid(1.0, 2.0, 3.0));
    }

    #[test]
    fn cuboid_counts_bounds_and_uvs() {
        let mesh = cuboid(2.0, 4.0, 6.0);
        assert_eq!(mesh.vertex_count(), 24);
        assert_eq!(mesh.triangle_count(), 12);
        assert!(mesh.validate().is_ok());
        let bb = &mesh.bounding_box;
        assert!((bb.min.x + 1.0).abs() < 1e-6 && (bb.max.x - 1.0).abs() < 1e-6);
        assert!((bb.min.y + 2.0).abs() < 1e-6 && (bb.max.y - 2.0).abs() < 1e-6);
        assert!((bb.min.z + 3.0).abs() < 1e-6 && (bb.max.z - 3.0).abs() < 1e-6);
        for v in &mesh.vertices {
            assert!(v.uv.iter().all(|c| (0.0..=1.0).contains(c)));
            let t = Vec3::new(v.tangent[0], v.tangent[1], v.tangent[2]);
            assert!(t.dot(&Vec3::from_array(v.normal)).abs() < 1e-6);
        }
    }
}
