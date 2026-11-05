//! What a [`LivingGenerator`](crate::LivingGenerator) makes: a skeleton that
//! holds a prop's identity, and poses that say how far one variant of it has
//! drifted.
//!
//! Poses are small and relative to the skeleton, so a ring of them can be
//! cached apart from any mesh and blended per segment without one.

use flint_core::quat::{quat_nlerp, quat_rotate_vec3};
use flint_core::Vec3;
use serde::{Deserialize, Serialize};

use crate::skinning::SkinnedMeshData;
use crate::spec::VarianceConfig;
use crate::types::{BoundingBox, BranchSegment, MeshData};
use crate::{ProcGenError, Result};

/// How many times [`LivingSkeleton::with_variance`] corrects its size. A
/// fixed count, so the result doesn't depend on a tolerance. Limbs stay on the
/// trunk, so the box isn't quite proportional to the scale and one round isn't
/// enough, but each round leaves only a fraction of the error.
const KEEP_SIZE_ROUNDS: usize = 4;

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

    /// This transform with only `weight` of it left: 0 is the rest pose and 1
    /// is exactly this one, bit for bit.
    pub fn pulled(&self, weight: f32) -> Self {
        if weight >= 1.0 {
            return *self;
        }
        let scale = |s: f32| 1.0 + (s - 1.0) * weight;
        Self {
            rotation: quat_nlerp(&Self::IDENTITY.rotation, &self.rotation, weight),
            length_scale: scale(self.length_scale),
            radius_scale: scale(self.radius_scale),
        }
    }
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
        for k in 0..self.leaf_bindings.len() {
            let carry = self.card_carry(k, pose, &posed);
            for v in self.card_vertices(k) {
                let vertex = &mut mesh.vertices[v as usize];
                vertex.position = carry.place(Vec3::from_array(vertex.position)).to_array();
                vertex.normal = carry.turn(Vec3::from_array(vertex.normal)).to_array();
                let t = carry.turn(Vec3::new(
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

    /// The box around everything `pose` moves: the branches and the leaf
    /// cards. The trunk isn't in it, since a pose can't move the trunk.
    pub fn posed_bounds(&self, pose: &LivingPose) -> Result<BoundingBox> {
        let posed = self.posed_segments(pose)?;
        let mut points: Vec<[f32; 3]> =
            Vec::with_capacity(posed.len() * 2 + self.leaf_bindings.len() * self.card_index_count);
        for s in &posed {
            points.push(s.start.to_array());
            points.push(s.end.to_array());
        }
        for k in 0..self.leaf_bindings.len() {
            let carry = self.card_carry(k, pose, &posed);
            for v in self.card_vertices(k) {
                let at = Vec3::from_array(self.leaf_mesh.vertices[v as usize].position);
                points.push(carry.place(at).to_array());
            }
        }
        Ok(BoundingBox::from_positions(&points))
    }

    /// [`Self::posed_bounds`] for the rest pose, the size every pose is held to.
    pub fn rest_bounds(&self) -> BoundingBox {
        let rest = LivingPose {
            shape_seed: 0,
            segments: vec![SegmentPose::IDENTITY; self.segments.len()],
        };
        self.posed_bounds(&rest)
            .expect("the rest pose has one transform per segment")
    }

    /// The deepest level of the tree, which [`VarianceConfig::weight`] measures
    /// depth against.
    pub fn max_depth(&self) -> u32 {
        self.segments.iter().map(|s| s.depth).max().unwrap_or(0)
    }

    /// `full` held to `variance`: each segment pulled back toward the rest
    /// pose, the lineage's base, so only its weight of the difference is left.
    /// With `keep_size`, every segment's length then takes one shared scale
    /// that centres the box's sides on the rest pose's.
    ///
    /// One scale keeps the tree's proportions and leaves its limbs on the
    /// trunk, and that is why the box can't match on every side at once. Takes
    /// the pose from [`full_pose`](crate::LivingGenerator::full_pose).
    pub fn with_variance(
        &self,
        full: &LivingPose,
        variance: &VarianceConfig,
    ) -> Result<LivingPose> {
        self.check_pose(full)?;
        let max_depth = self.max_depth();
        let mut pose = LivingPose {
            shape_seed: full.shape_seed,
            segments: self
                .segments
                .iter()
                .zip(&full.segments)
                .map(|(rest, p)| p.pulled(variance.weight(rest.depth, max_depth)))
                .collect(),
        };
        if variance.keep_size {
            self.hold_size(&mut pose)?;
        }
        Ok(pose)
    }

    /// Scale `pose`'s lengths until its box's most stretched side is over the
    /// rest pose's by the same fraction its most squeezed side is under.
    fn hold_size(&self, pose: &mut LivingPose) -> Result<()> {
        let rest = self.rest_bounds();
        for _ in 0..KEEP_SIZE_ROUNDS {
            let scale = balancing_scale(&rest, &self.posed_bounds(pose)?);
            for s in &mut pose.segments {
                s.length_scale *= scale;
            }
        }
        Ok(())
    }

    /// How `pose` carries leaf card `k`, given the segments it poses.
    fn card_carry(&self, k: usize, pose: &LivingPose, posed: &[BranchSegment]) -> CardCarry {
        let card = &self.leaf_bindings[k];
        let (rest, now) = (
            &self.segments[card.segment as usize],
            &posed[card.segment as usize],
        );
        let rotation = pose.segments[card.segment as usize].rotation;
        let turned = Vec3::from_array(quat_rotate_vec3(&rotation, card.offset.to_array()));
        CardCarry {
            rotation,
            rest_centre: rest.start + (rest.end - rest.start) * card.along + card.offset,
            centre: now.start + (now.end - now.start) * card.along + turned,
        }
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

/// The rigid move a pose gives one leaf card.
struct CardCarry {
    rotation: [f32; 4],
    rest_centre: Vec3,
    centre: Vec3,
}

impl CardCarry {
    fn turn(&self, v: Vec3) -> Vec3 {
        Vec3::from_array(quat_rotate_vec3(&self.rotation, v.to_array()))
    }

    /// Where a point of the card in the rest pose ends up.
    fn place(&self, rest: Vec3) -> Vec3 {
        self.centre + self.turn(rest - self.rest_centre)
    }
}

/// The scale that centres `now`'s sides on `rest`'s: dividing by the midpoint
/// of the sides' ratios puts the largest and smallest equally far from 1. A
/// side with no length in the rest pose has no ratio and doesn't count.
fn balancing_scale(rest: &BoundingBox, now: &BoundingBox) -> f32 {
    let (rest, now) = (
        (rest.max - rest.min).to_array(),
        (now.max - now.min).to_array(),
    );
    let (mut least, mut most) = (f32::INFINITY, 0.0f32);
    for (now, rest) in now.iter().zip(&rest) {
        if *rest > 1e-6 && *now > 1e-6 {
            let ratio = now / rest;
            least = least.min(ratio);
            most = most.max(ratio);
        }
    }
    if most > 0.0 {
        2.0 / (least + most)
    } else {
        1.0
    }
}

/// One lineage's skinned rest mesh and how its poses reach the bones.
///
/// Bone 0 is the trunk, which every pose leaves still. After it comes a bone
/// per segment, parents first, unless the tree has more segments than
/// [`MAX_SKINNED_BONES`](crate::MAX_SKINNED_BONES) leaves room for: then its
/// smallest twigs ride the bone of their nearest ancestor.
#[derive(Debug, Clone, PartialEq)]
pub struct LivingRig {
    /// The rest mesh, in the same place as the tree `generate` would bake.
    pub mesh: SkinnedMeshData,
    /// `segment_bones[id]` is the bone that moves segment `id`.
    pub segment_bones: Vec<u16>,
    /// `bone_segments[bone]` is the segment a bone follows; `None` for the trunk.
    pub bone_segments: Vec<Option<u32>>,
}

impl LivingRig {
    /// `pose` as bone matrices (column-major, what the skinned pipeline
    /// uploads), written over `out`. Each takes a rest vertex to where the
    /// pose puts it, and all of a pose's scale is in here: the skeleton's
    /// local transforms can't hold it, since a parent's stretch would shear
    /// its children.
    ///
    /// A segment's bone turns it by the pose's rotation, stretches it along
    /// itself by the length scale and across by the radius scale, and puts
    /// its head on the posed end of its parent. So a tube stays joined under
    /// any pose, and blending two poses first keeps that true.
    pub fn palette(
        &self,
        skeleton: &LivingSkeleton,
        pose: &LivingPose,
        out: &mut Vec<[[f32; 4]; 4]>,
    ) -> Result<()> {
        let posed = skeleton.posed_segments(pose)?;
        out.clear();
        out.extend(self.bone_segments.iter().map(|segment| match segment {
            None => IDENTITY_BONE,
            Some(id) => {
                let id = *id as usize;
                let (rest, p) = (&skeleton.segments[id], &pose.segments[id]);
                segment_bone(rest.start, rest.direction(), p, posed[id].start)
            }
        }));
        Ok(())
    }

    /// The rest mesh moved by `palette` on the CPU, the way the GPU does it:
    /// a mesh to look at or measure, not a way to draw a pose every frame.
    pub fn skinned_mesh(&self, palette: &[[[f32; 4]; 4]]) -> MeshData {
        let mut mesh = self.mesh.rest_mesh();
        for (out, v) in mesh.vertices.iter_mut().zip(&self.mesh.vertices) {
            let mut blend = [[0.0f32; 4]; 4];
            for (joint, weight) in v.joint_indices.iter().zip(&v.joint_weights) {
                if *weight > 0.0 {
                    for (column, bone) in blend.iter_mut().zip(&palette[*joint as usize]) {
                        for (c, b) in column.iter_mut().zip(bone) {
                            *c += b * weight;
                        }
                    }
                }
            }
            let apply = |p: [f32; 3], w: f32| -> [f32; 3] {
                std::array::from_fn(|r| {
                    blend[0][r] * p[0] + blend[1][r] * p[1] + blend[2][r] * p[2] + blend[3][r] * w
                })
            };
            out.position = apply(v.position, 1.0);
            out.normal = Vec3::from_array(apply(v.normal, 0.0))
                .normalized()
                .to_array();
            let t = Vec3::from_array(apply([v.tangent[0], v.tangent[1], v.tangent[2]], 0.0))
                .normalized();
            out.tangent = [t.x, t.y, t.z, v.tangent[3]];
        }
        mesh.recompute_bounds();
        mesh
    }
}

const IDENTITY_BONE: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// The bone matrix for a segment leaving `head` along unit `dir` at rest, and
/// leaving `posed_head` under `p`: turn * stretch about the head, where a
/// stretch of `l` along `dir` and `r` across it is `r I + (l - r) dir dir^T`.
fn segment_bone(head: Vec3, dir: Vec3, p: &SegmentPose, posed_head: Vec3) -> [[f32; 4]; 4] {
    let (l, r) = (p.length_scale, p.radius_scale);
    let d = dir.to_array();
    let mut linear = [[0.0f32; 3]; 3];
    for (c, column) in linear.iter_mut().enumerate() {
        let mut stretched = [0.0f32; 3];
        for (row, s) in stretched.iter_mut().enumerate() {
            *s = (l - r) * d[row] * d[c] + if row == c { r } else { 0.0 };
        }
        *column = quat_rotate_vec3(&p.rotation, stretched);
    }
    let h = head.to_array();
    let moved: [f32; 3] =
        std::array::from_fn(|r| linear[0][r] * h[0] + linear[1][r] * h[1] + linear[2][r] * h[2]);
    let translation = posed_head.to_array();
    [
        [linear[0][0], linear[0][1], linear[0][2], 0.0],
        [linear[1][0], linear[1][1], linear[1][2], 0.0],
        [linear[2][0], linear[2][1], linear[2][2], 0.0],
        [
            translation[0] - moved[0],
            translation[1] - moved[1],
            translation[2] - moved[2],
            1.0,
        ],
    ]
}
