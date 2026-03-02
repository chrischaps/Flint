//! A mock generator that produces a unit cube.
//!
//! Useful for testing the generation pipeline end-to-end without depending
//! on any real geometry algorithms.

use crate::generator::{GenerationCost, Generator};
use crate::output::{GeneratorOutput, OutputKind};
use crate::rng::SeededRng;
use crate::spec::ProcGenSpec;
use crate::types::{BoundingBox, MaterialData, MeshData, Vertex};
use crate::Result;

/// A mock generator that always produces a unit cube centered at the origin.
///
/// The cube has 24 vertices (4 per face for correct per-face normals),
/// 36 indices (12 triangles), and a single default material. It draws one
/// RNG value to prove the pipeline carries randomness through.
pub struct MockGenerator;

impl Generator for MockGenerator {
    fn type_name(&self) -> &str {
        "mock"
    }

    fn output_kind(&self) -> OutputKind {
        OutputKind::Mesh
    }

    fn generate(&self, _spec: &ProcGenSpec, rng: &mut SeededRng) -> Result<GeneratorOutput> {
        // Draw one value to prove the RNG pipeline works.
        let _proof = rng.next_f32();

        let mesh = build_unit_cube();
        Ok(GeneratorOutput::Mesh(mesh))
    }

    fn param_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    fn estimate_cost(&self, _spec: &ProcGenSpec) -> GenerationCost {
        GenerationCost {
            estimated_vertices: 24,
            estimated_triangles: 12,
            estimated_texture_bytes: 0,
            estimated_generation_ms: 0.1,
        }
    }
}

/// Build a unit cube: side length 1, centered at the origin.
///
/// 6 faces × 4 vertices = 24 vertices (separate vertices per face for
/// correct hard-edge normals). 6 faces × 2 triangles × 3 indices = 36.
fn build_unit_cube() -> MeshData {
    let h = 0.5_f32; // half-extent

    // (position, normal, tangent, uv) per face.
    type FaceRow = ([f32; 3], [f32; 3], [f32; 4], [[f32; 2]; 4]);
    #[rustfmt::skip]
    let face_data: [FaceRow; 6] = [
        // +X face
        ([ h, 0.0, 0.0], [ 1.0, 0.0, 0.0], [ 0.0, 0.0,-1.0, 1.0], [[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]]),
        // -X face
        ([-h, 0.0, 0.0], [-1.0, 0.0, 0.0], [ 0.0, 0.0, 1.0, 1.0], [[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]]),
        // +Y face
        ([ 0.0, h, 0.0], [ 0.0, 1.0, 0.0], [ 1.0, 0.0, 0.0, 1.0], [[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]]),
        // -Y face
        ([ 0.0,-h, 0.0], [ 0.0,-1.0, 0.0], [ 1.0, 0.0, 0.0, 1.0], [[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]]),
        // +Z face
        ([ 0.0, 0.0, h], [ 0.0, 0.0, 1.0], [ 1.0, 0.0, 0.0, 1.0], [[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]]),
        // -Z face
        ([ 0.0, 0.0,-h], [ 0.0, 0.0,-1.0], [-1.0, 0.0, 0.0, 1.0], [[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]]),
    ];

    // For each face, build 4 vertices offset from the face center.
    let mut vertices = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);

    for (face_center, normal, tangent, uvs) in &face_data {
        let base = vertices.len() as u32;

        // Compute the two tangent axes on the face plane.
        let t = [tangent[0], tangent[1], tangent[2]];
        let b = cross(normal, &t);

        // 4 corners: (-h,-h), (+h,-h), (+h,+h), (-h,+h) in tangent space.
        let offsets: [[f32; 2]; 4] = [[-h, -h], [h, -h], [h, h], [-h, h]];

        for (i, offset) in offsets.iter().enumerate() {
            vertices.push(Vertex {
                position: [
                    face_center[0] + t[0] * offset[0] + b[0] * offset[1],
                    face_center[1] + t[1] * offset[0] + b[1] * offset[1],
                    face_center[2] + t[2] * offset[0] + b[2] * offset[1],
                ],
                normal: *normal,
                tangent: *tangent,
                uv: uvs[i],
            });
        }

        // Two triangles: 0-1-2 and 0-2-3.
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    let positions: Vec<[f32; 3]> = vertices.iter().map(|v| v.position).collect();
    MeshData {
        vertices,
        indices,
        materials: vec![MaterialData::default()],
        bounding_box: BoundingBox::from_positions(&positions),
    }
}

/// Simple cross product for `[f32; 3]`.
fn cross(a: &[f32; 3], b: &[f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::ProcGenSpec;

    fn mock_spec() -> ProcGenSpec {
        ProcGenSpec::parse(
            r#"
generator = "mock"
[meta]
name = "test"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
"#,
        )
        .unwrap()
    }

    #[test]
    fn mock_type_name() {
        assert_eq!(MockGenerator.type_name(), "mock");
    }

    #[test]
    fn mock_output_kind() {
        assert_eq!(MockGenerator.output_kind(), OutputKind::Mesh);
    }

    #[test]
    fn mock_generates_valid_cube() {
        let spec = mock_spec();
        let mut rng = SeededRng::new(42);
        let output = MockGenerator.generate(&spec, &mut rng).unwrap();
        let mesh = output.as_mesh().unwrap();

        assert_eq!(mesh.vertex_count(), 24);
        assert_eq!(mesh.triangle_count(), 12);
        assert_eq!(mesh.materials.len(), 1);
        assert!(mesh.validate().is_ok());
    }

    #[test]
    fn mock_cube_bounds() {
        let mesh = build_unit_cube();
        let bb = &mesh.bounding_box;
        // Unit cube centered at origin → bounds should be ±0.5.
        let eps = 1e-5;
        assert!((bb.min.x - (-0.5)).abs() < eps);
        assert!((bb.min.y - (-0.5)).abs() < eps);
        assert!((bb.min.z - (-0.5)).abs() < eps);
        assert!((bb.max.x - 0.5).abs() < eps);
        assert!((bb.max.y - 0.5).abs() < eps);
        assert!((bb.max.z - 0.5).abs() < eps);
    }

    #[test]
    fn mock_param_schema_is_valid_json() {
        let schema = MockGenerator.param_schema();
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn mock_estimate_cost_nonzero() {
        let spec = mock_spec();
        let cost = MockGenerator.estimate_cost(&spec);
        assert_eq!(cost.estimated_vertices, 24);
        assert_eq!(cost.estimated_triangles, 12);
    }

    #[test]
    fn mock_deterministic_across_calls() {
        let spec = mock_spec();
        let mut rng_a = SeededRng::new(42);
        let mut rng_b = SeededRng::new(42);
        let a = MockGenerator.generate(&spec, &mut rng_a).unwrap();
        let b = MockGenerator.generate(&spec, &mut rng_b).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn unit_cube_normals_are_unit_length() {
        let mesh = build_unit_cube();
        for v in &mesh.vertices {
            let len = (v.normal[0].powi(2) + v.normal[1].powi(2) + v.normal[2].powi(2)).sqrt();
            assert!((len - 1.0).abs() < 1e-5, "normal not unit length: {len}");
        }
    }
}
