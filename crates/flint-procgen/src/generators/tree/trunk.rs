//! Core trunk generation — Perlin-displaced spine extruded into a tapered mesh.

use crate::algorithms::mesh_builder::extrude::{
    build_spine_from_positions, extrude_along_curve, ExtrudeParams,
};
use crate::algorithms::mesh_builder::normals::compute_normals;
use crate::algorithms::mesh_builder::tangents::compute_tangents;
use crate::algorithms::noise::{NoiseSource, PerlinNoise};
use crate::types::{ImageData, MaterialData, MeshData};
use crate::Result;
use crate::SeededRng;
use flint_core::Vec3;

use super::bark::generate_bark_normal_map;
use super::TrunkParams;

/// Output of trunk generation, containing geometry, normal map, and attachment info.
#[derive(Debug, Clone)]
pub struct TrunkOutput {
    /// The trunk mesh with bark material.
    pub mesh: MeshData,
    /// Procedural bark normal map.
    pub normal_map: ImageData,
    /// World-space position of the trunk tip (for branch attachment).
    pub tip_position: Vec3,
    /// Forward direction at the trunk tip.
    pub tip_direction: Vec3,
}

/// Generate a trunk mesh from the given parameters and RNG.
///
/// This is the main entry point for Task 3.1. It:
/// 1. Generates a noise-displaced vertical spine
/// 2. Extrudes a tapered tube along the spine
/// 3. Recomputes normals and tangents
/// 4. Generates a procedural bark normal map
/// 5. Sets up the bark material
pub fn generate_trunk(params: &TrunkParams, rng: &mut SeededRng) -> Result<TrunkOutput> {
    // --- Spine generation ---
    let mut spine_rng = rng.fork("trunk_spine");
    let noise_x = PerlinNoise::new(spine_rng.fork("noise_x").seed());
    let noise_z = PerlinNoise::new(spine_rng.fork("noise_z").seed());

    let num_points = params.segments + 1;
    let freq = 1.0; // frequency for spine noise sampling
    let mut positions = Vec::with_capacity(num_points as usize);

    for i in 0..num_points {
        let t = i as f32 / params.segments as f32;
        let y = t * params.height;

        let x = if params.curve_noise > 0.0 {
            noise_x.sample_2d(y as f64 * freq, 0.0) as f32 * params.curve_noise
        } else {
            0.0
        };
        let z = if params.curve_noise > 0.0 {
            noise_z.sample_2d(y as f64 * freq, 0.0) as f32 * params.curve_noise
        } else {
            0.0
        };

        positions.push(Vec3::new(x, y, z));
    }

    // Record tip info before extrusion
    let tip_position = *positions.last().unwrap();
    let tip_direction = if positions.len() >= 2 {
        let n = positions.len();
        let d = positions[n - 1] - positions[n - 2];
        let len = d.length();
        if len > 1e-10 {
            Vec3::new(d.x / len, d.y / len, d.z / len)
        } else {
            Vec3::UP
        }
    } else {
        Vec3::UP
    };

    // --- Mesh extrusion ---
    let spine = build_spine_from_positions(&positions, params.radius_base, params.radius_top);
    let mut mesh = extrude_along_curve(&ExtrudeParams {
        spine,
        radial_segments: params.radial_segments,
        caps: true,
        uv_length_scale: 1.0,
    })?;

    // Recompute normals and tangents for correct lighting
    compute_normals(&mut mesh.vertices, &mesh.indices);
    compute_tangents(&mut mesh.vertices, &mesh.indices);

    // --- Material ---
    mesh.materials = vec![MaterialData {
        name: "bark".into(),
        base_color: params.bark_color,
        metallic: 0.0,
        roughness: params.bark_roughness,
    }];

    // --- Bark normal map ---
    let mut bark_rng = rng.fork("bark");
    let normal_map = generate_bark_normal_map(
        &mut bark_rng,
        params.bark_normal_resolution,
        params.bark_normal_strength,
    );

    Ok(TrunkOutput {
        mesh,
        normal_map,
        tip_position,
        tip_direction,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_params() -> TrunkParams {
        TrunkParams::default()
    }

    #[test]
    fn mesh_validity() {
        let params = default_params();
        let mut rng = SeededRng::new(42);
        let output = generate_trunk(&params, &mut rng).unwrap();
        assert!(
            output.mesh.validate().is_ok(),
            "mesh should be valid: {:?}",
            output.mesh.validate()
        );

        // Check no degenerate triangles (zero-area)
        let verts = &output.mesh.vertices;
        let indices = &output.mesh.indices;
        for tri in indices.chunks_exact(3) {
            let p0 = Vec3::from_array(verts[tri[0] as usize].position);
            let p1 = Vec3::from_array(verts[tri[1] as usize].position);
            let p2 = Vec3::from_array(verts[tri[2] as usize].position);

            let e0 = p1 - p0;
            let e1 = p2 - p0;
            let cross = e0.cross(&e1);
            let area = cross.length() * 0.5;
            assert!(
                area > 1e-10,
                "degenerate triangle with area {area}: [{:?}, {:?}, {:?}]",
                p0,
                p1,
                p2
            );
        }
    }

    #[test]
    fn bounding_box_sanity() {
        let params = TrunkParams {
            height: 8.0,
            radius_base: 0.5,
            ..default_params()
        };
        let mut rng = SeededRng::new(42);
        let output = generate_trunk(&params, &mut rng).unwrap();

        let size = output.mesh.bounding_box.size();

        // Height should be roughly trunk_height (within 20% for noise displacement)
        assert!(
            (size.y - params.height).abs() < params.height * 0.3,
            "bbox height {:.2} should be near trunk height {:.2}",
            size.y,
            params.height
        );

        // Width should be roughly 2 * radius_base (plus some noise margin)
        assert!(
            size.x < params.radius_base * 4.0,
            "bbox width {:.2} too large for radius_base {:.2}",
            size.x,
            params.radius_base
        );
        assert!(
            size.z < params.radius_base * 4.0,
            "bbox depth {:.2} too large for radius_base {:.2}",
            size.z,
            params.radius_base
        );
    }

    #[test]
    fn determinism_same_seed() {
        let params = default_params();
        let mut rng1 = SeededRng::new(12345);
        let mut rng2 = SeededRng::new(12345);

        let out1 = generate_trunk(&params, &mut rng1).unwrap();
        let out2 = generate_trunk(&params, &mut rng2).unwrap();

        // Vertex-for-vertex equality
        assert_eq!(
            out1.mesh.vertices.len(),
            out2.mesh.vertices.len(),
            "vertex count should match"
        );
        for (i, (v1, v2)) in out1
            .mesh
            .vertices
            .iter()
            .zip(out2.mesh.vertices.iter())
            .enumerate()
        {
            assert_eq!(v1.position, v2.position, "vertex {i} position differs");
        }
        assert_eq!(out1.mesh.indices, out2.mesh.indices);
    }

    #[test]
    fn different_seeds_differ() {
        let params = TrunkParams {
            curve_noise: 0.5, // ensure noise is visible
            ..default_params()
        };
        let mut rng1 = SeededRng::new(1);
        let mut rng2 = SeededRng::new(2);

        let out1 = generate_trunk(&params, &mut rng1).unwrap();
        let out2 = generate_trunk(&params, &mut rng2).unwrap();

        // At least some spine positions should differ
        let differs = out1
            .mesh
            .vertices
            .iter()
            .zip(out2.mesh.vertices.iter())
            .any(|(v1, v2)| v1.position != v2.position);
        assert!(differs, "different seeds should produce different meshes");
    }

    #[test]
    fn zero_curve_noise_straight_trunk() {
        let params = TrunkParams {
            curve_noise: 0.0,
            segments: 20,
            ..default_params()
        };
        let mut rng = SeededRng::new(42);
        let output = generate_trunk(&params, &mut rng).unwrap();

        // The bounding box center X and Z should be near zero
        let center = output.mesh.bounding_box.center();
        assert!(
            center.x.abs() < 1e-5,
            "straight trunk center.x should be ~0, got {:.6}",
            center.x
        );
        assert!(
            center.z.abs() < 1e-5,
            "straight trunk center.z should be ~0, got {:.6}",
            center.z
        );
    }

    #[test]
    fn normal_map_validity() {
        let params = default_params();
        let mut rng = SeededRng::new(42);
        let output = generate_trunk(&params, &mut rng).unwrap();

        assert_eq!(output.normal_map.width, params.bark_normal_resolution);
        assert_eq!(output.normal_map.height, params.bark_normal_resolution);
        assert!(output.normal_map.validate().is_ok());

        // Z channel always > 127 (positive hemisphere)
        let res = params.bark_normal_resolution as usize;
        for y in 0..res {
            for x in 0..res {
                let offset = (y * res + x) * 4;
                let b = output.normal_map.pixels[offset + 2];
                assert!(b > 127, "normal map Z at ({x},{y}) = {b}, expected > 127");
            }
        }
    }

    #[test]
    fn tip_output() {
        let params = TrunkParams {
            height: 6.0,
            curve_noise: 0.0,
            ..default_params()
        };
        let mut rng = SeededRng::new(42);
        let output = generate_trunk(&params, &mut rng).unwrap();

        // Tip position Y should be approximately the trunk height
        assert!(
            (output.tip_position.y - params.height).abs() < 0.1,
            "tip_position.y = {:.2}, expected ~{:.2}",
            output.tip_position.y,
            params.height
        );

        // Tip direction should be roughly upward for a straight trunk
        assert!(
            output.tip_direction.y > 0.9,
            "tip_direction should be roughly upward, got {:?}",
            output.tip_direction
        );
    }

    #[test]
    fn material_is_bark() {
        let params = default_params();
        let mut rng = SeededRng::new(42);
        let output = generate_trunk(&params, &mut rng).unwrap();

        assert_eq!(output.mesh.materials.len(), 1);
        assert_eq!(output.mesh.materials[0].name, "bark");
        assert_eq!(output.mesh.materials[0].metallic, 0.0);
        assert_eq!(output.mesh.materials[0].roughness, params.bark_roughness);
        assert_eq!(output.mesh.materials[0].base_color, params.bark_color);
    }

    #[test]
    fn vertex_normals_are_unit_length() {
        let params = default_params();
        let mut rng = SeededRng::new(42);
        let output = generate_trunk(&params, &mut rng).unwrap();

        for (i, v) in output.mesh.vertices.iter().enumerate() {
            let n = Vec3::from_array(v.normal);
            let len = n.length();
            assert!(
                (len - 1.0).abs() < 0.01,
                "vertex {i} normal length = {len:.4}, expected ~1.0"
            );
        }
    }
}
