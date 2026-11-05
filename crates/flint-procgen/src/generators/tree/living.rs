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

use flint_core::Vec3;

use crate::algorithms::mesh_builder::MeshBuilder;
use crate::generator::LivingGenerator;
use crate::living::{LeafBinding, LivingPose, LivingSkeleton, SegmentPose};
use crate::spec::ProcGenSpec;
use crate::types::{BranchSegment, MeshData};
use crate::{ProcGenError, Result, SeededRng};

use super::branches::{extract_tips, grow_branch_segments, segments_to_mesh};
use super::leaves::{card_index_count, generate_leaves};
use super::trunk::trunk_spine;
use super::{
    generate_trunk, join_bark_and_leaves, BranchMethod, BranchParams, LeafParams, TreeGenerator,
    TrunkParams, TrunkSpine,
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

    fn pose(
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
    use crate::{Generator, GeneratorOutput};
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
                let pose = TreeGenerator.pose(&spec, &skeleton, shape_seed).unwrap();
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
}
