//! Analytic two-bone inverse kinematics (ADR 0070).
//!
//! The chain is root → mid → tip with fixed segment lengths `a` (root→mid) and
//! `b` (mid→tip). Given a target for the tip and a pole point that says which
//! side the mid joint bends toward, the solution is closed-form: the law of
//! cosines fixes the angle at the root, and the bend plane is spanned by the
//! root→target line and the pole. Pure geometry, no world or entities; the
//! animation crate's `ik_pass` turns the result into node rotations.

use crate::Vec3;

/// Joint positions produced by [`solve_two_bone`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TwoBoneSolution {
    /// The mid joint (elbow / knee).
    pub mid: Vec3,
    /// Where the tip lands: the target when reachable, else the closest point
    /// on the root→target line with the chain fully extended (or fully folded).
    pub tip: Vec3,
}

/// Solve a two-bone chain. Returns `None` when the target sits on the root or
/// the pole is colinear with the root→target line, so the bend plane is
/// undefined; callers keep the previous pose in that case.
pub fn solve_two_bone(
    root: Vec3,
    target: Vec3,
    pole: Vec3,
    upper_len: f32,
    fore_len: f32,
) -> Option<TwoBoneSolution> {
    const EPS: f32 = 1e-3;
    let a = upper_len;
    let b = fore_len;
    if a <= 0.0 || b <= 0.0 {
        return None;
    }
    let to_target = target - root;
    let d = to_target.length();
    if d < 1e-6 {
        return None;
    }
    let t_hat = to_target * (1.0 / d);

    // Keep a hair inside the reachable annulus so the mid direction stays well
    // defined at full extension and full fold.
    let d_c = d.clamp((a - b).abs() * (1.0 + EPS), (a + b) * (1.0 - EPS));

    let to_pole = pole - root;
    let n = t_hat.cross(&to_pole);
    if n.length() < 1e-6 {
        return None;
    }
    let n = n.normalized();
    // In-plane direction perpendicular to the target line, on the pole's side.
    let u = n.cross(&t_hat);

    let cos_alpha = ((a * a + d_c * d_c - b * b) / (2.0 * a * d_c)).clamp(-1.0, 1.0);
    let alpha = cos_alpha.acos();
    let mid = root + t_hat * (a * alpha.cos()) + u * (a * alpha.sin());
    let tip = root + t_hat * d_c;
    Some(TwoBoneSolution { mid, tip })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    fn dist(a: Vec3, b: Vec3) -> f32 {
        (a - b).length()
    }

    #[test]
    fn reachable_target_lands_exactly_on_the_pole_side() {
        let root = Vec3::new(0.0, 1.0, 0.0);
        let target = Vec3::new(0.3, 0.9, -0.2);
        let pole = Vec3::new(0.5, 0.0, 0.0);
        let s = solve_two_bone(root, target, pole, 0.25, 0.2).unwrap();
        assert!(dist(s.tip, target) < 1e-5, "{:?}", s.tip);
        assert!(close(dist(root, s.mid), 0.25));
        assert!(close(dist(s.mid, s.tip), 0.2));
        // The mid joint sits on the pole's side of the root→target line.
        let t_hat = (target - root).normalized();
        let perp = (s.mid - root) - t_hat * (s.mid - root).dot(&t_hat);
        let pole_perp = (pole - root) - t_hat * (pole - root).dot(&t_hat);
        assert!(perp.dot(&pole_perp) > 0.0);
    }

    #[test]
    fn flipping_the_pole_flips_the_bend() {
        let root = Vec3::ZERO;
        let target = Vec3::new(0.3, 0.0, 0.0);
        let up = solve_two_bone(root, target, Vec3::new(0.0, 1.0, 0.0), 0.25, 0.25).unwrap();
        let down = solve_two_bone(root, target, Vec3::new(0.0, -1.0, 0.0), 0.25, 0.25).unwrap();
        assert!(up.mid.y > 0.1 && down.mid.y < -0.1, "{:?} {:?}", up.mid, down.mid);
        assert!(close(up.mid.y, -down.mid.y));
    }

    #[test]
    fn overreach_straightens_toward_the_target() {
        let root = Vec3::ZERO;
        let target = Vec3::new(2.0, 0.0, 0.0);
        let s = solve_two_bone(root, target, Vec3::new(0.0, 1.0, 0.0), 0.25, 0.25).unwrap();
        assert!(s.tip.x < 0.5 && s.tip.x > 0.49, "{:?}", s.tip);
        assert!(close(s.tip.y, 0.0) && close(s.tip.z, 0.0));
        assert!(s.mid.y.abs() < 0.02 && close(dist(root, s.mid), 0.25));
    }

    #[test]
    fn degenerate_inputs_return_none() {
        let root = Vec3::ZERO;
        let target = Vec3::new(0.3, 0.0, 0.0);
        // Pole on the target line: no bend plane.
        assert!(solve_two_bone(root, target, Vec3::new(1.0, 0.0, 0.0), 0.25, 0.25).is_none());
        // Target on the root.
        assert!(solve_two_bone(root, root, Vec3::new(0.0, 1.0, 0.0), 0.25, 0.25).is_none());
        assert!(solve_two_bone(root, target, Vec3::new(0.0, 1.0, 0.0), 0.0, 0.25).is_none());
    }
}
