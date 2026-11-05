//! One lineage of a living prop, as files you can look at: the rest pose and
//! a few poses as GLBs, and the skeleton and poses as JSON.
//!
//! ```text
//! cargo run -p flint-procgen --example living_poses -- <spec> --out DIR [--poses N] [--lineage N]
//! ```
//!
//! Everything goes through the public contract (`structure`, `pose`,
//! `posed_mesh`), so what this writes is what a caller
//! of `LivingGenerator` gets. Each pose's mesh is rebuilt from scratch; the
//! skinned mesh that makes this cheap is #209.

use flint_procgen::generators::tree::{generate_trunk, TrunkParams};
use flint_procgen::{
    mesh_to_glb, LivingGenerator, LivingPose, LivingSkeleton, ProcGenSpec, SeededRng, SegmentPose,
    TreeGenerator,
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

    let skeleton = TreeGenerator.structure(&spec, lineage)?;
    eprintln!(
        "lineage {lineage}: {} segments, {} leaf cards",
        skeleton.segments.len(),
        skeleton.leaf_bindings.len()
    );

    // Only the JSON still needs the trunk's spine: posed_mesh builds its own.
    let trunk_params = TrunkParams::from_toml(&spec.params)?;
    let trunk = generate_trunk(&trunk_params, &mut SeededRng::new(lineage).fork("trunk"))?;

    let at_rest = LivingPose {
        shape_seed: lineage,
        segments: vec![SegmentPose::IDENTITY; skeleton.segments.len()],
    };
    let rest = TreeGenerator.posed_mesh(&spec, &skeleton, &at_rest)?;
    std::fs::write(out.join("rest.glb"), mesh_to_glb(&rest)?)?;

    let mut pose_json = Vec::new();
    for shape_seed in 0..poses {
        let pose = TreeGenerator.pose(&spec, &skeleton, shape_seed)?;
        let posed = TreeGenerator.posed_mesh(&spec, &skeleton, &pose)?;
        std::fs::write(
            out.join(format!("pose{shape_seed}.glb")),
            mesh_to_glb(&posed)?,
        )?;
        pose_json.push(pose_to_json(&pose));
    }

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
