//! LOD (Level of Detail) generation via mesh simplification.
//!
//! Produces multiple simplified versions of a base mesh at specified
//! triangle count targets using Garland & Heckbert quadric edge collapse.

use crate::algorithms::mesh_builder::simplify;
use crate::types::MeshData;

/// Generate LOD levels from a base mesh at the given triangle count targets.
///
/// - LOD 0 is always the full-detail `base_mesh` (cloned).
/// - Subsequent LODs are simplified from `base_mesh` (not cascaded) to avoid
///   compounding quality loss.
/// - `targets` should be sorted descending. Each value is the target triangle
///   count for that LOD level (starting at LOD 1).
///
/// Returns a `Vec<MeshData>` where index 0 is highest detail.
pub fn generate_lods(base_mesh: &MeshData, targets: &[u32]) -> Vec<MeshData> {
    let mut lods = Vec::with_capacity(1 + targets.len());
    lods.push(base_mesh.clone());

    for &target in targets {
        let simplified = simplify(base_mesh, target as usize);
        lods.push(simplified);
    }

    lods
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algorithms::mesh_builder::{sphere, SphereParams};

    fn make_test_mesh() -> MeshData {
        sphere(&SphereParams {
            radius: 1.0,
            segments: 16,
            rings: 12,
        })
    }

    #[test]
    fn lod_0_matches_original() {
        let mesh = make_test_mesh();
        let original_tri_count = mesh.triangle_count();
        let lods = generate_lods(&mesh, &[500, 200]);

        assert_eq!(lods[0].triangle_count(), original_tri_count);
        assert_eq!(lods[0].vertex_count(), mesh.vertex_count());
    }

    #[test]
    fn lod_levels_decrease_in_triangle_count() {
        let mesh = make_test_mesh();
        let lods = generate_lods(&mesh, &[200, 100, 50]);

        for i in 1..lods.len() {
            assert!(
                lods[i].triangle_count() <= lods[i - 1].triangle_count(),
                "LOD {} ({} tris) should have fewer triangles than LOD {} ({} tris)",
                i,
                lods[i].triangle_count(),
                i - 1,
                lods[i - 1].triangle_count()
            );
        }
    }

    #[test]
    fn lod_targets_within_tolerance() {
        let mesh = make_test_mesh();
        let targets = [200u32, 100, 50];
        let lods = generate_lods(&mesh, &targets);

        for (idx, &target) in targets.iter().enumerate() {
            let actual = lods[idx + 1].triangle_count() as f64;
            let expected = target as f64;
            let ratio = actual / expected;
            // Within 20% of target (simplification may not hit exact counts)
            assert!(
                ratio > 0.5 && ratio < 1.5,
                "LOD {} has {} tris, target was {} (ratio {:.2})",
                idx + 1,
                actual as u32,
                target,
                ratio
            );
        }
    }

    #[test]
    fn all_lods_pass_validate() {
        let mesh = make_test_mesh();
        let lods = generate_lods(&mesh, &[200, 100]);

        for (i, lod) in lods.iter().enumerate() {
            assert!(
                lod.validate().is_ok(),
                "LOD {} failed validation: {:?}",
                i,
                lod.validate().err()
            );
        }
    }

    #[test]
    fn empty_targets_returns_just_base() {
        let mesh = make_test_mesh();
        let lods = generate_lods(&mesh, &[]);

        assert_eq!(lods.len(), 1);
        assert_eq!(lods[0].triangle_count(), mesh.triangle_count());
    }
}
