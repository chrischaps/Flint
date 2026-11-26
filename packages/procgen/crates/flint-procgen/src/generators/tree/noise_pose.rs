//! Noise poses (#506): a tree that lives without regrowing.
//!
//! Regrowth needs the same segments from every seed, which only the built-in
//! L-system gives. A noise pose keeps the lineage's segments and turns each one
//! by smooth noise sampled at its middle, so it works on any structure: a
//! space-colonization pine, a text-grammar willow, the grove's oak. Proven by
//! #252's prototype (`living_spike noise`), whose field this is, unchanged.

use flint_core::quat::quat_from_axis_angle;
use flint_core::Vec3;

use crate::living::{LivingPose, LivingSkeleton, SegmentPose};
use crate::spec::NoiseConfig;
use crate::types::BranchSegment;
use crate::SeededRng;

/// Plane waves summed per channel. Four is enough to stop the field reading
/// as one stripe, and few enough to stay smooth.
const WAVES: usize = 4;

/// The pose `shape_seed` gives `skeleton`: each rest segment bent by the
/// noise at its midpoint, never twisted, with a small change of length.
///
/// A twist would only spin a tube's leaves, so the turn's axis is the noise
/// with its component along the segment taken out. Children start at their
/// parent's posed end ([`LivingSkeleton::posed_segments`]), so limbs stay
/// attached whatever the noise does.
pub(super) fn noise_pose(
    skeleton: &LivingSkeleton,
    noise: &NoiseConfig,
    shape_seed: u64,
) -> LivingPose {
    let field = Field::new(skeleton.lineage_seed, shape_seed, noise.wavelength);
    let turn = noise.turn.to_radians();
    let segments = skeleton
        .segments
        .iter()
        .map(|s| {
            let mid = (s.start + s.end) * 0.5;
            let dir = s.direction();
            if dir.length() < 0.5 {
                return SegmentPose::IDENTITY;
            }
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
                length_scale: (1.0 + noise.stretch * field.at(3, mid)).clamp(0.5, 1.5),
                radius_scale: 1.0,
                slide: [0.0; 3],
            }
        })
        .collect();
    LivingPose {
        shape_seed,
        segments,
    }
}

/// Four channels of smooth noise with unit variance, each a sum of plane
/// waves whose directions, frequencies and phases come from the shape seed's
/// own fork of the lineage. One pose, one field: the next pose is a new
/// field, not the same one moved on, so poses don't drift one way.
struct Field {
    waves: [[(Vec3, f32); WAVES]; 4],
}

impl Field {
    fn new(lineage_seed: u64, shape_seed: u64, wavelength: f32) -> Self {
        let mut rng = SeededRng::new(lineage_seed).fork(&format!("noise{shape_seed}"));
        let k = std::f32::consts::TAU / wavelength.max(NoiseConfig::MIN_WAVELENGTH);
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

/// Rewrite each segment's `depth` as its branch order: 1 for a limb that
/// leaves the trunk, one more at every fork for each child but the one that
/// carries its parent on (the thickest, then the straightest, then the
/// first grown).
///
/// Space colonization's own depth counts growth steps along a limb (31
/// levels on the pine), so per-level stagger spread a tree over five beats
/// and it never held, and the variance weight read a limb's base as its
/// tips' kin. Branch order is what the L-system's depth already is. Parents
/// come before their children ([`super::branches::link_segments`]).
pub(super) fn set_branch_orders(segments: &mut [BranchSegment]) {
    let mut heir: Vec<Option<u32>> = vec![None; segments.len()];
    for s in segments.iter() {
        let Some(p) = s.parent else { continue };
        let parent = &segments[p as usize];
        let carries_on = match heir[p as usize] {
            None => true,
            Some(c) => {
                let c = &segments[c as usize];
                let straight = |x: &BranchSegment| x.direction().dot(&parent.direction());
                s.radius_start
                    .total_cmp(&c.radius_start)
                    .then(straight(s).total_cmp(&straight(c)))
                    .is_gt()
            }
        };
        if carries_on {
            heir[p as usize] = Some(s.id);
        }
    }
    for i in 0..segments.len() {
        let order = match segments[i].parent {
            None => 1,
            Some(p) => segments[p as usize].depth + u32::from(heir[p as usize] != Some(i as u32)),
        };
        segments[i].depth = order;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(id: u32, parent: Option<u32>, start: Vec3, end: Vec3, radius: f32) -> BranchSegment {
        BranchSegment {
            start,
            end,
            radius_start: radius,
            radius_end: radius,
            depth: 99,
            id,
            parent,
        }
    }

    #[test]
    fn branch_order_counts_forks_from_the_trunk() {
        let up = Vec3::UP;
        let side = Vec3::RIGHT;
        let mut segments = vec![
            // A limb off the trunk, three steps long.
            segment(0, None, Vec3::ZERO, up, 0.3),
            segment(1, Some(0), up, up * 2.0, 0.25),
            // A thin side twig, then the limb carrying on beside it.
            segment(2, Some(1), up * 2.0, up * 2.0 + side, 0.05),
            segment(3, Some(1), up * 2.0, up * 3.0, 0.2),
            // A twig off the twig, and the twig going on.
            segment(4, Some(2), up * 2.0 + side, up * 2.5 + side * 1.5, 0.02),
            segment(5, Some(2), up * 2.0 + side, up * 2.0 + side * 2.0, 0.04),
            // A second limb off the trunk.
            segment(6, None, up * 0.5, up * 0.5 - side, 0.2),
        ];
        set_branch_orders(&mut segments);
        let orders: Vec<u32> = segments.iter().map(|s| s.depth).collect();
        assert_eq!(orders, [1, 1, 2, 1, 3, 2, 1]);
    }

    #[test]
    fn of_two_as_thick_the_straighter_carries_on() {
        let up = Vec3::UP;
        let mut segments = vec![
            segment(0, None, Vec3::ZERO, up, 0.1),
            segment(1, Some(0), up, up + Vec3::RIGHT, 0.1),
            segment(2, Some(0), up, up * 2.0, 0.1),
        ];
        set_branch_orders(&mut segments);
        assert_eq!(segments[1].depth, 2);
        assert_eq!(segments[2].depth, 1);
    }

    #[test]
    fn the_field_has_about_unit_variance_and_is_smooth() {
        let field = Field::new(7, 3, 3.0);
        let samples: Vec<f32> = (0..2000)
            .map(|i| {
                let t = i as f32 * 0.173;
                field.at(
                    i % 4,
                    Vec3::new(t.sin() * 9.0, t * 0.05, (t * 1.3).cos() * 9.0),
                )
            })
            .collect();
        let mean = samples.iter().sum::<f32>() / samples.len() as f32;
        let var = samples.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / samples.len() as f32;
        assert!((0.6..1.4).contains(&var), "variance {var}");
        // A centimetre apart, a 3 m field barely changes.
        let p = Vec3::new(1.0, 2.0, 3.0);
        assert!((field.at(0, p) - field.at(0, p + Vec3::RIGHT * 0.01)).abs() < 0.05);
    }
}
