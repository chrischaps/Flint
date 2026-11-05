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
        _ => {
            eprintln!("usage: living_spike counts|drift|frames ... (see the file's header)");
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
