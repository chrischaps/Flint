//! A path through a list of points, for a camera to fly (#1158).
//!
//! Centripetal Catmull-Rom through every point, read by distance: `u` from 0 to 1 is the start to the
//! end at an even speed, however unevenly the points are spaced. A spline
//! entity's path is sampled once at load; this one is made from a script's
//! list of points, so it is cheap enough to build every frame (a table of
//! [`STEPS_PER_SEGMENT`] lengths a segment).
//!
//! Centripetal, not the uniform curve of [`crate::spline::catmull_rom`]: a
//! uniform curve through unevenly spaced points swings wide and can loop
//! back on itself near a short segment, which a camera shows as a stall
//! and a reverse. The centripetal curve (knots spaced by the square root
//! of each chord) never forms a loop or a cusp inside a segment.
//!
//! [`PointPath::frame`] adds the direction of travel and a bank: a roll
//! into the turn from the path's own curvature, smoothed over a window of
//! its length so it never jumps where two segments meet (Catmull-Rom's
//! second derivative does).

use flint_core::Vec3;

/// Lengths measured a segment: the table distance is read from.
pub const STEPS_PER_SEGMENT: usize = 16;

/// The share of a path's length its curvature is measured over, either side.
pub const BANK_WINDOW: f32 = 0.03;

/// A Catmull-Rom path through points, read by distance along it.
#[derive(Debug, Clone)]
pub struct PointPath {
    points: Vec<Vec3>,
    closed: bool,
    /// Distance from the start at each step, `segments * STEPS + 1` of them.
    lengths: Vec<f32>,
}

/// Where a path puts a camera at one `u`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathFrame {
    pub position: Vec3,
    /// Unit direction of travel.
    pub forward: Vec3,
    /// Roll into the turn, radians: positive leans the camera's up to its
    /// right, as a turn to the right does (`set_camera_roll`'s sense).
    pub roll: f32,
}

impl PointPath {
    /// A path through `points`; a closed one runs from the last back to the
    /// first. `None` with fewer than two points (three for a closed path)
    /// or all of them in one place.
    pub fn new(points: Vec<Vec3>, closed: bool) -> Option<Self> {
        if points.len() < if closed { 3 } else { 2 } {
            return None;
        }
        let mut path = Self {
            points,
            closed,
            lengths: Vec::new(),
        };
        let steps = path.segments() * STEPS_PER_SEGMENT;
        let mut lengths = Vec::with_capacity(steps + 1);
        let mut total = 0.0;
        let mut last = path.at_param(0.0);
        lengths.push(0.0);
        for i in 1..=steps {
            let p = path.at_param(i as f32 / STEPS_PER_SEGMENT as f32);
            total += (p - last).length();
            lengths.push(total);
            last = p;
        }
        if total <= 1e-6 {
            return None;
        }
        path.lengths = lengths;
        Some(path)
    }

    /// Whether the path closes on itself.
    pub fn closed(&self) -> bool {
        self.closed
    }

    /// The path's length, metres.
    pub fn length(&self) -> f32 {
        *self.lengths.last().unwrap_or(&0.0)
    }

    fn segments(&self) -> usize {
        if self.closed {
            self.points.len()
        } else {
            self.points.len() - 1
        }
    }

    /// Control point `i`: a closed path wraps; an open one's ends are
    /// mirrored, so the curve leaves its first point heading at its second.
    fn control(&self, i: isize) -> Vec3 {
        let n = self.points.len() as isize;
        if self.closed {
            return self.points[i.rem_euclid(n) as usize];
        }
        if i < 0 {
            let (a, b) = (self.points[0], self.points[1]);
            return a * 2.0 - b;
        }
        if i >= n {
            let (a, b) = (self.points[n as usize - 1], self.points[n as usize - 2]);
            return a * 2.0 - b;
        }
        self.points[i as usize]
    }

    /// The curve at parameter `x`: segment `floor(x)`, `fract(x)` into it.
    fn at_param(&self, x: f32) -> Vec3 {
        let segments = self.segments();
        let x = x.clamp(0.0, segments as f32);
        let i = (x.floor() as usize).min(segments - 1);
        let t = x - i as f32;
        let i = i as isize;
        centripetal(
            self.control(i - 1),
            self.control(i),
            self.control(i + 1),
            self.control(i + 2),
            t,
        )
    }

    /// Distance `s` along the path as a curve parameter.
    fn param_at(&self, s: f32) -> f32 {
        let s = s.clamp(0.0, self.length());
        // The first step whose length reaches s
        let k = self.lengths.partition_point(|&l| l < s).max(1);
        let (a, b) = (self.lengths[k - 1], self.lengths[k]);
        let f = if b > a { (s - a) / (b - a) } else { 0.0 };
        ((k - 1) as f32 + f) / STEPS_PER_SEGMENT as f32
    }

    /// `u` as a distance: a closed path wraps, an open one stops at its ends.
    fn distance_of(&self, u: f32) -> f32 {
        let u = if self.closed {
            u.rem_euclid(1.0)
        } else {
            u.clamp(0.0, 1.0)
        };
        u * self.length()
    }

    /// The point at distance `s` (wrapped round a closed path).
    pub fn at_distance(&self, s: f32) -> Vec3 {
        let s = if self.closed {
            s.rem_euclid(self.length())
        } else {
            s
        };
        self.at_param(self.param_at(s))
    }

    /// The point `u` of the way along, by distance.
    pub fn at(&self, u: f32) -> Vec3 {
        self.at_distance(self.distance_of(u))
    }

    /// The unit direction of travel at distance `s`.
    fn tangent_at(&self, s: f32) -> Vec3 {
        let e = (self.length() * 1e-3).max(1e-4);
        let (a, b) = if self.closed {
            (s - e, s + e)
        } else {
            ((s - e).max(0.0), (s + e).min(self.length()))
        };
        (self.at_distance(b) - self.at_distance(a)).normalized()
    }

    /// The camera at `u`: where, which way it travels, and its roll into
    /// the turn. `bank` is how hard it leans: the roll's tangent is `bank`
    /// times the turn's sideways curvature (one over its radius, metres),
    /// so `bank = 5` rolls 45 degrees in a turn of 5 m radius. The roll is
    /// held to `max_bank` radians either way; `bank = 0` keeps it level.
    pub fn frame(&self, u: f32, bank: f32, max_bank: f32) -> PathFrame {
        let s = self.distance_of(u);
        let position = self.at_distance(s);
        let forward = self.tangent_at(s);
        let roll = if bank == 0.0 {
            0.0
        } else {
            let lateral = self.lateral_curvature(s, forward);
            (bank * lateral)
                .atan()
                .clamp(-max_bank.abs(), max_bank.abs())
        };
        PathFrame {
            position,
            forward,
            roll,
        }
    }

    /// The turn's curvature across the direction of travel at `s`, 1/m:
    /// positive to the right. Measured as the tangent's change over
    /// [`BANK_WINDOW`] of the length either side, so it moves smoothly.
    fn lateral_curvature(&self, s: f32, forward: Vec3) -> f32 {
        let h = (self.length() * BANK_WINDOW).max(1e-3);
        let (a, b) = if self.closed {
            (s - h, s + h)
        } else {
            ((s - h).max(0.0), (s + h).min(self.length()))
        };
        if b - a <= 1e-6 {
            return 0.0;
        }
        let turn = (self.tangent_at(b) - self.tangent_at(a)) * (1.0 / (b - a));
        let right = forward.cross(&Vec3::UP);
        if right.length() < 1e-4 {
            // Straight up or down: no sideways to lean to
            return 0.0;
        }
        turn.dot(&right.normalized())
    }
}

/// The centripetal Catmull-Rom segment from `p1` to `p2`, `t` 0 to 1
/// (Barry and Goldman's pyramid, knots spaced by the square root of each
/// chord). Coincident neighbours get a tiny knot gap rather than a
/// division by zero.
fn centripetal(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32) -> Vec3 {
    let gap = |a: Vec3, b: Vec3| (a - b).length().sqrt().max(1e-4);
    let t0 = 0.0;
    let t1 = t0 + gap(p0, p1);
    let t2 = t1 + gap(p1, p2);
    let t3 = t2 + gap(p2, p3);
    let t = t1 + (t2 - t1) * t;
    let lerp = |a: Vec3, b: Vec3, ta: f32, tb: f32| {
        a * ((tb - t) / (tb - ta)) + b * ((t - ta) / (tb - ta))
    };
    let a1 = lerp(p0, p1, t0, t1);
    let a2 = lerp(p1, p2, t1, t2);
    let a3 = lerp(p2, p3, t2, t3);
    let b1 = lerp(a1, a2, t0, t2);
    let b2 = lerp(a2, a3, t1, t3);
    lerp(b1, b2, t1, t2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: Vec3, b: Vec3, tol: f32) -> bool {
        (a - b).length() < tol
    }

    /// A circle of radius `r` round the origin at height 1, `n` points,
    /// going clockwise seen from above (a right turn throughout).
    fn circle(r: f32, n: usize) -> Vec<Vec3> {
        (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                // Clockwise from above with Y up: x = sin, z = -cos runs
                // -Z toward +X, which is a turn to the right
                Vec3::new(r * a.sin(), 1.0, -r * a.cos())
            })
            .collect()
    }

    #[test]
    fn a_path_passes_through_its_points_and_ends_on_its_last() {
        let pts = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(4.0, 1.0, 0.0),
            Vec3::new(4.0, 1.0, 6.0),
        ];
        let path = PointPath::new(pts.clone(), false).unwrap();
        assert!(near(path.at(0.0), pts[0], 1e-4));
        assert!(near(path.at(1.0), pts[2], 1e-4));
        assert!(
            near(path.at(2.0), pts[2], 1e-4),
            "an open path holds its end"
        );
        // The middle point is on the curve, somewhere along it
        let closest = (0..=1000)
            .map(|i| (path.at(i as f32 / 1000.0) - pts[1]).length())
            .fold(f32::MAX, f32::min);
        assert!(closest < 0.02, "{closest}");
    }

    #[test]
    fn u_is_distance_so_the_speed_is_even_however_the_points_are_spaced() {
        // One short segment, one long: by segment the camera would crawl
        // then race
        let pts = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
        ];
        let path = PointPath::new(pts, false).unwrap();
        assert!((path.length() - 10.0).abs() < 0.05, "{}", path.length());
        let step = path.length() / 20.0;
        // The arc between neighbouring u, in fine chords
        let arc = |u0: f32, u1: f32| {
            (0..50)
                .map(|k| {
                    let a = u0 + (u1 - u0) * k as f32 / 50.0;
                    let b = u0 + (u1 - u0) * (k + 1) as f32 / 50.0;
                    (path.at(b) - path.at(a)).length()
                })
                .sum::<f32>()
        };
        let steps: Vec<f32> = (0..20)
            .map(|i| arc(i as f32 / 20.0, (i + 1) as f32 / 20.0))
            .collect();
        for s in &steps {
            assert!((s - step).abs() < 0.03 * step, "{steps:?}");
        }
    }

    #[test]
    fn unevenly_spaced_points_never_make_the_camera_double_back() {
        // A uniform Catmull-Rom runs backwards inside the short segment
        // here; the centripetal path only ever moves forward
        let pts = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
        ];
        let path = PointPath::new(pts, false).unwrap();
        let xs: Vec<f32> = (0..=400).map(|i| path.at(i as f32 / 400.0).x).collect();
        assert!(xs.windows(2).all(|w| w[1] >= w[0] - 1e-5), "{xs:?}");
        // The uniform curve the spline entities use does double back
        let p = |x| Vec3::new(x, 0.0, 0.0);
        let uniform: Vec<f32> = (0..=100)
            .map(|i| {
                crate::spline::catmull_rom(p(-1.0), p(0.0), p(1.0), p(10.0), i as f32 / 100.0).x
            })
            .collect();
        assert!(uniform.windows(2).any(|w| w[1] < w[0] - 1e-3));
    }

    #[test]
    fn a_closed_path_wraps_round() {
        let path = PointPath::new(circle(5.0, 8), true).unwrap();
        assert!(near(path.at(0.25), path.at(1.25), 1e-3));
        assert!(near(path.at(-0.25), path.at(0.75), 1e-3));
        // A circle's length, near enough for eight points
        let c = std::f32::consts::TAU * 5.0;
        assert!((path.length() - c).abs() / c < 0.01, "{}", path.length());
    }

    #[test]
    fn a_right_turn_banks_right_by_its_radius_and_a_straight_stays_level() {
        let path = PointPath::new(circle(5.0, 12), true).unwrap();
        for i in 0..20 {
            let f = path.frame(i as f32 / 20.0, 5.0, 1.5);
            // tan(roll) = bank / radius = 1: 45 degrees, to the right
            assert!(
                (f.roll - std::f32::consts::FRAC_PI_4).abs() < 0.03,
                "u {}: roll {}",
                i as f32 / 20.0,
                f.roll
            );
        }
        // Run the circle the other way: a left turn, a roll to the left
        let mut left = circle(5.0, 12);
        left.reverse();
        let path = PointPath::new(left, true).unwrap();
        assert!(path.frame(0.3, 5.0, 1.5).roll < -0.7);
        // A straight line has nothing to lean into
        let line = PointPath::new(
            vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, -10.0)],
            false,
        )
        .unwrap();
        assert_eq!(line.frame(0.5, 5.0, 1.5).roll, 0.0);
    }

    #[test]
    fn the_bank_is_held_to_its_limit_and_moves_smoothly_through_the_points() {
        let path = PointPath::new(circle(5.0, 6), true).unwrap();
        assert!((path.frame(0.1, 100.0, 0.3).roll - 0.3).abs() < 1e-5);
        // Across the knots, the roll never jumps between neighbouring frames
        let pts = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(5.0, 0.0, -5.0),
            Vec3::new(0.0, 0.0, -10.0),
            Vec3::new(5.0, 0.0, -15.0),
        ];
        let path = PointPath::new(pts, false).unwrap();
        let rolls: Vec<f32> = (0..=600)
            .map(|i| path.frame(i as f32 / 600.0, 4.0, 1.0).roll)
            .collect();
        let worst = rolls
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(worst < 0.03, "largest step between frames {worst}");
    }

    #[test]
    fn too_few_points_or_none_apart_is_no_path() {
        assert!(PointPath::new(vec![Vec3::ZERO], false).is_none());
        assert!(PointPath::new(vec![Vec3::ZERO, Vec3::ONE], true).is_none());
        assert!(PointPath::new(vec![Vec3::ONE, Vec3::ONE], false).is_none());
    }
}
