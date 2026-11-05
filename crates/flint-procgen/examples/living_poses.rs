//! One lineage of a living prop, as files you can look at: the rest pose and
//! a few poses as GLBs, and the skeleton and poses as JSON. It also prints how
//! far each pose's box moved and how far each level of the tree turned.
//!
//! ```text
//! cargo run -p flint-procgen --example living_poses -- <spec> --out DIR [--poses N] [--lineage N]
//!     [--amount A] [--steady S] [--keep-size true|false]
//! ```
//!
//! `--amount`, `--steady` and `--keep-size` override the spec's `[living]
//! variance`. Everything goes through the public contract (`structure`, `rig`,
//! `full_pose`, `with_variance`, `posed_segments`, `posed_leaf_mesh`), so what
//! this writes is what a caller of `LivingGenerator` gets. `poseN.glb` is
//! rebuilt from scratch, and `poseN_skinned.glb` is `skinned.glb` moved by that
//! pose's bone matrices, so the pair shows what skinning gives up against an
//! exact rebuild. It also prints how far the poses deform the mesh.

use flint_procgen::algorithms::mesh_builder::MeshBuilder;
use flint_procgen::generators::tree::branches::segments_to_mesh;
use flint_procgen::generators::tree::{
    generate_trunk, join_bark_and_leaves, BranchParams, TrunkParams,
};
use flint_procgen::{
    mesh_to_glb, skinned_mesh_to_glb, BoundingBox, BranchSegment, LivingGenerator, LivingPose,
    LivingSkeleton, MeshData, ProcGenSpec, SeededRng, TreeGenerator, VarianceConfig,
};
use serde_json::json;
use std::path::PathBuf;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|w| w[0] == name)
        .map(|w| w[1].as_str())
}

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().ok_or("usage: living_poses <spec> --out DIR")?;
    let out = PathBuf::from(opt(&args, "--out").ok_or("living_poses needs --out DIR")?);
    let poses: u64 = opt(&args, "--poses").unwrap_or("8").parse()?;
    std::fs::create_dir_all(&out)?;

    let mut spec = ProcGenSpec::from_file(path.as_ref())?;
    spec.lod = None;
    let lineage = match (opt(&args, "--lineage"), &spec.living) {
        (Some(n), _) => n.parse()?,
        (None, Some(living)) => living.lineage_seed,
        (None, None) => return Err("the spec has no [living] table; pass --lineage N".into()),
    };

    let mut variance: VarianceConfig = spec
        .living
        .as_ref()
        .map_or_else(Default::default, |living| living.variance);
    if let Some(amount) = opt(&args, "--amount") {
        variance.amount = amount.parse()?;
    }
    if let Some(steady) = opt(&args, "--steady") {
        variance.steady = steady.parse()?;
    }
    if let Some(keep_size) = opt(&args, "--keep-size") {
        variance.keep_size = keep_size.parse()?;
    }

    let skeleton = TreeGenerator.structure(&spec, lineage)?;
    eprintln!(
        "lineage {lineage}: {} segments, {} leaf cards, {variance:?}",
        skeleton.segments.len(),
        skeleton.leaf_bindings.len()
    );

    // The trunk is the lineage's, so every pose shares this one mesh.
    let trunk_params = TrunkParams::from_toml(&spec.params)?;
    let trunk = generate_trunk(&trunk_params, &mut SeededRng::new(lineage).fork("trunk"))?;
    let radial = BranchParams::from_toml(&spec.params)?.radial_segments;
    let tree = |segments: &[BranchSegment], leaves: &MeshData| -> Res<MeshData> {
        let mut bark = MeshBuilder::new();
        bark.merge(&trunk.mesh);
        bark.merge(&segments_to_mesh(segments, radial)?);
        bark.compute_normals();
        bark.compute_tangents();
        let mut bark = bark.build();
        bark.materials = trunk.mesh.materials.clone();
        Ok(join_bark_and_leaves(&bark, leaves))
    };

    let rest = tree(&skeleton.segments, &skeleton.leaf_mesh)?;
    std::fs::write(out.join("rest.glb"), mesh_to_glb(&rest)?)?;

    let rig = TreeGenerator.rig(&spec, lineage)?;
    std::fs::write(out.join("skinned.glb"), skinned_mesh_to_glb(&rig.mesh)?)?;
    eprintln!(
        "skinned: {} bones, {} vertices, {} triangles",
        rig.mesh.bone_count(),
        rig.mesh.vertex_count(),
        rig.mesh.triangle_count()
    );

    let rest_box = skeleton.rest_bounds();
    let mut pose_json = Vec::new();
    let mut palette = Vec::new();
    let (mut lengths, mut radii) = (Vec::new(), Vec::new());
    let (mut widths, mut full_widths) = (Vec::new(), Vec::new());
    let levels = skeleton.max_depth() as usize + 1;
    let (mut turned, mut counted) = (vec![0.0f32; levels], vec![0u32; levels]);
    for shape_seed in 0..poses {
        let full = TreeGenerator.full_pose(&spec, &skeleton, shape_seed)?;
        let pose = skeleton.with_variance(&full, &variance)?;
        let sides = side_differences(&rest_box, &skeleton.posed_bounds(&pose)?);
        eprintln!(
            "pose {shape_seed}: box sides x {:+5.1}%  y {:+5.1}%  z {:+5.1}%",
            sides[0] * 100.0,
            sides[1] * 100.0,
            sides[2] * 100.0,
        );
        rig.palette(&skeleton, &pose, &mut palette)?;
        std::fs::write(
            out.join(format!("pose{shape_seed}_skinned.glb")),
            mesh_to_glb(&rig.skinned_mesh(&palette))?,
        )?;
        for (rest, p) in skeleton.segments.iter().zip(&pose.segments) {
            turned[rest.depth as usize] += 2.0 * p.rotation[3].clamp(-1.0, 1.0).acos();
            counted[rest.depth as usize] += 1;
            lengths.push(p.length_scale);
            radii.push(p.radius_scale);
            if let Some(parent) = rest.parent {
                let parent = parent as usize;
                widths.push(joint_width(&pose.segments[parent].rotation, &p.rotation));
                let f = &full.segments;
                full_widths.push(joint_width(
                    &f[parent].rotation,
                    &f[rest.id as usize].rotation,
                ));
            }
        }
        let posed = tree(
            &skeleton.posed_segments(&pose)?,
            &skeleton.posed_leaf_mesh(&pose)?,
        )?;
        std::fs::write(
            out.join(format!("pose{shape_seed}.glb")),
            mesh_to_glb(&posed)?,
        )?;
        pose_json.push(pose_to_json(&pose));
    }

    for (depth, (angle, n)) in turned.iter().zip(&counted).enumerate() {
        if *n > 0 {
            eprintln!(
                "depth {depth}: turns {:5.1} deg on average, weight {:.3}",
                (angle / *n as f32).to_degrees(),
                variance.weight(depth as u32, skeleton.max_depth()),
            );
        }
    }

    report("length scale", &mut lengths);
    report("radius scale", &mut radii);
    report("joint ring width", &mut widths);
    report("undamped", &mut full_widths);

    let doc = json!({
        "spec": spec.meta.name,
        "lineage_seed": lineage,
        "trunk": trunk.spine.iter().map(|p| p.to_array()).collect::<Vec<_>>(),
        "segments": skeleton.segments.iter().map(|s| json!({
            "id": s.id,
            "parent": s.parent,
            "depth": s.depth,
            "start": s.start.to_array(),
            "end": s.end.to_array(),
            "radius": s.mean_radius(),
        })).collect::<Vec<_>>(),
        "cards": card_summary(&skeleton),
        "poses": pose_json,
    });
    std::fs::write(out.join("living.json"), serde_json::to_string(&doc)?)?;
    Ok(())
}

/// Each side of `now` over `rest`'s, less 1: +0.05 is 5% longer.
fn side_differences(rest: &BoundingBox, now: &BoundingBox) -> [f32; 3] {
    let (rest, now) = (
        (rest.max - rest.min).to_array(),
        (now.max - now.min).to_array(),
    );
    [0, 1, 2].map(|axis| now[axis] / rest[axis] - 1.0)
}

/// How wide a ring shared half and half by two bones stays, in the plane of
/// the bend, under this turn between them: 1 is untouched, and 0 is the
/// pinch of two bones half a turn apart.
fn joint_width(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    // cos(half the angle between them) is |dot| for unit quaternions.
    dot.abs().min(1.0)
}

fn report(label: &str, values: &mut [f32]) {
    if values.is_empty() {
        return;
    }
    values.sort_by(f32::total_cmp);
    let at = |q: f32| values[((values.len() - 1) as f32 * q) as usize];
    eprintln!(
        "{label:>17}: min {:.3}  p5 {:.3}  p50 {:.3}  p95 {:.3}  max {:.3}",
        at(0.0),
        at(0.05),
        at(0.5),
        at(0.95),
        at(1.0)
    );
}

fn pose_to_json(pose: &LivingPose) -> serde_json::Value {
    json!({
        "shape_seed": pose.shape_seed,
        "segments": pose.segments.iter()
            .map(|s| json!([s.rotation, s.length_scale, s.radius_scale]))
            .collect::<Vec<_>>(),
    })
}

fn card_summary(skeleton: &LivingSkeleton) -> Vec<serde_json::Value> {
    skeleton
        .leaf_bindings
        .iter()
        .map(|c| json!([c.segment, c.along, c.offset.to_array()]))
        .collect()
}
