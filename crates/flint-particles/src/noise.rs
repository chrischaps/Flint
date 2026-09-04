//! Deterministic hash-based 3D gradient noise for turbulence forces.
//!
//! No tables, no allocation: each lattice corner's gradient comes from an
//! integer hash of its coordinates and the caller's seed, so two runs with
//! the same seed produce identical fields. Curl noise is deliberately not
//! implemented (six extra samples per particle); see ADR 0068.

#[inline]
fn hash3(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8DA6_B343)
        ^ (y as u32).wrapping_mul(0xD816_3841)
        ^ (z as u32).wrapping_mul(0xCB1A_B31F)
        ^ seed.wrapping_mul(0x9E37_79B9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h
}

/// Pseudo-random unit-ish gradient at a lattice corner.
#[inline]
fn gradient(x: i32, y: i32, z: i32, seed: u32) -> [f32; 3] {
    let h = hash3(x, y, z, seed);
    // Three signed components from separate bit ranges, in [-1, 1].
    let gx = ((h & 0x3FF) as f32 / 511.5) - 1.0;
    let gy = (((h >> 10) & 0x3FF) as f32 / 511.5) - 1.0;
    let gz = (((h >> 20) & 0x3FF) as f32 / 511.5) - 1.0;
    [gx, gy, gz]
}

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Perlin-style gradient noise in roughly `[-1, 1]`.
pub fn noise3(p: [f32; 3], seed: u32) -> f32 {
    let xi = p[0].floor();
    let yi = p[1].floor();
    let zi = p[2].floor();
    let (x0, y0, z0) = (xi as i32, yi as i32, zi as i32);
    let fx = p[0] - xi;
    let fy = p[1] - yi;
    let fz = p[2] - zi;
    let u = fade(fx);
    let v = fade(fy);
    let w = fade(fz);

    let dot = |cx: i32, cy: i32, cz: i32, dx: f32, dy: f32, dz: f32| {
        let g = gradient(cx, cy, cz, seed);
        g[0] * dx + g[1] * dy + g[2] * dz
    };

    let n000 = dot(x0, y0, z0, fx, fy, fz);
    let n100 = dot(x0 + 1, y0, z0, fx - 1.0, fy, fz);
    let n010 = dot(x0, y0 + 1, z0, fx, fy - 1.0, fz);
    let n110 = dot(x0 + 1, y0 + 1, z0, fx - 1.0, fy - 1.0, fz);
    let n001 = dot(x0, y0, z0 + 1, fx, fy, fz - 1.0);
    let n101 = dot(x0 + 1, y0, z0 + 1, fx - 1.0, fy, fz - 1.0);
    let n011 = dot(x0, y0 + 1, z0 + 1, fx, fy - 1.0, fz - 1.0);
    let n111 = dot(x0 + 1, y0 + 1, z0 + 1, fx - 1.0, fy - 1.0, fz - 1.0);

    let nx00 = lerp(n000, n100, u);
    let nx10 = lerp(n010, n110, u);
    let nx01 = lerp(n001, n101, u);
    let nx11 = lerp(n011, n111, u);
    let nxy0 = lerp(nx00, nx10, v);
    let nxy1 = lerp(nx01, nx11, v);
    // Gradient noise with unit-cube gradients peaks near ±0.87; rescale.
    (lerp(nxy0, nxy1, w) * 1.15).clamp(-1.0, 1.0)
}

/// Fractal sum of `octaves` noise layers (each doubling frequency, halving
/// amplitude), normalised back into roughly `[-1, 1]`.
pub fn fbm3(p: [f32; 3], seed: u32, octaves: u32) -> f32 {
    let octaves = octaves.clamp(1, 6);
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    let mut q = p;
    for i in 0..octaves {
        sum += noise3(q, seed.wrapping_add(i * 0x51_7C_C1)) * amp;
        norm += amp;
        amp *= 0.5;
        q = [q[0] * 2.0 + 17.3, q[1] * 2.0 - 9.1, q[2] * 2.0 + 4.7];
    }
    sum / norm
}

/// A vector field: three decorrelated noise samples, one per axis.
pub fn noise3_vec(p: [f32; 3], seed: u32, octaves: u32) -> [f32; 3] {
    [
        fbm3(p, seed, octaves),
        fbm3(
            [p[0] + 31.7, p[1] - 12.3, p[2] + 5.9],
            seed ^ 0x5F3_759D,
            octaves,
        ),
        fbm3(
            [p[0] - 7.1, p[1] + 23.9, p[2] - 41.3],
            seed ^ 0x2545_F491,
            octaves,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_deterministic_and_bounded() {
        let a = noise3([0.3, 1.7, -2.2], 7);
        let b = noise3([0.3, 1.7, -2.2], 7);
        assert_eq!(a, b);
        for i in 0..500 {
            let f = i as f32 * 0.137;
            let p = [f.sin() * 10.0, f * 0.7, (f * 1.3).cos() * 5.0];
            let n = noise3(p, 11);
            assert!((-1.0..=1.0).contains(&n), "{n} out of range");
            let v = noise3_vec(p, 11, 3);
            for c in v {
                assert!((-1.0..=1.0).contains(&c));
            }
        }
    }

    #[test]
    fn seed_changes_field() {
        let p = [1.5, 2.5, 3.5];
        assert_ne!(noise3(p, 1), noise3(p, 2));
    }

    #[test]
    fn noise_is_continuous() {
        let p = [0.9999, 0.5, 0.5];
        let q = [1.0001, 0.5, 0.5];
        assert!((noise3(p, 3) - noise3(q, 3)).abs() < 0.01);
    }
}
