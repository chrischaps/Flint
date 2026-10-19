//! A height field placed in the world, and a ray against it (ADR 0095).
//!
//! [`Heightmap`] knows its samples but not where it lies: the editor and
//! [`Terrain`](crate::Terrain) put it at the origin, `width` by `depth`.
//! A game that builds its own ground (from a function, a server, a mesh it
//! makes itself) needs to ask the same questions of it at any place and
//! scale: how high is the ground here, and where does this ray meet it.
//! [`GroundField`] answers both, so click-to-move and a camera rig can find
//! the real ground on a hill instead of a plane at some guessed height.
//!
//! Samples sit on the vertices of a grid spanning `origin .. origin + size`
//! in x and z, as `Heightmap::sample` reads them: column 0 at `origin[0]`,
//! the last column at `origin[0] + size[0]`. Between them the ground is
//! bilinear. World y is `sample × height_scale + base_y`.

use crate::heightmap::Heightmap;

/// A [`Heightmap`] placed in the world. Build it once (it scans the samples
/// for their range) and share it; every query takes `&self`.
pub struct GroundField {
    heightmap: Heightmap,
    origin: [f32; 2],
    size: [f32; 2],
    height_scale: f32,
    base_y: f32,
    /// World y of the lowest and highest sample: the box a ray is clipped to.
    y_range: [f32; 2],
}

impl GroundField {
    /// Place `heightmap` with its first sample at `origin` (world x, z) and
    /// its last at `origin + size`. A sample `h` stands at world y
    /// `h × height_scale + base_y`.
    ///
    /// Panics if the heightmap is narrower than 2 × 2 samples, or `size`
    /// isn't positive: there'd be no ground between the samples.
    pub fn new(
        heightmap: Heightmap,
        origin: [f32; 2],
        size: [f32; 2],
        height_scale: f32,
        base_y: f32,
    ) -> Self {
        assert!(
            heightmap.width >= 2 && heightmap.depth >= 2,
            "a ground field needs at least 2x2 samples, got {}x{}",
            heightmap.width,
            heightmap.depth
        );
        assert!(
            size[0] > 0.0 && size[1] > 0.0,
            "a ground field's size must be positive, got {size:?}"
        );
        let (lo, hi) = heightmap
            .heights()
            .iter()
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &h| {
                (lo.min(h), hi.max(h))
            });
        let (a, b) = (lo * height_scale + base_y, hi * height_scale + base_y);
        Self {
            heightmap,
            origin,
            size,
            height_scale,
            base_y,
            y_range: [a.min(b), a.max(b)],
        }
    }

    pub fn heightmap(&self) -> &Heightmap {
        &self.heightmap
    }

    pub fn origin(&self) -> [f32; 2] {
        self.origin
    }

    pub fn size(&self) -> [f32; 2] {
        self.size
    }

    /// World y of the lowest and highest sample.
    pub fn y_range(&self) -> [f32; 2] {
        self.y_range
    }

    /// The spacing of the samples in world units, along x and z.
    pub fn texel(&self) -> [f32; 2] {
        [
            self.size[0] / (self.heightmap.width - 1) as f32,
            self.size[1] / (self.heightmap.depth - 1) as f32,
        ]
    }

    /// Whether (x, z) lies over the field, edges included.
    pub fn contains(&self, x: f32, z: f32) -> bool {
        let (u, v) = self.uv(x, z);
        (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&v)
    }

    /// World y of the ground at (x, z), or `None` off the field.
    pub fn height_at(&self, x: f32, z: f32) -> Option<f32> {
        self.contains(x, z).then(|| self.height_clamped(x, z))
    }

    /// Where a ray first comes down onto the ground, as a world point, or
    /// `None` if it misses. `dir` needn't be unit length.
    ///
    /// The ray is clipped to the field's box (its x/z extent and the range
    /// of its heights) first, so it can start anywhere: far off the field,
    /// or high above it. One that comes in through the field's side below
    /// the ground meets the ground at the edge, as if the field had a
    /// wall there. One that starts under the ground is reported where it
    /// next comes down onto it from above, if it does. The hit is
    /// found by stepping one texel at a time over the ground and bisecting
    /// the step that crosses it, so a ridge thinner than a texel can be
    /// stepped over.
    pub fn ray(&self, origin: [f32; 3], dir: [f32; 3]) -> Option<[f32; 3]> {
        let lo = [self.origin[0], self.y_range[0], self.origin[1]];
        let hi = [
            self.origin[0] + self.size[0],
            self.y_range[1],
            self.origin[1] + self.size[1],
        ];
        let [tx, tz] = self.texel();
        march(origin, dir, lo, hi, tx.min(tz), |x, z| {
            self.height_clamped(x, z)
        })
    }

    fn uv(&self, x: f32, z: f32) -> (f32, f32) {
        (
            (x - self.origin[0]) / self.size[0],
            (z - self.origin[1]) / self.size[1],
        )
    }

    fn height_clamped(&self, x: f32, z: f32) -> f32 {
        let (u, v) = self.uv(x, z);
        self.heightmap.sample(u, v) * self.height_scale + self.base_y
    }
}

/// The first point where a ray comes down onto the ground `height(x, z)`,
/// within the box `lo..hi`. `step` is the ground's texel: the ray advances
/// that far over the ground per sample, then the crossing step is bisected.
pub(crate) fn march(
    origin: [f32; 3],
    dir: [f32; 3],
    lo: [f32; 3],
    hi: [f32; 3],
    step: f32,
    height: impl Fn(f32, f32) -> f32,
) -> Option<[f32; 3]> {
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len.is_nan() || len <= 1e-12 || step.is_nan() || step <= 0.0 {
        return None;
    }
    let d = [dir[0] / len, dir[1] / len, dir[2] / len];

    // Slab clip to the box. A flat field (lo y == hi y) still has a box: a
    // ray through its plane enters and leaves at the same t.
    let (mut t_in, mut t_out) = (0.0f32, f32::INFINITY);
    for axis in 0..3 {
        if d[axis].abs() < 1e-12 {
            if origin[axis] < lo[axis] || origin[axis] > hi[axis] {
                return None;
            }
            continue;
        }
        let a = (lo[axis] - origin[axis]) / d[axis];
        let b = (hi[axis] - origin[axis]) / d[axis];
        t_in = t_in.max(a.min(b));
        t_out = t_out.min(a.max(b));
    }
    if t_in.is_nan() || t_out.is_nan() || t_in > t_out {
        return None;
    }

    let at = |t: f32| {
        [
            origin[0] + d[0] * t,
            origin[1] + d[1] * t,
            origin[2] + d[2] * t,
        ]
    };
    // Above the ground: positive. On it: zero.
    let above = |t: f32| {
        let p = at(t);
        p[1] - height(p[0], p[2])
    };

    // One texel over the ground per step; a vertical ray takes the box's
    // height in one step, which is exact, since the ground under it is one
    // height.
    let over = (d[0] * d[0] + d[2] * d[2]).sqrt();
    let span = t_out - t_in;
    let dt = if over > 1e-9 {
        (step / over).min(span)
    } else {
        span
    };
    let steps = if dt > 0.0 {
        ((span / dt).ceil() as u64).clamp(1, 1 << 22)
    } else {
        1
    };

    let mut prev_t = t_in;
    let mut prev = above(t_in);
    // On the ground as it enters, or coming in through the field's side
    // under the ground: it meets the field's edge.
    if prev == 0.0 || (prev < 0.0 && t_in > 0.0) {
        let p = at(t_in);
        return Some([p[0], height(p[0], p[2]), p[2]]);
    }
    for i in 1..=steps {
        let t = if i == steps {
            t_out
        } else {
            t_in + dt * i as f32
        };
        let cur = above(t);
        if prev > 0.0 && cur <= 0.0 {
            let (mut a, mut b) = (prev_t, t);
            for _ in 0..24 {
                let mid = 0.5 * (a + b);
                if above(mid) > 0.0 {
                    a = mid;
                } else {
                    b = mid;
                }
            }
            let p = at(0.5 * (a + b));
            return Some([p[0], height(p[0], p[2]), p[2]]);
        }
        prev_t = t;
        prev = cur;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ramp along x: y = x / 4 over 0..100, sampled every `texel` units,
    /// placed at `origin`.
    fn ramp(origin: [f32; 2], texel: f32) -> GroundField {
        let n = (100.0 / texel) as u32 + 1;
        let mut heights = Vec::with_capacity((n * n) as usize);
        for _row in 0..n {
            for col in 0..n {
                heights.push(col as f32 * texel / 4.0);
            }
        }
        GroundField::new(
            Heightmap::from_raw(heights, n, n),
            origin,
            [100.0, 100.0],
            1.0,
            0.0,
        )
    }

    /// Hills: a sum of sines, sampled every 2 units over 200 x 200.
    fn hills() -> (GroundField, impl Fn(f32, f32) -> f32) {
        let f = |x: f32, z: f32| 12.0 * (x * 0.031).sin() * (z * 0.047).cos() + 15.0;
        let n = 101u32;
        let mut heights = Vec::with_capacity((n * n) as usize);
        for row in 0..n {
            for col in 0..n {
                heights.push(f(-50.0 + col as f32 * 2.0, 30.0 + row as f32 * 2.0));
            }
        }
        let field = GroundField::new(
            Heightmap::from_raw(heights, n, n),
            [-50.0, 30.0],
            [200.0, 200.0],
            1.0,
            0.0,
        );
        (field, f)
    }

    #[test]
    fn height_at_reads_the_placed_field() {
        let g = ramp([1000.0, -500.0], 1.0);
        assert!((g.height_at(1040.0, -450.0).unwrap() - 10.0).abs() < 1e-3);
        assert!((g.height_at(1040.5, -450.0).unwrap() - 10.125).abs() < 1e-3);
        assert_eq!(g.height_at(999.0, -450.0), None);
        assert_eq!(g.height_at(1040.0, -399.0), None);
        assert_eq!(g.y_range(), [0.0, 25.0]);
    }

    #[test]
    fn a_ray_down_lands_on_the_slope() {
        let g = ramp([0.0, 0.0], 1.0);
        let hit = g.ray([60.0, 500.0, 20.0], [0.0, -1.0, 0.0]).unwrap();
        assert!((hit[0] - 60.0).abs() < 1e-3 && (hit[2] - 20.0).abs() < 1e-3);
        assert!((hit[1] - 15.0).abs() < 1e-3, "{hit:?}");
    }

    #[test]
    fn a_slant_ray_finds_the_slope_not_the_plane_below() {
        // From high on the low side, looking up the slope: a plane at y = 0
        // would put the hit far past the true point.
        let g = ramp([0.0, 0.0], 2.0);
        let truth = [70.0f32, 17.5, 50.0];
        let eye = [10.0f32, 60.0, 50.0];
        let dir = [truth[0] - eye[0], truth[1] - eye[1], truth[2] - eye[2]];
        let hit = g.ray(eye, dir).unwrap();
        let err = ((hit[0] - truth[0]).powi(2) + (hit[2] - truth[2]).powi(2)).sqrt();
        assert!(err < 1e-2, "{hit:?}");
    }

    #[test]
    fn a_ray_from_off_the_field_comes_in() {
        let g = ramp([0.0, 0.0], 1.0);
        // Starts 400 units off the field's low edge, comes down in the middle.
        let eye = [-400.0f32, 100.0, 50.0];
        let truth = [40.0f32, 10.0, 50.0];
        let dir = [truth[0] - eye[0], truth[1] - eye[1], truth[2] - eye[2]];
        let hit = g.ray(eye, dir).unwrap();
        assert!((hit[0] - 40.0).abs() < 1e-2, "{hit:?}");
    }

    #[test]
    fn misses_are_none() {
        let g = ramp([0.0, 0.0], 1.0);
        // Up, into the sky.
        assert_eq!(g.ray([50.0, 100.0, 50.0], [0.0, 1.0, 0.0]), None);
        // Down, beside the field.
        assert_eq!(g.ray([150.0, 100.0, 50.0], [0.0, -1.0, 0.0]), None);
        // Level, above the highest ground.
        assert_eq!(g.ray([-10.0, 30.0, 50.0], [1.0, 0.0, 0.0]), None);
        // No direction.
        assert_eq!(g.ray([50.0, 100.0, 50.0], [0.0, 0.0, 0.0]), None);
    }

    #[test]
    fn a_ray_under_the_ground_comes_out_first() {
        // Level at y = 5, starting under the ramp's high side and running
        // down it, never comes down onto it from above: no hit.
        let g = ramp([0.0, 0.0], 1.0);
        assert_eq!(g.ray([90.0, 5.0, 50.0], [-1.0, 0.0, 0.0]), None);
    }

    #[test]
    fn a_ray_in_through_the_side_under_the_ground_meets_the_edge() {
        // Level at y = 10, coming in at the ramp's high edge (ground 25).
        let g = ramp([0.0, 0.0], 1.0);
        let hit = g.ray([300.0, 10.0, 50.0], [-1.0, 0.0, 0.0]).unwrap();
        assert!(
            (hit[0] - 100.0).abs() < 1e-3 && (hit[1] - 25.0).abs() < 1e-3,
            "{hit:?}"
        );
    }

    #[test]
    fn hits_on_hills_lie_on_the_ground_within_a_texel() {
        let (g, f) = hills();
        let eye = [-120.0f32, 90.0, -40.0];
        let mut n = 0;
        for i in 0..20 {
            for j in 0..20 {
                // Aim a little under points on the true surface (the
                // bilinear ground can sit a hair below the sines), so every
                // ray comes down by the target. The first hit may be a hill
                // in front of it, so check the hit is on the ground and no
                // farther than the target.
                let tx = -40.0 + i as f32 * 9.5;
                let tz = 40.0 + j as f32 * 9.5;
                let target = [tx, f(tx, tz) - 0.1, tz];
                let dir = [target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]];
                let hit = g.ray(eye, dir).unwrap();
                assert!((hit[1] - g.height_at(hit[0], hit[2]).unwrap()).abs() < 1e-3);
                let d_hit = (hit[0] - eye[0]).hypot(hit[2] - eye[2]);
                let d_target = (tx - eye[0]).hypot(tz - eye[2]);
                assert!(d_hit <= d_target + 2.0, "{hit:?} past {target:?}");
                if d_target - d_hit < 2.0 {
                    n += 1;
                }
            }
        }
        // Most targets are in plain view of the eye, and land on themselves.
        assert!(n > 200, "{n}");
    }
}
