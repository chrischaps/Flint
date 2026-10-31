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
    /// The trunk's centreline from root to tip, the rings' centres (for
    /// attaching branches along it).
    pub spine: Vec<Vec3>,
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
    let positions = trunk_spine(params, rng);

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
    // The bark map wraps once around the base and tiles up the trunk with
    // square texels, rather than stretching one tile over the whole height.
    let spine = build_spine_from_positions(&positions, params.radius_base, params.radius_top);
    let spine_length: f32 = positions.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    let circumference = std::f32::consts::TAU * params.radius_base;
    let mut mesh = extrude_along_curve(&ExtrudeParams {
        spine,
        radial_segments: params.radial_segments,
        caps: true,
        uv_length_scale: (spine_length / circumference).max(1.0),
    })?;

    // Recompute normals and tangents for correct lighting
    compute_normals(&mut mesh.vertices, &mesh.indices);
    compute_tangents(&mut mesh.vertices, &mesh.indices);

    // --- Bark normal map ---
    let mut bark_rng = rng.fork("bark");
    let normal_map = generate_bark_normal_map(
        &mut bark_rng,
        params.bark_normal_resolution,
        params.bark_normal_strength,
    );

    // --- Material ---
    mesh.materials = vec![MaterialData {
        name: "bark".into(),
        base_color: params.bark_color,
        metallic: 0.0,
        roughness: params.bark_roughness,
        normal_map: Some(normal_map.clone()),
    }];

    Ok(TrunkOutput {
        mesh,
        normal_map,
        tip_position,
        tip_direction,
        spine: positions,
    })
}

/// Wavelength of the trunk's main sway, as a fraction of its height.
const SWAY_WAVELENGTH: f32 = 0.6;

/// Fraction of the height over which the root eases from vertical into the
/// sway, so the trunk leaves the ground straight up.
const ROOT_EASE: f32 = 0.15;

/// Generate the trunk's spine: `segments + 1` points from the origin up to
/// about `height`, swayed sideways by two octaves of Perlin noise.
///
/// The noise is read along a line between lattice rows. Read along a row
/// (`y = 0`), Perlin is zero at every whole unit, which pinned the old spine
/// back to the axis once a metre and always at the tip.
///
/// If the sway bends the spine more sharply than the tube's radius allows,
/// it is scaled down until every joint fits, so the extruded rings never
/// cross each other.
pub(super) fn trunk_spine(params: &TrunkParams, rng: &mut SeededRng) -> Vec<Vec3> {
    let mut spine_rng = rng.fork("trunk_spine");
    let noise_x = PerlinNoise::new(spine_rng.fork("noise_x").seed());
    let noise_z = PerlinNoise::new(spine_rng.fork("noise_z").seed());

    let freq = 1.0 / (SWAY_WAVELENGTH * params.height) as f64;
    let sway = |noise: &PerlinNoise, y: f32| -> f32 {
        let y = y as f64 * freq;
        let n = noise.sample_2d(y, 0.5) + 0.35 * noise.sample_2d(y * 2.0 + 17.3, 3.5);
        (n / 1.35) as f32
    };

    let n = params.segments as usize;
    let offsets: Vec<(f32, f32)> = (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let y = t * params.height;
            let ease = smoothstep(t / ROOT_EASE);
            (
                (sway(&noise_x, y) - sway(&noise_x, 0.0)) * ease,
                (sway(&noise_z, y) - sway(&noise_z, 0.0)) * ease,
            )
        })
        .collect();

    let build = |amplitude: f32| -> Vec<Vec3> {
        offsets
            .iter()
            .enumerate()
            .map(|(i, &(x, z))| {
                let y = i as f32 / n as f32 * params.height;
                Vec3::new(x * amplitude, y, z * amplitude)
            })
            .collect()
    };

    let mut amplitude = params.curve_noise;
    let mut positions = build(amplitude);
    while amplitude > 1e-4 && !bends_fit(&positions, params) {
        amplitude *= 0.8;
        positions = build(amplitude);
    }
    if !bends_fit(&positions, params) {
        positions = build(0.0);
    }
    positions
}

/// Whether every joint of the spine turns gently enough for the tube around
/// it: the inside of a bend through angle θ needs `r·tan(θ/2)` of each
/// neighbouring segment, so adjacent rings cross once that exceeds half of
/// the shorter one.
pub(super) fn bends_fit(positions: &[Vec3], params: &TrunkParams) -> bool {
    let n = positions.len().saturating_sub(1).max(1) as f32;
    positions.windows(3).enumerate().all(|(i, w)| {
        let a = w[1] - w[0];
        let b = w[2] - w[1];
        let (la, lb) = (a.length(), b.length());
        if la < 1e-6 || lb < 1e-6 {
            return false;
        }
        let cos = (a.dot(&b) / (la * lb)).clamp(-1.0, 1.0);
        let half = cos.acos() * 0.5;
        let t = (i + 1) as f32 / n;
        let radius = params.radius_base + (params.radius_top - params.radius_base) * t;
        radius * half.tan() < 0.5 * la.min(lb)
    })
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
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

    fn horizontal(p: Vec3) -> f32 {
        (p.x * p.x + p.z * p.z).sqrt()
    }

    /// The sway reaches most of `curve_noise` somewhere on every seed, and the
    /// tip is not pulled back to the axis (as reading Perlin along a lattice
    /// row used to do at every whole metre).
    #[test]
    fn curve_noise_is_visible_along_the_whole_spine() {
        let params = TrunkParams {
            height: 8.0,
            segments: 16,
            curve_noise: 0.4,
            ..default_params()
        };
        let mut tips_off_axis = 0;
        for seed in 0..32 {
            let spine = trunk_spine(&params, &mut SeededRng::new(seed));
            let widest = spine.iter().map(|&p| horizontal(p)).fold(0.0, f32::max);
            assert!(
                widest > 0.1 * params.curve_noise,
                "seed {seed}: spine strays only {widest:.3} m"
            );
            let on_axis = spine[1..].iter().filter(|&&p| horizontal(p) < 1e-4).count();
            assert_eq!(
                on_axis, 0,
                "seed {seed}: {on_axis} spine points on the axis"
            );
            if horizontal(*spine.last().unwrap()) > 0.05 {
                tips_off_axis += 1;
            }
        }
        assert!(
            tips_off_axis > 24,
            "only {tips_off_axis}/32 tips leave the axis"
        );
    }

    /// The trunk is rooted at the origin and leaves the ground vertically.
    #[test]
    fn spine_is_rooted_at_the_origin() {
        let params = TrunkParams {
            curve_noise: 0.5,
            segments: 20,
            ..default_params()
        };
        for seed in 0..16 {
            let spine = trunk_spine(&params, &mut SeededRng::new(seed));
            assert_eq!(spine[0], Vec3::new(0.0, 0.0, 0.0));
            let first = spine[1] - spine[0];
            assert!(
                horizontal(first) < 0.1 * first.y,
                "seed {seed}: the root leans {:?}",
                first
            );
            assert!((spine.last().unwrap().y - params.height).abs() < 1e-4);
        }
    }

    /// However large `curve_noise` gets, no joint bends tighter than the
    /// tube's radius, so neighbouring rings never cross.
    #[test]
    fn heavy_curve_noise_never_self_intersects() {
        for (curve_noise, segments) in [(0.5, 10), (2.0, 12), (10.0, 24), (50.0, 40)] {
            let params = TrunkParams {
                curve_noise,
                segments,
                radius_base: 0.6,
                ..default_params()
            };
            for seed in 0..32 {
                let spine = trunk_spine(&params, &mut SeededRng::new(seed));
                assert!(
                    bends_fit(&spine, &params),
                    "curve_noise {curve_noise}, seed {seed}: a joint bends too sharply"
                );
                let out = generate_trunk(&params, &mut SeededRng::new(seed)).unwrap();
                assert!(out.mesh.validate().is_ok());
            }
        }
    }

    /// The taper runs from `radius_base` at the bottom ring to `radius_top`
    /// at the top: each ring's distance from its spine point matches.
    #[test]
    fn radius_tapers_from_base_to_top() {
        let params = TrunkParams {
            curve_noise: 0.0,
            radius_base: 0.5,
            radius_top: 0.1,
            ..default_params()
        };
        let out = generate_trunk(&params, &mut SeededRng::new(3)).unwrap();
        let ring_radius = |y: f32| {
            out.mesh
                .vertices
                .iter()
                .filter(|v| (v.position[1] - y).abs() < 1e-4)
                .map(|v| horizontal(Vec3::from_array(v.position)))
                .fold(0.0, f32::max)
        };
        assert!((ring_radius(0.0) - 0.5).abs() < 1e-3);
        assert!((ring_radius(params.height) - 0.1).abs() < 1e-3);
        assert!((ring_radius(params.height * 0.5) - 0.3).abs() < 1e-3);
    }

    /// Texels are square on the trunk's surface: the bark map wraps once
    /// around the base and repeats up the height instead of stretching.
    #[test]
    fn bark_uvs_repeat_up_the_trunk() {
        let params = TrunkParams {
            height: 6.0,
            radius_base: 0.3,
            curve_noise: 0.0,
            ..default_params()
        };
        let out = generate_trunk(&params, &mut SeededRng::new(1)).unwrap();
        let top_v = out
            .mesh
            .vertices
            .iter()
            .map(|v| v.uv[1])
            .fold(0.0, f32::max);
        let expected = 6.0 / (std::f32::consts::TAU * 0.3);
        assert!(
            (top_v - expected).abs() < 1e-3,
            "top v = {top_v}, expected {expected}"
        );
    }

    #[test]
    fn bark_material_carries_the_normal_map() {
        let out = generate_trunk(&default_params(), &mut SeededRng::new(5)).unwrap();
        assert_eq!(
            out.mesh.materials[0].normal_map.as_ref(),
            Some(&out.normal_map)
        );
    }
}
