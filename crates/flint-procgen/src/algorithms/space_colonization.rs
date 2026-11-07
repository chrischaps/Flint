//! Space colonization algorithm for organic branching structures.
//!
//! Grows a tree graph toward a cloud of attraction points scattered within a
//! crown volume (Runions et al., *Modeling Trees with a Space Colonization
//! Algorithm*, 2007). Branches compete for the space they grow into, so the
//! canopy fills its volume with adaptive branching density, which reads as
//! more natural than an L-system for broadleaf crowns, roots and veins.
//!
//! # Algorithm Overview
//!
//! 1. Scatter attraction points inside a [`CrownVolume`] via rejection
//!    sampling ([`scatter_attraction_points`])
//! 2. Grow a straight trunk from the root upward
//! 3. Each iteration, every attraction point pulls on its closest node within
//!    `influence_distance`; every pulled node grows one `step_size` toward the
//!    mean of its pulls
//! 4. Remove attraction points within `kill_distance` of a node
//! 5. Stop when no points remain, none can pull, or `max_iterations` is hit
//! 6. Assign branching depth and radii via the pipe model
//!    (`r_parent^n = sum(r_child^n)`, da Vinci's rule at n = 2)
//! 7. Convert the graph to `Vec<BranchSegment>` ([`branch_graph_to_segments`]),
//!    the same format the L-system produces
//!
//! Each attraction point remembers its closest node; nodes are never removed,
//! so an iteration only compares points against the nodes it just added. That
//! keeps a step at O(points x new nodes) instead of O(points x all nodes).

use crate::rng::SeededRng;
use crate::types::BranchSegment;
use crate::{ProcGenError, Result};
use flint_core::Vec3;

// ─── A. Crown Volume ────────────────────────────────────────────────────────

/// Defines the 3D shape within which attraction points are scattered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CrownVolume {
    /// A sphere centered at `center` with the given `radius`.
    Sphere { center: Vec3, radius: f32 },
    /// An ellipsoid centered at `center` with semi-axis lengths `radii` (rx, ry, rz).
    Ellipsoid { center: Vec3, radii: Vec3 },
    /// Upper hemisphere (y >= center.y) centered at `center` with the given `radius`.
    Hemisphere { center: Vec3, radius: f32 },
    /// An axis-aligned box from `min` to `max` (hedges, espaliers, root beds).
    Box { min: Vec3, max: Vec3 },
    /// An upright cone: a disc of `radius` at `base`, narrowing to a point
    /// `height` above it (conifers).
    Cone {
        base: Vec3,
        radius: f32,
        height: f32,
    },
}

impl CrownVolume {
    /// Check the volume encloses some space, so sampling terminates.
    fn validate(&self) -> Result<()> {
        let ok = match *self {
            CrownVolume::Sphere { radius, .. } | CrownVolume::Hemisphere { radius, .. } => {
                radius > 0.0
            }
            CrownVolume::Ellipsoid { radii, .. } => radii.x > 0.0 && radii.y > 0.0 && radii.z > 0.0,
            CrownVolume::Box { min, max } => min.x <= max.x && min.y <= max.y && min.z <= max.z,
            CrownVolume::Cone { radius, height, .. } => radius > 0.0 && height > 0.0,
        };
        if ok {
            Ok(())
        } else {
            Err(ProcGenError::InvalidParameter {
                name: "crown".into(),
                reason: "volume must have positive extent".into(),
            })
        }
    }

    /// Sample a random point inside this volume using rejection sampling.
    fn sample(&self, rng: &mut SeededRng) -> Vec3 {
        match *self {
            CrownVolume::Sphere { center, radius } => loop {
                let x = rng.next_range(-radius, radius);
                let y = rng.next_range(-radius, radius);
                let z = rng.next_range(-radius, radius);
                if x * x + y * y + z * z <= radius * radius {
                    return center + Vec3::new(x, y, z);
                }
            },
            CrownVolume::Ellipsoid { center, radii } => loop {
                let x = rng.next_range(-radii.x, radii.x);
                let y = rng.next_range(-radii.y, radii.y);
                let z = rng.next_range(-radii.z, radii.z);
                let nx = x / radii.x;
                let ny = y / radii.y;
                let nz = z / radii.z;
                if nx * nx + ny * ny + nz * nz <= 1.0 {
                    return center + Vec3::new(x, y, z);
                }
            },
            CrownVolume::Hemisphere { center, radius } => loop {
                let x = rng.next_range(-radius, radius);
                let y = rng.next_range(0.0, radius);
                let z = rng.next_range(-radius, radius);
                if x * x + y * y + z * z <= radius * radius {
                    return center + Vec3::new(x, y, z);
                }
            },
            CrownVolume::Box { min, max } => Vec3::new(
                rng.next_range(min.x, max.x),
                rng.next_range(min.y, max.y),
                rng.next_range(min.z, max.z),
            ),
            CrownVolume::Cone {
                base,
                radius,
                height,
            } => loop {
                let x = rng.next_range(-radius, radius);
                let y = rng.next_range(0.0, height);
                let z = rng.next_range(-radius, radius);
                let r = radius * (1.0 - y / height);
                if x * x + z * z <= r * r {
                    return base + Vec3::new(x, y, z);
                }
            },
        }
    }
}

/// Scatter `count` attraction points inside `volume`, deterministically from `rng`.
pub fn scatter_attraction_points(
    volume: &CrownVolume,
    count: u32,
    rng: &mut SeededRng,
) -> Vec<Vec3> {
    (0..count).map(|_| volume.sample(rng)).collect()
}

// ─── B. Parameters ──────────────────────────────────────────────────────────

/// Parameters controlling the space colonization algorithm.
#[derive(Debug, Clone, PartialEq)]
pub struct SpaceColonizationParams {
    /// Shape of the attraction point cloud.
    pub crown: CrownVolume,
    /// Number of attraction points to scatter (default 500). Zero grows the
    /// trunk alone.
    pub num_attraction_points: u32,
    /// Remove a point when a node is this close (default 0.5).
    pub kill_distance: f32,
    /// Maximum distance a point can attract a node (default 4.0).
    pub influence_distance: f32,
    /// New nodes are placed this far from their parent (default 0.3).
    pub step_size: f32,
    /// Root node position (default origin).
    pub root_position: Vec3,
    /// Number of straight-up trunk nodes before colonization (default 5).
    pub trunk_steps: u32,
    /// Safety cap on the main colonization loop (default 200).
    pub max_iterations: u32,
    /// Maximum number of tree nodes — budget guard (default 5000).
    pub max_nodes: u32,
    /// Root radius for the pipe model (default 0.3).
    pub base_radius: f32,
    /// Terminal node radius before scaling to `base_radius` (default 0.02).
    pub tip_radius: f32,
    /// Pipe model exponent `n` in `r_parent^n = sum(r_child^n)` (default 2.0,
    /// da Vinci's rule). Larger values keep inner branches thicker.
    pub pipe_exponent: f32,
}

impl Default for SpaceColonizationParams {
    fn default() -> Self {
        Self {
            crown: CrownVolume::Sphere {
                center: Vec3::new(0.0, 8.0, 0.0),
                radius: 4.0,
            },
            num_attraction_points: 500,
            kill_distance: 0.5,
            influence_distance: 4.0,
            step_size: 0.3,
            root_position: Vec3::ZERO,
            trunk_steps: 5,
            max_iterations: 200,
            max_nodes: 5000,
            base_radius: 0.3,
            tip_radius: 0.02,
            pipe_exponent: 2.0,
        }
    }
}

// ─── C. Branch Graph ────────────────────────────────────────────────────────

/// One node of a grown branch graph. Node 0 is the root; every other node's
/// `parent` has a lower index, so a forward pass visits parents first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BranchNode {
    /// World-space position.
    pub position: Vec3,
    /// Pipe-model radius at this node.
    pub radius: f32,
    /// Index of the parent node; `None` only for the root.
    pub parent: Option<usize>,
    /// Branching depth: 0 on the trunk, +1 past each fork.
    pub depth: u32,
}

/// The outcome of a colonization run.
#[derive(Debug, Clone)]
pub struct Colonization {
    /// The grown tree graph (empty when nothing grew).
    pub nodes: Vec<BranchNode>,
    /// Growth iterations run.
    pub iterations: u32,
    /// Attraction points left unconsumed (out of reach, or the cap was hit).
    pub unconsumed: usize,
}

// ─── D. SpaceColonization ───────────────────────────────────────────────────

/// Space colonization tree generator.
///
/// Grows organic branching structures by iteratively extending a tree graph
/// toward a cloud of attraction points scattered within a crown volume.
#[derive(Debug)]
pub struct SpaceColonization {
    params: SpaceColonizationParams,
}

impl SpaceColonization {
    /// Create a new generator, validating parameters.
    pub fn new(params: SpaceColonizationParams) -> Result<Self> {
        if params.kill_distance <= 0.0 {
            return Err(ProcGenError::InvalidParameter {
                name: "kill_distance".into(),
                reason: "must be > 0".into(),
            });
        }
        if params.kill_distance >= params.influence_distance {
            return Err(ProcGenError::InvalidParameter {
                name: "kill_distance".into(),
                reason: "must be less than influence_distance".into(),
            });
        }
        if params.step_size <= 0.0 {
            return Err(ProcGenError::InvalidParameter {
                name: "step_size".into(),
                reason: "must be > 0".into(),
            });
        }
        if params.pipe_exponent < 1.0 {
            return Err(ProcGenError::InvalidParameter {
                name: "pipe_exponent".into(),
                reason: "must be >= 1".into(),
            });
        }
        params.crown.validate()?;
        Ok(Self { params })
    }

    /// The validated parameters.
    pub fn params(&self) -> &SpaceColonizationParams {
        &self.params
    }

    /// Scatter attraction points in the crown and grow the graph toward them.
    pub fn grow(&self, rng: &mut SeededRng) -> Result<Colonization> {
        let p = &self.params;
        let points = scatter_attraction_points(&p.crown, p.num_attraction_points, rng);
        self.colonize(&points)
    }

    /// Grow the graph from `root_position` toward the given attraction points.
    ///
    /// Deterministic: the same points always give the same graph. Fails with
    /// [`ProcGenError::BudgetExceeded`] past `max_nodes`.
    pub fn colonize(&self, attractors: &[Vec3]) -> Result<Colonization> {
        let p = &self.params;
        let trunk: Vec<Vec3> = (0..=p.trunk_steps)
            .map(|i| p.root_position + Vec3::new(0.0, p.step_size * i as f32, 0.0))
            .collect();
        self.colonize_from(&trunk, attractors)
    }

    /// Grow toward the attraction points from a given chain of nodes, which
    /// become nodes `0..chain.len()`, each the parent of the next.
    ///
    /// Any node of the chain can sprout, so a tree's real trunk, passed as
    /// the chain, grows branches all the way up rather than only from its
    /// tip. `root_position` and `trunk_steps` are ignored.
    pub fn colonize_from(&self, chain: &[Vec3], attractors: &[Vec3]) -> Result<Colonization> {
        let p = &self.params;
        let kill_sq = p.kill_distance * p.kill_distance;
        let influence_sq = p.influence_distance * p.influence_distance;
        if chain.is_empty() {
            return Err(ProcGenError::InvalidParameter {
                name: "chain".into(),
                reason: "colonization needs at least a root node".into(),
            });
        }

        let mut graph = Graph::default();
        for (i, &pos) in chain.iter().enumerate() {
            graph.push(pos, i.checked_sub(1));
        }
        graph.check_budget(p.max_nodes)?;

        // Every live attraction point with its closest node so far.
        let mut live: Vec<Attractor> = attractors
            .iter()
            .map(|&point| Attractor {
                point,
                closest: 0,
                dist_sq: f32::INFINITY,
            })
            .collect();
        let mut seen = 0; // nodes below this index are folded into `live`

        let mut iterations = 0;
        while iterations < p.max_iterations {
            // Fold in the nodes added since last time, and kill what they reach.
            let fresh = seen..graph.positions.len();
            seen = fresh.end;
            live.retain_mut(|a| {
                for ni in fresh.clone() {
                    let d = dist_sq(graph.positions[ni], a.point);
                    if d < a.dist_sq {
                        a.dist_sq = d;
                        a.closest = ni;
                    }
                }
                a.dist_sq >= kill_sq
            });
            if live.is_empty() {
                break;
            }

            // Sum each node's pulls toward the points it is closest to, and
            // remember the nearest of them.
            let n = graph.positions.len();
            let mut pull: Vec<Vec3> = vec![Vec3::ZERO; n];
            let mut nearest: Vec<Option<&Attractor>> = vec![None; n];
            for a in live.iter().filter(|a| a.dist_sq <= influence_sq) {
                let dir = a.point - graph.positions[a.closest];
                pull[a.closest] = pull[a.closest] + dir * (1.0 / a.dist_sq.sqrt());
                if nearest[a.closest].is_none_or(|b| a.dist_sq < b.dist_sq) {
                    nearest[a.closest] = Some(a);
                }
            }

            let before = graph.positions.len();
            for (ni, dir) in pull.into_iter().enumerate() {
                let Some(near) = nearest[ni] else { continue };
                let tolerance = p.step_size * 0.05;
                let from = graph.positions[ni];
                let len = dir.length();
                let mut pos = from + dir * (p.step_size / len.max(1e-6));
                if len < 1e-6 || graph.has_child_near(ni, pos, tolerance) {
                    // Pulls that cancel out, or that already grew a child no
                    // closer to them, would stall; step toward the nearest
                    // point instead, which always gets closer to it.
                    let toward = near.point - from;
                    pos = from + toward * (p.step_size.min(near.dist_sq.sqrt()) / toward.length());
                    if graph.has_child_near(ni, pos, tolerance) {
                        continue;
                    }
                }
                graph.push(pos, Some(ni));
            }
            graph.check_budget(p.max_nodes)?;
            iterations += 1;
            if graph.positions.len() == before {
                break; // stalled: the rest is out of reach
            }
        }

        // A lone root is not a branch.
        if graph.positions.len() < 2 {
            return Ok(Colonization {
                nodes: Vec::new(),
                iterations,
                unconsumed: live.len(),
            });
        }

        let depths = graph.depths();
        let radii = pipe_radii(&graph.parents, p.tip_radius, p.pipe_exponent, p.base_radius);
        let nodes = (0..graph.positions.len())
            .map(|i| BranchNode {
                position: graph.positions[i],
                radius: radii[i],
                parent: graph.parents[i],
                depth: depths[i],
            })
            .collect();
        Ok(Colonization {
            nodes,
            iterations,
            unconsumed: live.len(),
        })
    }

    /// Run the space colonization algorithm and produce branch segments.
    pub fn generate_segments(&self, rng: &mut SeededRng) -> Result<Vec<BranchSegment>> {
        Ok(branch_graph_to_segments(&self.grow(rng)?.nodes))
    }
}

/// An attraction point and the closest node it has seen.
struct Attractor {
    point: Vec3,
    closest: usize,
    dist_sq: f32,
}

/// The growing graph, struct-of-arrays, with child lists for fork detection.
#[derive(Default)]
struct Graph {
    positions: Vec<Vec3>,
    parents: Vec<Option<usize>>,
    children: Vec<Vec<usize>>,
}

impl Graph {
    fn push(&mut self, position: Vec3, parent: Option<usize>) {
        let idx = self.positions.len();
        if let Some(p) = parent {
            self.children[p].push(idx);
        }
        self.positions.push(position);
        self.parents.push(parent);
        self.children.push(Vec::new());
    }

    fn has_child_near(&self, node: usize, pos: Vec3, tolerance: f32) -> bool {
        self.children[node]
            .iter()
            .any(|&c| dist_sq(self.positions[c], pos) < tolerance * tolerance)
    }

    fn check_budget(&self, max_nodes: u32) -> Result<()> {
        if self.positions.len() > max_nodes as usize {
            return Err(ProcGenError::BudgetExceeded(format!(
                "node count {} exceeds max_nodes {}",
                self.positions.len(),
                max_nodes
            )));
        }
        Ok(())
    }

    /// Branching depth per node: +1 below each fork (2+ children).
    fn depths(&self) -> Vec<u32> {
        let mut depths = vec![0u32; self.positions.len()];
        for i in 1..self.positions.len() {
            let parent = self.parents[i].expect("non-root node has a parent");
            let fork = self.children[parent].len() >= 2;
            depths[i] = depths[parent] + u32::from(fork);
        }
        depths
    }
}

// ─── E. Pipe Model Radii ────────────────────────────────────────────────────

/// Compute radii using the pipe model.
///
/// Leaves get `tip_radius`. Working bottom-up, each parent's radius is
/// `(sum(r_child^n))^(1/n)`. Finally, all radii are scaled so the root
/// gets exactly `base_radius`. `parents` must list parents before children.
fn pipe_radii(parents: &[Option<usize>], tip_radius: f32, n: f32, base_radius: f32) -> Vec<f32> {
    let mut sum = vec![0.0f32; parents.len()];
    let mut radii = vec![0.0f32; parents.len()];
    for i in (0..parents.len()).rev() {
        radii[i] = if sum[i] > 0.0 {
            sum[i].powf(1.0 / n)
        } else {
            tip_radius
        };
        if let Some(p) = parents[i] {
            sum[p] += radii[i].powf(n);
        }
    }
    if let Some(&root) = radii.first() {
        if root > 1e-10 {
            let scale = base_radius / root;
            radii.iter_mut().for_each(|r| *r *= scale);
        }
    }
    radii
}

// ─── F. Graph to Segments ───────────────────────────────────────────────────

/// Convert a branch graph into one [`BranchSegment`] per parent-child edge,
/// the format the L-system produces, so the tree mesher takes either.
pub fn branch_graph_to_segments(nodes: &[BranchNode]) -> Vec<BranchSegment> {
    nodes
        .iter()
        .filter_map(|node| {
            let parent = &nodes[node.parent?];
            Some(BranchSegment {
                start: parent.position,
                end: node.position,
                radius_start: parent.radius,
                radius_end: node.radius,
                depth: node.depth,
            })
        })
        .collect()
}

// ─── G. Utility ─────────────────────────────────────────────────────────────

/// Squared distance between two points.
fn dist_sq(a: Vec3, b: Vec3) -> f32 {
    let d = a - b;
    d.dot(&d)
}

// ─── H. Convenience Constructors ────────────────────────────────────────────

/// Create a deciduous tree with a spherical crown.
///
/// The crown sphere sits atop a straight trunk of the given height.
pub fn deciduous_tree(
    trunk_height: f32,
    crown_radius: f32,
    base_radius: f32,
) -> Result<SpaceColonization> {
    let step_size = 0.3;
    let trunk_steps = (trunk_height / step_size).round().max(1.0) as u32;
    SpaceColonization::new(SpaceColonizationParams {
        crown: CrownVolume::Sphere {
            center: Vec3::new(0.0, trunk_height + crown_radius, 0.0),
            radius: crown_radius,
        },
        trunk_steps,
        step_size,
        base_radius,
        ..Default::default()
    })
}

/// Create a columnar tree with a tall, narrow ellipsoid crown.
///
/// Good for cypresses, poplars, and other upright forms.
pub fn columnar_tree(
    trunk_height: f32,
    crown_height: f32,
    crown_width: f32,
    base_radius: f32,
) -> Result<SpaceColonization> {
    let step_size = 0.3;
    let trunk_steps = (trunk_height / step_size).round().max(1.0) as u32;
    let half_height = crown_height * 0.5;
    SpaceColonization::new(SpaceColonizationParams {
        crown: CrownVolume::Ellipsoid {
            center: Vec3::new(0.0, trunk_height + half_height, 0.0),
            radii: Vec3::new(crown_width * 0.5, half_height, crown_width * 0.5),
        },
        trunk_steps,
        step_size,
        base_radius,
        ..Default::default()
    })
}

/// Create a canopy tree with a hemispherical crown.
///
/// Good for spreading canopy shapes like acacias.
pub fn canopy_tree(
    trunk_height: f32,
    crown_radius: f32,
    base_radius: f32,
) -> Result<SpaceColonization> {
    let step_size = 0.3;
    let trunk_steps = (trunk_height / step_size).round().max(1.0) as u32;
    SpaceColonization::new(SpaceColonizationParams {
        crown: CrownVolume::Hemisphere {
            center: Vec3::new(0.0, trunk_height, 0.0),
            radius: crown_radius,
        },
        trunk_steps,
        step_size,
        base_radius,
        ..Default::default()
    })
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── 1. Validation ──────────────────────────────────────────────────

    #[test]
    fn reject_zero_kill_distance() {
        let params = SpaceColonizationParams {
            kill_distance: 0.0,
            ..Default::default()
        };
        let result = SpaceColonization::new(params);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProcGenError::InvalidParameter { .. }
        ));
    }

    #[test]
    fn reject_kill_ge_influence() {
        let params = SpaceColonizationParams {
            kill_distance: 5.0,
            influence_distance: 4.0,
            ..Default::default()
        };
        assert!(SpaceColonization::new(params).is_err());

        let params2 = SpaceColonizationParams {
            kill_distance: 4.0,
            influence_distance: 4.0,
            ..Default::default()
        };
        assert!(SpaceColonization::new(params2).is_err());
    }

    #[test]
    fn reject_zero_step_size() {
        let params = SpaceColonizationParams {
            step_size: 0.0,
            ..Default::default()
        };
        assert!(SpaceColonization::new(params).is_err());
    }

    #[test]
    fn reject_degenerate_volume_and_exponent() {
        let flat = SpaceColonizationParams {
            crown: CrownVolume::Ellipsoid {
                center: Vec3::ZERO,
                radii: Vec3::new(1.0, 0.0, 1.0),
            },
            ..Default::default()
        };
        assert!(SpaceColonization::new(flat).is_err());

        let thin_pipe = SpaceColonizationParams {
            pipe_exponent: 0.5,
            ..Default::default()
        };
        assert!(SpaceColonization::new(thin_pipe).is_err());
    }

    // ── 1b. Issue #21 acceptance ───────────────────────────────────────

    /// A crown just above the root, close enough to colonize without a trunk.
    fn near_crown(points: u32) -> SpaceColonizationParams {
        SpaceColonizationParams {
            crown: CrownVolume::Sphere {
                center: Vec3::new(0.0, 3.0, 0.0),
                radius: 2.5,
            },
            num_attraction_points: points,
            trunk_steps: 0,
            max_iterations: 500,
            ..Default::default()
        }
    }

    #[test]
    fn hundred_points_all_consumed_within_500_iterations() {
        let gen = SpaceColonization::new(near_crown(100)).unwrap();
        let out = gen.grow(&mut SeededRng::new(7)).unwrap();
        assert_eq!(out.unconsumed, 0, "every attraction point is consumed");
        assert!(out.iterations <= 500, "took {} iterations", out.iterations);
    }

    #[test]
    fn graph_is_a_valid_tree() {
        let gen = SpaceColonization::new(Default::default()).unwrap();
        let nodes = gen.grow(&mut SeededRng::new(42)).unwrap().nodes;
        assert!(nodes.len() > 10);
        assert_eq!(nodes.iter().filter(|n| n.parent.is_none()).count(), 1);
        assert!(nodes[0].parent.is_none(), "node 0 is the root");
        for (i, n) in nodes.iter().enumerate().skip(1) {
            let p = n.parent.expect("non-root has a parent");
            assert!(p < i, "parent {p} of node {i} comes first");
        }
    }

    #[test]
    fn graph_nodes_are_deterministic() {
        let gen = SpaceColonization::new(Default::default()).unwrap();
        let a = gen.grow(&mut SeededRng::new(3)).unwrap().nodes;
        let b = gen.grow(&mut SeededRng::new(3)).unwrap().nodes;
        assert_eq!(a, b);
    }

    #[test]
    fn root_is_thickest_and_radius_falls_with_depth() {
        let gen = SpaceColonization::new(Default::default()).unwrap();
        let nodes = gen.grow(&mut SeededRng::new(42)).unwrap().nodes;
        let root = nodes[0].radius;
        assert!(nodes.iter().all(|n| n.radius <= root + 1e-6));

        let max_depth = nodes.iter().map(|n| n.depth).max().unwrap();
        let mean = |d: u32| {
            let rs: Vec<f32> = nodes
                .iter()
                .filter(|n| n.depth == d)
                .map(|n| n.radius)
                .collect();
            rs.iter().sum::<f32>() / rs.len() as f32
        };
        assert!(mean(0) > mean(max_depth / 2));
        assert!(mean(max_depth / 2) > mean(max_depth));

        // And strictly per edge: a child is never thicker than its parent.
        for n in &nodes[1..] {
            assert!(n.radius <= nodes[n.parent.unwrap()].radius + 1e-6);
        }
    }

    #[test]
    fn higher_pipe_exponent_keeps_tips_thicker() {
        let tip_mean = |n: f32| {
            let gen = SpaceColonization::new(SpaceColonizationParams {
                pipe_exponent: n,
                ..Default::default()
            })
            .unwrap();
            let segs = gen.generate_segments(&mut SeededRng::new(42)).unwrap();
            segs.iter()
                .map(|s| s.radius_end)
                .fold(f32::INFINITY, f32::min)
        };
        assert!(tip_mean(3.0) > tip_mean(2.0));
    }

    #[test]
    fn segments_match_lsystem_format() {
        let gen = SpaceColonization::new(Default::default()).unwrap();
        let nodes = gen.grow(&mut SeededRng::new(42)).unwrap().nodes;
        let segs = branch_graph_to_segments(&nodes);
        assert_eq!(segs.len(), nodes.len() - 1, "one segment per edge");
        for s in &segs {
            assert!(s.length() > 0.0);
            assert!(s.radius_start > 0.0 && s.radius_end > 0.0);
            assert!(s.radius_end <= s.radius_start + 1e-6);
        }
    }

    #[test]
    fn zero_points_give_an_empty_graph() {
        let gen = SpaceColonization::new(near_crown(0)).unwrap();
        let out = gen.colonize(&[]).unwrap();
        assert!(out.nodes.is_empty());
        assert!(branch_graph_to_segments(&out.nodes).is_empty());

        // With a trunk, the trunk alone grows.
        let trunk = SpaceColonization::new(SpaceColonizationParams {
            num_attraction_points: 0,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            trunk
                .generate_segments(&mut SeededRng::new(1))
                .unwrap()
                .len(),
            5
        );
    }

    #[test]
    fn one_point_grows_a_single_unforked_branch() {
        let gen = SpaceColonization::new(near_crown(1)).unwrap();
        let target = Vec3::new(1.0, 2.0, 0.5);
        let out = gen.colonize(&[target]).unwrap();
        assert_eq!(out.unconsumed, 0);
        assert!(out.nodes.len() >= 2);
        assert!(out.nodes.iter().all(|n| n.depth == 0), "no forks");
        for (i, n) in out.nodes.iter().enumerate().skip(1) {
            assert_eq!(n.parent, Some(i - 1), "a single chain");
        }
        let tip = out.nodes.last().unwrap().position;
        assert!(
            dist_sq(tip, target) < 0.5 * 0.5,
            "the tip reaches the point"
        );
    }

    #[test]
    fn extreme_points_stay_out_of_reach() {
        let gen = SpaceColonization::new(near_crown(3)).unwrap();
        let far = [
            Vec3::new(1.0e6, 0.0, 0.0),
            Vec3::new(0.0, -1.0e6, 0.0),
            Vec3::new(0.0, 0.0, 1.0e6),
        ];
        let out = gen.colonize(&far).unwrap();
        assert!(out.nodes.is_empty());
        assert_eq!(out.unconsumed, 3);
        assert!(out.iterations <= 1, "stops at once instead of spinning");
    }

    #[test]
    fn box_points_within_bounds() {
        let (min, max) = (Vec3::new(-2.0, 1.0, -0.5), Vec3::new(2.0, 3.0, 0.5));
        let pts =
            scatter_attraction_points(&CrownVolume::Box { min, max }, 500, &mut SeededRng::new(1));
        assert_eq!(pts.len(), 500);
        for p in pts {
            assert!(p.x >= min.x && p.x <= max.x);
            assert!(p.y >= min.y && p.y <= max.y);
            assert!(p.z >= min.z && p.z <= max.z);
        }
    }

    #[test]
    fn box_crown_grows_a_hedge() {
        let gen = SpaceColonization::new(SpaceColonizationParams {
            crown: CrownVolume::Box {
                min: Vec3::new(-3.0, 0.5, -0.4),
                max: Vec3::new(3.0, 2.0, 0.4),
            },
            trunk_steps: 1,
            ..Default::default()
        })
        .unwrap();
        let segs = gen.generate_segments(&mut SeededRng::new(5)).unwrap();
        let reach = segs.iter().map(|s| s.end.x.abs()).fold(0.0f32, f32::max);
        assert!(reach > 2.0, "the hedge spreads sideways, reach {reach}");
    }

    // ── 2. Basic convergence ───────────────────────────────────────────

    #[test]
    fn basic_convergence_produces_segments() {
        let gen = SpaceColonization::new(Default::default()).unwrap();
        let mut rng = SeededRng::new(42);
        let segments = gen.generate_segments(&mut rng).unwrap();

        assert!(
            !segments.is_empty(),
            "should produce at least some segments"
        );
        for seg in &segments {
            assert!(seg.start.x.is_finite());
            assert!(seg.start.y.is_finite());
            assert!(seg.start.z.is_finite());
            assert!(seg.end.x.is_finite());
            assert!(seg.end.y.is_finite());
            assert!(seg.end.z.is_finite());
            assert!(seg.radius_start > 0.0);
            assert!(seg.radius_end > 0.0);
        }
    }

    // ── 3. Attraction point consumption ────────────────────────────────

    #[test]
    fn nodes_reach_into_crown() {
        let gen = SpaceColonization::new(Default::default()).unwrap();
        let mut rng = SeededRng::new(42);
        let segments = gen.generate_segments(&mut rng).unwrap();

        // The default crown is a sphere at (0, 8, 0) radius 4,
        // so some segments should have y > 5 (into the crown region).
        let has_crown_segments = segments.iter().any(|seg| seg.end.y > 5.0);
        assert!(
            has_crown_segments,
            "branches should grow into the crown volume"
        );
    }

    // ── 4. Determinism ─────────────────────────────────────────────────

    #[test]
    fn same_seed_same_output() {
        let gen = SpaceColonization::new(Default::default()).unwrap();

        let mut rng1 = SeededRng::new(42);
        let seg1 = gen.generate_segments(&mut rng1).unwrap();

        let mut rng2 = SeededRng::new(42);
        let seg2 = gen.generate_segments(&mut rng2).unwrap();

        assert_eq!(seg1.len(), seg2.len());
        for (a, b) in seg1.iter().zip(seg2.iter()) {
            assert_eq!(a.start.x, b.start.x);
            assert_eq!(a.start.y, b.start.y);
            assert_eq!(a.start.z, b.start.z);
            assert_eq!(a.end.x, b.end.x);
            assert_eq!(a.end.y, b.end.y);
            assert_eq!(a.end.z, b.end.z);
            assert_eq!(a.radius_start, b.radius_start);
            assert_eq!(a.radius_end, b.radius_end);
            assert_eq!(a.depth, b.depth);
        }
    }

    #[test]
    fn different_seeds_differ() {
        let gen = SpaceColonization::new(Default::default()).unwrap();

        let mut rng1 = SeededRng::new(42);
        let seg1 = gen.generate_segments(&mut rng1).unwrap();

        let mut rng2 = SeededRng::new(999);
        let seg2 = gen.generate_segments(&mut rng2).unwrap();

        // Either different count or at least one position differs
        let differ = seg1.len() != seg2.len()
            || seg1.iter().zip(seg2.iter()).any(|(a, b)| {
                (a.end.x - b.end.x).abs() > 1e-6
                    || (a.end.y - b.end.y).abs() > 1e-6
                    || (a.end.z - b.end.z).abs() > 1e-6
            });
        assert!(differ, "different seeds should produce different trees");
    }

    // ── 5. Budget exceeded ─────────────────────────────────────────────

    #[test]
    fn budget_exceeded_with_low_max_nodes() {
        let params = SpaceColonizationParams {
            max_nodes: 10,
            num_attraction_points: 1000,
            crown: CrownVolume::Sphere {
                center: Vec3::new(0.0, 3.0, 0.0),
                radius: 3.0,
            },
            influence_distance: 10.0,
            kill_distance: 0.1,
            trunk_steps: 5,
            ..Default::default()
        };
        let gen = SpaceColonization::new(params).unwrap();
        let mut rng = SeededRng::new(42);
        let result = gen.generate_segments(&mut rng);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            ProcGenError::BudgetExceeded(..)
        ));
    }

    // ── 6. Depth at forks ──────────────────────────────────────────────

    #[test]
    fn depth_at_forks() {
        let gen = SpaceColonization::new(Default::default()).unwrap();
        let mut rng = SeededRng::new(42);
        let segments = gen.generate_segments(&mut rng).unwrap();

        let has_depth_gt_0 = segments.iter().any(|seg| seg.depth > 0);
        assert!(
            has_depth_gt_0,
            "should have segments with depth > 0 past fork points"
        );
    }

    // ── 7. Pipe model radii ────────────────────────────────────────────

    #[test]
    fn radii_taper_outward() {
        let gen = SpaceColonization::new(Default::default()).unwrap();
        let mut rng = SeededRng::new(42);
        let segments = gen.generate_segments(&mut rng).unwrap();
        assert!(!segments.is_empty());

        let max_radius = segments
            .iter()
            .map(|s| s.radius_start.max(s.radius_end))
            .fold(0.0f32, f32::max);

        // Max radius should be close to base_radius
        let base = 0.3; // default base_radius
        assert!(
            (max_radius - base).abs() < 0.05,
            "max radius {max_radius} should be close to base_radius {base}"
        );

        // Terminal segments should have small radii
        let min_radius = segments
            .iter()
            .map(|s| s.radius_start.min(s.radius_end))
            .fold(f32::INFINITY, f32::min);
        assert!(
            min_radius < base * 0.5,
            "terminal radii should be much smaller than base radius"
        );
    }

    // ── 8. Crown volume bounds ─────────────────────────────────────────

    #[test]
    fn sphere_points_within_bounds() {
        let center = Vec3::new(1.0, 5.0, -2.0);
        let radius = 3.0;
        let volume = CrownVolume::Sphere { center, radius };
        let mut rng = SeededRng::new(42);

        for _ in 0..1000 {
            let pt = volume.sample(&mut rng);
            let dx = pt.x - center.x;
            let dy = pt.y - center.y;
            let dz = pt.z - center.z;
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            assert!(
                dist <= radius + 1e-6,
                "sphere point at distance {dist} exceeds radius {radius}"
            );
        }
    }

    #[test]
    fn ellipsoid_points_within_bounds() {
        let center = Vec3::new(0.0, 4.0, 0.0);
        let radii = Vec3::new(2.0, 5.0, 3.0);
        let volume = CrownVolume::Ellipsoid { center, radii };
        let mut rng = SeededRng::new(42);

        for _ in 0..1000 {
            let pt = volume.sample(&mut rng);
            let nx = (pt.x - center.x) / radii.x;
            let ny = (pt.y - center.y) / radii.y;
            let nz = (pt.z - center.z) / radii.z;
            let norm = nx * nx + ny * ny + nz * nz;
            assert!(
                norm <= 1.0 + 1e-6,
                "ellipsoid point at normalized distance {norm} is outside"
            );
        }
    }

    #[test]
    fn cone_points_narrow_toward_the_apex() {
        let base = Vec3::new(0.0, 2.0, 0.0);
        let (radius, height) = (3.0, 6.0);
        let volume = CrownVolume::Cone {
            base,
            radius,
            height,
        };
        let mut rng = SeededRng::new(7);
        let (mut low, mut high) = (0, 0);
        for _ in 0..2000 {
            let pt = volume.sample(&mut rng);
            let y = pt.y - base.y;
            assert!(
                (-1e-6..=height + 1e-6).contains(&y),
                "y={y} outside the cone"
            );
            let r = (pt.x * pt.x + pt.z * pt.z).sqrt();
            let allowed = radius * (1.0 - y / height);
            assert!(r <= allowed + 1e-5, "r={r} exceeds {allowed} at y={y}");
            if y < height * 0.5 {
                low += 1;
            } else {
                high += 1;
            }
        }
        // The lower half of a cone holds 7/8 of its volume.
        assert!(low > high * 4, "low {low} vs high {high}");
    }

    #[test]
    fn hemisphere_points_within_bounds() {
        let center = Vec3::new(0.0, 3.0, 0.0);
        let radius = 4.0;
        let volume = CrownVolume::Hemisphere { center, radius };
        let mut rng = SeededRng::new(42);

        for _ in 0..1000 {
            let pt = volume.sample(&mut rng);
            let dx = pt.x - center.x;
            let dy = pt.y - center.y;
            let dz = pt.z - center.z;
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            assert!(
                dist <= radius + 1e-6,
                "hemisphere point at distance {dist} exceeds radius {radius}"
            );
            assert!(
                pt.y >= center.y - 1e-6,
                "hemisphere point y={} should be >= center.y={}",
                pt.y,
                center.y
            );
        }
    }

    // ── 9. Convenience constructors ────────────────────────────────────

    #[test]
    fn deciduous_tree_produces_output() {
        let gen = deciduous_tree(4.0, 3.0, 0.2).unwrap();
        let mut rng = SeededRng::new(42);
        let segments = gen.generate_segments(&mut rng).unwrap();
        assert!(!segments.is_empty());
    }

    #[test]
    fn columnar_tree_produces_output() {
        let gen = columnar_tree(3.0, 6.0, 2.0, 0.15).unwrap();
        let mut rng = SeededRng::new(42);
        let segments = gen.generate_segments(&mut rng).unwrap();
        assert!(!segments.is_empty());
    }

    #[test]
    fn canopy_tree_produces_output() {
        let gen = canopy_tree(5.0, 4.0, 0.25).unwrap();
        let mut rng = SeededRng::new(42);
        let segments = gen.generate_segments(&mut rng).unwrap();
        assert!(!segments.is_empty());
    }

    // ── 10. Edge cases ─────────────────────────────────────────────────

    #[test]
    fn trunk_steps_zero_still_works() {
        let params = SpaceColonizationParams {
            trunk_steps: 0,
            crown: CrownVolume::Sphere {
                center: Vec3::new(0.0, 3.0, 0.0),
                radius: 3.0,
            },
            ..Default::default()
        };
        let gen = SpaceColonization::new(params).unwrap();
        let mut rng = SeededRng::new(42);
        let segments = gen.generate_segments(&mut rng).unwrap();
        // Should still produce segments as the colonization loop runs
        assert!(!segments.is_empty());
    }

    #[test]
    fn unreachable_points_produce_trunk_only() {
        // Place the crown far away with a tiny influence distance
        let params = SpaceColonizationParams {
            crown: CrownVolume::Sphere {
                center: Vec3::new(0.0, 1000.0, 0.0),
                radius: 1.0,
            },
            influence_distance: 0.5,
            kill_distance: 0.1,
            trunk_steps: 5,
            ..Default::default()
        };
        let gen = SpaceColonization::new(params).unwrap();
        let mut rng = SeededRng::new(42);
        let segments = gen.generate_segments(&mut rng).unwrap();

        // Should only have trunk segments (trunk_steps = 5 nodes = 5 segments)
        assert_eq!(
            segments.len(),
            5,
            "unreachable crown should yield trunk-only output"
        );

        // All segments should be along the Y axis (straight trunk)
        for seg in &segments {
            assert!(
                seg.start.x.abs() < 1e-6 && seg.start.z.abs() < 1e-6,
                "trunk should be along Y axis"
            );
        }
    }
}
