//! What a [`LivingGenerator`](crate::LivingGenerator) makes: a skeleton that
//! holds a prop's identity, and poses that say how far one variant of it has
//! drifted.
//!
//! Poses are small and relative to the skeleton, so a ring of them can be
//! cached apart from any mesh and blended per segment without one.

use flint_core::quat::quat_rotate_vec3;
use flint_core::Vec3;
use serde::{Deserialize, Serialize};

use crate::types::{BranchSegment, MeshData};
use crate::{ProcGenError, Result};

/// One segment's transform in a pose, relative to the skeleton.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SegmentPose {
    /// Turns the rest segment's direction to the pose's, in the prop's space
    /// and not its parent's, as a quaternion `[x, y, z, w]`. It doesn't
    /// compound down the tree: a child's rotation already includes whatever
    /// its parent did.
    pub rotation: [f32; 4],
    /// The pose's length over the rest length.
    pub length_scale: f32,
    /// The pose's mean radius over the rest mean radius.
    pub radius_scale: f32,
}

impl SegmentPose {
    /// The rest pose.
    pub const IDENTITY: Self = Self {
        rotation: [0.0, 0.0, 0.0, 1.0],
        length_scale: 1.0,
        radius_scale: 1.0,
    };
}

/// One variant of a prop: a transform per segment, in segment-id order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LivingPose {
    /// The shape seed this pose came from.
    pub shape_seed: u64,
    /// `segments[id]` is the transform of the skeleton's segment `id`.
    pub segments: Vec<SegmentPose>,
}

/// A leaf card's place on the skeleton. It rides its segment rigidly.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LeafBinding {
    /// The id of the segment nearest the card's centre.
    pub segment: u32,
    /// How far along that segment the nearest point is, 0 (start) to 1 (end).
    pub along: f32,
    /// From that point to the card's centre, in the rest pose.
    pub offset: Vec3,
}

/// What every pose of one lineage shares: the prop's identity.
#[derive(Debug, Clone, PartialEq)]
pub struct LivingSkeleton {
    /// The seed the structure grew from.
    pub lineage_seed: u64,
    /// The rest pose's segments, each with its stable id and parent. A
    /// parent always comes before its children.
    pub segments: Vec<BranchSegment>,
    /// The leaf cards in the rest pose, whole and in order: card `k` is
    /// `card_index_count` indices starting at `k * card_index_count`.
    pub leaf_mesh: MeshData,
    /// Indices one leaf card takes in `leaf_mesh`.
    pub card_index_count: usize,
    /// `leaf_bindings[k]` is where card `k` rides.
    pub leaf_bindings: Vec<LeafBinding>,
}

impl LivingSkeleton {
    /// The vertices of leaf card `k`, each once.
    pub fn card_vertices(&self, k: usize) -> Vec<u32> {
        let first = k * self.card_index_count;
        let mut verts = self.leaf_mesh.indices[first..first + self.card_index_count].to_vec();
        verts.sort_unstable();
        verts.dedup();
        verts
    }

    /// The segments as `pose` holds them: each child starts at its parent's
    /// posed end, so the tree stays connected whatever the transforms say,
    /// and a segment off the trunk keeps its rest start.
    ///
    /// This also works on a blend of two poses, which is what keeps a blended
    /// tree whole.
    pub fn posed_segments(&self, pose: &LivingPose) -> Result<Vec<BranchSegment>> {
        self.check_pose(pose)?;
        let mut posed: Vec<BranchSegment> = Vec::with_capacity(self.segments.len());
        for (rest, p) in self.segments.iter().zip(&pose.segments) {
            let start = match rest.parent {
                Some(parent) => posed[parent as usize].end,
                None => rest.start,
            };
            let dir = Vec3::from_array(quat_rotate_vec3(&p.rotation, rest.direction().to_array()));
            posed.push(BranchSegment {
                start,
                end: start + dir * (rest.length() * p.length_scale),
                radius_start: rest.radius_start * p.radius_scale,
                radius_end: rest.radius_end * p.radius_scale,
                ..*rest
            });
        }
        Ok(posed)
    }

    /// The leaf mesh as `pose` holds it, each card carried rigidly by its
    /// segment. Vertex and card order match [`Self::leaf_mesh`].
    pub fn posed_leaf_mesh(&self, pose: &LivingPose) -> Result<MeshData> {
        let posed = self.posed_segments(pose)?;
        let mut mesh = self.leaf_mesh.clone();
        for (k, card) in self.leaf_bindings.iter().enumerate() {
            let (rest, now) = (
                &self.segments[card.segment as usize],
                &posed[card.segment as usize],
            );
            let q = &pose.segments[card.segment as usize].rotation;
            let turn = |v: Vec3| Vec3::from_array(quat_rotate_vec3(q, v.to_array()));

            let rest_centre = rest.start + (rest.end - rest.start) * card.along + card.offset;
            let centre = now.start + (now.end - now.start) * card.along + turn(card.offset);
            for v in self.card_vertices(k) {
                let vertex = &mut mesh.vertices[v as usize];
                let local = Vec3::from_array(vertex.position) - rest_centre;
                vertex.position = (centre + turn(local)).to_array();
                vertex.normal = turn(Vec3::from_array(vertex.normal)).to_array();
                let t = turn(Vec3::new(
                    vertex.tangent[0],
                    vertex.tangent[1],
                    vertex.tangent[2],
                ));
                vertex.tangent = [t.x, t.y, t.z, vertex.tangent[3]];
            }
        }
        mesh.recompute_bounds();
        Ok(mesh)
    }

    fn check_pose(&self, pose: &LivingPose) -> Result<()> {
        if pose.segments.len() == self.segments.len() {
            return Ok(());
        }
        Err(ProcGenError::InvalidParameter {
            name: "pose".into(),
            reason: format!(
                "it has {} segments and the skeleton has {}: a pose of another lineage or spec",
                pose.segments.len(),
                self.segments.len()
            ),
        })
    }
}
