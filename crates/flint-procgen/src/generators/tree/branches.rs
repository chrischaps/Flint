//! Branch generation — wires L-system and space colonization algorithms
//! to produce branch geometry attached to a trunk tip.

use crate::algorithms::lsystem::{stochastic_tree_lsystem, LSystemParams};
use crate::algorithms::mesh_builder::extrude::{
    build_spine_from_positions, extrude_along_curve, ExtrudeParams,
};
use crate::algorithms::mesh_builder::MeshBuilder;
use crate::algorithms::space_colonization::{
    CrownVolume, SpaceColonization, SpaceColonizationParams,
};
use crate::types::{BranchSegment, MaterialData, MeshData};
use crate::{Result, SeededRng};
use flint_core::spline::rotate_around_axis;
use flint_core::Vec3;

use super::{BranchMethod, BranchOutput, BranchParams, CrownShape};

// ─── Public API ─────────────────────────────────────────────────────────────

/// Generate branches from parameters and trunk attachment info.
///
/// Dispatches to the appropriate algorithm (L-system or space colonization),
/// converts the resulting segments to mesh geometry, and extracts terminal
/// tip positions for leaf placement.
pub fn generate_branches(
    params: &BranchParams,
    trunk_tip: Vec3,
    trunk_dir: Vec3,
    trunk_radius_top: f32,
    rng: &mut SeededRng,
) -> Result<BranchOutput> {
    let segments = match params.method {
        BranchMethod::LSystem => {
            generate_lsystem_branches(params, trunk_tip, trunk_dir, trunk_radius_top, rng)?
        }
        BranchMethod::SpaceColonization => {
            generate_colonization_branches(params, trunk_tip, trunk_dir, trunk_radius_top, rng)?
        }
    };

    if segments.is_empty() {
        // Return empty but valid output
        let empty_mesh = MeshBuilder::new().build();
        return Ok(BranchOutput {
            mesh: empty_mesh,
            tip_positions: Vec::new(),
            tip_directions: Vec::new(),
            segment_count: 0,
        });
    }

    let (tip_positions, tip_directions) = extract_tips(&segments);
    let mesh = segments_to_mesh(&segments, params.radial_segments)?;

    Ok(BranchOutput {
        mesh,
        tip_positions,
        tip_directions,
        segment_count: segments.len(),
    })
}

// ─── L-System Path ──────────────────────────────────────────────────────────

/// Generate branch segments using the L-system algorithm.
///
/// 1. Builds an L-system with stochastic angle variation
/// 2. Generates segments starting at origin heading +Y
/// 3. Transforms segments from L-system space to world space
/// 4. Filters out depth-0 segments (the L-system "trunk")
fn generate_lsystem_branches(
    params: &BranchParams,
    trunk_tip: Vec3,
    trunk_dir: Vec3,
    trunk_radius_top: f32,
    rng: &mut SeededRng,
) -> Result<Vec<BranchSegment>> {
    let base_angle = (params.branch_angle_min + params.branch_angle_max) / 2.0;
    let variation = (params.branch_angle_max - params.branch_angle_min) / 2.0;
    let angle_variation = variation + params.lsystem_angle_variation;

    // Initial branch length derived from trunk parameters
    let initial_length = trunk_radius_top * 8.0; // reasonable starting branch length
    let initial_radius = trunk_radius_top * 0.8;

    let lsystem_params = LSystemParams {
        default_angle_deg: base_angle,
        length_decay: params.branch_length_falloff,
        radius_decay: params.branch_radius_falloff,
        max_depth: params.lsystem_iterations,
        max_segments: params.max_segments,
    };

    let lsystem = stochastic_tree_lsystem(
        initial_length,
        initial_radius,
        base_angle,
        angle_variation,
        lsystem_params,
    )?;

    let mut lsystem_rng = rng.fork("lsystem");
    let mut segments = lsystem.generate_segments(&mut lsystem_rng)?;

    // Transform segments from L-system space (+Y heading at origin)
    // to world space (trunk_dir heading at trunk_tip)
    transform_segments(&mut segments, trunk_tip, trunk_dir);

    // Filter out depth-0 segments (the L-system's internal "trunk" —
    // we already have the real trunk from Task 3.1)
    segments.retain(|seg| seg.depth > 0);

    Ok(segments)
}

// ─── Space Colonization Path ────────────────────────────────────────────────

/// Generate branch segments using the space colonization algorithm.
///
/// Positions the crown volume above the trunk tip and runs the colonization
/// algorithm directly in world space.
fn generate_colonization_branches(
    params: &BranchParams,
    trunk_tip: Vec3,
    trunk_dir: Vec3,
    trunk_radius_top: f32,
    rng: &mut SeededRng,
) -> Result<Vec<BranchSegment>> {
    // Crown center is positioned along trunk direction above the tip
    let crown_center = trunk_tip + trunk_dir * params.crown_radius;

    let crown = match params.crown_shape {
        CrownShape::Sphere => CrownVolume::Sphere {
            center: crown_center,
            radius: params.crown_radius,
        },
        CrownShape::Ellipsoid => {
            let height = params.crown_radius * params.crown_height_ratio;
            CrownVolume::Ellipsoid {
                center: crown_center,
                radii: Vec3::new(params.crown_radius, height, params.crown_radius),
            }
        }
        CrownShape::Hemisphere => CrownVolume::Hemisphere {
            center: trunk_tip, // hemisphere grows upward from trunk tip
            radius: params.crown_radius,
        },
    };

    let sc_params = SpaceColonizationParams {
        crown,
        num_attraction_points: params.num_attraction_points,
        kill_distance: params.kill_distance,
        influence_distance: params.influence_distance,
        step_size: params.step_size,
        root_position: trunk_tip,
        trunk_steps: 0, // no internal trunk — we already have the real one
        max_iterations: 200,
        max_nodes: params.max_segments,
        base_radius: trunk_radius_top,
        tip_radius: trunk_radius_top * 0.05,
    };

    let gen = SpaceColonization::new(sc_params)?;
    let mut sc_rng = rng.fork("colonization");
    gen.generate_segments(&mut sc_rng)
}

// ─── Segment-to-Mesh Conversion ────────────────────────────────────────────

/// Convert branch segments into a mesh using per-segment extrusion.
///
/// Each segment becomes a small tapered tube. Radial resolution decreases
/// with depth for performance. Degenerate segments (near-zero length) are
/// skipped.
pub fn segments_to_mesh(segments: &[BranchSegment], base_radial_segments: u32) -> Result<MeshData> {
    let mut builder = MeshBuilder::new();

    for seg in segments {
        let len = seg.length();
        if len < 1e-6 {
            continue; // skip degenerate segments
        }

        // Decrease radial resolution for deeper branches
        let radial = (base_radial_segments as i32 - seg.depth as i32).max(3) as u32;

        let spine = build_spine_from_positions(
            &[seg.start, seg.end],
            seg.radius_start,
            seg.radius_end,
        );

        let tube = extrude_along_curve(&ExtrudeParams {
            spine,
            radial_segments: radial,
            caps: false, // no caps to avoid Z-fighting at junctions
            uv_length_scale: 1.0,
        })?;

        builder.merge(&tube);
    }

    builder.compute_normals();
    builder.compute_tangents();

    // Set bark material
    let mut mesh = builder.build();
    mesh.materials = vec![MaterialData {
        name: "bark".into(),
        base_color: [0.34, 0.2, 0.1, 1.0],
        metallic: 0.0,
        roughness: 0.9,
    }];

    Ok(mesh)
}

// ─── Tip Extraction ─────────────────────────────────────────────────────────

/// Identify terminal branch tips for leaf placement.
///
/// A segment is terminal if its end position is not the start of any other
/// segment. Returns (positions, directions) of all terminal endpoints.
pub fn extract_tips(segments: &[BranchSegment]) -> (Vec<Vec3>, Vec<Vec3>) {
    // Collect all start positions into a set for lookup
    let starts: std::collections::HashSet<[i32; 3]> = segments
        .iter()
        .map(|seg| quantize_position(seg.start))
        .collect();

    let mut positions = Vec::new();
    let mut directions = Vec::new();

    for seg in segments {
        let end_key = quantize_position(seg.end);
        if !starts.contains(&end_key) {
            positions.push(seg.end);
            let dir = seg.direction();
            directions.push(if dir.length() > 1e-6 { dir } else { Vec3::UP });
        }
    }

    (positions, directions)
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Quantize a Vec3 position to a grid for HashSet-based lookup.
/// Uses a resolution fine enough to distinguish separate segment endpoints.
fn quantize_position(v: Vec3) -> [i32; 3] {
    const SCALE: f32 = 10000.0;
    [
        (v.x * SCALE).round() as i32,
        (v.y * SCALE).round() as i32,
        (v.z * SCALE).round() as i32,
    ]
}

/// Transform segments from L-system space (origin, +Y heading) to world space
/// (trunk_tip position, trunk_dir heading).
///
/// Uses Rodrigues' rotation formula to rotate from +Y to trunk_dir, then
/// translates by trunk_tip.
fn transform_segments(segments: &mut [BranchSegment], trunk_tip: Vec3, trunk_dir: Vec3) {
    let from = Vec3::UP; // L-system heading is +Y
    let to = trunk_dir.normalized();

    // Compute rotation axis and angle
    let dot = from.dot(&to).clamp(-1.0, 1.0);

    if (dot - 1.0).abs() < 1e-6 {
        // Already aligned — just translate
        for seg in segments.iter_mut() {
            seg.start = seg.start + trunk_tip;
            seg.end = seg.end + trunk_tip;
        }
        return;
    }

    if (dot + 1.0).abs() < 1e-6 {
        // Anti-parallel — rotate 180° around any perpendicular axis
        for seg in segments.iter_mut() {
            seg.start = Vec3::new(-seg.start.x, -seg.start.y, -seg.start.z) + trunk_tip;
            seg.end = Vec3::new(-seg.end.x, -seg.end.y, -seg.end.z) + trunk_tip;
        }
        return;
    }

    let axis = from.cross(&to).normalized();
    let angle = dot.acos();

    for seg in segments.iter_mut() {
        seg.start = rotate_around_axis(seg.start, axis, angle) + trunk_tip;
        seg.end = rotate_around_axis(seg.end, axis, angle) + trunk_tip;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_branch_params() -> BranchParams {
        BranchParams::default()
    }

    // ── TOML Parsing Tests ──────────────────────────────────────────────

    #[test]
    fn branch_params_from_toml_defaults() {
        let toml_val: toml::Value = toml::toml! {
            some_unrelated_field = true
        }
        .into();
        let p = BranchParams::from_toml(&toml_val).unwrap();
        assert_eq!(p.branch_levels, 3);
        assert_eq!(p.radial_segments, 6);
        assert!(matches!(p.method, BranchMethod::LSystem));
    }

    #[test]
    fn branch_params_from_toml_full() {
        let toml_val: toml::Value = toml::toml! {
            branch_method = "space_colonization"
            branch_levels = 4
            branch_angle_min = 15.0
            branch_angle_max = 45.0
            branch_length_falloff = 0.65
            branch_radius_falloff = 0.5
            branch_max_segments = 8000
            branch_radial_segments = 8
            crown_shape = "ellipsoid"
            crown_radius = 4.0
            crown_height_ratio = 2.0
            num_attraction_points = 800
            kill_distance = 0.3
            influence_distance = 5.0
            step_size = 0.25
            lsystem_iterations = 4
            lsystem_angle_variation = 15.0
        }
        .into();
        let p = BranchParams::from_toml(&toml_val).unwrap();
        assert!(matches!(p.method, BranchMethod::SpaceColonization));
        assert_eq!(p.branch_levels, 4);
        assert!((p.branch_angle_min - 15.0).abs() < 1e-5);
        assert!((p.branch_angle_max - 45.0).abs() < 1e-5);
        assert!((p.branch_length_falloff - 0.65).abs() < 1e-5);
        assert!((p.branch_radius_falloff - 0.5).abs() < 1e-5);
        assert_eq!(p.max_segments, 8000);
        assert_eq!(p.radial_segments, 8);
        assert!(matches!(p.crown_shape, CrownShape::Ellipsoid));
        assert!((p.crown_radius - 4.0).abs() < 1e-5);
        assert!((p.crown_height_ratio - 2.0).abs() < 1e-5);
        assert_eq!(p.num_attraction_points, 800);
        assert!((p.kill_distance - 0.3).abs() < 1e-5);
        assert!((p.influence_distance - 5.0).abs() < 1e-5);
        assert!((p.step_size - 0.25).abs() < 1e-5);
        assert_eq!(p.lsystem_iterations, 4);
        assert!((p.lsystem_angle_variation - 15.0).abs() < 1e-5);
    }

    #[test]
    fn branch_params_invalid_method() {
        let toml_val: toml::Value = toml::toml! {
            branch_method = "magic_tree"
        }
        .into();
        assert!(BranchParams::from_toml(&toml_val).is_err());
    }

    // ── L-System Path Tests ─────────────────────────────────────────────

    #[test]
    fn lsystem_produces_segments() {
        let params = default_branch_params();
        let trunk_tip = Vec3::new(0.0, 5.0, 0.0);
        let trunk_dir = Vec3::UP;
        let mut rng = SeededRng::new(42);

        let segments =
            generate_lsystem_branches(&params, trunk_tip, trunk_dir, 0.1, &mut rng).unwrap();

        assert!(!segments.is_empty(), "L-system should produce segments");
        for seg in &segments {
            assert!(seg.start.x.is_finite());
            assert!(seg.start.y.is_finite());
            assert!(seg.start.z.is_finite());
            assert!(seg.end.x.is_finite());
            assert!(seg.end.y.is_finite());
            assert!(seg.end.z.is_finite());
        }
    }

    #[test]
    fn lsystem_segments_attached_to_trunk() {
        let params = default_branch_params();
        let trunk_tip = Vec3::new(0.0, 5.0, 0.0);
        let trunk_dir = Vec3::UP;
        let mut rng = SeededRng::new(42);

        let segments =
            generate_lsystem_branches(&params, trunk_tip, trunk_dir, 0.1, &mut rng).unwrap();

        // At least one segment should start near the trunk tip
        let near_tip = segments.iter().any(|seg| {
            let d = seg.start - trunk_tip;
            d.length() < 2.0 // within reasonable distance of trunk tip
        });
        assert!(
            near_tip,
            "at least one branch segment should start near the trunk tip"
        );
    }

    #[test]
    fn lsystem_determinism() {
        let params = default_branch_params();
        let trunk_tip = Vec3::new(0.0, 5.0, 0.0);
        let trunk_dir = Vec3::UP;

        let mut rng1 = SeededRng::new(42);
        let seg1 =
            generate_lsystem_branches(&params, trunk_tip, trunk_dir, 0.1, &mut rng1).unwrap();

        let mut rng2 = SeededRng::new(42);
        let seg2 =
            generate_lsystem_branches(&params, trunk_tip, trunk_dir, 0.1, &mut rng2).unwrap();

        assert_eq!(seg1.len(), seg2.len());
        for (a, b) in seg1.iter().zip(seg2.iter()) {
            assert!((a.start.x - b.start.x).abs() < 1e-4);
            assert!((a.start.y - b.start.y).abs() < 1e-4);
            assert!((a.start.z - b.start.z).abs() < 1e-4);
            assert!((a.end.x - b.end.x).abs() < 1e-4);
        }
    }

    // ── Space Colonization Path Tests ───────────────────────────────────

    #[test]
    fn colonization_produces_segments() {
        let params = BranchParams {
            method: BranchMethod::SpaceColonization,
            ..default_branch_params()
        };
        let trunk_tip = Vec3::new(0.0, 5.0, 0.0);
        let trunk_dir = Vec3::UP;
        let mut rng = SeededRng::new(42);

        let segments =
            generate_colonization_branches(&params, trunk_tip, trunk_dir, 0.1, &mut rng).unwrap();

        assert!(
            !segments.is_empty(),
            "space colonization should produce segments"
        );
        for seg in &segments {
            assert!(seg.start.x.is_finite());
            assert!(seg.end.y.is_finite());
        }
    }

    #[test]
    fn colonization_root_at_trunk_tip() {
        let params = BranchParams {
            method: BranchMethod::SpaceColonization,
            ..default_branch_params()
        };
        let trunk_tip = Vec3::new(1.0, 7.0, -2.0);
        let trunk_dir = Vec3::UP;
        let mut rng = SeededRng::new(42);

        let segments =
            generate_colonization_branches(&params, trunk_tip, trunk_dir, 0.1, &mut rng).unwrap();

        // First segment should start at the root position (trunk tip)
        assert!(!segments.is_empty());
        let first = &segments[0];
        assert!(
            (first.start.x - trunk_tip.x).abs() < 0.01
                && (first.start.y - trunk_tip.y).abs() < 0.01
                && (first.start.z - trunk_tip.z).abs() < 0.01,
            "first segment start {:?} should be at trunk tip {:?}",
            first.start,
            trunk_tip
        );
    }

    #[test]
    fn colonization_determinism() {
        let params = BranchParams {
            method: BranchMethod::SpaceColonization,
            ..default_branch_params()
        };
        let trunk_tip = Vec3::new(0.0, 5.0, 0.0);
        let trunk_dir = Vec3::UP;

        let mut rng1 = SeededRng::new(42);
        let seg1 =
            generate_colonization_branches(&params, trunk_tip, trunk_dir, 0.1, &mut rng1).unwrap();

        let mut rng2 = SeededRng::new(42);
        let seg2 =
            generate_colonization_branches(&params, trunk_tip, trunk_dir, 0.1, &mut rng2).unwrap();

        assert_eq!(seg1.len(), seg2.len());
        for (a, b) in seg1.iter().zip(seg2.iter()) {
            assert_eq!(a.start.x, b.start.x);
            assert_eq!(a.end.y, b.end.y);
        }
    }

    // ── Mesh & Integration Tests ────────────────────────────────────────

    #[test]
    fn segments_to_mesh_validity() {
        let segments = vec![
            BranchSegment {
                start: Vec3::new(0.0, 5.0, 0.0),
                end: Vec3::new(1.0, 6.0, 0.0),
                radius_start: 0.1,
                radius_end: 0.08,
                depth: 1,
            },
            BranchSegment {
                start: Vec3::new(1.0, 6.0, 0.0),
                end: Vec3::new(2.0, 6.5, 0.5),
                radius_start: 0.08,
                radius_end: 0.05,
                depth: 2,
            },
        ];

        let mesh = segments_to_mesh(&segments, 6).unwrap();
        assert!(mesh.validate().is_ok(), "mesh should be valid");
        assert!(mesh.vertex_count() > 0);
        assert!(mesh.triangle_count() > 0);
    }

    #[test]
    fn no_degenerate_triangles() {
        let params = default_branch_params();
        let trunk_tip = Vec3::new(0.0, 5.0, 0.0);
        let trunk_dir = Vec3::UP;
        let mut rng = SeededRng::new(42);

        let output = generate_branches(&params, trunk_tip, trunk_dir, 0.1, &mut rng).unwrap();

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
    fn both_methods_produce_valid_output() {
        let trunk_tip = Vec3::new(0.0, 5.0, 0.0);
        let trunk_dir = Vec3::UP;

        // L-system
        let params_ls = BranchParams {
            method: BranchMethod::LSystem,
            ..default_branch_params()
        };
        let mut rng1 = SeededRng::new(42);
        let out_ls = generate_branches(&params_ls, trunk_tip, trunk_dir, 0.1, &mut rng1).unwrap();
        assert!(out_ls.mesh.validate().is_ok());
        assert!(out_ls.segment_count > 0);

        // Space colonization
        let params_sc = BranchParams {
            method: BranchMethod::SpaceColonization,
            ..default_branch_params()
        };
        let mut rng2 = SeededRng::new(42);
        let out_sc = generate_branches(&params_sc, trunk_tip, trunk_dir, 0.1, &mut rng2).unwrap();
        assert!(out_sc.mesh.validate().is_ok());
        assert!(out_sc.segment_count > 0);
    }

    #[test]
    fn merged_tree_mesh_valid() {
        use crate::generators::tree::{generate_tree, TrunkParams};

        let trunk_params = TrunkParams::default();
        let branch_params = default_branch_params();
        let mut rng = SeededRng::new(42);

        let tree = generate_tree(&trunk_params, &branch_params, &mut rng).unwrap();
        assert!(
            tree.mesh.validate().is_ok(),
            "combined tree mesh should be valid"
        );
        assert!(tree.mesh.vertex_count() > 0);
        assert!(tree.mesh.triangle_count() > 0);
    }

    #[test]
    fn branch_tips_extracted() {
        let params = default_branch_params();
        let trunk_tip = Vec3::new(0.0, 5.0, 0.0);
        let trunk_dir = Vec3::UP;
        let mut rng = SeededRng::new(42);

        let output = generate_branches(&params, trunk_tip, trunk_dir, 0.1, &mut rng).unwrap();
        assert!(
            !output.tip_positions.is_empty(),
            "should extract at least one branch tip"
        );
        assert_eq!(output.tip_positions.len(), output.tip_directions.len());

        // Tips should be at finite positions
        for pos in &output.tip_positions {
            assert!(pos.x.is_finite());
            assert!(pos.y.is_finite());
            assert!(pos.z.is_finite());
        }
    }

    // ── Helper Tests ────────────────────────────────────────────────────

    #[test]
    fn transform_identity_when_aligned() {
        let mut segments = vec![BranchSegment {
            start: Vec3::ZERO,
            end: Vec3::new(0.0, 1.0, 0.0),
            radius_start: 0.1,
            radius_end: 0.05,
            depth: 0,
        }];

        let tip = Vec3::new(0.0, 5.0, 0.0);
        transform_segments(&mut segments, tip, Vec3::UP);

        // Should just translate
        assert!((segments[0].start.y - 5.0).abs() < 1e-4);
        assert!((segments[0].end.y - 6.0).abs() < 1e-4);
    }

    #[test]
    fn extract_tips_finds_terminal_segments() {
        let segments = vec![
            // Chain: A → B → C (terminal)
            BranchSegment {
                start: Vec3::ZERO,
                end: Vec3::new(1.0, 0.0, 0.0),
                radius_start: 0.1,
                radius_end: 0.08,
                depth: 0,
            },
            BranchSegment {
                start: Vec3::new(1.0, 0.0, 0.0),
                end: Vec3::new(2.0, 1.0, 0.0),
                radius_start: 0.08,
                radius_end: 0.05,
                depth: 1,
            },
            // Branch: A → D (terminal)
            BranchSegment {
                start: Vec3::ZERO,
                end: Vec3::new(-1.0, 1.0, 0.0),
                radius_start: 0.1,
                radius_end: 0.06,
                depth: 1,
            },
        ];

        let (tips, dirs) = extract_tips(&segments);
        // C and D are terminal — their ends are not the start of any other segment
        assert_eq!(tips.len(), 2, "should find 2 terminal tips");
        assert_eq!(dirs.len(), 2);
    }
}
