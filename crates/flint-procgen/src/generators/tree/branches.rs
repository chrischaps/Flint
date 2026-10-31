//! Branch generation — wires L-system and space colonization algorithms
//! to produce branch geometry attached along a trunk.
//!
//! Both methods give a list of [`BranchSegment`]s. Segments that carry on
//! from one another are joined into limbs, and each limb is swept as one
//! tube, so a bend shares a ring instead of leaving a wedge-shaped crack.

use std::collections::HashMap;
use std::f32::consts::TAU;

use crate::algorithms::lsystem::{stochastic_tree_lsystem, Grammar, LSystemParams, TurtleConfig};
use crate::algorithms::mesh_builder::extrude::{
    build_spine_from_positions, extrude_along_curve, ExtrudeParams,
};
use crate::algorithms::mesh_builder::MeshBuilder;
use crate::algorithms::space_colonization::{
    scatter_attraction_points, Colonization, CrownVolume, SpaceColonization,
    SpaceColonizationParams,
};
use crate::types::{BranchSegment, MaterialData, MeshData};
use crate::{ProcGenError, Result, SeededRng};
use flint_core::spline::rotate_around_axis;
use flint_core::Vec3;

use super::{BranchMethod, BranchOutput, BranchParams, CrownShape};

// ─── Trunk Spine ────────────────────────────────────────────────────────────

/// The trunk as the branches see it: its centreline and its taper.
#[derive(Debug, Clone)]
pub struct TrunkSpine {
    /// Ring centres from root to tip, at least two.
    pub points: Vec<Vec3>,
    /// Radius at the root.
    pub radius_base: f32,
    /// Radius at the tip.
    pub radius_top: f32,
}

impl TrunkSpine {
    /// A straight trunk `height` long from `base` along `dir`.
    pub fn straight(base: Vec3, dir: Vec3, height: f32, radius_base: f32, radius_top: f32) -> Self {
        Self {
            points: vec![base, base + dir.normalized() * height],
            radius_base,
            radius_top,
        }
    }

    /// Position, heading and radius `t` of the way up (0 = root, 1 = tip).
    ///
    /// `t` counts rings, the way the trunk's extrusion tapers its radius, so
    /// the radius here is the radius of the trunk's surface at that point.
    pub fn at(&self, t: f32) -> (Vec3, Vec3, f32) {
        let n = self.points.len() - 1;
        let x = t.clamp(0.0, 1.0) * n as f32;
        let i = (x.floor() as usize).min(n - 1);
        let (a, b) = (self.points[i], self.points[i + 1]);
        let along = b - a;
        let dir = if along.length() > 1e-6 {
            along.normalized()
        } else {
            Vec3::UP
        };
        let t = t.clamp(0.0, 1.0);
        let radius = self.radius_base * (1.0 - t) + self.radius_top * t;
        (a + along * (x - i as f32), dir, radius)
    }

    /// The tip's position.
    pub fn tip(&self) -> Vec3 {
        *self.points.last().expect("a trunk spine has points")
    }

    /// Length of the centreline.
    pub fn length(&self) -> f32 {
        self.points.windows(2).map(|w| (w[1] - w[0]).length()).sum()
    }

    /// Distance from `p` to the centreline.
    pub fn distance_to(&self, p: Vec3) -> f32 {
        self.points
            .windows(2)
            .map(|w| {
                let ab = w[1] - w[0];
                let len_sq = ab.dot(&ab);
                let s = if len_sq > 1e-12 {
                    ((p - w[0]).dot(&ab) / len_sq).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (p - (w[0] + ab * s)).length()
            })
            .fold(f32::INFINITY, f32::min)
    }
}

// ─── Public API ─────────────────────────────────────────────────────────────

/// Generate branches from parameters and the trunk they grow from.
///
/// Dispatches to the appropriate algorithm (L-system or space colonization),
/// converts the resulting segments to mesh geometry, and extracts terminal
/// tip positions for leaf placement. The built-in L-system rule and space
/// colonization attach limbs along the trunk from `branch_start` up; a text
/// grammar grows from the tip, since its own trunk carries on from there.
pub fn generate_branches(
    params: &BranchParams,
    trunk: &TrunkSpine,
    rng: &mut SeededRng,
) -> Result<BranchOutput> {
    if trunk.points.len() < 2 {
        return Err(ProcGenError::InvalidParameter {
            name: "trunk".into(),
            reason: "branches need a trunk spine of at least two points".into(),
        });
    }

    let segments = match params.method {
        BranchMethod::LSystem => generate_lsystem_branches(params, trunk, rng)?,
        BranchMethod::SpaceColonization => generate_colonization_branches(params, trunk, rng)?,
    };

    if segments.is_empty() {
        // Return empty but valid output
        let empty_mesh = MeshBuilder::new().build();
        return Ok(BranchOutput {
            mesh: empty_mesh,
            tip_positions: Vec::new(),
            tip_directions: Vec::new(),
            segment_count: 0,
            segments: Vec::new(),
        });
    }

    let (tip_positions, tip_directions) = extract_tips(&segments);
    let mesh = segments_to_mesh(&segments, params.radial_segments)?;
    let segment_count = segments.len();

    Ok(BranchOutput {
        mesh,
        tip_positions,
        tip_directions,
        segment_count,
        segments,
    })
}

// ─── L-System Path ──────────────────────────────────────────────────────────

/// Generate branch segments using the L-system algorithm.
///
/// 1. Builds an L-system with stochastic angle variation, heading +Y
/// 2. Drops its depth-0 "trunk" (we have the real one) and scales the rest
///    so the crown reaches `crown_radius`
/// 3. Splits the crown into scaffold limbs, the subtrees that left the
///    L-system's trunk, and attaches them up the real trunk from
///    `branch_start` to the tip: the biggest lowest, as on a grown tree
fn generate_lsystem_branches(
    params: &BranchParams,
    trunk: &TrunkSpine,
    rng: &mut SeededRng,
) -> Result<Vec<BranchSegment>> {
    let (trunk_tip, trunk_dir, radius_top) = trunk.at(1.0);
    if let Some(grammar) = &params.lsystem_grammar {
        return generate_grammar_branches(params, grammar, trunk_tip, trunk_dir, radius_top, rng);
    }

    let base_angle = (params.branch_angle_min + params.branch_angle_max) / 2.0;
    let variation = (params.branch_angle_max - params.branch_angle_min) / 2.0;
    let angle_variation = variation + params.lsystem_angle_variation;

    // The depth-0 "internal trunk" segment is removed after generation, so the
    // first visible branches are depth-1 with radius = initial_radius * radius_decay.
    // Set initial_radius so that depth-1 branches match the real trunk's top radius.
    // Lengths are in units of the first step and scaled to the crown below.
    let initial_radius = radius_top / params.branch_radius_falloff.max(0.01);

    let lsystem_params = LSystemParams {
        default_angle_deg: base_angle,
        length_decay: params.branch_length_falloff,
        radius_decay: params.branch_radius_falloff,
        max_depth: params.lsystem_iterations,
        max_segments: params.max_segments,
    };

    let lsystem = stochastic_tree_lsystem(
        1.0,
        initial_radius,
        base_angle,
        angle_variation,
        lsystem_params,
    )?;

    let mut lsystem_rng = rng.fork("lsystem");
    let mut segments = lsystem.generate_segments(&mut lsystem_rng)?;

    // The L-system axiom starts with a depth-0 Forward segment (the "internal
    // trunk"); every scaffold limb forks from its end. Re-root there, then
    // drop it: the real trunk takes its place.
    let branch_origin = segments
        .iter()
        .find(|seg| seg.depth == 0)
        .map(|seg| seg.end)
        .unwrap_or(Vec3::ZERO);
    for seg in segments.iter_mut() {
        seg.start = seg.start - branch_origin;
        seg.end = seg.end - branch_origin;
    }
    segments.retain(|seg| seg.depth > 0);

    // Before, the crown's size came from the trunk's top radius, which left a
    // one-metre tuft on most trees. Its reach is the crown radius now.
    let reach = segments
        .iter()
        .map(|seg| seg.end.length())
        .fold(0.0f32, f32::max);
    if reach > 1e-6 {
        let scale = params.crown_radius / reach;
        for seg in segments.iter_mut() {
            seg.start = seg.start * scale;
            seg.end = seg.end * scale;
        }
    }

    let mut scaffolds = scaffold_limbs(&segments);
    // Biggest first (stable, so ties keep the L-system's order), and the
    // first goes lowest.
    scaffolds.sort_by_key(|limb| std::cmp::Reverse(limb.len()));

    let n = scaffolds.len();
    let mut placed = Vec::with_capacity(segments.len());
    for (k, limb) in scaffolds.iter().enumerate() {
        let t = if n > 1 {
            params.branch_start + (1.0 - params.branch_start) * k as f32 / (n - 1) as f32
        } else {
            1.0
        };
        let (at, dir, radius) = trunk.at(t);
        // A lower limb leaves a thicker part of the trunk and carries more
        // crown, so it starts thicker too, though thinner than the trunk.
        let thicken = (radius / radius_top.max(1e-6)).sqrt();
        let cap = limb_cap(radius, t >= 1.0);
        for &i in limb {
            let seg = &segments[i];
            placed.push(BranchSegment {
                start: rotate_up_to(seg.start, dir) + at,
                end: rotate_up_to(seg.end, dir) + at,
                radius_start: (seg.radius_start * thicken).min(cap),
                radius_end: (seg.radius_end * thicken).min(cap),
                depth: seg.depth,
            });
        }
    }

    sleeve_the_tip(&mut placed, trunk);
    Ok(placed)
}

/// How thick a limb may be where it leaves a trunk `radius` thick. The limb
/// that carries on from the very tip may match the trunk; a side limb as
/// thick as the trunk reads as a second trunk, so it is held to 70%.
fn limb_cap(radius: f32, from_tip: bool) -> f32 {
    if from_tip {
        radius
    } else {
        radius * 0.7
    }
}

/// Give the limb that leaves the trunk's tip a short sleeve back down the
/// trunk, so its first joint is the tip itself. The joint's ring is then
/// mitred between the trunk's heading and the limb's, instead of the limb
/// starting square on the trunk's cap and leaving its rim standing proud.
///
/// The sleeve is a hair thinner than the trunk, so inside it it never shows.
fn sleeve_the_tip(segments: &mut Vec<BranchSegment>, trunk: &TrunkSpine) {
    let (tip, dir, radius) = trunk.at(1.0);
    // The thickest limb at the tip is the one the sleeve will carry on into.
    let Some(leaving) = segments
        .iter()
        .filter(|seg| (seg.start - tip).length() < 1e-4 && seg.length() > 1e-6)
        .max_by(|a, b| a.radius_start.total_cmp(&b.radius_start))
    else {
        return;
    };
    let n = trunk.points.len();
    let last = (trunk.points[n - 1] - trunk.points[n - 2]).length();
    let length = (radius * 3.0).min(last * 0.9);
    if length < 1e-4 {
        return;
    }
    let r = radius.min(leaving.radius_start) * 0.97;
    // The ring at a bend is tilted by half the turn, so it must be wider by
    // 1 / cos(half) to keep the tube's thickness through it; held to 1.25,
    // where it starts to read as a collar.
    let half = dir.dot(&leaving.direction()).clamp(-1.0, 1.0).acos() * 0.5;
    let joint = r / half.cos().max(0.8);
    segments.insert(
        0,
        BranchSegment {
            start: tip - dir * length,
            end: tip,
            radius_start: r,
            radius_end: joint,
            depth: 1,
        },
    );
}

/// Group a crown's segments into the subtrees that start at the origin, each
/// listed root first. Segments that hang from no root are dropped.
fn scaffold_limbs(segments: &[BranchSegment]) -> Vec<Vec<usize>> {
    let mut starting: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
    for (i, seg) in segments.iter().enumerate() {
        starting
            .entry(quantize_position(seg.start))
            .or_default()
            .push(i);
    }

    let origin = quantize_position(Vec3::ZERO);
    let mut taken = vec![false; segments.len()];
    let mut limbs = Vec::new();
    for &root in starting.get(&origin).into_iter().flatten() {
        if taken[root] {
            continue;
        }
        taken[root] = true;
        let mut limb = vec![root];
        let mut next = 0;
        while next < limb.len() {
            let end = quantize_position(segments[limb[next]].end);
            next += 1;
            if end == origin {
                continue;
            }
            for &child in starting.get(&end).into_iter().flatten() {
                if !taken[child] {
                    taken[child] = true;
                    limb.push(child);
                }
            }
        }
        limbs.push(limb);
    }
    limbs
}

/// Grow the crown from a text grammar.
///
/// Unlike the built-in rule, every segment is kept: the grammar's depth-0
/// segments carry the trunk on from its tip, at the trunk's top radius.
/// Without `lsystem_step` the crown is drawn with unit steps and then scaled
/// so its farthest point is `lsystem_size` from the trunk tip; radii are
/// left alone, so a bigger crown doesn't get fatter branches.
fn generate_grammar_branches(
    params: &BranchParams,
    grammar: &Grammar,
    trunk_tip: Vec3,
    trunk_dir: Vec3,
    trunk_radius_top: f32,
    rng: &mut SeededRng,
) -> Result<Vec<BranchSegment>> {
    let angle = params
        .lsystem_angle
        .unwrap_or((params.branch_angle_min + params.branch_angle_max) / 2.0);
    let config = TurtleConfig {
        step_length: params.lsystem_step.unwrap_or(1.0),
        radius: trunk_radius_top,
        length_decay: params.branch_length_falloff,
        radius_decay: params.branch_radius_falloff,
        tropism_strength: params.lsystem_tropism,
        max_segments: params.max_segments,
        ..TurtleConfig::default().with_angle(angle)
    };

    let mut grammar_rng = rng.fork("lsystem");
    let mut segments = grammar.generate(
        params.lsystem_iterations as usize,
        &config,
        &mut grammar_rng,
    )?;

    if params.lsystem_step.is_none() {
        let reach = segments
            .iter()
            .map(|seg| seg.end.length())
            .fold(0.0f32, f32::max);
        if reach > 1e-6 {
            let scale = params.lsystem_size / reach;
            for seg in segments.iter_mut() {
                seg.start = seg.start * scale;
                seg.end = seg.end * scale;
            }
        }
    }

    transform_segments(&mut segments, trunk_tip, trunk_dir);
    Ok(segments)
}

// ─── Space Colonization Path ────────────────────────────────────────────────

/// Generate branch segments using the space colonization algorithm.
///
/// The trunk from `branch_start` to its tip is handed to the algorithm as a
/// chain of nodes, so limbs sprout wherever the crown's points reach the
/// trunk, not only from its tip. The chain itself is left out of the
/// segments, since the real trunk is already there.
fn generate_colonization_branches(
    params: &BranchParams,
    trunk: &TrunkSpine,
    rng: &mut SeededRng,
) -> Result<Vec<BranchSegment>> {
    let crown = colonize_crown(params, trunk, rng)?;
    let nodes = &crown.grown.nodes;
    let chain = crown.chain_radii.len();

    // Each node's attachment: the chain node its limb left the trunk from.
    let top = chain - 1;
    let mut attach = vec![0usize; nodes.len()];
    let mut segments = Vec::with_capacity(nodes.len().saturating_sub(chain));
    for (i, node) in nodes.iter().enumerate().skip(chain) {
        let parent = node.parent.expect("only the root has no parent");
        attach[i] = if parent < chain {
            parent
        } else {
            attach[parent]
        };
        // The pipe model sizes limbs from the crown alone; never let one
        // outgrow the trunk where it leaves it.
        let cap = limb_cap(crown.chain_radii[attach[i]], attach[i] == top);
        segments.push(BranchSegment {
            start: nodes[parent].position,
            end: node.position,
            radius_start: nodes[parent].radius.min(cap),
            radius_end: node.radius.min(cap),
            depth: node.depth.max(1),
        });
    }
    sleeve_the_tip(&mut segments, trunk);
    Ok(segments)
}

/// A colonized crown, with the trunk chain it grew from.
struct GrownCrown {
    /// The whole graph; nodes `0..chain_radii.len()` are the trunk chain.
    grown: Colonization,
    /// The real trunk's radius at each chain node.
    chain_radii: Vec<f32>,
}

/// Run space colonization from the upper trunk toward the crown volume.
fn colonize_crown(
    params: &BranchParams,
    trunk: &TrunkSpine,
    rng: &mut SeededRng,
) -> Result<GrownCrown> {
    let (_, trunk_dir, radius_top) = trunk.at(1.0);

    // The crown's floor is where branching starts, so the volume wraps the
    // upper trunk and its points draw limbs from the trunk's side. (With
    // `branch_start = 1` it sits on the tip.)
    let (floor, _, _) = trunk.at(params.branch_start);
    let crown = match params.crown_shape {
        CrownShape::Sphere => CrownVolume::Sphere {
            center: floor + trunk_dir * params.crown_radius,
            radius: params.crown_radius,
        },
        CrownShape::Ellipsoid => {
            let height = params.crown_radius * params.crown_height_ratio;
            CrownVolume::Ellipsoid {
                center: floor + trunk_dir * height,
                radii: Vec3::new(params.crown_radius, height, params.crown_radius),
            }
        }
        CrownShape::Hemisphere => CrownVolume::Hemisphere {
            center: floor,
            radius: params.crown_radius,
        },
        CrownShape::Cone => CrownVolume::Cone {
            base: floor,
            radius: params.crown_radius,
            height: params.crown_radius * params.crown_height_ratio,
        },
        CrownShape::Box => {
            let r = params.crown_radius;
            let height = r * params.crown_height_ratio;
            CrownVolume::Box {
                min: floor + Vec3::new(-r, 0.0, -r),
                max: floor + Vec3::new(r, height, r),
            }
        }
    };

    // The trunk from branch_start up, a node every step: any of them can
    // sprout a limb.
    let start = params.branch_start;
    let steps = ((trunk.length() * (1.0 - start)) / params.step_size.max(1e-3))
        .ceil()
        .max(1.0) as usize;
    let (chain, chain_radii): (Vec<Vec3>, Vec<f32>) = (0..=steps)
        .map(|i| {
            let (pos, _, radius) = trunk.at(start + (1.0 - start) * i as f32 / steps as f32);
            (pos, radius)
        })
        .unzip();

    let sc_params = SpaceColonizationParams {
        crown,
        num_attraction_points: params.num_attraction_points,
        kill_distance: params.kill_distance,
        influence_distance: params.influence_distance,
        step_size: params.step_size,
        root_position: chain[0],
        trunk_steps: 0,
        max_iterations: 200,
        max_nodes: params.max_segments.saturating_add(chain.len() as u32),
        base_radius: chain_radii[0],
        tip_radius: radius_top * 0.05,
        pipe_exponent: params.pipe_exponent,
    };

    let gen = SpaceColonization::new(sc_params)?;
    let mut sc_rng = rng.fork("colonization");
    let points = scatter_attraction_points(
        &gen.params().crown,
        params.num_attraction_points,
        &mut sc_rng,
    );
    let grown = gen.colonize_from(&chain, &points)?;
    Ok(GrownCrown { grown, chain_radii })
}

// ─── Segment-to-Mesh Conversion ────────────────────────────────────────────

/// One unbroken run of segments, each carrying on from the last.
#[derive(Debug, Clone)]
struct Limb {
    points: Vec<Vec3>,
    radii: Vec<f32>,
}

/// Join segments into limbs. Where a segment ends, the child that carries on
/// most like it (thickest, then straightest) continues its limb; the other
/// children start limbs of their own, from inside it. Degenerate segments
/// (near-zero length) are skipped.
fn join_limbs(segments: &[BranchSegment]) -> Vec<Limb> {
    let live: Vec<usize> = (0..segments.len())
        .filter(|&i| segments[i].length() >= 1e-6)
        .collect();
    let mut starting: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
    for &i in &live {
        starting
            .entry(quantize_position(segments[i].start))
            .or_default()
            .push(i);
    }

    let mut next: Vec<Option<usize>> = vec![None; segments.len()];
    let mut continued = vec![false; segments.len()];
    for &i in &live {
        let seg = &segments[i];
        let dir = seg.direction();
        let best = starting
            .get(&quantize_position(seg.end))
            .into_iter()
            .flatten()
            .copied()
            .filter(|&c| c != i && !continued[c])
            .max_by(|&a, &b| {
                let (a, b) = (&segments[a], &segments[b]);
                a.radius_start
                    .total_cmp(&b.radius_start)
                    .then(dir.dot(&a.direction()).total_cmp(&dir.dot(&b.direction())))
            });
        if let Some(c) = best {
            next[i] = Some(c);
            continued[c] = true;
        }
    }

    let mut limbs = Vec::new();
    for &first in live.iter().filter(|&&i| !continued[i]) {
        let seg = &segments[first];
        let mut limb = Limb {
            points: vec![seg.start, seg.end],
            radii: vec![seg.radius_start, seg.radius_end],
        };
        let mut at = first;
        // A limb can't be longer than every segment; the bound guards loops.
        for _ in 0..live.len() {
            let Some(c) = next[at] else { break };
            limb.points.push(segments[c].end);
            limb.radii.push(segments[c].radius_end);
            at = c;
        }
        limbs.push(limb);
    }
    limbs
}

/// Sides around a limb `radius` thick, when the thickest gets `base` sides:
/// thin twigs drop to three, which is all a twig needs on screen.
fn radial_segments_for(radius: f32, thickest: f32, base: u32) -> u32 {
    let share = (radius / thickest.max(1e-6)).clamp(0.0, 1.0).sqrt();
    ((base as f32 * share).round() as u32).clamp(3, base.max(3))
}

/// A limb's last ring is at most this fraction of the one before.
const TIP_TAPER: f32 = 0.4;

/// Convert branch segments into a mesh, one tapered tube per limb.
///
/// Each limb's rings sit at its joints, so the bark is unbroken through a
/// bend, and the tip tapers and is capped. Thinner limbs get fewer sides. The bark
/// texture wraps once around a limb's base and repeats along it with square
/// texels, as it does on the trunk.
pub fn segments_to_mesh(segments: &[BranchSegment], base_radial_segments: u32) -> Result<MeshData> {
    let mut builder = MeshBuilder::new();
    let limbs = join_limbs(segments);
    let thickest = limbs.iter().map(|l| l.radii[0]).fold(0.0f32, f32::max);

    for limb in &limbs {
        let mut radii = limb.radii.clone();
        let n = radii.len();
        if n >= 2 {
            // Taper to the tip, rather than end on a sawn-off stub.
            radii[n - 1] = radii[n - 1].min(radii[n - 2] * TIP_TAPER);
        }
        let mut spine = build_spine_from_positions(&limb.points, 0.0, 0.0);
        for (point, &radius) in spine.iter_mut().zip(&radii) {
            point.radius = radius;
        }
        let length: f32 = limb.points.windows(2).map(|w| (w[1] - w[0]).length()).sum();
        let circumference = TAU * limb.radii[0].max(1e-4);

        // Caps close the tips. A limb's base cap sits inside the limb (or
        // trunk) it leaves, where nothing sees it.
        let tube = extrude_along_curve(&ExtrudeParams {
            spine,
            radial_segments: radial_segments_for(limb.radii[0], thickest, base_radial_segments),
            caps: true,
            uv_length_scale: (length / circumference).max(1.0),
        })?;

        builder.merge(&tube);
    }

    builder.compute_normals();
    builder.compute_tangents();

    // Set bark material
    let mut mesh = builder.build();
    mesh.materials = vec![MaterialData {
        name: "bark".into(),
        base_color: [0.34, 0.2, 0.1, 1.0],
        metallic: 0.0,
        roughness: 0.9,
        normal_map: None,
    }];

    Ok(mesh)
}

// ─── Tip Extraction ─────────────────────────────────────────────────────────

/// Identify terminal branch tips for leaf placement.
///
/// A segment is terminal if its end position is not the start of any other
/// segment. Returns (positions, directions) of all terminal endpoints.
pub fn extract_tips(segments: &[BranchSegment]) -> (Vec<Vec3>, Vec<Vec3>) {
    // Collect all start positions into a set for lookup
    let starts: std::collections::HashSet<[i32; 3]> = segments
        .iter()
        .map(|seg| quantize_position(seg.start))
        .collect();

    let mut positions = Vec::new();
    let mut directions = Vec::new();

    for seg in segments {
        let end_key = quantize_position(seg.end);
        if !starts.contains(&end_key) {
            positions.push(seg.end);
            let dir = seg.direction();
            directions.push(if dir.length() > 1e-6 { dir } else { Vec3::UP });
        }
    }

    (positions, directions)
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Quantize a Vec3 position to a grid for HashSet-based lookup.
/// Uses a resolution fine enough to distinguish separate segment endpoints.
fn quantize_position(v: Vec3) -> [i32; 3] {
    const SCALE: f32 = 10000.0;
    [
        (v.x * SCALE).round() as i32,
        (v.y * SCALE).round() as i32,
        (v.z * SCALE).round() as i32,
    ]
}

/// Transform segments from L-system space (origin, +Y heading) to world space
/// (trunk_tip position, trunk_dir heading).
fn transform_segments(segments: &mut [BranchSegment], trunk_tip: Vec3, trunk_dir: Vec3) {
    for seg in segments.iter_mut() {
        seg.start = rotate_up_to(seg.start, trunk_dir) + trunk_tip;
        seg.end = rotate_up_to(seg.end, trunk_dir) + trunk_tip;
    }
}

/// Rotate `v` by the rotation that turns +Y onto `dir` (Rodrigues).
fn rotate_up_to(v: Vec3, dir: Vec3) -> Vec3 {
    let to = dir.normalized();
    let dot = Vec3::UP.dot(&to).clamp(-1.0, 1.0);
    if (dot - 1.0).abs() < 1e-6 {
        v
    } else if (dot + 1.0).abs() < 1e-6 {
        // Anti-parallel: 180 degrees about X
        Vec3::new(v.x, -v.y, -v.z)
    } else {
        rotate_around_axis(v, Vec3::UP.cross(&to).normalized(), dot.acos())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_branch_params() -> BranchParams {
        BranchParams::default()
    }

    /// A straight trunk five metres tall, 0.3 at the root and 0.1 at the tip.
    fn test_trunk() -> TrunkSpine {
        TrunkSpine::straight(Vec3::ZERO, Vec3::UP, 5.0, 0.3, 0.1)
    }

    // ── TOML Parsing Tests ──────────────────────────────────────────────

    #[test]
    fn branch_params_from_toml_defaults() {
        let toml_val: toml::Value = toml::toml! {
            some_unrelated_field = true
        }
        .into();
        let p = BranchParams::from_toml(&toml_val).unwrap();
        assert_eq!(p.branch_levels, 3);
        assert_eq!(p.radial_segments, 6);
        assert!(matches!(p.method, BranchMethod::LSystem));
    }

    #[test]
    fn branch_params_from_toml_full() {
        let toml_val: toml::Value = toml::toml! {
            branch_method = "space_colonization"
            branch_levels = 4
            branch_angle_min = 15.0
            branch_angle_max = 45.0
            branch_length_falloff = 0.65
            branch_radius_falloff = 0.5
            branch_max_segments = 8000
            branch_radial_segments = 8
            crown_shape = "ellipsoid"
            crown_radius = 4.0
            crown_height_ratio = 2.0
            num_attraction_points = 800
            kill_distance = 0.3
            influence_distance = 5.0
            step_size = 0.25
            lsystem_iterations = 4
            lsystem_angle_variation = 15.0
        }
        .into();
        let p = BranchParams::from_toml(&toml_val).unwrap();
        assert!(matches!(p.method, BranchMethod::SpaceColonization));
        assert_eq!(p.branch_levels, 4);
        assert!((p.branch_angle_min - 15.0).abs() < 1e-5);
        assert!((p.branch_angle_max - 45.0).abs() < 1e-5);
        assert!((p.branch_length_falloff - 0.65).abs() < 1e-5);
        assert!((p.branch_radius_falloff - 0.5).abs() < 1e-5);
        assert_eq!(p.max_segments, 8000);
        assert_eq!(p.radial_segments, 8);
        assert!(matches!(p.crown_shape, CrownShape::Ellipsoid));
        assert!((p.crown_radius - 4.0).abs() < 1e-5);
        assert!((p.crown_height_ratio - 2.0).abs() < 1e-5);
        assert_eq!(p.num_attraction_points, 800);
        assert!((p.kill_distance - 0.3).abs() < 1e-5);
        assert!((p.influence_distance - 5.0).abs() < 1e-5);
        assert!((p.step_size - 0.25).abs() < 1e-5);
        assert_eq!(p.lsystem_iterations, 4);
        assert!((p.lsystem_angle_variation - 15.0).abs() < 1e-5);
    }

    #[test]
    fn branch_params_invalid_method() {
        let toml_val: toml::Value = toml::toml! {
            branch_method = "magic_tree"
        }
        .into();
        assert!(BranchParams::from_toml(&toml_val).is_err());
    }

    // ── Text Grammar Tests ──────────────────────────────────────────────

    #[test]
    fn branch_params_lsystem_preset() {
        let toml_val: toml::Value = toml::toml! {
            lsystem_preset = "fractal_plant"
        }
        .into();
        let p = BranchParams::from_toml(&toml_val).unwrap();
        assert!(p.lsystem_grammar.is_some());
        // The preset's own iterations and angle apply unless overridden.
        assert_eq!(p.lsystem_iterations, 5);
        assert_eq!(p.lsystem_angle, Some(22.5));

        let toml_val: toml::Value = toml::toml! {
            lsystem_preset = "fractal_plant"
            lsystem_iterations = 2
        }
        .into();
        assert_eq!(
            BranchParams::from_toml(&toml_val)
                .unwrap()
                .lsystem_iterations,
            2
        );

        let toml_val: toml::Value = toml::toml! {
            lsystem_preset = "baobab"
        }
        .into();
        let err = BranchParams::from_toml(&toml_val).unwrap_err().to_string();
        assert!(err.contains("fractal_plant"), "should list presets: {err}");
    }

    #[test]
    fn branch_params_lsystem_rules() {
        let toml_val: toml::Value = toml::toml! {
            lsystem_axiom = "X"
            lsystem_rules = ["X -> F[+X][-X]FX", "F -> FF"]
            lsystem_tropism = 0.2
        }
        .into();
        let p = BranchParams::from_toml(&toml_val).unwrap();
        assert_eq!(p.lsystem_grammar.unwrap().rules.len(), 2);
        assert!((p.lsystem_tropism - 0.2).abs() < 1e-6);

        // An axiom with no rules, or a broken rule, is an error.
        let toml_val: toml::Value = toml::toml! { lsystem_axiom = "X" }.into();
        assert!(BranchParams::from_toml(&toml_val).is_err());
        let toml_val: toml::Value = toml::toml! {
            lsystem_axiom = "X"
            lsystem_rules = ["X -> F[+X"]
        }
        .into();
        assert!(BranchParams::from_toml(&toml_val).is_err());
    }

    #[test]
    fn grammar_crown_starts_at_tip_and_fits_its_size() {
        let toml_val: toml::Value = toml::toml! {
            lsystem_preset = "weed"
            lsystem_iterations = 4
            lsystem_size = 3.0
        }
        .into();
        let params = BranchParams::from_toml(&toml_val).unwrap();
        let mut rng = SeededRng::new(42);
        let output = generate_branches(&params, &test_trunk(), &mut rng).unwrap();

        assert!(output.segment_count > 50);
        assert!((output.segments[0].start - test_trunk().tip()).length() < 1e-4);
        assert_eq!(output.segments[0].radius_start, 0.1);
        let reach = output
            .segments
            .iter()
            .map(|s| (s.end - test_trunk().tip()).length())
            .fold(0.0f32, f32::max);
        assert!((reach - 3.0).abs() < 1e-3, "reach {reach}");
        assert!(!output.tip_positions.is_empty());
    }

    // ── L-System Path Tests ─────────────────────────────────────────────

    #[test]
    fn lsystem_produces_segments() {
        let params = default_branch_params();
        let mut rng = SeededRng::new(42);

        let segments = generate_lsystem_branches(&params, &test_trunk(), &mut rng).unwrap();

        assert!(!segments.is_empty(), "L-system should produce segments");
        for seg in &segments {
            assert!(seg.start.x.is_finite());
            assert!(seg.start.y.is_finite());
            assert!(seg.start.z.is_finite());
            assert!(seg.end.x.is_finite());
            assert!(seg.end.y.is_finite());
            assert!(seg.end.z.is_finite());
        }
    }

    #[test]
    fn lsystem_segments_attached_to_trunk() {
        let params = default_branch_params();
        let mut rng = SeededRng::new(42);
        let segments = generate_lsystem_branches(&params, &test_trunk(), &mut rng).unwrap();
        assert_attached_along_trunk(&segments, &params, &test_trunk());
    }

    /// Every segment starts where another ends, or on the trunk's
    /// centreline between `branch_start` and the tip, and limbs leave the
    /// trunk at more than one height.
    fn assert_attached_along_trunk(
        segments: &[BranchSegment],
        params: &BranchParams,
        trunk: &TrunkSpine,
    ) {
        let lowest = trunk.at(params.branch_start).0.y;
        let mut heights = Vec::new();
        for seg in segments {
            let continues = segments.iter().any(|o| (o.end - seg.start).length() < 1e-4);
            if continues {
                continue;
            }
            let off = trunk.distance_to(seg.start);
            assert!(
                off < 1e-3,
                "limb starts {off} m off the trunk at {:?}",
                seg.start
            );
            assert!(
                seg.start.y >= lowest - 1e-3 && seg.start.y <= trunk.tip().y + 1e-3,
                "limb leaves the trunk at y = {}, outside {lowest}..{}",
                seg.start.y,
                trunk.tip().y
            );
            heights.push(seg.start.y);
        }
        assert!(!heights.is_empty(), "no limb leaves the trunk");
        let spread = heights.iter().copied().fold(f32::MIN, f32::max)
            - heights.iter().copied().fold(f32::MAX, f32::min);
        assert!(
            spread > 0.5,
            "limbs all leave within {spread} m of each other"
        );
    }

    #[test]
    fn lsystem_determinism() {
        let params = default_branch_params();

        let mut rng1 = SeededRng::new(42);
        let seg1 = generate_lsystem_branches(&params, &test_trunk(), &mut rng1).unwrap();

        let mut rng2 = SeededRng::new(42);
        let seg2 = generate_lsystem_branches(&params, &test_trunk(), &mut rng2).unwrap();

        assert_eq!(seg1.len(), seg2.len());
        for (a, b) in seg1.iter().zip(seg2.iter()) {
            assert!((a.start.x - b.start.x).abs() < 1e-4);
            assert!((a.start.y - b.start.y).abs() < 1e-4);
            assert!((a.start.z - b.start.z).abs() < 1e-4);
            assert!((a.end.x - b.end.x).abs() < 1e-4);
        }
    }

    // ── Space Colonization Path Tests ───────────────────────────────────

    #[test]
    fn colonization_produces_segments() {
        let params = BranchParams {
            method: BranchMethod::SpaceColonization,
            ..default_branch_params()
        };
        let mut rng = SeededRng::new(42);

        let segments = generate_colonization_branches(&params, &test_trunk(), &mut rng).unwrap();

        assert!(
            !segments.is_empty(),
            "space colonization should produce segments"
        );
        for seg in &segments {
            assert!(seg.start.x.is_finite());
            assert!(seg.end.y.is_finite());
        }
    }

    #[test]
    fn colonization_limbs_leave_the_trunk() {
        let params = BranchParams {
            method: BranchMethod::SpaceColonization,
            ..default_branch_params()
        };
        let mut rng = SeededRng::new(42);
        let segments = generate_colonization_branches(&params, &test_trunk(), &mut rng).unwrap();
        assert_attached_along_trunk(&segments, &params, &test_trunk());

        // No limb is thicker than the trunk where it leaves it.
        for seg in &segments {
            let t = seg.start.y / 5.0;
            if test_trunk().distance_to(seg.start) < 1e-3 {
                assert!(seg.radius_start <= test_trunk().at(t).2 + 1e-5);
            }
        }
    }

    #[test]
    fn colonization_determinism() {
        let params = BranchParams {
            method: BranchMethod::SpaceColonization,
            ..default_branch_params()
        };

        let mut rng1 = SeededRng::new(42);
        let seg1 = generate_colonization_branches(&params, &test_trunk(), &mut rng1).unwrap();

        let mut rng2 = SeededRng::new(42);
        let seg2 = generate_colonization_branches(&params, &test_trunk(), &mut rng2).unwrap();

        assert_eq!(seg1.len(), seg2.len());
        for (a, b) in seg1.iter().zip(seg2.iter()) {
            assert_eq!(a.start.x, b.start.x);
            assert_eq!(a.end.y, b.end.y);
        }
    }

    // ── Mesh & Integration Tests ────────────────────────────────────────

    #[test]
    fn segments_to_mesh_validity() {
        let segments = vec![
            BranchSegment {
                start: Vec3::new(0.0, 5.0, 0.0),
                end: Vec3::new(1.0, 6.0, 0.0),
                radius_start: 0.1,
                radius_end: 0.08,
                depth: 1,
            },
            BranchSegment {
                start: Vec3::new(1.0, 6.0, 0.0),
                end: Vec3::new(2.0, 6.5, 0.5),
                radius_start: 0.08,
                radius_end: 0.05,
                depth: 2,
            },
        ];

        let mesh = segments_to_mesh(&segments, 6).unwrap();
        assert!(mesh.validate().is_ok(), "mesh should be valid");
        assert!(mesh.vertex_count() > 0);
        assert!(mesh.triangle_count() > 0);
    }

    #[test]
    fn no_degenerate_triangles() {
        let params = default_branch_params();
        let mut rng = SeededRng::new(42);

        let output = generate_branches(&params, &test_trunk(), &mut rng).unwrap();

        let verts = &output.mesh.vertices;
        let indices = &output.mesh.indices;
        for tri in indices.chunks_exact(3) {
            let p0 = Vec3::from_array(verts[tri[0] as usize].position);
            let p1 = Vec3::from_array(verts[tri[1] as usize].position);
            let p2 = Vec3::from_array(verts[tri[2] as usize].position);

            let e0 = p1 - p0;
            let e1 = p2 - p0;
            let cross = e0.cross(&e1);
            let area = cross.length() * 0.5;
            assert!(
                area > 1e-10,
                "degenerate triangle with area {area}: [{:?}, {:?}, {:?}]",
                p0,
                p1,
                p2
            );
        }
    }

    #[test]
    fn both_methods_produce_valid_output() {
        // L-system
        let params_ls = BranchParams {
            method: BranchMethod::LSystem,
            ..default_branch_params()
        };
        let mut rng1 = SeededRng::new(42);
        let out_ls = generate_branches(&params_ls, &test_trunk(), &mut rng1).unwrap();
        assert!(out_ls.mesh.validate().is_ok());
        assert!(out_ls.segment_count > 0);

        // Space colonization
        let params_sc = BranchParams {
            method: BranchMethod::SpaceColonization,
            ..default_branch_params()
        };
        let mut rng2 = SeededRng::new(42);
        let out_sc = generate_branches(&params_sc, &test_trunk(), &mut rng2).unwrap();
        assert!(out_sc.mesh.validate().is_ok());
        assert!(out_sc.segment_count > 0);
    }

    #[test]
    fn merged_tree_mesh_valid() {
        use crate::generators::tree::{generate_tree, TrunkParams};

        let trunk_params = TrunkParams::default();
        let branch_params = default_branch_params();
        let mut rng = SeededRng::new(42);

        let tree = generate_tree(&trunk_params, &branch_params, &mut rng).unwrap();
        assert!(
            tree.mesh.validate().is_ok(),
            "combined tree mesh should be valid"
        );
        assert!(tree.mesh.vertex_count() > 0);
        assert!(tree.mesh.triangle_count() > 0);
    }

    #[test]
    fn branch_tips_extracted() {
        let params = default_branch_params();
        let mut rng = SeededRng::new(42);

        let output = generate_branches(&params, &test_trunk(), &mut rng).unwrap();
        assert!(
            !output.tip_positions.is_empty(),
            "should extract at least one branch tip"
        );
        assert_eq!(output.tip_positions.len(), output.tip_directions.len());

        // Tips should be at finite positions
        for pos in &output.tip_positions {
            assert!(pos.x.is_finite());
            assert!(pos.y.is_finite());
            assert!(pos.z.is_finite());
        }
    }

    // ── Helper Tests ────────────────────────────────────────────────────

    #[test]
    fn transform_identity_when_aligned() {
        let mut segments = vec![BranchSegment {
            start: Vec3::ZERO,
            end: Vec3::new(0.0, 1.0, 0.0),
            radius_start: 0.1,
            radius_end: 0.05,
            depth: 0,
        }];

        let tip = Vec3::new(0.0, 5.0, 0.0);
        transform_segments(&mut segments, tip, Vec3::UP);

        // Should just translate
        assert!((segments[0].start.y - 5.0).abs() < 1e-4);
        assert!((segments[0].end.y - 6.0).abs() < 1e-4);
    }

    #[test]
    fn extract_tips_finds_terminal_segments() {
        let segments = vec![
            // Chain: A → B → C (terminal)
            BranchSegment {
                start: Vec3::ZERO,
                end: Vec3::new(1.0, 0.0, 0.0),
                radius_start: 0.1,
                radius_end: 0.08,
                depth: 0,
            },
            BranchSegment {
                start: Vec3::new(1.0, 0.0, 0.0),
                end: Vec3::new(2.0, 1.0, 0.0),
                radius_start: 0.08,
                radius_end: 0.05,
                depth: 1,
            },
            // Branch: A → D (terminal)
            BranchSegment {
                start: Vec3::ZERO,
                end: Vec3::new(-1.0, 1.0, 0.0),
                radius_start: 0.1,
                radius_end: 0.06,
                depth: 1,
            },
        ];

        let (tips, dirs) = extract_tips(&segments);
        // C and D are terminal — their ends are not the start of any other segment
        assert_eq!(tips.len(), 2, "should find 2 terminal tips");
        assert_eq!(dirs.len(), 2);
    }

    // ── Task 3.2 acceptance ─────────────────────────────────────────────

    fn method_params(method: BranchMethod) -> BranchParams {
        BranchParams {
            method,
            ..default_branch_params()
        }
    }

    const METHODS: [BranchMethod; 2] = [BranchMethod::LSystem, BranchMethod::SpaceColonization];

    #[test]
    fn lsystem_branch_count_grows_with_iterations() {
        let counts: Vec<usize> = (1..=4)
            .map(|lsystem_iterations| {
                let params = BranchParams {
                    lsystem_iterations,
                    ..default_branch_params()
                };
                generate_lsystem_branches(&params, &test_trunk(), &mut SeededRng::new(42))
                    .unwrap()
                    .len()
            })
            .collect();
        assert!(
            counts.windows(2).all(|w| w[1] > w[0]),
            "segment counts by iteration: {counts:?}"
        );
    }

    #[test]
    fn lsystem_crown_reaches_the_crown_radius() {
        let params = BranchParams {
            crown_radius: 2.5,
            branch_start: 1.0,
            ..default_branch_params()
        };
        let segments =
            generate_lsystem_branches(&params, &test_trunk(), &mut SeededRng::new(42)).unwrap();
        let reach = segments
            .iter()
            .map(|s| (s.end - test_trunk().tip()).length())
            .fold(0.0f32, f32::max);
        assert!((reach - 2.5).abs() < 1e-3, "reach {reach}");
    }

    #[test]
    fn colonization_consumes_every_reachable_point() {
        for seed in [1, 42, 99] {
            let params = method_params(BranchMethod::SpaceColonization);
            let crown = colonize_crown(&params, &test_trunk(), &mut SeededRng::new(seed)).unwrap();
            let grown = &crown.grown;
            if grown.unconsumed == 0 || grown.iterations == 200 {
                continue;
            }
            // It stopped early: whatever is left must be out of every node's reach.
            let points = scatter_attraction_points(
                &CrownVolume::Sphere {
                    center: test_trunk().at(params.branch_start).0 + Vec3::UP * params.crown_radius,
                    radius: params.crown_radius,
                },
                params.num_attraction_points,
                &mut SeededRng::new(seed).fork("colonization"),
            );
            let kill = params.kill_distance;
            let left: Vec<Vec3> = points
                .into_iter()
                .filter(|p| {
                    grown
                        .nodes
                        .iter()
                        .all(|n| (n.position - *p).length() >= kill)
                })
                .collect();
            assert_eq!(left.len(), grown.unconsumed);
            for p in left {
                let nearest = grown
                    .nodes
                    .iter()
                    .map(|n| (n.position - p).length())
                    .fold(f32::INFINITY, f32::min);
                assert!(
                    nearest > params.influence_distance,
                    "seed {seed}: a point {nearest} m from the tree was left unconsumed"
                );
            }
        }
    }

    #[test]
    fn segments_join_into_limbs_through_bends() {
        let seg = |start: Vec3, end: Vec3, r0: f32, r1: f32| BranchSegment {
            start,
            end,
            radius_start: r0,
            radius_end: r1,
            depth: 1,
        };
        let a = Vec3::ZERO;
        let b = Vec3::new(0.0, 1.0, 0.0);
        let c = Vec3::new(0.3, 2.0, 0.0);
        let d = Vec3::new(0.4, 3.0, 0.2);
        let side = Vec3::new(-0.8, 1.6, 0.0);
        let segments = vec![
            seg(a, b, 0.2, 0.16),
            seg(b, side, 0.06, 0.03), // thinner: a side branch
            seg(b, c, 0.16, 0.12),
            seg(c, d, 0.12, 0.08),
        ];
        let limbs = join_limbs(&segments);
        assert_eq!(limbs.len(), 2);
        assert_eq!(limbs[0].points, vec![a, b, c, d]);
        assert_eq!(limbs[0].radii, vec![0.2, 0.16, 0.12, 0.08]);
        assert_eq!(limbs[1].points, vec![b, side]);

        // One tube of four rings, not three tubes of two: the bend shares a ring.
        let mesh = segments_to_mesh(&segments[2..], 6).unwrap();
        let ring = 7; // six sides and the seam
        assert_eq!(mesh.vertex_count(), 3 * ring + 2);
    }

    #[test]
    fn thin_limbs_get_fewer_sides() {
        assert_eq!(radial_segments_for(0.2, 0.2, 8), 8);
        assert_eq!(radial_segments_for(0.05, 0.2, 8), 4);
        assert_eq!(radial_segments_for(0.001, 0.2, 8), 3);
        assert_eq!(radial_segments_for(0.2, 0.2, 2), 3);
    }

    #[test]
    fn branch_start_is_validated() {
        let toml_val: toml::Value = toml::toml! { branch_start = 0.4 }.into();
        assert!((BranchParams::from_toml(&toml_val).unwrap().branch_start - 0.4).abs() < 1e-6);
        let toml_val: toml::Value = toml::toml! { branch_start = 1.5 }.into();
        assert!(BranchParams::from_toml(&toml_val).is_err());
    }

    /// The full tree for one method, from the default trunk.
    fn tree_for(method: BranchMethod, seed: u64) -> crate::generators::tree::TreeOutput {
        use crate::generators::tree::{generate_tree, TrunkParams};
        generate_tree(
            &TrunkParams::default(),
            &method_params(method),
            &mut SeededRng::new(seed),
        )
        .unwrap()
    }

    #[test]
    fn tree_mesh_winding_is_consistent() {
        // Where triangles share an edge they must run it in opposite
        // directions; a directed edge used twice means a flipped face.
        for method in METHODS {
            let mesh = tree_for(method, 42).mesh;
            let mut edges = std::collections::HashSet::new();
            for tri in mesh.indices.chunks_exact(3) {
                for (a, b) in [(tri[0], tri[1]), (tri[1], tri[2]), (tri[2], tri[0])] {
                    assert!(edges.insert((a, b)), "{method:?}: edge {a}->{b} used twice");
                }
            }
        }
    }

    #[test]
    fn limb_faces_point_outward() {
        let segments = vec![BranchSegment {
            start: Vec3::ZERO,
            end: Vec3::new(0.5, 2.0, 0.0),
            radius_start: 0.2,
            radius_end: 0.1,
            depth: 1,
        }];
        let mesh = segments_to_mesh(&segments, 6).unwrap();
        let axis = TrunkSpine {
            points: vec![segments[0].start, segments[0].end],
            radius_base: 0.2,
            radius_top: 0.1,
        };
        for tri in mesh.indices.chunks_exact(3) {
            let [p0, p1, p2] =
                [0, 1, 2].map(|k| Vec3::from_array(mesh.vertices[tri[k] as usize].position));
            let centroid = (p0 + p1 + p2) * (1.0 / 3.0);
            let normal = (p1 - p0).cross(&(p2 - p0));
            let along = (axis.points[1] - axis.points[0]).normalized();
            if normal.normalized().dot(&along).abs() > 0.9 {
                continue; // a cap, facing along the limb
            }
            let (t_pos, _, _) = {
                let ab = axis.points[1] - axis.points[0];
                let s = ((centroid - axis.points[0]).dot(&ab) / ab.dot(&ab)).clamp(0.0, 1.0);
                (axis.points[0] + ab * s, (), ())
            };
            assert!(
                normal.dot(&(centroid - t_pos)) > 0.0,
                "a side face points inward"
            );
        }
    }

    #[test]
    fn tree_meshes_are_valid_for_both_methods() {
        for method in METHODS {
            let mesh = tree_for(method, 7).mesh;
            assert!(mesh.validate().is_ok(), "{method:?}");
            for v in &mesh.vertices {
                let n = Vec3::from_array(v.normal);
                assert!(
                    (n.length() - 1.0).abs() < 1e-3,
                    "{method:?}: normal length {}",
                    n.length()
                );
            }
            for tri in mesh.indices.chunks_exact(3) {
                let [p0, p1, p2] =
                    [0, 1, 2].map(|k| Vec3::from_array(mesh.vertices[tri[k] as usize].position));
                let area = (p1 - p0).cross(&(p2 - p0)).length() * 0.5;
                assert!(area > 1e-10, "{method:?}: degenerate triangle");
            }
        }
    }

    #[test]
    fn trees_are_deterministic_for_both_methods() {
        for method in METHODS {
            let a = tree_for(method, 11).mesh;
            let b = tree_for(method, 11).mesh;
            assert_eq!(a.vertex_count(), b.vertex_count(), "{method:?}");
            for (va, vb) in a.vertices.iter().zip(&b.vertices) {
                assert_eq!(va.position, vb.position, "{method:?}");
            }
            assert_eq!(a.indices, b.indices, "{method:?}");
        }
    }

    #[test]
    fn tree_lods_do_not_stretch_into_sheets() {
        // Simplifying closed limb tubes once threw collapse points down a
        // tube's axis, stretching sheets several square metres across the
        // crown. The
        // tree is the showcase garden tree, leaves and seed included, which
        // did exactly that (its largest LOD triangle was 5.4 m2).
        use crate::generator::Generator;
        use crate::generators::tree::TreeGenerator;
        use crate::output::GeneratorOutput;
        use crate::seed::Seed;
        use crate::spec::ProcGenSpec;
        let seed = Seed::Derived("garden:tree".into()).resolve();
        for method in ["space_colonization", "lsystem"] {
            let spec = ProcGenSpec::parse(&GARDEN_TREE.replace("METHOD", method)).unwrap();
            let out = TreeGenerator
                .generate(&spec, &mut SeededRng::new(seed))
                .unwrap();
            let GeneratorOutput::MeshWithLods(lods) = out else {
                panic!("expected LODs");
            };
            for (level, lod) in lods.iter().enumerate() {
                let largest = lod
                    .indices
                    .chunks_exact(3)
                    .map(|tri| {
                        let [a, b, c] = [0, 1, 2]
                            .map(|k| Vec3::from_array(lod.vertices[tri[k] as usize].position));
                        (b - a).cross(&(c - a)).length() * 0.5
                    })
                    .fold(0.0f32, f32::max);
                assert!(
                    largest < 1.5,
                    "{method} LOD {level}: a {largest} m2 triangle"
                );
            }
        }
    }

    /// `showcase_game/specs/garden_tree.procgen.toml`, with the method open.
    const GARDEN_TREE: &str = r##"
generator = "tree_v1"
[meta]
name = "garden_tree"
version = "1.0.0"
[seed]
mode = "fixed"
value = 1
[params]
trunk_height = 6.0
trunk_radius_base = 0.45
trunk_radius_top = 0.15
trunk_segments = 14
radial_segments = 10
trunk_curve_noise = 0.06
bark_color_base = "#3A5A2A"
bark_roughness = 0.85
branch_method = "METHOD"
branch_levels = 4
branch_angle_min = 20.0
branch_angle_max = 50.0
branch_length_falloff = 0.6
branch_radius_falloff = 0.5
lsystem_iterations = 3
lsystem_angle_variation = 15.0
crown_shape = "sphere"
crown_radius = 4.0
leaf_style = "billboard"
leaf_density = 0.9
leaf_color_base = "#1A7A08"
leaf_color_variation = 0.12
leaf_size = 0.22
leaf_size_variation = 0.3
leaves_per_tip = 6
leaf_spread_radius = 0.5
leaf_along_branch_density = 0.4
leaf_along_branch_min_depth = 2
[[lod]]
level = 0
target_triangles = 10000
[[lod]]
level = 1
target_triangles = 2500
[[lod]]
level = 2
target_triangles = 600
"##;

    #[test]
    fn branch_method_selects_the_algorithm() {
        let parse = |method: &str| {
            let mut table = toml::map::Map::new();
            table.insert("branch_method".into(), toml::Value::String(method.into()));
            BranchParams::from_toml(&toml::Value::Table(table)).unwrap()
        };
        let ls = parse("lsystem");
        let sc = parse("space_colonization");
        assert_eq!(ls.method, BranchMethod::LSystem);
        assert_eq!(sc.method, BranchMethod::SpaceColonization);

        // Each runs its own path: the outputs match that path called directly.
        let via = |p: &BranchParams| {
            generate_branches(p, &test_trunk(), &mut SeededRng::new(3))
                .unwrap()
                .segments
        };
        let direct_ls =
            generate_lsystem_branches(&ls, &test_trunk(), &mut SeededRng::new(3)).unwrap();
        let direct_sc =
            generate_colonization_branches(&sc, &test_trunk(), &mut SeededRng::new(3)).unwrap();
        assert_eq!(via(&ls).len(), direct_ls.len());
        assert_eq!(via(&sc).len(), direct_sc.len());
        assert_ne!(direct_ls.len(), direct_sc.len());
    }
}
