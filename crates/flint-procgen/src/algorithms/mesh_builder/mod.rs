//! Mesh construction toolkit for procedural generation.
//!
//! Provides primitive shapes (cylinder, sphere, cone, tapered cylinder),
//! curve extrusion, UV mapping, normal/tangent computation, and mesh
//! simplification via quadric error metrics.

pub mod extrude;
pub mod normals;
pub mod primitives;
pub mod simplify;
pub mod tangents;
pub mod uv;

pub use extrude::{
    build_spine_from_positions, extrude_along_curve, spine_from_spline_samples, ExtrudeParams,
    ExtrudePoint,
};
pub use normals::compute_normals;
pub use primitives::{
    cone, cylinder, sphere, tapered_cylinder, ConeParams, CylinderParams, SphereParams,
    TaperedCylinderParams,
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
}
