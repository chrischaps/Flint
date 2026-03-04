//! Generates tapered cylinder geometry for limb chain segments.
//!
//! Each segment of a limb chain gets a tapered cylinder, positioned along the
//! bone chain and rigidly skinned to its corresponding bone.

use crate::algorithms::mesh_builder::primitives::{tapered_cylinder, TaperedCylinderParams};
use crate::generators::creature::skeleton_builder::LimbChainBones;
use crate::skinning::{SkeletonDef, SkinnedVertex};

use super::body_parts::{transform_normal, transform_point};

/// Generate geometry for all segments of a limb chain.
///
/// Returns skinned vertices and indices for the entire chain.
pub fn generate_limb_chain(
    chain: &LimbChainBones,
    _skeleton: &SkeletonDef,
    world_transforms: &[[f32; 16]],
) -> (Vec<SkinnedVertex>, Vec<u32>) {
    let mut all_verts = Vec::new();
    let mut all_indices = Vec::new();

    for (seg_i, seg) in chain.segments.iter().enumerate() {
        let bone_idx = chain.bone_indices[seg_i];
        let world = &world_transforms[bone_idx];

        let mesh = tapered_cylinder(&TaperedCylinderParams {
            radius_bottom: seg.radius_start,
            radius_top: seg.radius_end,
            height: seg.length,
            radial_segments: 8,
            height_segments: 2,
            caps: true,
        });

        let base_vertex = all_verts.len() as u32;

        for v in &mesh.vertices {
            // Flip from +Y to -Y convention: negate Y position and Y normal
            let flipped_pos = [v.position[0], -v.position[1], v.position[2]];
            let flipped_norm = [v.normal[0], -v.normal[1], v.normal[2]];
            let world_pos = transform_point(world, &flipped_pos);
            let world_norm = transform_normal(world, &flipped_norm);

            all_verts.push(SkinnedVertex {
                position: world_pos,
                normal: world_norm,
                tangent: v.tangent,
                uv: v.uv,
                joint_indices: [bone_idx as u16, 0, 0, 0],
                joint_weights: [1.0, 0.0, 0.0, 0.0],
            });
        }

        // Reverse triangle winding to maintain correct face orientation after Y-flip
        for tri in mesh.indices.chunks(3) {
            all_indices.push(tri[0] + base_vertex);
            all_indices.push(tri[2] + base_vertex);
            all_indices.push(tri[1] + base_vertex);
        }
    }

    (all_verts, all_indices)
}

// Re-export transform helpers from body_parts (they're pub(crate) there)
// Actually we import them from body_parts directly.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators::creature::skeleton_builder::LimbSegmentDef;
    use crate::skinning::BoneDef;

    #[test]
    fn generate_simple_limb() {
        let skeleton = SkeletonDef {
            bones: vec![
                BoneDef {
                    name: "root".into(),
                    parent: None,
                    rest_translation: [0.0, 0.0, 0.0],
                    rest_rotation: [0.0, 0.0, 0.0, 1.0],
                    rest_scale: [1.0, 1.0, 1.0],
                },
                BoneDef {
                    name: "seg0".into(),
                    parent: Some(0),
                    rest_translation: [0.5, 0.0, 0.0],
                    rest_rotation: [0.0, 0.0, 0.0, 1.0],
                    rest_scale: [1.0, 1.0, 1.0],
                },
                BoneDef {
                    name: "seg1".into(),
                    parent: Some(1),
                    rest_translation: [0.0, -1.0, 0.0],
                    rest_rotation: [0.0, 0.0, 0.0, 1.0],
                    rest_scale: [1.0, 1.0, 1.0],
                },
            ],
        };
        let world_transforms = skeleton.compute_world_transforms();

        let chain = LimbChainBones {
            chain_name: "leg_R".into(),
            bone_indices: vec![1, 2],
            segments: vec![
                LimbSegmentDef {
                    length: 1.0,
                    radius_start: 0.1,
                    radius_end: 0.08,
                    rest_angles: [0.0, 0.0, 0.0],
                },
                LimbSegmentDef {
                    length: 0.8,
                    radius_start: 0.08,
                    radius_end: 0.05,
                    rest_angles: [0.0, 0.0, 0.0],
                },
            ],
            material: "default".into(),
        };

        let (verts, indices) = generate_limb_chain(&chain, &skeleton, &world_transforms);
        assert!(!verts.is_empty());
        assert!(!indices.is_empty());
        assert!(indices.len() % 3 == 0);

        // First segment vertices should be skinned to bone 1
        // (all verts from the first tapered cylinder)
        assert_eq!(verts[0].joint_indices[0], 1);
    }
}
