//! Space colonization algorithm for organic branching structures.
//!
//! Produces `Vec<BranchSegment>` from a cloud of attraction points scattered
//! within a crown volume. The algorithm grows a tree graph toward those points,
//! creating natural-looking canopies with adaptive branching density.
//!
//! # Algorithm Overview
//!
//! 1. Scatter attraction points inside a [`CrownVolume`] via rejection sampling
//! 2. Grow a straight trunk from the root upward
//! 3. Iteratively extend the tree toward nearby attraction points
//! 4. Remove consumed attraction points when nodes grow close enough
//! 5. Assign branching depth and radii via the pipe model (da Vinci's rule)
//! 6. Convert the internal graph to `Vec<BranchSegment>`

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
}

impl CrownVolume {
    /// Sample a random point inside this volume using rejection sampling.
    fn sample(&self, rng: &mut SeededRng) -> [f32; 3] {
        match *self {
            CrownVolume::Sphere { center, radius } => loop {
                let x = rng.next_range(-radius, radius);
                let y = rng.next_range(-radius, radius);
                let z = rng.next_range(-radius, radius);
                if x * x + y * y + z * z <= radius * radius {
                    return [center.x + x, center.y + y, center.z + z];
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
                    return [center.x + x, center.y + y, center.z + z];
                }
            },
            CrownVolume::Hemisphere { center, radius } => loop {
                let x = rng.next_range(-radius, radius);
                let y = rng.next_range(0.0, radius);
                let z = rng.next_range(-radius, radius);
                if x * x + y * y + z * z <= radius * radius {
                    return [center.x + x, center.y + y, center.z + z];
                }
            },
        }
    }
}

// ─── B. Parameters ──────────────────────────────────────────────────────────

/// Parameters controlling the space colonization algorithm.
#[derive(Debug, Clone, PartialEq)]
pub struct SpaceColonizationParams {
    /// Shape of the attraction point cloud.
    pub crown: CrownVolume,
    /// Number of attraction points to scatter (default 500).
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
    /// Terminal node radius (default 0.02).
    pub tip_radius: f32,
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
        }
    }
}

// ─── C. Internal Graph Node ─────────────────────────────────────────────────

/// Internal tree graph node used during colonization.
struct BranchNode {
    position: [f32; 3],
    parent: Option<usize>,
    children: Vec<usize>,
    depth: u32,
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
        if params.num_attraction_points == 0 {
            return Err(ProcGenError::InvalidParameter {
                name: "num_attraction_points".into(),
                reason: "must be > 0".into(),
            });
        }
        Ok(Self { params })
    }

    /// Run the space colonization algorithm and produce branch segments.
    pub fn generate_segments(&self, rng: &mut SeededRng) -> Result<Vec<BranchSegment>> {
        let p = &self.params;

        // 1. Scatter attraction points
        let mut attractors: Vec<[f32; 3]> = Vec::with_capacity(p.num_attraction_points as usize);
        for _ in 0..p.num_attraction_points {
            attractors.push(p.crown.sample(rng));
        }

        // 2. Grow trunk
        let mut nodes: Vec<BranchNode> = Vec::new();
        nodes.push(BranchNode {
            position: [p.root_position.x, p.root_position.y, p.root_position.z],
            parent: None,
            children: Vec::new(),
            depth: 0,
        });

        let trunk_step_len = if p.trunk_steps > 0 {
            p.step_size
        } else {
            0.0
        };

        for i in 0..p.trunk_steps {
            let parent_pos = nodes.last().unwrap().position;
            let new_pos = [
                parent_pos[0],
                parent_pos[1] + trunk_step_len,
                parent_pos[2],
            ];
            let parent_idx = nodes.len() - 1;
            let child_idx = nodes.len();
            nodes[parent_idx].children.push(child_idx);
            nodes.push(BranchNode {
                position: new_pos,
                parent: Some(parent_idx),
                children: Vec::new(),
                depth: 0,
            });

            if nodes.len() > p.max_nodes as usize {
                return Err(ProcGenError::BudgetExceeded(format!(
                    "node count {} exceeds max_nodes {} during trunk growth",
                    nodes.len(),
                    p.max_nodes
                )));
            }
            let _ = i;
        }

        // 3. Colonization loop
        let kill_dist_sq = p.kill_distance * p.kill_distance;
        let influence_dist_sq = p.influence_distance * p.influence_distance;

        for _iteration in 0..p.max_iterations {
            if attractors.is_empty() {
                break;
            }

            // For each attraction point, find the closest node within influence distance
            // influence_map: node_index -> list of direction vectors toward attractors
            let mut influence_map: Vec<(usize, Vec<[f32; 3]>)> = Vec::new();

            for attractor in &attractors {
                let mut closest_idx: Option<usize> = None;
                let mut closest_dist_sq = f32::INFINITY;

                for (ni, node) in nodes.iter().enumerate() {
                    let dsq = dist_sq(&node.position, attractor);
                    if dsq < closest_dist_sq && dsq <= influence_dist_sq {
                        closest_dist_sq = dsq;
                        closest_idx = Some(ni);
                    }
                }

                if let Some(ni) = closest_idx {
                    let dir = [
                        attractor[0] - nodes[ni].position[0],
                        attractor[1] - nodes[ni].position[1],
                        attractor[2] - nodes[ni].position[2],
                    ];
                    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
                    if len > 1e-10 {
                        let normalized = [dir[0] / len, dir[1] / len, dir[2] / len];
                        // Find or create entry in influence_map
                        if let Some(entry) = influence_map.iter_mut().find(|(idx, _)| *idx == ni) {
                            entry.1.push(normalized);
                        } else {
                            influence_map.push((ni, vec![normalized]));
                        }
                    }
                }
            }

            if influence_map.is_empty() {
                // No attractors influenced any node — stall
                break;
            }

            // For each influenced node, average directions and create a new child
            let mut new_nodes: Vec<(usize, [f32; 3])> = Vec::new();
            for (node_idx, dirs) in &influence_map {
                let count = dirs.len() as f32;
                let avg = [
                    dirs.iter().map(|d| d[0]).sum::<f32>() / count,
                    dirs.iter().map(|d| d[1]).sum::<f32>() / count,
                    dirs.iter().map(|d| d[2]).sum::<f32>() / count,
                ];
                let len = (avg[0] * avg[0] + avg[1] * avg[1] + avg[2] * avg[2]).sqrt();
                if len > 1e-10 {
                    let norm = [avg[0] / len, avg[1] / len, avg[2] / len];
                    let parent_pos = nodes[*node_idx].position;
                    let child_pos = [
                        parent_pos[0] + norm[0] * p.step_size,
                        parent_pos[1] + norm[1] * p.step_size,
                        parent_pos[2] + norm[2] * p.step_size,
                    ];
                    new_nodes.push((*node_idx, child_pos));
                }
            }

            for (parent_idx, child_pos) in new_nodes {
                let child_idx = nodes.len();
                nodes[parent_idx].children.push(child_idx);
                nodes.push(BranchNode {
                    position: child_pos,
                    parent: Some(parent_idx),
                    children: Vec::new(),
                    depth: 0,
                });

                if nodes.len() > p.max_nodes as usize {
                    return Err(ProcGenError::BudgetExceeded(format!(
                        "node count {} exceeds max_nodes {}",
                        nodes.len(),
                        p.max_nodes
                    )));
                }
            }

            // Remove attraction points within kill_distance of any node
            attractors.retain(|att| {
                for node in &nodes {
                    if dist_sq(&node.position, att) < kill_dist_sq {
                        return false;
                    }
                }
                true
            });
        }

        // 4. Assign depths — increment only at forks (2+ children)
        assign_depths(&mut nodes);

        // 5. Assign radii via pipe model (da Vinci's rule)
        let radii = compute_pipe_radii(&nodes, p.base_radius, p.tip_radius);

        // 6. Convert to BranchSegments
        let mut segments = Vec::new();
        for (i, node) in nodes.iter().enumerate() {
            if let Some(parent_idx) = node.parent {
                let parent = &nodes[parent_idx];
                segments.push(BranchSegment {
                    start: Vec3::new(parent.position[0], parent.position[1], parent.position[2]),
                    end: Vec3::new(node.position[0], node.position[1], node.position[2]),
                    radius_start: radii[parent_idx],
                    radius_end: radii[i],
                    depth: node.depth,
                });
            }
        }

        Ok(segments)
    }
}

// ─── E. Depth Assignment ────────────────────────────────────────────────────

/// Assign branching depth to each node. Depth increments at fork points
/// (nodes with 2+ children).
fn assign_depths(nodes: &mut [BranchNode]) {
    if nodes.is_empty() {
        return;
    }
    nodes[0].depth = 0;
    // Process in index order; parents always have lower indices than children.
    for i in 0..nodes.len() {
        let parent_depth = nodes[i].depth;
        let is_fork = nodes[i].children.len() >= 2;
        let child_depth = if is_fork {
            parent_depth + 1
        } else {
            parent_depth
        };
        // Collect children to avoid borrow conflict
        let children: Vec<usize> = nodes[i].children.clone();
        for &ci in &children {
            nodes[ci].depth = child_depth;
        }
    }
}

// ─── F. Pipe Model Radii ────────────────────────────────────────────────────

/// Compute radii using the pipe model (da Vinci's rule).
///
/// Terminal nodes get `tip_radius`. Working bottom-up, each parent's radius
/// is `sqrt(sum(r_child^2))`. Finally, all radii are scaled so the root
/// gets exactly `base_radius`.
fn compute_pipe_radii(nodes: &[BranchNode], base_radius: f32, tip_radius: f32) -> Vec<f32> {
    let n = nodes.len();
    if n == 0 {
        return Vec::new();
    }

    let mut radii = vec![0.0f32; n];

    // Terminal nodes get tip_radius
    for (i, node) in nodes.iter().enumerate() {
        if node.children.is_empty() {
            radii[i] = tip_radius;
        }
    }

    // Bottom-up: process in reverse index order
    for i in (0..n).rev() {
        if !nodes[i].children.is_empty() {
            let sum_sq: f32 = nodes[i].children.iter().map(|&ci| radii[ci] * radii[ci]).sum();
            radii[i] = sum_sq.sqrt();
        }
    }

    // Scale so root gets base_radius
    let root_radius = radii[0];
    if root_radius > 1e-10 {
        let scale = base_radius / root_radius;
        for r in &mut radii {
            *r *= scale;
        }
    }

    radii
}

// ─── G. Utility ─────────────────────────────────────────────────────────────

/// Squared distance between two `[f32; 3]` points.
fn dist_sq(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    let dz = a[2] - b[2];
    dx * dx + dy * dy + dz * dz
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
    fn reject_zero_attraction_points() {
        let params = SpaceColonizationParams {
            num_attraction_points: 0,
            ..Default::default()
        };
        assert!(SpaceColonization::new(params).is_err());
    }

    // ── 2. Basic convergence ───────────────────────────────────────────

    #[test]
    fn basic_convergence_produces_segments() {
        let gen = SpaceColonization::new(Default::default()).unwrap();
        let mut rng = SeededRng::new(42);
        let segments = gen.generate_segments(&mut rng).unwrap();

        assert!(!segments.is_empty(), "should produce at least some segments");
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
        let has_crown_segments = segments
            .iter()
            .any(|seg| seg.end.y > 5.0);
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
            let dx = pt[0] - center.x;
            let dy = pt[1] - center.y;
            let dz = pt[2] - center.z;
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
            let nx = (pt[0] - center.x) / radii.x;
            let ny = (pt[1] - center.y) / radii.y;
            let nz = (pt[2] - center.z) / radii.z;
            let norm = nx * nx + ny * ny + nz * nz;
            assert!(
                norm <= 1.0 + 1e-6,
                "ellipsoid point at normalized distance {norm} is outside"
            );
        }
    }

    #[test]
    fn hemisphere_points_within_bounds() {
        let center = Vec3::new(0.0, 3.0, 0.0);
        let radius = 4.0;
        let volume = CrownVolume::Hemisphere { center, radius };
        let mut rng = SeededRng::new(42);

        for _ in 0..1000 {
            let pt = volume.sample(&mut rng);
            let dx = pt[0] - center.x;
            let dy = pt[1] - center.y;
            let dz = pt[2] - center.z;
            let dist = (dx * dx + dy * dy + dz * dz).sqrt();
            assert!(
                dist <= radius + 1e-6,
                "hemisphere point at distance {dist} exceeds radius {radius}"
            );
            assert!(
                pt[1] >= center.y - 1e-6,
                "hemisphere point y={} should be >= center.y={}",
                pt[1],
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
