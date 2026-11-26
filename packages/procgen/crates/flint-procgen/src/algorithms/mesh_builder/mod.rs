//! Mesh construction toolkit for procedural generation.
//!
//! Provides primitive shapes (cylinder, sphere, cone, tapered cylinder,
//! cuboid), curve extrusion with arbitrary cross-sections, UV mapping, normal/tangent computation, and mesh
//! simplification via quadric error metrics.

pub mod extrude;
pub mod normals;
pub mod primitives;
pub mod simplify;
pub mod tangents;
pub mod uv;

pub use extrude::{
    build_spine_from_positions, extrude_along_curve, extrude_profile_along_curve,
    spine_from_spline_samples, CrossSection, ExtrudeParams, ExtrudePoint,
};
pub use normals::compute_normals;
pub use primitives::{
    cone, cuboid, cylinder, ellipsoid, sphere, tapered_cylinder, ConeParams, CylinderParams,
    EllipsoidParams, SphereParams, TaperedCylinderParams,
};
pub use simplify::simplify;
pub use tangents::compute_tangents;
pub use uv::{apply_cylindrical_uv, apply_planar_uv, ProjectionAxis};

use crate::types::{BoundingBox, MaterialData, MeshData, SubMesh, Vertex};
use flint_core::Vec3;

/// Incremental mesh builder for composing geometry from primitives and raw data.
pub struct MeshBuilder {
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    materials: Vec<MaterialData>,
    submeshes: Vec<SubMesh>,
}

impl MeshBuilder {
    /// Create an empty mesh builder.
    pub fn new() -> Self {
        Self {
            vertices: Vec::new(),
            indices: Vec::new(),
            materials: Vec::new(),
            submeshes: Vec::new(),
        }
    }

    /// Create with pre-allocated capacity.
    pub fn with_capacity(verts: usize, indices: usize) -> Self {
        Self {
            vertices: Vec::with_capacity(verts),
            indices: Vec::with_capacity(indices),
            materials: Vec::new(),
            submeshes: Vec::new(),
        }
    }

    /// Add a single vertex, returning its index.
    pub fn add_vertex(&mut self, vertex: Vertex) -> u32 {
        let idx = self.vertices.len() as u32;
        self.vertices.push(vertex);
        idx
    }

    /// Add a triangle from three vertex indices.
    pub fn add_triangle(&mut self, i0: u32, i1: u32, i2: u32) {
        self.indices.push(i0);
        self.indices.push(i1);
        self.indices.push(i2);
    }

    /// Add a quad as two triangles (i0-i1-i2, i0-i2-i3).
    pub fn add_quad(&mut self, i0: u32, i1: u32, i2: u32, i3: u32) {
        self.add_triangle(i0, i1, i2);
        self.add_triangle(i0, i2, i3);
    }

    /// Add a material, returning its index.
    pub fn add_material(&mut self, material: MaterialData) -> usize {
        let idx = self.materials.len();
        self.materials.push(material);
        idx
    }

    /// Merge another MeshData into this builder, offsetting indices.
    pub fn merge(&mut self, mesh: &MeshData) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&mesh.vertices);
        self.indices.extend(mesh.indices.iter().map(|i| i + base));
    }

    /// Merge another MeshData with a column-major affine transform applied
    /// (the layout of `Transform::to_matrix`).
    ///
    /// Positions take the full matrix. Normals take the inverse-transpose of
    /// its linear part, so non-uniform scale keeps them perpendicular to the
    /// surface; tangents take the linear part. A mirroring transform
    /// (negative determinant) reverses each triangle's winding and flips the
    /// tangent handedness, so the merged copy still faces outward.
    pub fn merge_transformed(&mut self, mesh: &MeshData, matrix: &[[f32; 4]; 4]) {
        let base = self.vertices.len() as u32;
        let m = matrix;
        // Linear part as rows: lin[row][col] = m[col][row].
        let lin = [
            [m[0][0], m[1][0], m[2][0]],
            [m[0][1], m[1][1], m[2][1]],
            [m[0][2], m[1][2], m[2][2]],
        ];
        // Cofactor matrix = det * inverse-transpose; the det factor washes out
        // on normalisation, except its sign, which `cof_sign` restores.
        let cof = [
            [
                lin[1][1] * lin[2][2] - lin[1][2] * lin[2][1],
                lin[1][2] * lin[2][0] - lin[1][0] * lin[2][2],
                lin[1][0] * lin[2][1] - lin[1][1] * lin[2][0],
            ],
            [
                lin[0][2] * lin[2][1] - lin[0][1] * lin[2][2],
                lin[0][0] * lin[2][2] - lin[0][2] * lin[2][0],
                lin[0][1] * lin[2][0] - lin[0][0] * lin[2][1],
            ],
            [
                lin[0][1] * lin[1][2] - lin[0][2] * lin[1][1],
                lin[0][2] * lin[1][0] - lin[0][0] * lin[1][2],
                lin[0][0] * lin[1][1] - lin[0][1] * lin[1][0],
            ],
        ];
        let det = lin[0][0] * cof[0][0] + lin[0][1] * cof[0][1] + lin[0][2] * cof[0][2];
        let mirrored = det < 0.0;
        let cof_sign = if mirrored { -1.0 } else { 1.0 };

        let apply = |mat: &[[f32; 3]; 3], v: [f32; 3]| -> Vec3 {
            Vec3::new(
                mat[0][0] * v[0] + mat[0][1] * v[1] + mat[0][2] * v[2],
                mat[1][0] * v[0] + mat[1][1] * v[1] + mat[1][2] * v[2],
                mat[2][0] * v[0] + mat[2][1] * v[1] + mat[2][2] * v[2],
            )
        };

        self.vertices.extend(mesh.vertices.iter().map(|v| {
            let p = apply(&lin, v.position) + Vec3::new(m[3][0], m[3][1], m[3][2]);
            let n = apply(&cof, v.normal) * cof_sign;
            let t = apply(&lin, [v.tangent[0], v.tangent[1], v.tangent[2]]);
            let n = if n.length() > 1e-12 {
                n.normalized()
            } else {
                n
            };
            let t = if t.length() > 1e-12 {
                t.normalized()
            } else {
                t
            };
            Vertex {
                position: p.to_array(),
                normal: n.to_array(),
                tangent: [t.x, t.y, t.z, v.tangent[3] * cof_sign],
                uv: v.uv,
            }
        }));
        for tri in mesh.indices.chunks_exact(3) {
            if mirrored {
                self.add_triangle(tri[0] + base, tri[2] + base, tri[1] + base);
            } else {
                self.add_triangle(tri[0] + base, tri[1] + base, tri[2] + base);
            }
        }
    }

    /// Merge another MeshData and record a [`SubMesh`] for the added indices.
    pub fn merge_as_submesh(&mut self, mesh: &MeshData, material_index: usize) {
        let index_start = self.indices.len() as u32;
        self.merge(mesh);
        let index_count = mesh.indices.len() as u32;
        self.submeshes.push(SubMesh {
            index_start,
            index_count,
            material_index,
        });
    }

    /// Merge another MeshData with its materials, keeping its submeshes.
    ///
    /// The mesh's materials are appended to this builder's, and each of its
    /// submeshes (or the whole mesh, when it has none) is recorded against
    /// its material's new index.
    pub fn merge_with_materials(&mut self, mesh: &MeshData) {
        let material_base = self.materials.len();
        let index_base = self.indices.len() as u32;
        self.materials.extend(mesh.materials.iter().cloned());
        self.merge(mesh);
        if mesh.submeshes.is_empty() {
            if !mesh.indices.is_empty() {
                self.submeshes.push(SubMesh {
                    index_start: index_base,
                    index_count: mesh.indices.len() as u32,
                    material_index: material_base,
                });
            }
        } else {
            self.submeshes
                .extend(mesh.submeshes.iter().map(|s| SubMesh {
                    index_start: index_base + s.index_start,
                    index_count: s.index_count,
                    material_index: material_base + s.material_index,
                }));
        }
    }

    /// Translate all vertex positions by an offset.
    pub fn translate(&mut self, offset: Vec3) {
        for v in &mut self.vertices {
            let p = Vec3::from_array(v.position) + offset;
            v.position = p.to_array();
        }
    }

    /// Uniformly scale all vertex positions around the origin.
    pub fn scale(&mut self, factor: f32) {
        for v in &mut self.vertices {
            let p = Vec3::from_array(v.position) * factor;
            v.position = p.to_array();
        }
    }

    /// Compute area-weighted smooth normals for the current geometry.
    pub fn compute_normals(&mut self) {
        normals::compute_normals(&mut self.vertices, &self.indices);
    }

    /// Compute MikkTSpace-style tangents for the current geometry.
    pub fn compute_tangents(&mut self) {
        tangents::compute_tangents(&mut self.vertices, &self.indices);
    }

    /// Finalize into a MeshData with computed bounding box.
    pub fn build(self) -> MeshData {
        let bbox = BoundingBox::from_positions(
            &self.vertices.iter().map(|v| v.position).collect::<Vec<_>>(),
        );
        MeshData {
            vertices: self.vertices,
            indices: self.indices,
            materials: if self.materials.is_empty() {
                vec![MaterialData::default()]
            } else {
                self.materials
            },
            submeshes: self.submeshes,
            bounding_box: bbox,
        }
    }

    /// Finalize with normals and tangents computed automatically.
    pub fn build_with_computed_normals_and_tangents(mut self) -> MeshData {
        self.compute_normals();
        self.compute_tangents();
        self.build()
    }

    /// Generate a cylinder and merge it into the builder.
    pub fn add_cylinder(&mut self, params: &CylinderParams) {
        let mesh = cylinder(params);
        self.merge(&mesh);
    }

    /// Generate a sphere and merge it into the builder.
    pub fn add_sphere(&mut self, params: &SphereParams) {
        let mesh = sphere(params);
        self.merge(&mesh);
    }

    /// Generate a cone and merge it into the builder.
    pub fn add_cone(&mut self, params: &ConeParams) {
        let mesh = cone(params);
        self.merge(&mesh);
    }

    /// Generate a tapered cylinder and merge it into the builder.
    pub fn add_tapered_cylinder(&mut self, params: &TaperedCylinderParams) {
        let mesh = tapered_cylinder(params);
        self.merge(&mesh);
    }

    /// Extrude a tube along a curve and merge it into the builder.
    pub fn add_extruded_tube(&mut self, params: &ExtrudeParams) -> crate::Result<()> {
        let mesh = extrude_along_curve(params)?;
        self.merge(&mesh);
        Ok(())
    }
}

impl Default for MeshBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_vertex(pos: [f32; 3]) -> Vertex {
        Vertex {
            position: pos,
            normal: [0.0, 1.0, 0.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
        }
    }

    #[test]
    fn builder_add_and_build() {
        let mut b = MeshBuilder::new();
        let i0 = b.add_vertex(default_vertex([0.0, 0.0, 0.0]));
        let i1 = b.add_vertex(default_vertex([1.0, 0.0, 0.0]));
        let i2 = b.add_vertex(default_vertex([0.0, 1.0, 0.0]));
        b.add_triangle(i0, i1, i2);

        let mesh = b.build();
        assert_eq!(mesh.vertex_count(), 3);
        assert_eq!(mesh.triangle_count(), 1);
        assert!(mesh.validate().is_ok());
    }

    #[test]
    fn builder_merge_offsets_indices() {
        let mesh1 = cylinder(&CylinderParams {
            radius: 1.0,
            height: 2.0,
            radial_segments: 4,
            height_segments: 1,
            caps: false,
        });
        let mesh2 = sphere(&SphereParams {
            radius: 0.5,
            segments: 4,
            rings: 3,
        });

        let total_verts = mesh1.vertex_count() + mesh2.vertex_count();
        let total_tris = mesh1.triangle_count() + mesh2.triangle_count();

        let mut b = MeshBuilder::new();
        b.merge(&mesh1);
        b.merge(&mesh2);
        let merged = b.build();

        assert_eq!(merged.vertex_count(), total_verts);
        assert_eq!(merged.triangle_count(), total_tris);
        assert!(merged.validate().is_ok());
    }

    #[test]
    fn builder_translate() {
        let mut b = MeshBuilder::new();
        b.add_vertex(default_vertex([1.0, 2.0, 3.0]));
        b.translate(Vec3::new(10.0, 20.0, 30.0));

        let mesh = b.build();
        let p = mesh.vertices[0].position;
        assert!((p[0] - 11.0).abs() < 1e-5);
        assert!((p[1] - 22.0).abs() < 1e-5);
        assert!((p[2] - 33.0).abs() < 1e-5);
    }

    #[test]
    fn builder_scale() {
        let mut b = MeshBuilder::new();
        b.add_vertex(default_vertex([1.0, 2.0, 3.0]));
        b.scale(2.0);

        let mesh = b.build();
        let p = mesh.vertices[0].position;
        assert!((p[0] - 2.0).abs() < 1e-5);
        assert!((p[1] - 4.0).abs() < 1e-5);
        assert!((p[2] - 6.0).abs() < 1e-5);
    }

    #[test]
    fn build_computes_bbox() {
        let mut b = MeshBuilder::new();
        b.add_vertex(default_vertex([-1.0, -2.0, -3.0]));
        b.add_vertex(default_vertex([4.0, 5.0, 6.0]));

        let mesh = b.build();
        let bb = &mesh.bounding_box;
        assert!((bb.min.x + 1.0).abs() < 1e-5);
        assert!((bb.max.x - 4.0).abs() < 1e-5);
        assert!((bb.min.y + 2.0).abs() < 1e-5);
        assert!((bb.max.y - 5.0).abs() < 1e-5);
    }

    #[test]
    fn builder_delegates_to_primitives() {
        let mut b = MeshBuilder::new();
        b.add_cylinder(&CylinderParams {
            radius: 1.0,
            height: 2.0,
            radial_segments: 8,
            height_segments: 4,
            caps: true,
        });
        b.add_sphere(&SphereParams {
            radius: 1.0,
            segments: 8,
            rings: 6,
        });

        let mesh = b.build();
        assert!(mesh.validate().is_ok());
        assert!(mesh.vertex_count() > 0);
    }

    #[test]
    fn builder_add_quad() {
        let mut b = MeshBuilder::new();
        let i0 = b.add_vertex(default_vertex([0.0, 0.0, 0.0]));
        let i1 = b.add_vertex(default_vertex([1.0, 0.0, 0.0]));
        let i2 = b.add_vertex(default_vertex([1.0, 1.0, 0.0]));
        let i3 = b.add_vertex(default_vertex([0.0, 1.0, 0.0]));
        b.add_quad(i0, i1, i2, i3);

        let mesh = b.build();
        assert_eq!(mesh.triangle_count(), 2);
        assert!(mesh.validate().is_ok());
    }

    #[test]
    fn build_with_computed_normals_and_tangents() {
        let mut b = MeshBuilder::new();
        b.add_vertex(Vertex {
            position: [0.0, 0.0, 0.0],
            normal: [0.0; 3],
            tangent: [0.0; 4],
            uv: [0.0, 0.0],
        });
        b.add_vertex(Vertex {
            position: [1.0, 0.0, 0.0],
            normal: [0.0; 3],
            tangent: [0.0; 4],
            uv: [1.0, 0.0],
        });
        b.add_vertex(Vertex {
            position: [0.0, 0.0, 1.0],
            normal: [0.0; 3],
            tangent: [0.0; 4],
            uv: [0.0, 1.0],
        });
        b.add_triangle(0, 1, 2);

        let mesh = b.build_with_computed_normals_and_tangents();
        assert!(mesh.validate().is_ok());

        // Normals should be computed (not zero)
        for v in &mesh.vertices {
            let len = Vec3::from_array(v.normal).length();
            assert!((len - 1.0).abs() < 1e-4);
        }
    }

    fn translation(x: f32, y: f32, z: f32) -> [[f32; 4]; 4] {
        [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [x, y, z, 1.0],
        ]
    }

    /// Every triangle's vertex normals are perpendicular to its edges (for a
    /// flat-shaded mesh) and agree with its winding.
    fn assert_flat_normals_match_faces(mesh: &MeshData) {
        for tri in mesh.indices.chunks_exact(3) {
            let p = |i: u32| Vec3::from_array(mesh.vertices[i as usize].position);
            let e1 = p(tri[1]) - p(tri[0]);
            let e2 = p(tri[2]) - p(tri[0]);
            let face = e1.cross(&e2).normalized();
            for &i in tri {
                let v = &mesh.vertices[i as usize];
                let n = Vec3::from_array(v.normal);
                assert!((n.length() - 1.0).abs() < 1e-5, "normal not unit");
                assert!(n.dot(&face) > 0.9999, "normal off face: {}", n.dot(&face));
                let t = Vec3::new(v.tangent[0], v.tangent[1], v.tangent[2]);
                assert!(t.dot(&n).abs() < 1e-5, "tangent leaves the surface");
            }
        }
    }

    #[test]
    fn merge_two_unit_cubes_bounds() {
        let cube = cuboid(1.0, 1.0, 1.0);
        let mut b = MeshBuilder::new();
        b.merge_transformed(&cube, &translation(-2.0, 0.0, 0.0));
        b.merge_transformed(&cube, &translation(3.0, 1.0, 0.5));
        let mesh = b.build();

        assert_eq!(mesh.vertex_count(), 48);
        assert_eq!(mesh.triangle_count(), 24);
        assert!(mesh.validate().is_ok());
        let bb = &mesh.bounding_box;
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        assert!(close(bb.min.x, -2.5) && close(bb.max.x, 3.5));
        assert!(close(bb.min.y, -0.5) && close(bb.max.y, 1.5));
        assert!(close(bb.min.z, -0.5) && close(bb.max.z, 1.0));
        assert_flat_normals_match_faces(&mesh);
    }

    #[test]
    fn merge_matches_transform_to_matrix() {
        let t = flint_core::Transform::default()
            .with_position(Vec3::new(1.0, 2.0, 3.0))
            .with_rotation(Vec3::new(30.0, 45.0, 10.0))
            .with_scale(Vec3::new(2.0, 2.0, 2.0));
        let cube = cuboid(1.0, 1.0, 1.0);
        let mut b = MeshBuilder::new();
        b.merge_transformed(&cube, &t.to_matrix());
        let mesh = b.build();

        // A corner lands where the matrix sends it.
        let m = t.to_matrix();
        let src = cube.vertices[0].position;
        let expect: Vec<f32> = (0..3)
            .map(|r| m[0][r] * src[0] + m[1][r] * src[1] + m[2][r] * src[2] + m[3][r])
            .collect();
        for k in 0..3 {
            assert!((mesh.vertices[0].position[k] - expect[k]).abs() < 1e-5);
        }
        assert_flat_normals_match_faces(&mesh);
    }

    #[test]
    fn merge_non_uniform_scale_and_shear_keeps_normals_perpendicular() {
        // Squash Y, stretch X, and shear X by Y: a plain linear transform of
        // the normals would tilt them off the faces.
        let m = [
            [3.0, 0.0, 0.0, 0.0],
            [1.5, 0.25, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let mut b = MeshBuilder::new();
        b.merge_transformed(&cuboid(1.0, 1.0, 1.0), &m);
        assert_flat_normals_match_faces(&b.build());
    }

    #[test]
    fn merge_mirror_flips_winding_and_handedness() {
        let mirror = [
            [-1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let cube = cuboid(1.0, 1.0, 1.0);
        let mut b = MeshBuilder::new();
        b.merge_transformed(&cube, &mirror);
        let mesh = b.build();
        assert_flat_normals_match_faces(&mesh);
        for (a, src) in mesh.vertices.iter().zip(&cube.vertices) {
            assert_eq!(a.tangent[3], -src.tangent[3]);
        }
    }

    #[test]
    fn compute_normals_unit_length_on_primitives() {
        let mut b = MeshBuilder::new();
        b.merge(&sphere(&SphereParams {
            radius: 2.0,
            segments: 12,
            rings: 8,
        }));
        b.merge_transformed(&cuboid(1.0, 2.0, 1.0), &translation(4.0, 0.0, 0.0));
        b.compute_normals();
        for v in &b.build().vertices {
            let n = Vec3::from_array(v.normal);
            assert!((n.dot(&n) - 1.0).abs() < 1e-4);
        }
    }
}
