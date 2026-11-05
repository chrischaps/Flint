//! The tree's living side: one structure per lineage, a stream of poses.
//!
//! The structure is the tree `generate` bakes from the lineage seed. A pose
//! regrows only the branches, from its own fork of the lineage, against the
//! lineage's trunk, so limbs stay attached and the trunk holds still. Leaves
//! are placed once, from the lineage, because their count changes per seed.
//!
//! This works because the built-in L-system rule rewrites every `F` the same
//! way and draws only angles from the RNG: every seed grows the same segments
//! in the same order (#207). `generate` itself is untouched.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use flint_core::quat::{quat_conjugate, quat_mul, quat_normalize, quat_rotate_vec3};
use flint_core::Vec3;

use crate::algorithms::mesh_builder::MeshBuilder;
use crate::generator::LivingGenerator;
use crate::living::BOIL_BONES;
use crate::living::{LeafBinding, LivingPose, LivingRig, LivingSkeleton, SegmentPose};
use crate::skinning::{BoneDef, SkeletonDef, SkinnedMeshData, SkinnedVertex, MAX_SKINNED_BONES};
use crate::spec::ProcGenSpec;
use crate::types::{BranchSegment, MeshData};
use crate::{ProcGenError, Result, SeededRng};

use super::branches::{extract_tips, grow_branch_segments, segments_to_mesh, tube_influences};
use super::leaves::{card_index_count, generate_leaves};
use super::trunk::trunk_spine;
use super::{
    generate_tree, generate_trunk, join_bark_and_leaves, BranchMethod, BranchParams, LeafParams,
    TreeGenerator, TrunkParams, TrunkSpine,
};

impl LivingGenerator for TreeGenerator {
    fn check_living(&self, spec: &ProcGenSpec) -> Result<()> {
        let branch = BranchParams::from_toml(&spec.params)?;
        let reason = match branch.method {
            BranchMethod::SpaceColonization => {
                "space colonization grows a different structure for every seed, and pinning \
                 its attraction cloud would freeze the shape too"
            }
            BranchMethod::LSystem if branch.lsystem_grammar.is_some() => {
                "a text grammar can't drift: a stochastic one grows a different structure for \
                 every seed, and a fixed one grows the same tree for every seed"
            }
            BranchMethod::LSystem => return Ok(()),
        };
        Err(ProcGenError::LivingUnsupported(format!(
            "{reason}; it gets shallow wooze only"
        )))
    }

    fn structure(&self, spec: &ProcGenSpec, lineage_seed: u64) -> Result<LivingSkeleton> {
        self.check_living(spec)?;
        let branch = BranchParams::from_toml(&spec.params)?;
        let leaf = LeafParams::from_toml(&spec.params)?;
        let spine = lineage_trunk(spec, lineage_seed)?;

        // The same forks `generate` takes, so this is the tree it bakes.
        let mut lineage = SeededRng::new(lineage_seed);
        let segments = grow_branch_segments(&branch, &spine, &mut lineage.fork("branches"))?;

        let (tips, tip_directions) = extract_tips(&segments);
        let leaf_mesh = generate_leaves(
            &tips,
            &tip_directions,
            &segments,
            &leaf,
            &mut lineage.fork("leaves"),
        )?;
        let card_indices = card_index_count(leaf.style);
        let leaf_bindings = bind_cards(&leaf_mesh, card_indices, &segments);

        Ok(LivingSkeleton {
            lineage_seed,
            segments,
            leaf_mesh,
            card_index_count: card_indices,
            leaf_bindings,
        })
    }

    fn rig(&self, spec: &ProcGenSpec, lineage_seed: u64) -> Result<LivingRig> {
        let skeleton = self.structure(spec, lineage_seed)?;
        let branch = BranchParams::from_toml(&spec.params)?;
        let trunk = TrunkParams::from_toml(&spec.params)?;
        let tree = generate_tree(&trunk, &branch, &mut SeededRng::new(lineage_seed))?;

        // The buffer's last bones are the jitter layer's, at run time.
        let (segment_bones, bone_segments) =
            assign_bones(&skeleton.segments, MAX_SKINNED_BONES - 1 - BOIL_BONES);
        let joints = joint_table(
            &skeleton,
            tree.trunk_vertices,
            &tube_influences(&skeleton.segments, branch.radial_segments),
            &segment_bones,
        );

        let rest = join_bark_and_leaves(&tree.mesh, &skeleton.leaf_mesh);
        if joints.len() != rest.vertices.len() {
            return Err(ProcGenError::InvalidMesh(format!(
                "skinned {} vertices of a tree with {}",
                joints.len(),
                rest.vertices.len()
            )));
        }
        let mesh = SkinnedMeshData {
            vertices: rest
                .vertices
                .iter()
                .zip(&joints)
                .map(|(v, (joint_indices, joint_weights))| SkinnedVertex {
                    position: v.position,
                    normal: v.normal,
                    tangent: v.tangent,
                    uv: v.uv,
                    joint_indices: *joint_indices,
                    joint_weights: *joint_weights,
                })
                .collect(),
            indices: rest.indices,
            materials: rest.materials,
            submeshes: rest.submeshes,
            bounding_box: rest.bounding_box,
            skeleton: skeleton_def(&skeleton.segments, &bone_segments, &segment_bones),
        };
        mesh.validate()?;
        let first_leaf_vertex = mesh.vertices.len() - skeleton.leaf_mesh.vertices.len();
        let leaf_parts = mesh
            .submeshes
            .iter()
            .filter(|sub| sub.index_count > 0)
            .enumerate()
            .filter(|(_, sub)| mesh.indices[sub.index_start as usize] as usize >= first_leaf_vertex)
            .map(|(part, _)| part)
            .collect();
        Ok(LivingRig {
            card_boil: skeleton.card_boil(bone_segments.len() as u16),
            mesh,
            segment_bones,
            bone_segments,
            first_leaf_vertex,
            leaf_parts,
        })
    }

    fn full_pose(
        &self,
        spec: &ProcGenSpec,
        skeleton: &LivingSkeleton,
        shape_seed: u64,
    ) -> Result<LivingPose> {
        self.check_living(spec)?;
        let regrown = regrow(spec, skeleton.lineage_seed, shape_seed)?;
        let same_structure = regrown.len() == skeleton.segments.len()
            && regrown
                .iter()
                .zip(&skeleton.segments)
                .all(|(a, b)| a.depth == b.depth);
        if !same_structure {
            return Err(ProcGenError::LivingUnsupported(format!(
                "shape seed {shape_seed} regrew {} segments against the skeleton's {}, so the \
                 two can't blend; was this skeleton made from another spec?",
                regrown.len(),
                skeleton.segments.len()
            )));
        }
        Ok(LivingPose {
            shape_seed,
            segments: skeleton
                .segments
                .iter()
                .zip(&regrown)
                .map(|(rest, now)| segment_pose(rest, now))
                .collect(),
        })
    }

    fn posed_mesh(
        &self,
        spec: &ProcGenSpec,
        skeleton: &LivingSkeleton,
        pose: &LivingPose,
    ) -> Result<MeshData> {
        // The trunk is the lineage's, the same fork `generate` takes for it.
        let trunk = generate_trunk(
            &TrunkParams::from_toml(&spec.params)?,
            &mut SeededRng::new(skeleton.lineage_seed).fork("trunk"),
        )?;
        let radial = BranchParams::from_toml(&spec.params)?.radial_segments;

        let mut bark = MeshBuilder::new();
        bark.merge(&trunk.mesh);
        bark.merge(&segments_to_mesh(&skeleton.posed_segments(pose)?, radial)?);
        bark.compute_normals();
        bark.compute_tangents();
        let mut bark = bark.build();
        bark.materials = trunk.mesh.materials.clone();
        Ok(join_bark_and_leaves(
            &bark,
            &skeleton.posed_leaf_mesh(pose)?,
        ))
    }
}

/// The trunk every pose of a lineage grows from.
fn lineage_trunk(spec: &ProcGenSpec, lineage_seed: u64) -> Result<TrunkSpine> {
    let trunk = TrunkParams::from_toml(&spec.params)?;
    let mut lineage = SeededRng::new(lineage_seed);
    Ok(TrunkSpine {
        points: trunk_spine(&trunk, &mut lineage.fork("trunk")),
        radius_base: trunk.radius_base,
        radius_top: trunk.radius_top,
    })
}

/// The branches as `shape_seed` grows them, on the lineage's trunk.
fn regrow(spec: &ProcGenSpec, lineage_seed: u64, shape_seed: u64) -> Result<Vec<BranchSegment>> {
    let branch = BranchParams::from_toml(&spec.params)?;
    let spine = lineage_trunk(spec, lineage_seed)?;
    let mut fork = SeededRng::new(lineage_seed).fork(&format!("pose{shape_seed}"));
    grow_branch_segments(&branch, &spine, &mut fork)
}

/// `now` relative to `rest`. A degenerate segment has nothing to turn or
/// scale, so it stays as it is.
fn segment_pose(rest: &BranchSegment, now: &BranchSegment) -> SegmentPose {
    let ratio = |now: f32, rest: f32| if rest > 1e-6 { now / rest } else { 1.0 };
    SegmentPose {
        rotation: rotation_between(rest.direction(), now.direction()),
        length_scale: ratio(now.length(), rest.length()),
        radius_scale: ratio(now.mean_radius(), rest.mean_radius()),
    }
}

/// The shortest rotation from unit vector `a` to unit vector `b`, as a
/// quaternion `[x, y, z, w]`. Identity if either is zero.
fn rotation_between(a: Vec3, b: Vec3) -> [f32; 4] {
    if a.length() < 0.5 || b.length() < 0.5 {
        return SegmentPose::IDENTITY.rotation;
    }
    let c = a.dot(&b);
    let axis = a.cross(&b);
    if c < -0.9999 {
        // Opposite: half a turn about anything perpendicular to `a`.
        let other = if a.x.abs() < 0.9 {
            Vec3::RIGHT
        } else {
            Vec3::UP
        };
        let p = a.cross(&other).normalized();
        return [p.x, p.y, p.z, 0.0];
    }
    flint_core::quat::quat_normalize(&[axis.x, axis.y, axis.z, 1.0 + c])
}

/// Which bone each segment rides, and which segment each bone follows, with
/// at most `budget` bones for segments and bone 0 left for the trunk.
///
/// While there are too many, the shortest segment with nothing left hanging
/// from it folds into its parent: the tree loses the bones that show least,
/// from the tips in, and a bone's parent always survives it.
fn assign_bones(segments: &[BranchSegment], budget: usize) -> (Vec<u16>, Vec<Option<u32>>) {
    let mut folded = vec![false; segments.len()];
    let excess = segments.len().saturating_sub(budget);
    if excess > 0 {
        let mut children = vec![0usize; segments.len()];
        for parent in segments.iter().filter_map(|s| s.parent) {
            children[parent as usize] += 1;
        }
        // Lengths are never negative, so their bits order as they do.
        let terminal = |s: &BranchSegment| Reverse((s.length().to_bits(), s.id));
        let mut ready: BinaryHeap<_> = segments
            .iter()
            .filter(|s| children[s.id as usize] == 0)
            .map(terminal)
            .collect();
        for _ in 0..excess {
            let Some(Reverse((_, id))) = ready.pop() else {
                break;
            };
            folded[id as usize] = true;
            if let Some(parent) = segments[id as usize].parent {
                children[parent as usize] -= 1;
                if children[parent as usize] == 0 {
                    ready.push(terminal(&segments[parent as usize]));
                }
            }
        }
    }

    let mut segment_bones = Vec::with_capacity(segments.len());
    let mut bone_segments = vec![None];
    for s in segments {
        let bone = if folded[s.id as usize] {
            s.parent.map_or(0, |parent| segment_bones[parent as usize])
        } else {
            bone_segments.push(Some(s.id));
            (bone_segments.len() - 1) as u16
        };
        segment_bones.push(bone);
    }
    (segment_bones, bone_segments)
}

/// The rest skeleton: the trunk at the origin, then each bone at its
/// segment's start with +Y along the segment, relative to its parent's bone.
/// Its transforms hold no scale, so turning a bone turns everything past it.
fn skeleton_def(
    segments: &[BranchSegment],
    bone_segments: &[Option<u32>],
    segment_bones: &[u16],
) -> SkeletonDef {
    let frame = |s: &BranchSegment| rotation_between(Vec3::UP, s.direction());
    let bones = bone_segments
        .iter()
        .map(|bone| {
            let Some(id) = bone else {
                return BoneDef {
                    name: "trunk".into(),
                    ..BoneDef::default()
                };
            };
            let s = &segments[*id as usize];
            let (parent_bone, parent_head, parent_frame) = match s.parent {
                Some(p) => {
                    let parent = &segments[p as usize];
                    (segment_bones[p as usize], parent.start, frame(parent))
                }
                None => (0, Vec3::ZERO, SegmentPose::IDENTITY.rotation),
            };
            let to_parent = quat_conjugate(&parent_frame);
            BoneDef {
                name: format!("seg_{id:04}"),
                parent: Some(parent_bone as usize),
                rest_translation: quat_rotate_vec3(&to_parent, (s.start - parent_head).to_array()),
                rest_rotation: quat_normalize(&quat_mul(&to_parent, &frame(s))),
                rest_scale: [1.0, 1.0, 1.0],
            }
        })
        .collect();
    SkeletonDef { bones }
}

/// Each vertex's joints and weights, in the order of the tree's mesh: the
/// trunk on bone 0, the branch tubes as `tubes` says, then each leaf card
/// wholly on the bone of the segment it's bound to. A vertex whose two
/// segments ride one bone just has that bone.
fn joint_table(
    skeleton: &LivingSkeleton,
    trunk_vertices: usize,
    tubes: &[(u32, Option<u32>)],
    segment_bones: &[u16],
) -> Vec<([u16; 4], [f32; 4])> {
    const TRUNK: ([u16; 4], [f32; 4]) = ([0, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]);
    let rigid = |bone: u16| ([bone, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]);

    let mut joints = vec![TRUNK; trunk_vertices];
    joints.extend(tubes.iter().map(|&(a, b)| {
        let a = segment_bones[a as usize];
        match b.map(|b| segment_bones[b as usize]) {
            Some(b) if b != a => ([a, b, 0, 0], [0.5, 0.5, 0.0, 0.0]),
            _ => rigid(a),
        }
    }));

    let first_leaf = joints.len();
    joints.resize(first_leaf + skeleton.leaf_mesh.vertices.len(), TRUNK);
    for (k, card) in skeleton.leaf_bindings.iter().enumerate() {
        let bone = rigid(segment_bones[card.segment as usize]);
        for v in skeleton.card_vertices(k) {
            joints[first_leaf + v as usize] = bone;
        }
    }
    joints
}

/// Bind each leaf card to the segment nearest its centre.
fn bind_cards(
    leaves: &MeshData,
    card_indices: usize,
    segments: &[BranchSegment],
) -> Vec<LeafBinding> {
    if card_indices == 0 || segments.is_empty() {
        return Vec::new();
    }
    leaves
        .indices
        .chunks(card_indices)
        .map(|card| {
            let mut verts = card.to_vec();
            verts.sort_unstable();
            verts.dedup();
            let centre = verts
                .iter()
                .map(|&v| Vec3::from_array(leaves.vertices[v as usize].position))
                .fold(Vec3::ZERO, |sum, p| sum + p)
                * (1.0 / verts.len() as f32);
            let (segment, along, at) = segments
                .iter()
                .map(|s| {
                    let ab = s.end - s.start;
                    let along =
                        ((centre - s.start).dot(&ab) / ab.dot(&ab).max(1e-9)).clamp(0.0, 1.0);
                    (s.id, along, s.start + ab * along)
                })
                .min_by(|a, b| (centre - a.2).length().total_cmp(&(centre - b.2).length()))
                .expect("segments is not empty");
            LeafBinding {
                segment,
                along,
                offset: centre - at,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators::tree::{generate_tree, join_bark_and_leaves};
    use crate::{Generator, GeneratorOutput, LivingConfig, VarianceConfig};
    use std::path::PathBuf;

    const SHAPE_SEEDS: u64 = 100;
    const LINEAGE: u64 = 7;

    fn load(relative: &str) -> ProcGenSpec {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(relative);
        let mut spec = ProcGenSpec::from_file(&path).unwrap();
        spec.lod = None;
        spec
    }

    /// The built-in L-system specs in the repo.
    fn lsystem_specs() -> Vec<ProcGenSpec> {
        vec![
            load("specs/oak_tree.procgen.toml"),
            load("specs/oak_tree.procgen2.toml"),
            load("specs/examples/oak_tree.procgen.toml"),
        ]
    }

    fn links(segments: &[BranchSegment]) -> Vec<(u32, Option<u32>, u32)> {
        segments.iter().map(|s| (s.id, s.parent, s.depth)).collect()
    }

    fn close(a: Vec3, b: Vec3, tolerance: f32) -> bool {
        (a - b).length() < tolerance
    }

    #[test]
    fn structure_holds_for_100_shape_seeds() {
        for spec in lsystem_specs() {
            for lineage in [LINEAGE, 1, 12345] {
                let skeleton = TreeGenerator.structure(&spec, lineage).unwrap();
                assert_eq!(
                    skeleton,
                    TreeGenerator.structure(&spec, lineage).unwrap(),
                    "{}: structure isn't a function of the lineage",
                    spec.meta.name
                );
                let expected = links(&skeleton.segments);
                for shape_seed in 0..SHAPE_SEEDS {
                    // What a pose is made from: ids, parents and count all match.
                    let regrown = regrow(&spec, lineage, shape_seed).unwrap();
                    assert_eq!(
                        links(&regrown),
                        expected,
                        "{} lineage {lineage}: shape seed {shape_seed} grew another structure",
                        spec.meta.name
                    );
                    let pose = TreeGenerator.pose(&spec, &skeleton, shape_seed).unwrap();
                    assert_eq!(pose.segments.len(), expected.len());
                }
            }
        }
    }

    #[test]
    fn the_living_example_is_supported_and_its_lineage_is_set() {
        let spec = load("specs/examples/oak_living.procgen.toml");
        assert_eq!(spec.living.as_ref().unwrap().lineage_seed, LINEAGE);
        TreeGenerator.check_living(&spec).unwrap();
    }

    #[test]
    fn segments_form_a_hierarchy() {
        for spec in lsystem_specs() {
            let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
            let mut roots = 0;
            for (i, s) in skeleton.segments.iter().enumerate() {
                assert_eq!(s.id as usize, i);
                match s.parent {
                    Some(p) => {
                        assert!((p as usize) < i, "parent {p} comes after child {i}");
                        let parent = &skeleton.segments[p as usize];
                        assert!(close(parent.end, s.start, 1e-3));
                    }
                    None => roots += 1,
                }
            }
            assert!(roots > 0, "nothing leaves the trunk");
            assert!(roots < skeleton.segments.len(), "nothing has a parent");
        }
    }

    #[test]
    fn the_skeleton_is_the_tree_generate_bakes_from_the_lineage_seed() {
        for spec in lsystem_specs() {
            let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
            let trunk = TrunkParams::from_toml(&spec.params).unwrap();
            let branch = BranchParams::from_toml(&spec.params).unwrap();
            let mut rng = SeededRng::new(LINEAGE);
            let tree = generate_tree(&trunk, &branch, &mut rng).unwrap();
            assert_eq!(skeleton.segments, tree.branch_segments);

            let baked = match TreeGenerator
                .generate(&spec, &mut SeededRng::new(LINEAGE))
                .unwrap()
            {
                GeneratorOutput::Mesh(mesh) => mesh,
                other => panic!("expected a mesh, got {:?}", other.kind()),
            };
            assert_eq!(baked, join_bark_and_leaves(&tree.mesh, &skeleton.leaf_mesh));
        }
    }

    fn pose_digest(spec: &ProcGenSpec) -> u64 {
        let skeleton = TreeGenerator.structure(spec, LINEAGE).unwrap();
        let mut digest = 0xcbf29ce484222325u64;
        let mut mix = |bits: u32| {
            digest ^= bits as u64;
            digest = digest.wrapping_mul(0x100000001b3);
        };
        for shape_seed in 0..8 {
            let pose = TreeGenerator.pose(spec, &skeleton, shape_seed).unwrap();
            for s in &pose.segments {
                s.rotation.iter().for_each(|c| mix(c.to_bits()));
                mix(s.length_scale.to_bits());
                mix(s.radius_scale.to_bits());
            }
        }
        digest
    }

    /// The digest printed by a fresh process of this test binary: its own
    /// hash seeds and memory layout, which is all a second run would change.
    fn digest_in_new_process() -> String {
        let test = "generators::tree::living::tests::pose_is_bit_identical_across_runs";
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture"])
            .env("FLINT_LIVING_PRINT_DIGEST", "1")
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|l| l.strip_prefix("digest=").map(str::to_owned))
            .unwrap_or_else(|| panic!("no digest in {out:?}"))
    }

    #[test]
    fn pose_is_bit_identical_across_runs() {
        let spec = load("specs/examples/oak_tree.procgen.toml");
        if std::env::var_os("FLINT_LIVING_PRINT_DIGEST").is_some() {
            println!("digest={:x}", pose_digest(&spec));
            return;
        }
        let here = format!("{:x}", pose_digest(&spec));
        assert_eq!(here, format!("{:x}", pose_digest(&spec)));
        assert_eq!(here, digest_in_new_process());
        assert_eq!(here, digest_in_new_process());
    }

    #[test]
    fn poses_drift_and_differ() {
        let spec = load("specs/examples/oak_tree.procgen.toml");
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        let a = TreeGenerator.pose(&spec, &skeleton, 1).unwrap();
        let b = TreeGenerator.pose(&spec, &skeleton, 2).unwrap();
        assert_ne!(a, b);
        let turned = a.segments.iter().filter(|s| s.rotation[3] < 0.9999).count();
        assert!(
            turned > a.segments.len() / 2,
            "only {turned} of {} segments turned",
            a.segments.len()
        );
    }

    #[test]
    fn another_lineage_is_another_tree() {
        let spec = load("specs/examples/oak_tree.procgen.toml");
        let a = TreeGenerator.structure(&spec, 7).unwrap();
        let b = TreeGenerator.structure(&spec, 8).unwrap();
        assert_ne!(a.segments, b.segments);
    }

    #[test]
    fn a_pose_rebuilds_the_tree_it_regrew() {
        for spec in lsystem_specs() {
            let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
            for shape_seed in 0..8 {
                let pose = TreeGenerator
                    .full_pose(&spec, &skeleton, shape_seed)
                    .unwrap();
                let posed = skeleton.posed_segments(&pose).unwrap();
                let regrown = regrow(&spec, LINEAGE, shape_seed).unwrap();
                for (a, b) in posed.iter().zip(&regrown) {
                    assert!(
                        close(a.start, b.start, 1e-3) && close(a.end, b.end, 1e-3),
                        "seed {shape_seed} segment {}: {:?} vs {:?}",
                        a.id,
                        a,
                        b
                    );
                    assert!((a.mean_radius() - b.mean_radius()).abs() < 1e-3);
                }
            }
        }
    }

    #[test]
    fn the_trunk_holds_still_and_limbs_stay_attached() {
        let spec = load("specs/examples/oak_tree.procgen.toml");
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        let spine = lineage_trunk(&spec, LINEAGE).unwrap();
        for shape_seed in 0..SHAPE_SEEDS {
            let pose = TreeGenerator.pose(&spec, &skeleton, shape_seed).unwrap();
            let posed = skeleton.posed_segments(&pose).unwrap();
            // The pose's own regrowth: limbs start where the skeleton's do.
            let regrown = regrow(&spec, LINEAGE, shape_seed).unwrap();
            for (rest, (now, grown)) in skeleton.segments.iter().zip(posed.iter().zip(&regrown)) {
                if rest.parent.is_none() {
                    assert_eq!(now.start, rest.start);
                    assert!(close(grown.start, rest.start, 1e-4));
                    // Whichever limb this is, it starts on the lineage's trunk.
                    assert!(spine.distance_to(now.start) < 1e-3);
                }
            }
        }
    }

    #[test]
    fn leaf_cards_hold_for_100_shape_seeds() {
        for spec in lsystem_specs() {
            let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
            let cards = skeleton.leaf_bindings.len();
            assert!(cards > 0, "{}: no leaf cards", spec.meta.name);
            assert_eq!(
                cards * skeleton.card_index_count,
                skeleton.leaf_mesh.indices.len()
            );
            for shape_seed in 0..SHAPE_SEEDS {
                let pose = TreeGenerator.pose(&spec, &skeleton, shape_seed).unwrap();
                let posed = skeleton.posed_segments(&pose).unwrap();
                let leaves = skeleton.posed_leaf_mesh(&pose).unwrap();
                assert_eq!(leaves.indices, skeleton.leaf_mesh.indices);
                assert_eq!(leaves.vertices.len(), skeleton.leaf_mesh.vertices.len());

                // Spot-check the binding: a card sits exactly where it says.
                for (k, card) in skeleton.leaf_bindings.iter().enumerate().step_by(37) {
                    let s = &posed[card.segment as usize];
                    let at = s.start + (s.end - s.start) * card.along;
                    let verts = skeleton.card_vertices(k);
                    let centre = verts
                        .iter()
                        .map(|&v| Vec3::from_array(leaves.vertices[v as usize].position))
                        .fold(Vec3::ZERO, |sum, p| sum + p)
                        * (1.0 / verts.len() as f32);
                    let gap = (centre - at).length();
                    assert!(
                        (gap - card.offset.length()).abs() < 1e-3,
                        "card {k}, seed {shape_seed}: {gap} from its segment, bound at {}",
                        card.offset.length()
                    );
                }
            }
        }
    }

    /// Why leaf placement had to move: plain `generate` changes the card
    /// count with the seed.
    #[test]
    fn plain_generate_changes_its_leaf_count_per_seed() {
        let spec = load("specs/examples/oak_tree.procgen.toml");
        let baked: std::collections::BTreeSet<usize> = (0..8)
            .map(|seed| {
                let mesh = TreeGenerator
                    .generate(&spec, &mut SeededRng::new(seed))
                    .unwrap();
                match mesh {
                    GeneratorOutput::Mesh(m) => m.indices.len(),
                    other => panic!("expected a mesh, got {:?}", other.kind()),
                }
            })
            .collect();
        assert!(baked.len() > 1, "generate kept one size: {baked:?}");
    }

    #[test]
    fn a_card_binds_to_its_nearest_segment() {
        let spec = load("specs/oak_tree.procgen2.toml");
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        for (k, card) in skeleton.leaf_bindings.iter().enumerate() {
            assert!((0.0..=1.0).contains(&card.along));
            let s = &skeleton.segments[card.segment as usize];
            let centre = s.start + (s.end - s.start) * card.along + card.offset;
            for other in &skeleton.segments {
                let ab = other.end - other.start;
                let t = ((centre - other.start).dot(&ab) / ab.dot(&ab).max(1e-9)).clamp(0.0, 1.0);
                let gap = (centre - (other.start + ab * t)).length();
                assert!(
                    card.offset.length() <= gap + 1e-4,
                    "card {k} is nearer segment {} than its own {}",
                    other.id,
                    card.segment
                );
            }
        }
    }

    #[test]
    fn a_tree_without_leaves_has_no_cards() {
        let mut spec = load("specs/oak_tree.procgen2.toml");
        spec.params
            .as_table_mut()
            .unwrap()
            .insert("leaf_style".into(), "none".into());
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        assert!(skeleton.leaf_bindings.is_empty());
        let pose = TreeGenerator.pose(&spec, &skeleton, 1).unwrap();
        assert!(skeleton.posed_leaf_mesh(&pose).unwrap().indices.is_empty());
    }

    fn assert_unsupported(spec: &ProcGenSpec, says: &str) {
        for result in [
            TreeGenerator.check_living(spec).err(),
            TreeGenerator.structure(spec, LINEAGE).err(),
        ] {
            let message = result.expect("expected a refusal").to_string();
            assert!(message.contains(says), "{message}");
            assert!(message.contains("shallow wooze"), "{message}");
        }
    }

    #[test]
    fn space_colonization_reports_itself_unsupported() {
        assert_unsupported(
            &load("specs/examples/pine_tree.procgen.toml"),
            "space colonization",
        );
    }

    #[test]
    fn text_grammars_report_themselves_unsupported() {
        let mut spec = load("specs/oak_tree.procgen2.toml");
        spec.params
            .as_table_mut()
            .unwrap()
            .insert("lsystem_preset".into(), "stochastic_plant".into());
        assert_unsupported(&spec, "text grammar");
    }

    #[test]
    fn the_registry_finds_the_living_side() {
        let tree: &dyn Generator = &TreeGenerator;
        assert!(tree.as_living().is_some());
        let texture: &dyn Generator = &crate::TextureGenerator;
        assert!(texture.as_living().is_none());
    }

    #[test]
    fn a_pose_of_another_shape_is_refused() {
        let oak = load("specs/examples/oak_tree.procgen.toml");
        let small = load("specs/oak_tree.procgen2.toml");
        let skeleton = TreeGenerator.structure(&small, LINEAGE).unwrap();
        assert!(TreeGenerator.pose(&oak, &skeleton, 1).is_err());
        let other = TreeGenerator.structure(&oak, LINEAGE).unwrap();
        let pose = TreeGenerator.pose(&oak, &other, 1).unwrap();
        assert!(skeleton.posed_segments(&pose).is_err());
    }

    fn clock(glide: f32, stagger: crate::Stagger) -> crate::LivingClock {
        crate::LivingClock {
            step: 0.5,
            glide,
            stagger,
        }
    }

    #[test]
    fn a_snapping_stream_is_exactly_one_pose_until_the_beat() {
        let spec = load("specs/examples/oak_tree.procgen.toml");
        let skeleton = TreeGenerator.structure(&spec, 7).unwrap();
        let mut stream = crate::PoseStream::new(
            &TreeGenerator,
            &spec,
            &skeleton,
            clock(0.0, crate::Stagger::None),
        );
        let first = TreeGenerator.pose(&spec, &skeleton, 0).unwrap();
        let second = TreeGenerator.pose(&spec, &skeleton, 1).unwrap();
        for t in [0.0, 0.2, 0.49] {
            assert_eq!(stream.at(t).unwrap().segments, first.segments, "t = {t}");
        }
        assert_eq!(stream.at(0.5).unwrap().segments, second.segments);
    }

    #[test]
    fn a_gliding_stream_moves_every_frame_and_lands_on_the_next_pose() {
        let spec = load("specs/examples/oak_tree.procgen.toml");
        let skeleton = TreeGenerator.structure(&spec, 7).unwrap();
        let mut stream = crate::PoseStream::new(
            &TreeGenerator,
            &spec,
            &skeleton,
            clock(1.0, crate::Stagger::None),
        );
        let frames: Vec<_> = (0..=12)
            .map(|f| stream.at(f64::from(f) / 24.0).unwrap().segments)
            .collect();
        for pair in frames.windows(2) {
            assert_ne!(pair[0], pair[1]);
        }
        let next = TreeGenerator.pose(&spec, &skeleton, 1).unwrap();
        assert_eq!(frames[12], next.segments);
    }

    #[test]
    fn per_level_stagger_moves_the_levels_at_different_moments() {
        let spec = load("specs/examples/oak_tree.procgen.toml");
        let skeleton = TreeGenerator.structure(&spec, 7).unwrap();
        let mut stream = crate::PoseStream::new(
            &TreeGenerator,
            &spec,
            &skeleton,
            clock(0.0, crate::Stagger::PerLevel),
        );
        // At glide 0 each level snaps on its own frame: a frame that changes
        // some levels and leaves others alone is what stagger looks like.
        let (mut some_moved, mut some_held) = (false, false);
        let mut last = stream.at(0.0).unwrap();
        for f in 1..24 {
            let now = stream.at(f64::from(f) / 24.0).unwrap();
            let moved = now
                .segments
                .iter()
                .zip(&last.segments)
                .filter(|(a, b)| a != b);
            let moved = moved.count();
            if moved > 0 && moved < now.segments.len() {
                some_moved = true;
            }
            if moved == 0 {
                some_held = true;
            }
            last = now;
        }
        assert!(some_moved, "no frame moved only part of the tree");
        assert!(
            some_held,
            "every frame moved something: no hold left at glide 0"
        );
    }

    #[test]
    fn a_stream_gives_the_same_bits_however_it_is_walked() {
        let spec = load("specs/examples/oak_tree.procgen.toml");
        let skeleton = TreeGenerator.structure(&spec, 7).unwrap();
        let c = clock(0.3, crate::Stagger::PerLevel);
        let mut forward = crate::PoseStream::new(&TreeGenerator, &spec, &skeleton, c);
        let mut backward = crate::PoseStream::new(&TreeGenerator, &spec, &skeleton, c);
        let ahead: Vec<_> = (0..10)
            .map(|f| forward.at(f64::from(f) * 0.2).unwrap())
            .collect();
        for f in (0..10).rev() {
            assert_eq!(backward.at(f64::from(f) * 0.2).unwrap(), ahead[f as usize]);
        }
    }

    #[test]
    fn a_blended_pose_makes_a_whole_tree_with_every_leaf() {
        let spec = load("specs/examples/oak_tree.procgen.toml");
        let skeleton = TreeGenerator.structure(&spec, 7).unwrap();
        let mut stream = crate::PoseStream::new(
            &TreeGenerator,
            &spec,
            &skeleton,
            clock(1.0, crate::Stagger::PerLevel),
        );
        let halfway = stream.at(0.25).unwrap();
        let mesh = TreeGenerator
            .posed_mesh(&spec, &skeleton, &halfway)
            .unwrap();
        assert!(mesh
            .vertices
            .iter()
            .all(|v| v.position.iter().all(|x| x.is_finite())));
        assert!(mesh.submeshes.len() > 1, "bark and leaf shades");
        let leaf_triangles: usize = skeleton.leaf_mesh.indices.len() / 3;
        assert!(mesh.triangle_count() > leaf_triangles);
        assert_eq!(
            TreeGenerator
                .posed_mesh(&spec, &skeleton, &halfway)
                .unwrap(),
            mesh,
            "the same pose must rebuild the same mesh"
        );
    }

    /// A spec at the variance under test, with a `[living]` table if it had none.
    fn at_variance(relative: &str, variance: VarianceConfig) -> ProcGenSpec {
        let mut spec = load(relative);
        let mut living = spec.living.take().unwrap_or(LivingConfig {
            lineage_seed: LINEAGE,
            pose_ring: 8,
            step: 0.5,
            glide: 0.3,
            drift: Default::default(),
            jitter: Default::default(),
            stagger: Default::default(),
            variance,
        });
        living.variance = variance;
        spec.living = Some(living);
        spec
    }

    fn sides(bounds: &crate::BoundingBox) -> [f32; 3] {
        (bounds.max - bounds.min).to_array()
    }

    /// For each pose of an 8-pose ring, each side of its box over the rest
    /// pose's, less 1: +0.05 is 5% longer.
    fn side_differences(spec: &ProcGenSpec) -> Vec<[f32; 3]> {
        let skeleton = TreeGenerator.structure(spec, LINEAGE).unwrap();
        let rest = sides(&skeleton.rest_bounds());
        (0..8)
            .map(|shape_seed| {
                let pose = TreeGenerator.pose(spec, &skeleton, shape_seed).unwrap();
                let now = sides(&skeleton.posed_bounds(&pose).unwrap());
                [0, 1, 2].map(|axis| now[axis] / rest[axis] - 1.0)
            })
            .collect()
    }

    /// The largest difference on any side of each pose, sorted.
    fn worst_per_pose(spec: &ProcGenSpec) -> Vec<f32> {
        let mut worst: Vec<f32> = side_differences(spec)
            .iter()
            .map(|sides| sides.iter().fold(0.0f32, |w, d| w.max(d.abs())))
            .collect();
        worst.sort_by(f32::total_cmp);
        worst
    }

    const VARIANCE_SPECS: [&str; 3] = [
        "specs/examples/oak_living.procgen.toml",
        "specs/oak_tree.procgen.toml",
        "specs/oak_tree.procgen2.toml",
    ];

    /// How far a side of the default ring's box may move with `keep_size`.
    /// The 27-segment oak gets more room: a handful of tips decide its box.
    fn kept_limit(path: &str) -> f32 {
        if path.ends_with("procgen2.toml") {
            0.10
        } else {
            0.06
        }
    }

    const LOOSE: VarianceConfig = VarianceConfig {
        amount: 0.35,
        steady: 0.8,
        keep_size: false,
    };

    #[test]
    fn the_default_ring_holds_its_size() {
        for path in VARIANCE_SPECS {
            let kept = worst_per_pose(&at_variance(path, VarianceConfig::default()));
            let loose = worst_per_pose(&at_variance(path, LOOSE));
            let full = worst_per_pose(&at_variance(path, VarianceConfig::FULL));
            eprintln!(
                "{path}\n  keep_size {kept:.3?}\n  without   {loose:.3?}\n  full      {full:.3?}"
            );
            assert!(
                kept[7] < kept_limit(path),
                "{path}: keep_size let a side move {kept:?}"
            );
            assert!(loose[7] < 0.14, "{path}: without keep_size {loose:?}");
            assert!(kept[7] < loose[7], "{path}: {kept:?} against {loose:?}");
            assert!(loose[7] < full[7], "{path}: {loose:?} against {full:?}");
        }
    }

    #[test]
    fn keep_size_centres_the_sides_and_touches_only_length() {
        for path in VARIANCE_SPECS {
            let spec = at_variance(path, VarianceConfig::default());
            for sides in side_differences(&spec) {
                let over = sides.iter().fold(f32::MIN, |m, d| m.max(*d));
                let under = sides.iter().fold(f32::MAX, |m, d| m.min(*d));
                assert!(
                    (over + under).abs() < 0.005,
                    "{path}: sides {sides:?} aren't centred"
                );
            }

            let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
            for shape_seed in 0..8 {
                let full = TreeGenerator
                    .full_pose(&spec, &skeleton, shape_seed)
                    .unwrap();
                let kept = TreeGenerator.pose(&spec, &skeleton, shape_seed).unwrap();
                let unscaled = skeleton.with_variance(&full, &LOOSE).unwrap();
                let scale = kept.segments[0].length_scale / unscaled.segments[0].length_scale;
                for (a, b) in kept.segments.iter().zip(&unscaled.segments) {
                    assert_eq!(a.rotation, b.rotation);
                    assert_eq!(a.radius_scale, b.radius_scale);
                    assert!((a.length_scale / b.length_scale / scale - 1.0).abs() < 1e-4);
                }
            }
        }
    }

    /// The mean angle a pose turns the segments at each depth, over a ring.
    fn mean_turn_by_depth(spec: &ProcGenSpec) -> Vec<f32> {
        let skeleton = TreeGenerator.structure(spec, LINEAGE).unwrap();
        let levels = skeleton.max_depth() as usize + 1;
        let (mut sum, mut count) = (vec![0.0f32; levels], vec![0u32; levels]);
        for shape_seed in 0..8 {
            let pose = TreeGenerator.pose(spec, &skeleton, shape_seed).unwrap();
            for (rest, p) in skeleton.segments.iter().zip(&pose.segments) {
                sum[rest.depth as usize] += 2.0 * p.rotation[3].clamp(-1.0, 1.0).acos();
                count[rest.depth as usize] += 1;
            }
        }
        sum.iter()
            .zip(&count)
            .filter(|(_, n)| **n > 0)
            .map(|(s, n)| s / *n as f32)
            .collect()
    }

    #[test]
    fn first_level_limbs_turn_less_than_the_twigs() {
        for path in VARIANCE_SPECS {
            let turns = mean_turn_by_depth(&at_variance(path, VarianceConfig::default()));
            let (limbs, twigs) = (turns[0], *turns.last().unwrap());
            assert!(
                limbs * 2.0 < twigs,
                "{path}: limbs turn {limbs} rad and twigs {twigs} rad: {turns:?}"
            );
            let even = mean_turn_by_depth(&at_variance(path, VarianceConfig::FULL));
            assert!(
                even[0] * 0.5 > limbs,
                "{path}: full variance's limbs {} aren't well above {limbs}",
                even[0]
            );
        }
    }

    #[test]
    fn full_variance_is_the_pose_the_seed_grew_bit_for_bit() {
        for path in VARIANCE_SPECS {
            let spec = at_variance(path, VarianceConfig::FULL);
            let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
            for shape_seed in 0..8 {
                let full = TreeGenerator
                    .full_pose(&spec, &skeleton, shape_seed)
                    .unwrap();
                let pose = TreeGenerator.pose(&spec, &skeleton, shape_seed).unwrap();
                assert_eq!(pose, full);
            }
        }
    }

    #[test]
    fn amount_one_with_keep_size_on_moves_every_angle_and_only_rescales() {
        let spec = at_variance(
            "specs/examples/oak_living.procgen.toml",
            VarianceConfig {
                amount: 1.0,
                steady: 0.0,
                keep_size: true,
            },
        );
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        let full = TreeGenerator.full_pose(&spec, &skeleton, 3).unwrap();
        let pose = TreeGenerator.pose(&spec, &skeleton, 3).unwrap();
        for (a, b) in pose.segments.iter().zip(&full.segments) {
            assert_eq!(a.rotation, b.rotation);
            assert_eq!(a.radius_scale, b.radius_scale);
        }
    }

    #[test]
    fn amount_zero_is_the_rest_pose() {
        for keep_size in [false, true] {
            let spec = at_variance(
                "specs/examples/oak_living.procgen.toml",
                VarianceConfig {
                    amount: 0.0,
                    steady: 0.8,
                    keep_size,
                },
            );
            let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
            let pose = TreeGenerator.pose(&spec, &skeleton, 5).unwrap();
            for s in &pose.segments {
                assert_eq!(s.rotation, SegmentPose::IDENTITY.rotation);
                assert_eq!(s.radius_scale, 1.0);
                assert!((s.length_scale - 1.0).abs() < 1e-5, "{}", s.length_scale);
            }
        }
    }

    #[test]
    fn a_spec_without_a_living_table_takes_the_default_variance() {
        let spec = load("specs/oak_tree.procgen2.toml");
        assert!(spec.living.is_none());
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        let full = TreeGenerator.full_pose(&spec, &skeleton, 2).unwrap();
        assert_eq!(
            TreeGenerator.pose(&spec, &skeleton, 2).unwrap(),
            skeleton
                .with_variance(&full, &VarianceConfig::default())
                .unwrap()
        );
    }

    #[test]
    fn variance_can_be_retuned_without_regrowing() {
        let spec = at_variance(
            "specs/oak_tree.procgen2.toml",
            VarianceConfig {
                amount: 0.5,
                steady: 0.25,
                keep_size: false,
            },
        );
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        let full = TreeGenerator.full_pose(&spec, &skeleton, 4).unwrap();
        let retuned = skeleton
            .with_variance(&full, &spec.living.as_ref().unwrap().variance)
            .unwrap();
        assert_eq!(TreeGenerator.pose(&spec, &skeleton, 4).unwrap(), retuned);
    }

    // ── The skinned rest (#209) ─────────────────────────────────────────

    fn rig_of(spec: &ProcGenSpec) -> LivingRig {
        TreeGenerator.rig(spec, LINEAGE).unwrap()
    }

    fn weights_sum_to_one(mesh: &SkinnedMeshData, name: &str) {
        for (i, v) in mesh.vertices.iter().enumerate() {
            let sum: f32 = v.joint_weights.iter().sum();
            assert!(
                (sum - 1.0).abs() < 1e-6,
                "{name}: vertex {i}'s weights sum to {sum}"
            );
            for (joint, weight) in v.joint_indices.iter().zip(&v.joint_weights) {
                assert!(*weight >= 0.0);
                assert!(*weight == 0.0 || (*joint as usize) < mesh.skeleton.bones.len());
            }
        }
    }

    #[test]
    fn weights_sum_to_one_on_every_vertex() {
        for spec in lsystem_specs() {
            for lineage in [LINEAGE, 1, 12345] {
                let rig = TreeGenerator.rig(&spec, lineage).unwrap();
                weights_sum_to_one(&rig.mesh, &spec.meta.name);
            }
        }
    }

    #[test]
    fn the_living_example_bakes_a_skinned_mesh_from_its_lineage() {
        let spec = load("specs/examples/oak_living.procgen.toml");
        // Whatever seed the caller's RNG has: the tree is the lineage's.
        for seed in [0, 42, 999] {
            match TreeGenerator
                .generate(&spec, &mut SeededRng::new(seed))
                .unwrap()
            {
                GeneratorOutput::SkinnedMesh(mesh) => assert_eq!(mesh, rig_of(&spec).mesh),
                other => panic!("expected a skinned mesh, got {:?}", other.kind()),
            }
        }
    }

    #[test]
    fn a_spec_without_living_still_bakes_a_still_tree() {
        for spec in lsystem_specs() {
            assert!(spec.living.is_none());
            let out = TreeGenerator
                .generate(&spec, &mut SeededRng::new(LINEAGE))
                .unwrap();
            assert!(matches!(out, GeneratorOutput::Mesh(_)));
        }
    }

    #[test]
    fn a_living_table_on_a_spec_that_cant_live_still_bakes_a_still_tree() {
        let mut spec = load("specs/examples/oak_living.procgen.toml");
        spec.params
            .as_table_mut()
            .unwrap()
            .insert("lsystem_preset".into(), "stochastic_plant".into());
        let out = TreeGenerator
            .generate(&spec, &mut SeededRng::new(LINEAGE))
            .unwrap();
        assert!(matches!(out, GeneratorOutput::Mesh(_)));
    }

    #[test]
    fn the_rest_mesh_is_exactly_the_tree_generate_bakes() {
        for spec in lsystem_specs() {
            let baked = match TreeGenerator
                .generate(&spec, &mut SeededRng::new(LINEAGE))
                .unwrap()
            {
                GeneratorOutput::Mesh(mesh) => mesh,
                other => panic!("expected a mesh, got {:?}", other.kind()),
            };
            assert_eq!(rig_of(&spec).mesh.rest_mesh(), baked);
        }
    }

    #[test]
    fn the_oak_gets_a_bone_per_segment_under_the_trunk() {
        let spec = load("specs/examples/oak_living.procgen.toml");
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        let rig = rig_of(&spec);
        assert_eq!(rig.mesh.bone_count(), skeleton.segments.len() + 1);
        assert_eq!(rig.mesh.skeleton.bones[0].name, "trunk");
        assert_eq!(rig.mesh.skeleton.bones[0].parent, None);
        for (s, bone) in skeleton.segments.iter().zip(&rig.segment_bones) {
            assert_eq!(rig.bone_segments[*bone as usize], Some(s.id));
            let def = &rig.mesh.skeleton.bones[*bone as usize];
            let parent = def.parent.unwrap();
            assert!(parent < *bone as usize, "a parent comes before its child");
            match s.parent {
                Some(p) => assert_eq!(parent, rig.segment_bones[p as usize] as usize),
                None => assert_eq!(parent, 0),
            }
        }
    }

    #[test]
    fn each_bone_sits_on_its_segment_pointing_along_it() {
        let spec = load("specs/examples/oak_living.procgen.toml");
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        let rig = rig_of(&spec);
        let world = rig.mesh.skeleton.compute_world_transforms();
        for s in &skeleton.segments {
            let m = &world[rig.segment_bones[s.id as usize] as usize];
            let head = Vec3::new(m[12], m[13], m[14]);
            let along = Vec3::new(m[4], m[5], m[6]);
            assert!(close(head, s.start, 1e-3), "segment {}", s.id);
            assert!(close(along, s.direction(), 1e-3), "segment {}", s.id);
        }
    }

    /// Distance from `p` to the segment.
    fn gap_to(p: Vec3, s: &BranchSegment) -> f32 {
        let ab = s.end - s.start;
        let t = ((p - s.start).dot(&ab) / ab.dot(&ab).max(1e-9)).clamp(0.0, 1.0);
        (p - (s.start + ab * t)).length()
    }

    #[test]
    fn tube_vertices_are_skinned_to_the_segments_they_wrap() {
        for spec in lsystem_specs() {
            let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
            let rig = rig_of(&spec);
            let trunk = TrunkParams::from_toml(&spec.params).unwrap();
            let branch = BranchParams::from_toml(&spec.params).unwrap();
            let tree = generate_tree(&trunk, &branch, &mut SeededRng::new(LINEAGE)).unwrap();

            let first_tube = tree.trunk_vertices;
            let first_leaf = rig.mesh.vertices.len() - skeleton.leaf_mesh.vertices.len();
            assert!(first_tube > 0 && first_leaf > first_tube);

            for v in &rig.mesh.vertices[..first_tube] {
                assert_eq!(v.joint_indices[0], 0, "the trunk moves on its own bone");
                assert_eq!(v.joint_weights, [1.0, 0.0, 0.0, 0.0]);
            }

            let mut shared = 0;
            for (i, v) in rig.mesh.vertices[first_tube..first_leaf].iter().enumerate() {
                let p = Vec3::from_array(v.position);
                let influences: Vec<&BranchSegment> = v
                    .joint_indices
                    .iter()
                    .zip(&v.joint_weights)
                    .filter(|(_, w)| **w > 0.0)
                    .map(|(j, _)| {
                        let id = rig.bone_segments[*j as usize].expect("a tube on the trunk bone");
                        &skeleton.segments[id as usize]
                    })
                    .collect();
                // A ring sits at its radius from the segments it joins.
                let radius = influences
                    .iter()
                    .map(|s| s.radius_start.max(s.radius_end))
                    .fold(0.0f32, f32::max);
                for s in &influences {
                    assert!(
                        gap_to(p, s) <= radius * 1.01 + 1e-4,
                        "{}: tube vertex {i} is {} from segment {}, radius {radius}",
                        spec.meta.name,
                        gap_to(p, s),
                        s.id
                    );
                }
                if let [a, b] = influences[..] {
                    shared += 1;
                    assert_eq!(v.joint_weights, [0.5, 0.5, 0.0, 0.0]);
                    assert!(close(a.end, b.start, 1e-3), "a ring shared by a non-joint");
                }
            }
            assert!(
                shared > 0,
                "{}: no ring is shared at a joint",
                spec.meta.name
            );
        }
    }

    #[test]
    fn a_leaf_card_rides_one_bone_whole() {
        let spec = load("specs/examples/oak_living.procgen.toml");
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        let rig = rig_of(&spec);
        let first_leaf = rig.mesh.vertices.len() - skeleton.leaf_mesh.vertices.len();
        for (k, card) in skeleton.leaf_bindings.iter().enumerate() {
            let bone = rig.segment_bones[card.segment as usize];
            for v in skeleton.card_vertices(k) {
                let vertex = &rig.mesh.vertices[first_leaf + v as usize];
                assert_eq!(vertex.joint_indices[0], bone, "card {k}");
                assert_eq!(vertex.joint_weights, [1.0, 0.0, 0.0, 0.0], "card {k}");
            }
        }
    }

    fn palette_of(
        rig: &LivingRig,
        skeleton: &LivingSkeleton,
        pose: &LivingPose,
    ) -> Vec<[[f32; 4]; 4]> {
        let mut palette = Vec::new();
        rig.palette(skeleton, pose, &mut palette).unwrap();
        palette
    }

    fn transform(m: &[[f32; 4]; 4], p: Vec3) -> Vec3 {
        let a = p.to_array();
        Vec3::from_array(std::array::from_fn(|r| {
            m[0][r] * a[0] + m[1][r] * a[1] + m[2][r] * a[2] + m[3][r]
        }))
    }

    #[test]
    fn the_rest_pose_moves_nothing() {
        let spec = load("specs/examples/oak_living.procgen.toml");
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        let rig = rig_of(&spec);
        let rest = LivingPose {
            shape_seed: 0,
            segments: vec![SegmentPose::IDENTITY; skeleton.segments.len()],
        };
        let moved = rig.skinned_mesh(&palette_of(&rig, &skeleton, &rest));
        for (a, b) in moved.vertices.iter().zip(&rig.mesh.vertices) {
            let (a, b) = (Vec3::from_array(a.position), Vec3::from_array(b.position));
            assert!(close(a, b, 1e-5), "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn a_pose_puts_every_bone_where_the_pose_rebuilds_its_segment() {
        for spec in lsystem_specs() {
            let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
            let rig = rig_of(&spec);
            for shape_seed in 0..16 {
                let full = TreeGenerator
                    .full_pose(&spec, &skeleton, shape_seed)
                    .unwrap();
                let held = TreeGenerator.pose(&spec, &skeleton, shape_seed).unwrap();
                for pose in [full, held] {
                    let posed = skeleton.posed_segments(&pose).unwrap();
                    let palette = palette_of(&rig, &skeleton, &pose);
                    for (rest, now) in skeleton.segments.iter().zip(&posed) {
                        let bone = &palette[rig.segment_bones[rest.id as usize] as usize];
                        assert!(
                            close(transform(bone, rest.start), now.start, 1e-3)
                                && close(transform(bone, rest.end), now.end, 1e-3),
                            "{} seed {shape_seed} segment {}",
                            spec.meta.name,
                            rest.id
                        );
                    }
                }
            }
        }
    }

    /// Rings are single vertices shared by the two segments they join, so a
    /// limb can only come apart if those segments disagree about where their
    /// joint is. They mustn't, and a limb leaving another (whose base is its
    /// own ring, inside the parent) has to stay on it.
    #[test]
    fn no_pose_tears_a_joint_and_leaves_ride_their_branch() {
        let spec = load("specs/examples/oak_living.procgen.toml");
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        let rig = rig_of(&spec);
        let first_leaf = rig.mesh.vertices.len() - skeleton.leaf_mesh.vertices.len();
        for shape_seed in 0..SHAPE_SEEDS {
            // Undamped: the hardest a pose can pull on a joint.
            let pose = TreeGenerator
                .full_pose(&spec, &skeleton, shape_seed)
                .unwrap();
            let posed = skeleton.posed_segments(&pose).unwrap();
            let palette = palette_of(&rig, &skeleton, &pose);

            // Both sides of every joint put the joint in one place.
            for (rest, now) in skeleton.segments.iter().zip(&posed) {
                let Some(parent) = rest.parent else { continue };
                let parent_bone = &palette[rig.segment_bones[parent as usize] as usize];
                let own_bone = &palette[rig.segment_bones[rest.id as usize] as usize];
                let (a, b) = (
                    transform(parent_bone, rest.start),
                    transform(own_bone, rest.start),
                );
                assert!(
                    close(a, b, 2e-3),
                    "seed {shape_seed} joint {}: {a:?} vs {b:?}",
                    rest.id
                );
                assert!(close(b, now.start, 2e-3));
            }

            // A card's centre lands where the #208 rebuild puts its anchor,
            // give or take its offset from the branch.
            let skinned = rig.skinned_mesh(&palette);
            for (k, card) in skeleton.leaf_bindings.iter().enumerate().step_by(37) {
                let s = &posed[card.segment as usize];
                let anchor = s.start + (s.end - s.start) * card.along;
                let verts = skeleton.card_vertices(k);
                let centre = verts
                    .iter()
                    .map(|&v| Vec3::from_array(skinned.vertices[first_leaf + v as usize].position))
                    .fold(Vec3::ZERO, |sum, p| sum + p)
                    * (1.0 / verts.len() as f32);
                let p = &pose.segments[card.segment as usize];
                let stretch = p.length_scale.max(p.radius_scale).max(1.0);
                assert!(
                    (centre - anchor).length() <= card.offset.length() * stretch + 1e-3,
                    "card {k}, seed {shape_seed}"
                );
            }
        }
    }

    /// Two bones sharing a ring keep cos(turn / 2) of its width, which is the
    /// dot product of their rotations. The variance is what makes that safe:
    /// undamped, a joint can turn nearly half way round and pinch flat.
    #[test]
    fn the_default_variance_keeps_every_joint_open() {
        let spec = load("specs/examples/oak_living.procgen.toml");
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        let narrowest = |pose: &LivingPose| {
            skeleton
                .segments
                .iter()
                .filter_map(|s| {
                    let a = &pose.segments[s.parent? as usize].rotation;
                    let b = &pose.segments[s.id as usize].rotation;
                    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
                    Some(dot.abs())
                })
                .fold(1.0f32, f32::min)
        };
        let (mut held, mut full) = (1.0f32, 1.0f32);
        for shape_seed in 0..SHAPE_SEEDS {
            held = held.min(narrowest(
                &TreeGenerator.pose(&spec, &skeleton, shape_seed).unwrap(),
            ));
            full = full.min(narrowest(
                &TreeGenerator
                    .full_pose(&spec, &skeleton, shape_seed)
                    .unwrap(),
            ));
        }
        assert!(held > 0.6, "a held joint narrows to {held}");
        assert!(
            full < 0.1,
            "undamped joints stay at {full}: no test of this"
        );
    }

    #[test]
    fn a_huge_tree_folds_its_smallest_twigs_to_stay_in_budget() {
        let mut spec = load("specs/examples/oak_living.procgen.toml");
        spec.params
            .as_table_mut()
            .unwrap()
            .insert("lsystem_iterations".into(), 6.into());
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        assert!(
            skeleton.segments.len() > MAX_SKINNED_BONES,
            "{} segments is no test of the budget",
            skeleton.segments.len()
        );
        let rig = rig_of(&spec);
        assert_eq!(rig.mesh.bone_count() + BOIL_BONES, MAX_SKINNED_BONES);
        weights_sum_to_one(&rig.mesh, "huge");
        crate::skinned_mesh_to_glb(&rig.mesh).unwrap();

        for s in &skeleton.segments {
            let bone = rig.segment_bones[s.id as usize];
            match rig.bone_segments[bone as usize] {
                // Its own bone: so is its parent's, all the way up.
                Some(id) if id == s.id => {
                    if let Some(p) = s.parent {
                        let up = rig.segment_bones[p as usize] as usize;
                        assert_eq!(
                            rig.bone_segments[up],
                            Some(p),
                            "a kept bone lost its parent"
                        );
                    }
                }
                // Folded: the bone it rides is its nearest kept ancestor.
                _ => {
                    let up = s.parent.map_or(0, |p| rig.segment_bones[p as usize]);
                    assert_eq!(bone, up);
                }
            }
        }

        // A pose still lands on the bones that are left, with nothing torn.
        let pose = TreeGenerator.pose(&spec, &skeleton, 3).unwrap();
        let posed = skeleton.posed_segments(&pose).unwrap();
        let palette = palette_of(&rig, &skeleton, &pose);
        for id in rig.bone_segments.iter().flatten() {
            let (rest, now) = (&skeleton.segments[*id as usize], &posed[*id as usize]);
            let bone = &palette[rig.segment_bones[*id as usize] as usize];
            assert!(close(transform(bone, rest.end), now.end, 1e-3));
        }
    }

    #[test]
    fn assign_bones_folds_tips_before_what_they_hang_from() {
        let seg = |id: u32, parent: Option<u32>, length: f32| BranchSegment {
            start: Vec3::ZERO,
            end: Vec3::new(0.0, length, 0.0),
            radius_start: 0.1,
            radius_end: 0.1,
            depth: 1,
            id,
            parent,
        };
        // 0 -> 1 -> (2 short, 3 long)
        let segments = [
            seg(0, None, 1.0),
            seg(1, Some(0), 0.1),
            seg(2, Some(1), 0.2),
            seg(3, Some(1), 0.5),
        ];

        let (bones, kept) = assign_bones(&segments, 3);
        assert_eq!(bones, vec![1, 2, 2, 3], "the shortest tip goes first");
        assert_eq!(kept, vec![None, Some(0), Some(1), Some(3)]);

        // Segment 1 is shorter than both tips, but they hang from it.
        let (bones, kept) = assign_bones(&segments, 1);
        assert_eq!(bones, vec![1, 1, 1, 1]);
        assert_eq!(kept, vec![None, Some(0)]);

        let (bones, _) = assign_bones(&segments, 0);
        assert_eq!(bones, vec![0, 0, 0, 0], "everything can ride the trunk");
    }

    #[test]
    fn the_skinned_glb_keeps_its_bones_and_its_bark() {
        let spec = load("specs/examples/oak_living.procgen.toml");
        let rig = rig_of(&spec);
        let glb = crate::skinned_mesh_to_glb(&rig.mesh).unwrap();
        let (doc, buffers, images) = gltf::import_slice(&glb).unwrap();

        let skin = doc.skins().next().expect("a skin");
        assert_eq!(skin.joints().count(), rig.mesh.bone_count());
        assert_eq!(doc.images().count(), images.len());
        assert!(
            doc.materials().any(|m| m.normal_texture().is_some()),
            "the bark lost its normal map"
        );

        let mesh = doc.meshes().next().unwrap();
        assert_eq!(mesh.primitives().count(), rig.mesh.submeshes.len());
        let primitive = mesh.primitives().next().unwrap();
        let reader = primitive.reader(|b| Some(&buffers[b.index()]));
        let weights: Vec<[f32; 4]> = reader.read_weights(0).unwrap().into_f32().collect();
        assert_eq!(weights.len(), rig.mesh.vertices.len());
        for w in &weights {
            assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn rotation_between_turns_one_direction_to_the_other() {
        use flint_core::quat::quat_rotate_vec3;
        let cases = [
            (Vec3::UP, Vec3::RIGHT),
            (Vec3::UP, Vec3::UP),
            (Vec3::UP, Vec3::new(0.0, -1.0, 0.0)),
            (Vec3::RIGHT, Vec3::new(-1.0, 0.0, 0.0)),
            (
                Vec3::new(1.0, 2.0, 3.0).normalized(),
                Vec3::new(-2.0, 1.0, 0.5).normalized(),
            ),
        ];
        for (a, b) in cases {
            let q = rotation_between(a, b);
            let turned = Vec3::from_array(quat_rotate_vec3(&q, a.to_array()));
            assert!(close(turned, b, 1e-4), "{a:?} -> {b:?} gave {turned:?}");
        }
        assert_eq!(
            rotation_between(Vec3::ZERO, Vec3::UP),
            SegmentPose::IDENTITY.rotation
        );
    }

    /// Where a leaf vertex goes under `palette`, skinned as the shader does it.
    fn runtime_position(rig: &LivingRig, vertex: usize, palette: &[[[f32; 4]; 4]]) -> Vec3 {
        let (joints, weights) = rig.runtime_joints(vertex);
        let p = rig.mesh.vertices[vertex].position;
        let mut sum = [0.0f32; 4];
        for (joint, weight) in joints.iter().zip(&weights) {
            let m = &palette[*joint as usize];
            for (r, out) in sum.iter_mut().enumerate() {
                *out += weight * (m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r]);
            }
        }
        assert!((sum[3] - 1.0).abs() < 1e-5, "w = {}", sum[3]);
        Vec3::new(sum[0], sum[1], sum[2])
    }

    /// A rest palette, with `turns` on the jitter bones.
    fn jittered_rest(
        rig: &LivingRig,
        skeleton: &LivingSkeleton,
        turns: &[f32; crate::living::JITTER_CHANNELS],
    ) -> Vec<[[f32; 4]; 4]> {
        let rest = LivingPose {
            shape_seed: 0,
            segments: vec![SegmentPose::IDENTITY; skeleton.segments.len()],
        };
        let mut palette = palette_of(rig, skeleton, &rest);
        crate::living::boil_bones(turns, &mut palette);
        palette
    }

    #[test]
    fn the_oak_has_room_for_its_jitter_bones() {
        let spec = load("specs/examples/oak_living.procgen.toml");
        let rig = rig_of(&spec);
        assert_eq!(
            rig.bone_segments.len(),
            244,
            "it keeps every segment's bone"
        );
        assert!(rig.bone_segments.len() + BOIL_BONES <= MAX_SKINNED_BONES);
    }

    #[test]
    fn only_the_leaf_shades_are_leaf_parts() {
        let spec = load("specs/examples/oak_living.procgen.toml");
        let rig = rig_of(&spec);
        assert_eq!(
            rig.leaf_parts,
            vec![1, 2, 3, 4, 5],
            "bark first, five shades"
        );
    }

    #[test]
    fn still_jitter_leaves_every_vertex_exactly_where_it_was() {
        let spec = load("specs/examples/oak_living.procgen.toml");
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        let rig = rig_of(&spec);
        let palette = jittered_rest(&rig, &skeleton, &[0.0; 4]);
        for (i, v) in rig.mesh.vertices.iter().enumerate().step_by(53) {
            let at = runtime_position(&rig, i, &palette);
            assert!(close(at, Vec3::from_array(v.position), 1e-4), "vertex {i}");
        }
    }

    #[test]
    fn jitter_turns_each_card_about_its_centre_and_leaves_the_bark_alone() {
        let spec = load("specs/examples/oak_living.procgen.toml");
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        let rig = rig_of(&spec);
        let turn = 0.2;
        let palette = jittered_rest(&rig, &skeleton, &[turn; 4]);

        for i in (0..rig.first_leaf_vertex).step_by(101) {
            let at = runtime_position(&rig, i, &palette);
            let rest = Vec3::from_array(rig.mesh.vertices[i].position);
            assert!(close(at, rest, 1e-4), "bark vertex {i} moved");
        }

        let mut moved_cards = 0;
        for k in (0..skeleton.leaf_bindings.len()).step_by(29) {
            let verts = skeleton.card_vertices(k);
            let rest: Vec<Vec3> = verts
                .iter()
                .map(|&v| Vec3::from_array(skeleton.leaf_mesh.vertices[v as usize].position))
                .collect();
            let now: Vec<Vec3> = verts
                .iter()
                .map(|&v| runtime_position(&rig, rig.first_leaf_vertex + v as usize, &palette))
                .collect();
            let mean = |ps: &[Vec3]| {
                ps.iter().fold(Vec3::ZERO, |sum, p| sum + *p) * (1.0 / ps.len() as f32)
            };
            let (c0, c1) = (mean(&rest), mean(&now));
            assert!(close(c0, c1, 1e-4), "card {k}'s centre moved");

            for (a, b) in rest.iter().zip(&now) {
                let (r0, r1) = ((*a - c0).length(), (*b - c1).length());
                // A turn of `a` by a shear stretches by at most sqrt(1 + a^2).
                let most = r0 * (1.0 + turn * turn).sqrt();
                assert!(r1 <= most * 1.001, "card {k}: {r0} to {r1}");
                assert!(r1 >= r0 * 0.999, "card {k}: {r0} to {r1}");
                if (*a - *b).length() > r0 * 0.05 {
                    moved_cards += 1;
                }
            }

            // Two-sided: where two vertices share a place, jitter keeps them
            // together, or the faces would part.
            for (i, a) in rest.iter().enumerate() {
                for (j, b) in rest.iter().enumerate() {
                    if (*a - *b).length() < 1e-6 {
                        assert!(close(now[i], now[j], 1e-5), "card {k} tore");
                    }
                }
            }
        }
        assert!(moved_cards > 0, "nothing moved");
    }

    #[test]
    fn the_baked_mesh_keeps_its_weights_clean_of_the_jitter() {
        let spec = load("specs/examples/oak_living.procgen.toml");
        let rig = rig_of(&spec);
        weights_sum_to_one(&rig.mesh, "oak");
        assert!(rig.mesh.bone_count() == rig.bone_segments.len());
        for v in &rig.mesh.vertices {
            assert!(v
                .joint_indices
                .iter()
                .all(|&j| (j as usize) < rig.bone_segments.len()));
        }
        // And the runtime's joints stay inside the buffer.
        for i in (0..rig.mesh.vertices.len()).step_by(17) {
            let (joints, _) = rig.runtime_joints(i);
            assert!(joints.iter().all(|&j| (j as usize) < MAX_SKINNED_BONES));
        }
    }

    /// The issue's named risk: bones blended between poses against the strip's
    /// rebuild, away from either pose, where a long way round or a flattened
    /// curve would show. Centres of leaf cards and tube rings both.
    #[test]
    fn a_blended_pose_on_the_bones_matches_the_rebuild_between_poses() {
        let spec = load("specs/examples/oak_living.procgen.toml");
        let skeleton = TreeGenerator.structure(&spec, LINEAGE).unwrap();
        let rig = rig_of(&spec);
        let (a, b) = (
            TreeGenerator.pose(&spec, &skeleton, 3).unwrap(),
            TreeGenerator.pose(&spec, &skeleton, 4).unwrap(),
        );
        for weight in [0.25, 0.5, 0.75] {
            let segments = a
                .segments
                .iter()
                .zip(&b.segments)
                .map(|(a, b)| SegmentPose::blend(a, b, weight))
                .collect();
            let blended = LivingPose {
                shape_seed: 0,
                segments,
            };
            let posed = skeleton.posed_segments(&blended).unwrap();
            let palette = palette_of(&rig, &skeleton, &blended);
            for s in &skeleton.segments {
                let bone = &palette[rig.segment_bones[s.id as usize] as usize];
                let tip = transform(bone, s.end);
                assert!(
                    close(tip, posed[s.id as usize].end, 2e-3),
                    "segment {} at {weight}: {tip:?} vs {:?}",
                    s.id,
                    posed[s.id as usize].end
                );
            }
            // No joint pinches on the way there.
            for s in skeleton.segments.iter().filter(|s| s.parent.is_some()) {
                let (own, parent) = (
                    blended.segments[s.id as usize].rotation,
                    blended.segments[s.parent.unwrap() as usize].rotation,
                );
                let dot: f32 = own.iter().zip(&parent).map(|(a, b)| a * b).sum();
                assert!(dot.abs() > 0.6, "segment {} joint at {dot}", s.id);
            }
        }
    }
}
