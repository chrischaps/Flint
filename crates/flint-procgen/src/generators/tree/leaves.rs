//! Leaf geometry generation for procedural trees.
//!
//! Generates billboard quads or diamond-shaped mesh clusters at branch tips,
//! with configurable density, size variation, and color variation.

use crate::algorithms::mesh_builder::MeshBuilder;
use crate::types::{MaterialData, MeshData, Vertex};
use crate::Result;
use crate::SeededRng;
use flint_core::spline::rotate_around_axis;
use flint_core::Vec3;

use super::LeafParams;
use super::LeafStyle;

/// Generate leaf geometry at branch tips.
///
/// Dispatches to billboard or mesh cluster generation based on `params.style`.
/// Returns an empty mesh for [`LeafStyle::None`].
pub fn generate_leaves(
    tips: &[Vec3],
    tip_directions: &[Vec3],
    params: &LeafParams,
    rng: &mut SeededRng,
) -> Result<MeshData> {
    match params.style {
        LeafStyle::BillboardCluster => generate_billboard_leaves(tips, tip_directions, params, rng),
        LeafStyle::MeshCluster => generate_mesh_leaves(tips, tip_directions, params, rng),
        LeafStyle::None => {
            let builder = MeshBuilder::new();
            Ok(builder.build())
        }
    }
}

/// Normalize a Vec3, returning `fallback` if zero-length.
fn safe_normalize(v: Vec3, fallback: Vec3) -> Vec3 {
    let n = v.normalized();
    if n.length() < 1e-6 {
        fallback
    } else {
        n
    }
}

/// Generate billboard quad clusters at branch tips.
///
/// Each selected tip (sampled by `density`) gets `leaves_per_tip` quads
/// oriented perpendicular to the branch direction with random rotation.
fn generate_billboard_leaves(
    tips: &[Vec3],
    tip_directions: &[Vec3],
    params: &LeafParams,
    rng: &mut SeededRng,
) -> Result<MeshData> {
    let tip_count = tips.len().min(tip_directions.len());
    let estimated_leaves =
        (tip_count as f32 * params.density) as usize * params.leaves_per_tip as usize;
    let mut builder = MeshBuilder::with_capacity(estimated_leaves * 4, estimated_leaves * 6);

    builder.add_material(MaterialData {
        name: "leaf".into(),
        base_color: params.color_base,
        metallic: 0.0,
        roughness: 0.8,
    });

    for i in 0..tip_count {
        if !rng.next_bool(params.density as f64) {
            continue;
        }

        let tip = tips[i];
        let dir = safe_normalize(tip_directions[i], Vec3::UP);

        // Find a perpendicular basis
        let perp = perpendicular_to(dir);
        let bitangent = safe_normalize(dir.cross(&perp), Vec3::new(0.0, 0.0, 1.0));

        for _ in 0..params.leaves_per_tip {
            let scale = params.leaf_size
                * (1.0 + rng.next_range(-params.leaf_size_variation, params.leaf_size_variation));
            let half = scale * 0.5;

            // Random rotation around the branch direction
            let angle = rng.next_range(0.0, std::f32::consts::TAU);
            let u = rotate_around_axis(perp, dir, angle);
            let v = rotate_around_axis(bitangent, dir, angle);

            // Small random offset from the tip
            let offset = dir * rng.next_range(-0.05, 0.05)
                + u * rng.next_range(-0.1, 0.1)
                + v * rng.next_range(-0.1, 0.1);
            let center = tip + offset;

            let color = rng.next_color_variation(params.color_base, params.color_variation);
            let normal = safe_normalize(dir.cross(&u), Vec3::UP);

            let i0 = builder.add_vertex(Vertex {
                position: (center - u * half - v * half).to_array(),
                normal: normal.to_array(),
                tangent: [u.x, u.y, u.z, 1.0],
                uv: [0.0, 0.0],
            });
            let i1 = builder.add_vertex(Vertex {
                position: (center + u * half - v * half).to_array(),
                normal: normal.to_array(),
                tangent: [u.x, u.y, u.z, 1.0],
                uv: [1.0, 0.0],
            });
            let i2 = builder.add_vertex(Vertex {
                position: (center + u * half + v * half).to_array(),
                normal: normal.to_array(),
                tangent: [u.x, u.y, u.z, 1.0],
                uv: [1.0, 1.0],
            });
            let i3 = builder.add_vertex(Vertex {
                position: (center - u * half + v * half).to_array(),
                normal: normal.to_array(),
                tangent: [u.x, u.y, u.z, 1.0],
                uv: [0.0, 1.0],
            });

            // Color stored in material; per-vertex color not used in Vertex struct.
            // The color variation is aesthetic intent — we apply it via material on
            // the leaf group. For per-leaf color, we'd need vertex colors (future).
            let _ = color;

            builder.add_quad(i0, i1, i2, i3);
        }
    }

    Ok(builder.build())
}

/// Generate diamond/rhombus mesh clusters at branch tips.
///
/// Each selected tip gets `leaves_per_tip` small diamond shapes oriented
/// along the branch direction with random pitch/yaw offsets.
fn generate_mesh_leaves(
    tips: &[Vec3],
    tip_directions: &[Vec3],
    params: &LeafParams,
    rng: &mut SeededRng,
) -> Result<MeshData> {
    let tip_count = tips.len().min(tip_directions.len());
    let estimated_leaves =
        (tip_count as f32 * params.density) as usize * params.leaves_per_tip as usize;
    // Each diamond: 5 vertices (center + 4 points), 4 triangles
    let mut builder = MeshBuilder::with_capacity(estimated_leaves * 5, estimated_leaves * 12);

    builder.add_material(MaterialData {
        name: "leaf".into(),
        base_color: params.color_base,
        metallic: 0.0,
        roughness: 0.8,
    });

    for i in 0..tip_count {
        if !rng.next_bool(params.density as f64) {
            continue;
        }

        let tip = tips[i];
        let dir = safe_normalize(tip_directions[i], Vec3::UP);

        for _ in 0..params.leaves_per_tip {
            let scale = params.leaf_size
                * (1.0 + rng.next_range(-params.leaf_size_variation, params.leaf_size_variation));
            let half = scale * 0.5;

            // Random orientation offset
            let pitch_offset = rng.next_range(-0.5, 0.5);
            let yaw_offset = rng.next_range(-0.5, 0.5);

            let perp = perpendicular_to(dir);
            let bitangent = safe_normalize(dir.cross(&perp), Vec3::new(0.0, 0.0, 1.0));

            // Apply random pitch/yaw to the leaf direction
            let leaf_dir = safe_normalize(dir + perp * pitch_offset + bitangent * yaw_offset, dir);
            let leaf_perp = perpendicular_to(leaf_dir);
            let leaf_bi = safe_normalize(leaf_dir.cross(&leaf_perp), Vec3::new(0.0, 0.0, 1.0));

            // Small random offset from the tip
            let offset = dir * rng.next_range(-0.05, 0.05)
                + perp * rng.next_range(-0.1, 0.1)
                + bitangent * rng.next_range(-0.1, 0.1);
            let center = tip + offset;

            let _color = rng.next_color_variation(params.color_base, params.color_variation);
            let normal = leaf_dir;

            // Diamond shape: center, +X, +Y, -X, -Y in leaf space
            let i_center = builder.add_vertex(Vertex {
                position: center.to_array(),
                normal: normal.to_array(),
                tangent: [leaf_perp.x, leaf_perp.y, leaf_perp.z, 1.0],
                uv: [0.5, 0.5],
            });
            let i_right = builder.add_vertex(Vertex {
                position: (center + leaf_perp * half).to_array(),
                normal: normal.to_array(),
                tangent: [leaf_perp.x, leaf_perp.y, leaf_perp.z, 1.0],
                uv: [1.0, 0.5],
            });
            let i_top = builder.add_vertex(Vertex {
                position: (center + leaf_bi * half).to_array(),
                normal: normal.to_array(),
                tangent: [leaf_perp.x, leaf_perp.y, leaf_perp.z, 1.0],
                uv: [0.5, 0.0],
            });
            let i_left = builder.add_vertex(Vertex {
                position: (center - leaf_perp * half).to_array(),
                normal: normal.to_array(),
                tangent: [leaf_perp.x, leaf_perp.y, leaf_perp.z, 1.0],
                uv: [0.0, 0.5],
            });
            let i_bottom = builder.add_vertex(Vertex {
                position: (center - leaf_bi * half).to_array(),
                normal: normal.to_array(),
                tangent: [leaf_perp.x, leaf_perp.y, leaf_perp.z, 1.0],
                uv: [0.5, 1.0],
            });

            // 4 triangles forming the diamond
            builder.add_triangle(i_center, i_right, i_top);
            builder.add_triangle(i_center, i_top, i_left);
            builder.add_triangle(i_center, i_left, i_bottom);
            builder.add_triangle(i_center, i_bottom, i_right);
        }
    }

    Ok(builder.build())
}

/// Find a vector perpendicular to `v`.
fn perpendicular_to(v: Vec3) -> Vec3 {
    let candidate = if v.x.abs() < 0.9 {
        Vec3::RIGHT
    } else {
        Vec3::UP
    };
    let result = v.cross(&candidate).normalized();
    if result.length() < 1e-6 {
        Vec3::RIGHT
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_tips() -> (Vec<Vec3>, Vec<Vec3>) {
        let tips = vec![
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(1.0, 4.0, 0.5),
            Vec3::new(-0.5, 4.5, -0.3),
            Vec3::new(0.3, 3.5, 0.8),
            Vec3::new(-0.8, 4.0, 0.2),
            Vec3::new(0.5, 4.8, -0.5),
            Vec3::new(0.0, 3.8, 0.6),
            Vec3::new(-0.3, 4.2, 0.1),
            Vec3::new(0.7, 3.9, -0.2),
            Vec3::new(-0.6, 4.1, 0.4),
        ];
        let dirs = tips
            .iter()
            .map(|t| (*t - Vec3::new(0.0, 2.0, 0.0)).normalized())
            .collect();
        (tips, dirs)
    }

    #[test]
    fn billboard_leaves_produce_valid_mesh() {
        let (tips, dirs) = sample_tips();
        let params = LeafParams {
            style: LeafStyle::BillboardCluster,
            density: 1.0,
            leaves_per_tip: 3,
            ..LeafParams::default()
        };
        let mut rng = SeededRng::new(42);
        let mesh = generate_leaves(&tips, &dirs, &params, &mut rng).unwrap();

        assert!(mesh.validate().is_ok());
        assert!(mesh.vertex_count() > 0);
        // Each leaf is a quad: 4 verts, 2 tris (6 indices)
        assert_eq!(mesh.vertex_count() % 4, 0);
        assert_eq!(mesh.triangle_count() % 2, 0);
    }

    #[test]
    fn mesh_cluster_leaves_produce_valid_mesh() {
        let (tips, dirs) = sample_tips();
        let params = LeafParams {
            style: LeafStyle::MeshCluster,
            density: 1.0,
            leaves_per_tip: 3,
            ..LeafParams::default()
        };
        let mut rng = SeededRng::new(42);
        let mesh = generate_leaves(&tips, &dirs, &params, &mut rng).unwrap();

        assert!(mesh.validate().is_ok());
        assert!(mesh.vertex_count() > 0);
        // Each diamond: 5 verts, 4 tris
        assert_eq!(mesh.vertex_count() % 5, 0);
        assert_eq!(mesh.triangle_count() % 4, 0);
    }

    #[test]
    fn none_style_returns_empty_mesh() {
        let (tips, dirs) = sample_tips();
        let params = LeafParams {
            style: LeafStyle::None,
            ..LeafParams::default()
        };
        let mut rng = SeededRng::new(42);
        let mesh = generate_leaves(&tips, &dirs, &params, &mut rng).unwrap();
        assert_eq!(mesh.vertex_count(), 0);
        assert_eq!(mesh.triangle_count(), 0);
    }

    #[test]
    fn leaf_count_correlates_with_density() {
        let (tips, dirs) = sample_tips();

        let params_full = LeafParams {
            style: LeafStyle::BillboardCluster,
            density: 1.0,
            leaves_per_tip: 4,
            ..LeafParams::default()
        };
        let params_half = LeafParams {
            style: LeafStyle::BillboardCluster,
            density: 0.5,
            leaves_per_tip: 4,
            ..LeafParams::default()
        };

        let mut rng_full = SeededRng::new(42);
        let mesh_full = generate_leaves(&tips, &dirs, &params_full, &mut rng_full).unwrap();

        let mut rng_half = SeededRng::new(42);
        let mesh_half = generate_leaves(&tips, &dirs, &params_half, &mut rng_half).unwrap();

        // density=1.0 should produce roughly 2x the leaves of density=0.5
        assert!(
            mesh_full.vertex_count() > mesh_half.vertex_count(),
            "full density ({}) should produce more vertices than half density ({})",
            mesh_full.vertex_count(),
            mesh_half.vertex_count()
        );
    }

    #[test]
    fn determinism_same_seed_same_output() {
        let (tips, dirs) = sample_tips();
        let params = LeafParams::default();

        let mut rng_a = SeededRng::new(123);
        let mesh_a = generate_leaves(&tips, &dirs, &params, &mut rng_a).unwrap();

        let mut rng_b = SeededRng::new(123);
        let mesh_b = generate_leaves(&tips, &dirs, &params, &mut rng_b).unwrap();

        assert_eq!(mesh_a.vertex_count(), mesh_b.vertex_count());
        assert_eq!(mesh_a.triangle_count(), mesh_b.triangle_count());
        for (va, vb) in mesh_a.vertices.iter().zip(mesh_b.vertices.iter()) {
            assert_eq!(va.position, vb.position);
        }
    }

    #[test]
    fn billboard_leaves_per_tip_count() {
        // With density=1.0 and a single tip, we should get exactly leaves_per_tip quads
        let tips = vec![Vec3::new(0.0, 5.0, 0.0)];
        let dirs = vec![Vec3::UP];
        let params = LeafParams {
            style: LeafStyle::BillboardCluster,
            density: 1.0,
            leaves_per_tip: 6,
            ..LeafParams::default()
        };
        let mut rng = SeededRng::new(42);
        let mesh = generate_leaves(&tips, &dirs, &params, &mut rng).unwrap();

        // 6 quads × 4 verts = 24 verts, 6 quads × 2 tris = 12 tris
        assert_eq!(mesh.vertex_count(), 24);
        assert_eq!(mesh.triangle_count(), 12);
    }
}
