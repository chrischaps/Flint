//! Lightweight xorshift32 PRNG — no external crate needed.
//!
//! Every emitter owns one, seeded from the effect seed, the owning entity
//! and the emitter index, so spawn order across emitters never changes a
//! result and headless snapshots are reproducible (ADR 0068).

#[derive(Clone, Debug)]
pub struct ParticleRng {
    state: u32,
}

impl ParticleRng {
    pub fn new(seed: u32) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    fn next_u32(&mut self) -> u32 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.state = x;
        x
    }

    /// Returns a float in [0, 1)
    pub fn next_f32(&mut self) -> f32 {
        // 24 mantissa bits keeps the result strictly below 1.0.
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Returns a float in [min, max); order-insensitive.
    pub fn range(&mut self, min: f32, max: f32) -> f32 {
        let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
        lo + self.next_f32() * (hi - lo)
    }

    /// Returns an integer in [min, max] (inclusive); order-insensitive.
    pub fn range_u32(&mut self, min: u32, max: u32) -> u32 {
        let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
        if lo == hi {
            return lo;
        }
        lo + (self.next_u32() % (hi - lo + 1))
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f32) -> bool {
        if p >= 1.0 {
            return true;
        }
        if p <= 0.0 {
            return false;
        }
        self.next_f32() < p
    }

    /// Uniform point on the unit disc.
    pub fn unit_disc(&mut self) -> [f32; 2] {
        let r = self.next_f32().sqrt();
        let theta = self.range(0.0, std::f32::consts::TAU);
        [r * theta.cos(), r * theta.sin()]
    }

    /// Returns a random unit direction vector (uniformly on sphere surface)
    pub fn random_direction(&mut self) -> [f32; 3] {
        // Marsaglia method for uniform sphere sampling
        loop {
            let x = self.range(-1.0, 1.0);
            let y = self.range(-1.0, 1.0);
            let s = x * x + y * y;
            if s < 1.0 {
                let factor = 2.0 * (1.0 - s).sqrt();
                return [x * factor, y * factor, 1.0 - 2.0 * s];
            }
        }
    }

    /// Returns a direction within a cone around `base_dir` with half-angle `angle_deg`
    pub fn cone_direction(&mut self, base_dir: [f32; 3], angle_deg: f32) -> [f32; 3] {
        if angle_deg <= 0.0 {
            return normalize(base_dir);
        }
        if angle_deg >= 180.0 {
            return self.random_direction();
        }

        let angle_rad = angle_deg * std::f32::consts::PI / 180.0;
        let cos_angle = angle_rad.cos();

        // Random point in cone: uniform cos_theta in [cos_angle, 1], uniform phi in [0, 2pi]
        let cos_theta = self.range(cos_angle, 1.0);
        let sin_theta = (1.0 - cos_theta * cos_theta).sqrt();
        let phi = self.range(0.0, 2.0 * std::f32::consts::PI);

        // Local direction in cone around +Z
        let local = [sin_theta * phi.cos(), sin_theta * phi.sin(), cos_theta];

        // Rotate from +Z to base_dir
        rotate_to_basis(base_dir, local)
    }
}

/// Rotates `local` (assumed around +Z) to align with `forward`
pub fn rotate_to_basis(forward: [f32; 3], local: [f32; 3]) -> [f32; 3] {
    let (right, actual_up, fwd) = perpendicular_basis(forward);
    [
        right[0] * local[0] + actual_up[0] * local[1] + fwd[0] * local[2],
        right[1] * local[0] + actual_up[1] * local[1] + fwd[1] * local[2],
        right[2] * local[0] + actual_up[2] * local[1] + fwd[2] * local[2],
    ]
}

/// Orthonormal `(right, up, forward)` with `forward` along `dir`.
pub fn perpendicular_basis(dir: [f32; 3]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let fwd = normalize(dir);
    let up = if fwd[1].abs() > 0.99 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let right = normalize(cross(up, fwd));
    let actual_up = cross(fwd, right);
    (right, actual_up, fwd)
}

pub fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-10 {
        return [0.0, 1.0, 0.0];
    }
    [v[0] / len, v[1] / len, v[2] / len]
}

pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_range_bounds() {
        let mut rng = ParticleRng::new(42);
        for _ in 0..1000 {
            let v = rng.range(0.0, 10.0);
            assert!((0.0..10.0).contains(&v));
            // Descending ranges are tolerated.
            let w = rng.range(10.0, 0.0);
            assert!((0.0..10.0).contains(&w));
        }
    }

    #[test]
    fn rng_next_f32_strictly_below_one() {
        let mut rng = ParticleRng::new(0xFFFF_FFFF);
        for _ in 0..10_000 {
            assert!(rng.next_f32() < 1.0);
        }
    }

    #[test]
    fn rng_range_u32_inclusive() {
        let mut rng = ParticleRng::new(5);
        let mut seen_lo = false;
        let mut seen_hi = false;
        for _ in 0..2000 {
            let v = rng.range_u32(3, 6);
            assert!((3..=6).contains(&v));
            seen_lo |= v == 3;
            seen_hi |= v == 6;
        }
        assert!(seen_lo && seen_hi);
        assert_eq!(rng.range_u32(4, 4), 4);
    }

    #[test]
    fn rng_direction_unit_length() {
        let mut rng = ParticleRng::new(123);
        for _ in 0..100 {
            let d = rng.random_direction();
            let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            assert!((len - 1.0).abs() < 0.01);
        }
    }

    #[test]
    fn cone_direction_zero_spread() {
        let mut rng = ParticleRng::new(99);
        let dir = rng.cone_direction([0.0, 1.0, 0.0], 0.0);
        assert!((dir[0]).abs() < 0.01);
        assert!((dir[1] - 1.0).abs() < 0.01);
        assert!((dir[2]).abs() < 0.01);
    }

    #[test]
    fn unit_disc_inside_circle() {
        let mut rng = ParticleRng::new(3);
        for _ in 0..1000 {
            let [x, y] = rng.unit_disc();
            assert!(x * x + y * y <= 1.0 + 1e-5);
        }
    }
}
