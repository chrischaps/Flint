//! Lightweight xorshift32 PRNG — no external crate needed

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
        (self.next_u32() as f32) / (u32::MAX as f32)
    }

    /// Returns a float in [min, max)
    pub fn range(&mut self, min: f32, max: f32) -> f32 {
        min + self.next_f32() * (max - min)
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
            return base_dir;
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
fn rotate_to_basis(forward: [f32; 3], local: [f32; 3]) -> [f32; 3] {
    let fwd = normalize(forward);
    let up = if fwd[1].abs() > 0.99 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let right = normalize(cross(up, fwd));
    let actual_up = cross(fwd, right);

    [
        right[0] * local[0] + actual_up[0] * local[1] + fwd[0] * local[2],
        right[1] * local[0] + actual_up[1] * local[1] + fwd[1] * local[2],
        right[2] * local[0] + actual_up[2] * local[1] + fwd[2] * local[2],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-10 {
        return [0.0, 1.0, 0.0];
    }
    [v[0] / len, v[1] / len, v[2] / len]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
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
            assert!(v >= 0.0 && v < 10.0);
        }
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
}
