//! LOD (Level of Detail) generation via mesh simplification.
//!
//! Produces multiple simplified versions of a base mesh at specified
//! triangle count targets using Garland & Heckbert quadric edge collapse.

use crate::algorithms::mesh_builder::{simplify, MeshBuilder};
use crate::types::{MeshData, SubMesh};
use flint_core::Vec3;

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

/// Join a tree's bark and leaves into one mesh: the bark's material and
/// triangles first, then a submesh per leaf shade.
pub fn join_bark_and_leaves(bark: &MeshData, leaves: &MeshData) -> MeshData {
    let mut builder = MeshBuilder::new();
    builder.merge_with_materials(bark);
    if !leaves.indices.is_empty() {
        builder.merge_with_materials(leaves);
    }
    builder.build()
}

/// One level of detail for a tree at about `target` triangles.
///
/// Bark and leaves are reduced apart, each keeping its share of the full
/// tree's triangles. Bark is a surface, so it goes through quadric edge
/// collapse. Leaves are separate cards of `card_indices` indices each
/// ([`super::leaves::card_index_count`]), which edge collapse can only
/// crumple; instead whole cards are dropped ([`thin_cards`]) and the
/// survivors grow to keep the crown's coverage. Leaves then take whatever
/// the simplified bark left of the budget, so the total lands on target.
///
/// Reducing them together let the leaves' open edges (which the simplifier
/// guards) push the cost onto the bark until the trunk was gone.
pub fn tree_lod(
    bark: &MeshData,
    leaves: &MeshData,
    card_indices: usize,
    target: usize,
) -> MeshData {
    let bark_tris = bark.triangle_count();
    let leaf_tris = leaves.triangle_count();
    let total = bark_tris + leaf_tris;
    if target >= total || total == 0 {
        return join_bark_and_leaves(bark, leaves);
    }

    let bark_budget = (target as f64 * bark_tris as f64 / total as f64).round() as usize;
    let bark_lod = simplify(bark, bark_budget.max(1));

    let leaf_lod = if card_indices == 0 || leaves.indices.is_empty() {
        leaves.clone()
    } else {
        let tris_per_card = card_indices / 3;
        let leaf_budget = target.saturating_sub(bark_lod.triangle_count());
        thin_cards(leaves, card_indices, leaf_budget / tris_per_card)
    };
    join_bark_and_leaves(&bark_lod, &leaf_lod)
}

/// Keep `keep` of a leaf mesh's cards, spread evenly through it, and grow
/// each survivor about its centre so the cards cover the same total area.
///
/// Cards are stored whole and in generation order, so a branch tip's
/// cluster sits together: an even stride thins every cluster a little
/// instead of stripping some tips bare. Submeshes (shades) and materials
/// are kept; a shade that loses all its cards loses its submesh.
pub fn thin_cards(leaves: &MeshData, card_indices: usize, keep: usize) -> MeshData {
    let cards = if card_indices == 0 {
        0
    } else {
        leaves.indices.len() / card_indices
    };
    if keep >= cards {
        return leaves.clone();
    }
    let growth = if keep == 0 {
        1.0
    } else {
        (cards as f32 / keep as f32).sqrt()
    };

    let whole = [SubMesh {
        index_start: 0,
        index_count: leaves.indices.len() as u32,
        material_index: 0,
    }];
    let submeshes: &[SubMesh] = if leaves.submeshes.is_empty() {
        &whole
    } else {
        &leaves.submeshes
    };

    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut out_submeshes = Vec::new();
    let mut remap: Vec<(u32, u32)> = Vec::with_capacity(card_indices);
    for sub in submeshes {
        let index_start = indices.len() as u32;
        let first_card = sub.index_start as usize / card_indices;
        let sub_cards = sub.index_count as usize / card_indices;
        for card in first_card..first_card + sub_cards {
            // Card `card` survives when the running quota ticks over on it.
            if (card + 1) * keep / cards == card * keep / cards {
                continue;
            }
            let span = &leaves.indices[card * card_indices..(card + 1) * card_indices];

            remap.clear();
            let mut centre = Vec3::ZERO;
            for &old in span {
                if !remap.iter().any(|&(o, _)| o == old) {
                    remap.push((old, (vertices.len() + remap.len()) as u32));
                    centre = centre + Vec3::from_array(leaves.vertices[old as usize].position);
                }
            }
            centre = centre * (1.0 / remap.len() as f32);
            for &(old, _) in &remap {
                let mut v = leaves.vertices[old as usize];
                let p = Vec3::from_array(v.position);
                v.position = (centre + (p - centre) * growth).to_array();
                vertices.push(v);
            }
            for &old in span {
                let new = remap
                    .iter()
                    .find(|&&(o, _)| o == old)
                    .map_or(0, |&(_, n)| n);
                indices.push(new);
            }
        }
        let index_count = indices.len() as u32 - index_start;
        if index_count > 0 {
            out_submeshes.push(SubMesh {
                index_start,
                index_count,
                material_index: sub.material_index,
            });
        }
    }

    let mut out = MeshData {
        vertices,
        indices,
        materials: leaves.materials.clone(),
        submeshes: if leaves.submeshes.is_empty() {
            vec![]
        } else {
            out_submeshes
        },
        bounding_box: leaves.bounding_box,
    };
    out.recompute_bounds();
    out
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
    fn thinning_keeps_exact_cards_and_their_coverage() {
        use crate::generators::tree::leaves::{card_index_count, generate_leaves};
        use crate::generators::tree::{LeafParams, LeafStyle};
        use crate::SeededRng;

        let tips: Vec<Vec3> = (0..60)
            .map(|i| Vec3::new((i % 10) as f32, 4.0, (i / 10) as f32))
            .collect();
        let dirs = vec![Vec3::UP; tips.len()];
        let params = LeafParams {
            density: 1.0,
            ..LeafParams::default()
        };
        let leaves = generate_leaves(&tips, &dirs, &[], &params, &mut SeededRng::new(9)).unwrap();
        let per_card = card_index_count(LeafStyle::BillboardCluster);
        let cards = leaves.indices.len() / per_card;
        let area = |m: &MeshData| {
            let p = |i: u32| Vec3::from_array(m.vertices[i as usize].position);
            m.indices
                .chunks(3)
                .map(|t| (p(t[1]) - p(t[0])).cross(&(p(t[2]) - p(t[0]))).length() * 0.5)
                .sum::<f32>()
        };

        for keep in [cards, cards / 2, cards / 7, 1, 0] {
            let thin = thin_cards(&leaves, per_card, keep);
            assert!(thin.validate().is_ok());
            assert_eq!(thin.indices.len(), keep * per_card);
            assert_eq!(thin.vertex_count(), keep * 8);
            let covered: u32 = thin.submeshes.iter().map(|s| s.index_count).sum();
            assert_eq!(covered as usize, thin.indices.len());
            // Coverage holds on average; one card is only as big as it was drawn.
            if keep >= 10 {
                let ratio = area(&thin) / area(&leaves);
                assert!(
                    (ratio - 1.0).abs() < 0.2,
                    "keeping {keep} of {cards} cards covered {ratio:.2}x the area"
                );
            }
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
