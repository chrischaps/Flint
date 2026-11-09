//! Living Props spike (#207). Throwaway: it uses the tree generator as it is
//! and changes nothing in the engine.
//!
//! ```text
//! cargo run -p flint-procgen --example living_spike -- counts <spec>...
//! cargo run -p flint-procgen --example living_spike -- drift <spec> [--seed N]
//! cargo run -p flint-procgen --example living_spike -- frames <spec> --out DIR
//!     [--lineage N] [--poses N] [--step S] [--glide G[,G...]] [--stagger X]
//!     [--fps F] [--frames N] [--amount A] [--steady S] [--set key=value]...
//! ```
//!
//! `counts` grows a spec at seeds 1..=8 and prints its structure: segments,
//! depths, tips, leaf cards, and whether the parent links match seed 1's.
//!
//! `drift` nudges every numeric param of a spec by ±20% (integers by ±1) at
//! one seed, and says which nudges keep the branch structure.
//!
//! `frames` writes one GLB per frame of a tree drifting between poses on the
//! warped clock from the design: hold, then glide to the next pose over the
//! last `glide` of each `step`, with each branch depth offset by `stagger`
//! of a step. The trunk and the leaves' placement come from the lineage seed;
//! each pose regrows only the branches, from its own fork of that seed. With
//! several glides, one GLB per glide per frame (`g<i>_f<nnn>.glb`).
//!
//! ```text
//! cargo run -p flint-procgen --example living_spike -- noise <spec> --out DIR
//!     [--source noise|regrow] [--lineage N] [--fps F] [--frames N]
//!     [--turn DEG] [--wavelength M] [--stretch S] [--amount A] [--steady S]
//!     [--step S] [--glide G] [--stagger none|per_level|per_branch] [--poses N]
//! ```
//!
//! `noise` (#252) is the prototype of a pose source that doesn't regrow: each
//! pose turns every rest segment by smooth 3D noise sampled at its midpoint,
//! seeded by the shape seed, so neighbouring twigs bend together. It runs as a
//! `LivingGenerator` of its own through the engine's `PoseStream`, variance
//! and blending, so any tree the generator bakes can live, space colonization
//! and grammars included. `--source regrow` runs the same frames through the
//! tree's own regrowth instead, to compare the two on an L-system tree. Writes
//! `f<nnn>.glb` per frame (`--poses N` also writes `pose<k>.glb`, the ring's
//! first N poses held).

use flint_core::Vec3;
use flint_procgen::algorithms::mesh_builder::MeshBuilder;
use flint_procgen::generators::tree::branches::segments_to_mesh;
use flint_procgen::generators::tree::leaves::card_index_count;
use flint_procgen::generators::tree::{
    generate_branches, generate_leaves, generate_trunk, join_bark_and_leaves, BranchParams,
    LeafParams, TrunkParams, TrunkSpine,
};
use flint_procgen::{
    mesh_to_glb, BranchSegment, Generator, GeneratorOutput, MeshData, ProcGenSpec, SeededRng,
    TreeGenerator,
};
use std::collections::BTreeMap;

type Res<T> = Result<T, Box<dyn std::error::Error>>;

fn main() -> Res<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("counts") => counts(&args[1..]),
        Some("drift") => drift(&args[1..]),
        Some("frames") => frames(&args[1..]),
        Some("noise") => noise::run(&args[1..]),
        _ => {
            eprintln!("usage: living_spike counts|drift|frames|noise ... (see the file's header)");
            std::process::exit(2);
        }
    }
}

// ─── Specs and options ─────────────────────────────────────────────────────

fn load(path: &str, sets: &[(String, String)]) -> Res<ProcGenSpec> {
    let mut spec = ProcGenSpec::parse(&std::fs::read_to_string(path)?)?;
    spec.lod = None;
    for (k, v) in sets {
        let value: toml::Value = toml::from_str(&format!("v = {v}"))
            .map(|t: toml::Table| t["v"].clone())
            .unwrap_or_else(|_| toml::Value::String(v.clone()));
        spec.params
            .as_table_mut()
            .ok_or("params is not a table")?
            .insert(k.clone(), value);
    }
    Ok(spec)
}

fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|w| w[0] == name)
        .map(|w| w[1].as_str())
}

fn sets(args: &[String]) -> Vec<(String, String)> {
    args.windows(2)
        .filter(|w| w[0] == "--set")
        .filter_map(|w| w[1].split_once('=').map(|(k, v)| (k.into(), v.into())))
        .collect()
}

// ─── Structure ──────────────────────────────────────────────────────────────

/// For each segment, the index of the segment it grows from (`None` when it
/// starts on the trunk), found by matching its start to an earlier end.
fn parents(segs: &[BranchSegment]) -> Vec<Option<usize>> {
    segs.iter()
        .enumerate()
        .map(|(i, s)| {
            segs[..i]
                .iter()
                .enumerate()
                .filter(|(_, p)| (p.end - s.start).length() < 1e-4)
                .map(|(j, _)| j)
                .next_back()
        })
        .collect()
}

/// What has to match for two trees to blend segment by segment: each
/// segment's depth and parent.
fn signature(segs: &[BranchSegment]) -> Vec<(u32, Option<usize>)> {
    segs.iter().map(|s| s.depth).zip(parents(segs)).collect()
}

struct Grown {
    segments: Vec<BranchSegment>,
    tips: usize,
    cards: usize,
    verts: usize,
    bark_verts: usize,
}

fn grow(spec: &ProcGenSpec, seed: u64) -> Res<Grown> {
    let t = TrunkParams::from_toml(&spec.params)?;
    let b = BranchParams::from_toml(&spec.params)?;
    let l = LeafParams::from_toml(&spec.params)?;
    let mut rng = SeededRng::new(seed);
    let tree = flint_procgen::generators::tree::generate_tree(&t, &b, &mut rng)?;
    let leaves = generate_leaves(
        &tree.branch_tips,
        &tree.branch_tip_directions,
        &tree.branch_segments,
        &l,
        &mut rng.fork("leaves"),
    )?;
    // The generator's own output, to be sure this walks the same tree.
    let full = match TreeGenerator.generate(spec, &mut SeededRng::new(seed))? {
        GeneratorOutput::Mesh(m) => m.vertices.len(),
        _ => 0,
    };
    let per_card = card_index_count(l.style).max(1);
    Ok(Grown {
        tips: tree.branch_tips.len(),
        cards: leaves.indices.len() / per_card,
        verts: full,
        bark_verts: tree.mesh.vertices.len(),
        segments: tree.branch_segments,
    })
}

fn depth_histogram(segs: &[BranchSegment]) -> String {
    let mut h: BTreeMap<u32, usize> = BTreeMap::new();
    for s in segs {
        *h.entry(s.depth).or_default() += 1;
    }
    h.iter()
        .map(|(d, n)| format!("d{d}:{n}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn counts(args: &[String]) -> Res<()> {
    let set = sets(args);
    for path in args.iter().filter(|a| a.ends_with(".toml")) {
        let spec = load(path, &set)?;
        println!("## {path}");
        println!("| seed | segments | depths | tips | leaf cards | bark verts | total verts | same structure as seed 1 |");
        println!("|---|---|---|---|---|---|---|---|");
        let mut first = None;
        for seed in 1..=8u64 {
            let g = grow(&spec, seed)?;
            let sig = signature(&g.segments);
            let same = match &first {
                None => {
                    first = Some(sig);
                    "yes".to_string()
                }
                Some(f) if *f == sig => "yes".into(),
                Some(f) if f.len() == sig.len() => "same count, different links".into(),
                Some(_) => "no".into(),
            };
            println!(
                "| {seed} | {} | {} | {} | {} | {} | {} | {same} |",
                g.segments.len(),
                depth_histogram(&g.segments),
                g.tips,
                g.cards,
                g.bark_verts,
                g.verts
            );
        }
        println!();
    }
    Ok(())
}

fn drift(args: &[String]) -> Res<()> {
    let path = args.first().ok_or("drift needs a spec")?;
    let seed: u64 = opt(args, "--seed").unwrap_or("1").parse()?;
    let base = load(path, &sets(args))?;
    let g0 = grow(&base, seed)?;
    let sig0 = signature(&g0.segments);
    println!(
        "## {path}, seed {seed}: {} segments, {} leaf cards",
        g0.segments.len(),
        g0.cards
    );
    println!("| param | value | nudged to | structure | leaf cards |");
    println!("|---|---|---|---|---|");
    let table = base
        .params
        .as_table()
        .ok_or("params is not a table")?
        .clone();
    for (key, value) in &table {
        let nudges: Vec<toml::Value> = match value {
            toml::Value::Float(f) => vec![(f * 0.8).into(), (f * 1.2).into()],
            toml::Value::Integer(i) => vec![(i - 1).max(0).into(), (i + 1).into()],
            _ => continue,
        };
        let mut cells = Vec::new();
        for n in nudges {
            let mut spec = base.clone();
            spec.params
                .as_table_mut()
                .unwrap()
                .insert(key.clone(), n.clone());
            let cell = match grow(&spec, seed) {
                Ok(g) => {
                    let s = if signature(&g.segments) == sig0 {
                        "kept"
                    } else {
                        "changed"
                    };
                    let c = if g.cards == g0.cards {
                        "kept".to_string()
                    } else {
                        format!("{} → {}", g0.cards, g.cards)
                    };
                    (n.to_string(), s, c)
                }
                Err(e) => (n.to_string(), "error", e.to_string()),
            };
            cells.push(cell);
        }
        for (n, s, c) in cells {
            println!("| {key} | {value} | {n} | {s} | {c} |");
        }
    }
    Ok(())
}

// ─── Frames ─────────────────────────────────────────────────────────────────

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// The design's warped clock: `t' = floor(t/s) + ease(clamp((frac(t/s) -
/// (1 - g)) / g, 0, 1))`. Returns (pose index, blend toward the next).
fn warped(t: f32, step: f32, glide: f32) -> (i64, f32) {
    let u = t / step;
    let i = u.floor();
    let w = if glide <= 1e-6 {
        0.0
    } else {
        smoothstep((u - i - (1.0 - glide)) / glide)
    };
    (i as i64, w)
}

fn rotation_between(a: Vec3, b: Vec3) -> [[f32; 3]; 3] {
    let a = a.normalized();
    let b = b.normalized();
    let v = a.cross(&b);
    let c = a.dot(&b);
    if c < -0.9999 {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    let k = 1.0 / (1.0 + c);
    [
        [v.x * v.x * k + c, v.x * v.y * k - v.z, v.x * v.z * k + v.y],
        [v.y * v.x * k + v.z, v.y * v.y * k + c, v.y * v.z * k - v.x],
        [v.z * v.x * k - v.y, v.z * v.y * k + v.x, v.z * v.z * k + c],
    ]
}

fn rotate(m: &[[f32; 3]; 3], p: Vec3) -> Vec3 {
    Vec3::new(
        m[0][0] * p.x + m[0][1] * p.y + m[0][2] * p.z,
        m[1][0] * p.x + m[1][1] * p.y + m[1][2] * p.z,
        m[2][0] * p.x + m[2][1] * p.y + m[2][2] * p.z,
    )
}

fn lerp(a: Vec3, b: Vec3, w: f32) -> Vec3 {
    a * (1.0 - w) + b * w
}

/// A leaf card bound to the segment nearest its centre, so it rides that
/// segment as the branches blend.
struct Card {
    verts: Vec<u32>,
    seg: usize,
    t: f32,
    offset: Vec3,
    centre: Vec3,
}

fn bind_cards(leaves: &MeshData, per_card: usize, segs: &[BranchSegment]) -> Vec<Card> {
    leaves
        .indices
        .chunks(per_card)
        .map(|chunk| {
            let mut verts: Vec<u32> = chunk.to_vec();
            verts.sort_unstable();
            verts.dedup();
            let centre = verts
                .iter()
                .map(|&v| Vec3::from_array(leaves.vertices[v as usize].position))
                .fold(Vec3::ZERO, |a, b| a + b)
                * (1.0 / verts.len() as f32);
            let (seg, t, d) = segs
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let ab = s.end - s.start;
                    let t = ((centre - s.start).dot(&ab) / ab.dot(&ab).max(1e-9)).clamp(0.0, 1.0);
                    (i, t, (centre - (s.start + ab * t)).length())
                })
                .min_by(|a, b| a.2.total_cmp(&b.2))
                .unwrap_or((0, 0.0, 0.0));
            let _ = d;
            let s = &segs[seg];
            let at = s.start + (s.end - s.start) * t;
            Card {
                verts,
                seg,
                t,
                offset: centre - at,
                centre,
            }
        })
        .collect()
}

fn frames(args: &[String]) -> Res<()> {
    let path = args.first().ok_or("frames needs a spec")?;
    let out = std::path::PathBuf::from(opt(args, "--out").ok_or("frames needs --out DIR")?);
    let lineage: u64 = opt(args, "--lineage").unwrap_or("7").parse()?;
    let poses: usize = opt(args, "--poses").unwrap_or("8").parse()?;
    let step: f32 = opt(args, "--step").unwrap_or("0.5").parse()?;
    let stagger: f32 = opt(args, "--stagger").unwrap_or("0.15").parse()?;
    let fps: f32 = opt(args, "--fps").unwrap_or("12").parse()?;
    let nframes: usize = opt(args, "--frames").unwrap_or("24").parse()?;
    let glides: Vec<f32> = opt(args, "--glide")
        .unwrap_or("0.3")
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    std::fs::create_dir_all(&out)?;

    let spec = load(path, &sets(args))?;
    let tp = TrunkParams::from_toml(&spec.params)?;
    let bp = BranchParams::from_toml(&spec.params)?;
    let lp = LeafParams::from_toml(&spec.params)?;
    let mut root = SeededRng::new(lineage);

    // Lineage: the trunk, shared by every pose.
    let trunk = generate_trunk(&tp, &mut root.fork("trunk"))?;
    let spine = TrunkSpine {
        points: trunk.spine.clone(),
        radius_base: tp.radius_base,
        radius_top: tp.radius_top,
    };

    // Poses: branches regrown from each pose's own fork of the lineage.
    let mut pose_segs = Vec::with_capacity(poses);
    for k in 0..poses {
        let b = generate_branches(&bp, &spine, &mut root.fork(&format!("pose{k}")))?;
        pose_segs.push(b.segments);
    }
    let sig = signature(&pose_segs[0]);
    for (k, s) in pose_segs.iter().enumerate() {
        if signature(s) != sig {
            return Err(format!(
                "pose {k} has a different structure ({} segments vs {}); can't blend",
                s.len(),
                sig.len()
            )
            .into());
        }
    }
    let links = parents(&pose_segs[0]);

    // Variance: pull each pose toward pose 0 (the lineage's base) so only
    // `amount` of the difference is left, less on the trunk-side limbs and
    // more toward the twigs (`steady` 0 = even, 1 = all at the tips).
    let amount: f32 = opt(args, "--amount").unwrap_or("1").parse()?;
    let steady: f32 = opt(args, "--steady").unwrap_or("0").parse()?;
    let max_depth = pose_segs[0]
        .iter()
        .map(|s| s.depth)
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let base = pose_segs[0].clone();
    for segs in pose_segs.iter_mut().skip(1) {
        let mut ends: Vec<Vec3> = Vec::with_capacity(segs.len());
        for (i, link) in links.iter().enumerate() {
            let (b, s) = (&base[i], &segs[i]);
            let w = amount * ((1.0 - steady) + steady * (b.depth as f32 / max_depth).powf(1.3));
            let (vb, vs) = (b.end - b.start, s.end - s.start);
            let len = vb.length() * (1.0 - w) + vs.length() * w;
            let dir = lerp(vb.normalized(), vs.normalized(), w).normalized();
            let start = match link {
                Some(p) => ends[*p],
                None => lerp(b.start, s.start, w),
            };
            ends.push(start + dir * len);
            let s = &mut segs[i];
            s.radius_start = b.radius_start * (1.0 - w) + s.radius_start * w;
            s.radius_end = b.radius_end * (1.0 - w) + s.radius_end * w;
            s.start = start;
            s.end = start + dir * len;
        }
    }

    // Lineage: the leaves, placed on pose 0 and bound to its segments.
    let p0 = generate_branches(&bp, &spine, &mut root.fork("pose0"))?;
    let leaves = generate_leaves(
        &p0.tip_positions,
        &p0.tip_directions,
        &p0.segments,
        &lp,
        &mut root.fork("leaves"),
    )?;
    let cards = bind_cards(&leaves, card_index_count(lp.style).max(1), &pose_segs[0]);
    eprintln!(
        "lineage {lineage}: {} segments ({}), {} leaf cards, {poses} poses",
        sig.len(),
        depth_histogram(&pose_segs[0]),
        cards.len()
    );

    for (gi, &glide) in glides.iter().enumerate() {
        for f in 0..nframes {
            let t = f as f32 / fps;
            let mut blended: Vec<BranchSegment> = Vec::with_capacity(sig.len());
            for (i, link) in links.iter().enumerate() {
                let depth = pose_segs[0][i].depth as f32;
                let (n, w) = warped(t + stagger * step * depth, step, glide);
                let a = &pose_segs[n.rem_euclid(poses as i64) as usize][i];
                let b = &pose_segs[(n + 1).rem_euclid(poses as i64) as usize][i];
                let (va, vb) = (a.end - a.start, b.end - b.start);
                let len = va.length() * (1.0 - w) + vb.length() * w;
                let dir = lerp(va.normalized(), vb.normalized(), w).normalized();
                let start = match link {
                    Some(p) => blended[*p].end,
                    None => lerp(a.start, b.start, w),
                };
                blended.push(BranchSegment {
                    start,
                    end: start + dir * len,
                    radius_start: a.radius_start * (1.0 - w) + b.radius_start * w,
                    radius_end: a.radius_end * (1.0 - w) + b.radius_end * w,
                    depth: a.depth,
                    id: 0,
                    parent: None,
                });
            }

            let mut builder = MeshBuilder::new();
            builder.merge(&trunk.mesh);
            builder.merge(&segments_to_mesh(&blended, bp.radial_segments)?);
            builder.compute_normals();
            builder.compute_tangents();
            let mut bark = builder.build();
            bark.materials = trunk.mesh.materials.clone();

            let mut leaf = leaves.clone();
            for c in &cards {
                let (s0, s1) = (&pose_segs[0][c.seg], &blended[c.seg]);
                let r = rotation_between(s0.end - s0.start, s1.end - s1.start);
                let at = s1.start + (s1.end - s1.start) * c.t;
                let centre = at + rotate(&r, c.offset);
                for &v in &c.verts {
                    let vx = &mut leaf.vertices[v as usize];
                    let local = Vec3::from_array(vx.position) - c.centre;
                    vx.position = (centre + rotate(&r, local)).to_array();
                    vx.normal = rotate(&r, Vec3::from_array(vx.normal)).to_array();
                }
            }
            leaf.recompute_bounds();

            let mesh = join_bark_and_leaves(&bark, &leaf);
            let name = if glides.len() > 1 {
                format!("g{gi}_f{f:03}.glb")
            } else {
                format!("f{f:03}.glb")
            };
            std::fs::write(out.join(name), mesh_to_glb(&mesh)?)?;
        }
    }
    Ok(())
}

// ─── Noise poses (#252) ─────────────────────────────────────────────────────

mod noise {
    use super::{opt, Res};
    use flint_core::quat::quat_from_axis_angle;
    use flint_core::Vec3;
    use flint_procgen::algorithms::mesh_builder::MeshBuilder;
    use flint_procgen::generators::tree::branches::{extract_tips, segments_to_mesh};
    use flint_procgen::generators::tree::leaves::card_index_count;
    use flint_procgen::generators::tree::{
        generate_leaves, generate_tree, generate_trunk, join_bark_and_leaves, BranchParams,
        LeafParams, TrunkParams,
    };
    use flint_procgen::{
        mesh_to_glb, BranchSegment, LeafBinding, LivingClock, LivingGenerator, LivingPose,
        LivingRig, LivingSkeleton, MeshData, PoseStream, ProcGenError, ProcGenSpec, SeededRng,
        SegmentPose, Stagger, TreeGenerator,
    };

    /// Plane waves summed per channel. Four is enough to stop the field
    /// reading as one stripe, and few enough to stay smooth.
    const WAVES: usize = 4;

    /// The knobs of a noise pose.
    #[derive(Clone, Copy)]
    struct Noise {
        /// Degrees a segment turns at one standard deviation of the noise.
        turn: f32,
        /// Metres between swells: well above a limb's length and the limb
        /// turns whole, near it and the limb bends along its length.
        wavelength: f32,
        /// How much a segment's length can change, at one standard deviation.
        stretch: f32,
    }

    /// A tree that lives by noise instead of regrowing. Its structure is
    /// exactly what `generate` bakes from the lineage seed, whatever the
    /// branch method, since a pose never asks the generator again.
    struct NoiseTree(Noise);

    impl LivingGenerator for NoiseTree {
        fn check_living(&self, _spec: &ProcGenSpec) -> flint_procgen::Result<()> {
            Ok(())
        }

        fn structure(
            &self,
            spec: &ProcGenSpec,
            lineage_seed: u64,
        ) -> flint_procgen::Result<LivingSkeleton> {
            let trunk = TrunkParams::from_toml(&spec.params)?;
            let branch = BranchParams::from_toml(&spec.params)?;
            let leaf = LeafParams::from_toml(&spec.params)?;
            // `generate_tree` takes the "trunk" and "branches" forks, as
            // `generate` does, and links the segments.
            let mut lineage = SeededRng::new(lineage_seed);
            let tree = generate_tree(&trunk, &branch, &mut lineage)?;
            let segments = tree.branch_segments;
            let (tips, directions) = extract_tips(&segments);
            let leaf_mesh = generate_leaves(
                &tips,
                &directions,
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

        fn rig(&self, _spec: &ProcGenSpec, _lineage_seed: u64) -> flint_procgen::Result<LivingRig> {
            Err(ProcGenError::LivingUnsupported(
                "the noise pose is an offline prototype: no skinned rig".into(),
            ))
        }

        fn full_pose(
            &self,
            _spec: &ProcGenSpec,
            skeleton: &LivingSkeleton,
            shape_seed: u64,
        ) -> flint_procgen::Result<LivingPose> {
            let field = Field::new(skeleton.lineage_seed, shape_seed, self.0.wavelength);
            let turn = self.0.turn.to_radians();
            let segments = skeleton
                .segments
                .iter()
                .map(|s| {
                    let mid = (s.start + s.end) * 0.5;
                    let dir = s.direction();
                    if dir.length() < 0.5 {
                        return SegmentPose::IDENTITY;
                    }
                    // A bend, never a twist: a tube turned about its own axis
                    // only spins its leaves.
                    let raw = Vec3::new(field.at(0, mid), field.at(1, mid), field.at(2, mid));
                    let bend = (raw - dir * raw.dot(&dir)) * turn;
                    let angle = bend.length();
                    let rotation = if angle < 1e-6 {
                        SegmentPose::IDENTITY.rotation
                    } else {
                        quat_from_axis_angle((bend * (1.0 / angle)).to_array(), angle)
                    };
                    SegmentPose {
                        rotation,
                        length_scale: (1.0 + self.0.stretch * field.at(3, mid)).clamp(0.5, 1.5),
                        radius_scale: 1.0,
                    }
                })
                .collect();
            Ok(LivingPose {
                shape_seed,
                segments,
            })
        }

        fn posed_mesh(
            &self,
            spec: &ProcGenSpec,
            skeleton: &LivingSkeleton,
            pose: &LivingPose,
        ) -> flint_procgen::Result<MeshData> {
            posed_tree(spec, skeleton, pose)
        }
    }

    /// The tree's own `posed_mesh`: the lineage's trunk, the posed branches
    /// and the posed leaf cards.
    fn posed_tree(
        spec: &ProcGenSpec,
        skeleton: &LivingSkeleton,
        pose: &LivingPose,
    ) -> flint_procgen::Result<MeshData> {
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

    /// Four channels of smooth noise with unit variance, each a sum of plane
    /// waves whose directions, frequencies and phases come from the shape
    /// seed's own fork of the lineage. One pose, one field: the next pose is a
    /// new field, not the same one moved on, so poses don't drift one way.
    struct Field {
        waves: [[(Vec3, f32); WAVES]; 4],
    }

    impl Field {
        fn new(lineage_seed: u64, shape_seed: u64, wavelength: f32) -> Self {
            let mut rng = SeededRng::new(lineage_seed).fork(&format!("noise{shape_seed}"));
            let k = std::f32::consts::TAU / wavelength.max(0.05);
            let waves = std::array::from_fn(|_| {
                std::array::from_fn(|_| {
                    let d = Vec3::from_array(rng.next_vec3_unit());
                    let f = k * rng.next_range(0.7, 1.3);
                    (d * f, rng.next_range(0.0, std::f32::consts::TAU))
                })
            });
            Self { waves }
        }

        fn at(&self, channel: usize, p: Vec3) -> f32 {
            let sum: f32 = self.waves[channel]
                .iter()
                .map(|(w, phase)| (w.dot(&p) + phase).sin())
                .sum();
            // A sine has variance one half, so the sum's is WAVES / 2.
            sum / (WAVES as f32 * 0.5).sqrt()
        }
    }

    /// The tree's own card binding: each card rides the segment nearest it.
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

    pub fn run(args: &[String]) -> Res<()> {
        let path = args.first().ok_or("noise needs a spec")?;
        let out = std::path::PathBuf::from(opt(args, "--out").ok_or("noise needs --out DIR")?);
        let source = opt(args, "--source").unwrap_or("noise");
        let fps: f64 = opt(args, "--fps").unwrap_or("12").parse()?;
        let nframes: usize = opt(args, "--frames").unwrap_or("48").parse()?;
        let held: u64 = opt(args, "--poses").unwrap_or("0").parse()?;
        let noise = Noise {
            turn: opt(args, "--turn").unwrap_or("12").parse()?,
            wavelength: opt(args, "--wavelength").unwrap_or("3").parse()?,
            stretch: opt(args, "--stretch").unwrap_or("0.08").parse()?,
        };
        std::fs::create_dir_all(&out)?;

        // A spec with no [living] table gets one from the lineage alone, so
        // the defaults are the engine's.
        let mut text = std::fs::read_to_string(path)?;
        let lineage: u64 = opt(args, "--lineage").unwrap_or("7").parse()?;
        if !text.contains("\n[living]") {
            text.push_str(&format!("\n[living]\nlineage_seed = {lineage}\n"));
        }
        let mut spec = ProcGenSpec::parse(&text)?;
        spec.lod = None;
        let living = spec.living.as_mut().ok_or("no [living] table")?;
        living.lineage_seed = lineage;
        if let Some(a) = opt(args, "--amount") {
            living.variance.amount = a.parse()?;
        }
        if let Some(s) = opt(args, "--steady") {
            living.variance.steady = s.parse()?;
        }
        if let Some(s) = opt(args, "--step") {
            living.step = s.parse()?;
        }
        if let Some(g) = opt(args, "--glide") {
            living.glide = g.parse()?;
        }
        // Space colonization's depth counts growth steps along a limb, not
        // branch levels, so per-level stagger spreads a pine over many beats.
        if let Some(s) = opt(args, "--stagger") {
            living.stagger = match s {
                "none" => Stagger::None,
                "per_level" => Stagger::PerLevel,
                "per_branch" => Stagger::PerBranch,
                other => {
                    return Err(format!("--stagger: none|per_level|per_branch, not {other}").into())
                }
            };
        }
        let clock = LivingClock::from(&*living);

        let noisy = NoiseTree(noise);
        let generator: &dyn LivingGenerator = match source {
            "noise" => &noisy,
            "regrow" => &TreeGenerator,
            other => return Err(format!("--source is noise or regrow, not {other}").into()),
        };
        let started = std::time::Instant::now();
        let skeleton = generator.structure(&spec, lineage)?;
        let mut depths = std::collections::BTreeMap::<u32, usize>::new();
        for s in &skeleton.segments {
            *depths.entry(s.depth).or_default() += 1;
        }
        eprintln!(
            "{source}: lineage {lineage}, {} segments (depths {}..={}, {} levels), {} leaf cards, \
             step {} glide {} variance {:.2}/{:.2}",
            skeleton.segments.len(),
            depths.keys().next().unwrap_or(&0),
            skeleton.max_depth(),
            depths.len(),
            skeleton.leaf_bindings.len(),
            clock.step,
            clock.glide,
            spec.living.as_ref().map_or(0.0, |l| l.variance.amount),
            spec.living.as_ref().map_or(0.0, |l| l.variance.steady),
        );

        for k in 0..held {
            let pose = generator.pose(&spec, &skeleton, k)?;
            let mesh = generator.posed_mesh(&spec, &skeleton, &pose)?;
            std::fs::write(out.join(format!("pose{k}.glb")), mesh_to_glb(&mesh)?)?;
        }

        let mut stream = PoseStream::new(generator, &spec, &skeleton, clock);
        let mut tip_moves = Vec::with_capacity(nframes);
        let mut last: Option<Vec<BranchSegment>> = None;
        for f in 0..nframes {
            let pose = stream.at(f as f64 / fps)?;
            let posed = skeleton.posed_segments(&pose)?;
            if let Some(prev) = &last {
                let moved = posed
                    .iter()
                    .zip(prev)
                    .map(|(a, b)| (a.end - b.end).length())
                    .fold(0.0f32, f32::max);
                tip_moves.push(moved);
            }
            last = Some(posed);
            let mesh = generator.posed_mesh(&spec, &skeleton, &pose)?;
            std::fs::write(out.join(format!("f{f:03}.glb")), mesh_to_glb(&mesh)?)?;
        }
        let pattern: String = tip_moves
            .iter()
            .map(|&m| if m > 1e-4 { '#' } else { '.' })
            .collect();
        let most = tip_moves.iter().copied().fold(0.0f32, f32::max);
        eprintln!(
            "{nframes} frames in {:.1} s; the farthest any segment end moves in a frame: {most:.3} m\n{pattern}",
            started.elapsed().as_secs_f32()
        );
        Ok(())
    }
}
