//! Deterministic random number generation for procedural content.
//!
//! [`SeededRng`] wraps `ChaCha8Rng` for cross-platform deterministic output.
//! Given the same seed, the same sequence is produced on every platform and
//! every run — essential for reproducible procedural generation.

use flint_core::ContentHash;
use rand::Rng;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// A deterministic random number generator for procedural content.
///
/// Wraps `ChaCha8Rng` which is fast, cryptographically-derived (but used here
/// for determinism, not security), and produces identical output across all
/// platforms for a given seed.
pub struct SeededRng {
    inner: ChaCha8Rng,
    seed: u64,
}

impl SeededRng {
    /// Create a new RNG from a `u64` seed.
    pub fn new(seed: u64) -> Self {
        Self {
            inner: ChaCha8Rng::seed_from_u64(seed),
            seed,
        }
    }

    /// The seed this RNG was created with.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Uniform random `f32` in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        self.inner.random::<f32>()
    }

    /// Uniform random `f64` in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        self.inner.random::<f64>()
    }

    /// Uniform random `f32` in `[min, max)`.
    pub fn next_range(&mut self, min: f32, max: f32) -> f32 {
        self.inner.random_range(min..max)
    }

    /// Uniform random `i32` in `[min, max]` (inclusive).
    pub fn next_range_i32(&mut self, min: i32, max: i32) -> i32 {
        self.inner.random_range(min..=max)
    }

    /// Random boolean with the given probability of being `true`.
    pub fn next_bool(&mut self, probability: f64) -> bool {
        self.next_f64() < probability
    }

    /// Random point in an axis-aligned cube `[min, max)` per component.
    pub fn next_vec3(&mut self, min: f32, max: f32) -> [f32; 3] {
        [
            self.next_range(min, max),
            self.next_range(min, max),
            self.next_range(min, max),
        ]
    }

    /// Random unit vector via rejection sampling (uniform on the unit sphere).
    pub fn next_vec3_unit(&mut self) -> [f32; 3] {
        loop {
            let x = self.next_range(-1.0, 1.0);
            let y = self.next_range(-1.0, 1.0);
            let z = self.next_range(-1.0, 1.0);
            let len_sq = x * x + y * y + z * z;
            if len_sq > 1e-6 && len_sq <= 1.0 {
                let inv_len = 1.0 / len_sq.sqrt();
                return [x * inv_len, y * inv_len, z * inv_len];
            }
        }
    }

    /// Perturb a base RGBA color's RGB channels by up to `variation`, clamping
    /// to `[0, 1]`. Alpha is preserved.
    pub fn next_color_variation(&mut self, base: [f32; 4], variation: f32) -> [f32; 4] {
        [
            (base[0] + self.next_range(-variation, variation)).clamp(0.0, 1.0),
            (base[1] + self.next_range(-variation, variation)).clamp(0.0, 1.0),
            (base[2] + self.next_range(-variation, variation)).clamp(0.0, 1.0),
            base[3],
        ]
    }

    /// Perturb a base RGBA color in HSL space, preserving hue.
    ///
    /// Varies lightness by `±variation` and saturation by `±variation/2`.
    /// This prevents hue drift that can occur with independent per-channel
    /// RGB variation (e.g. brown shifting to magenta).
    pub fn next_color_variation_hsl(&mut self, base: [f32; 4], variation: f32) -> [f32; 4] {
        let (h, s, l) = rgb_to_hsl(base[0], base[1], base[2]);
        let l2 = (l + self.next_range(-variation, variation)).clamp(0.0, 1.0);
        let s2 = (s + self.next_range(-variation * 0.5, variation * 0.5)).clamp(0.0, 1.0);
        let (r, g, b) = hsl_to_rgb(h, s2, l2);
        [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0), base[3]]
    }

    /// Derive a child RNG from a label string.
    ///
    /// The child's seed is deterministic — the same parent state + label always
    /// produces the same child. Different labels produce different children.
    /// This advances the parent's state (consuming one value) so the parent
    /// continues with a different sequence after forking.
    pub fn fork(&mut self, label: &str) -> SeededRng {
        // Mix the parent's current seed with the label for a deterministic child.
        let combined = format!("{}:{}", self.seed, label);
        let hash = ContentHash::from_str(&combined);
        let bytes = hash.as_bytes();
        let child_seed = u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        // Advance parent state so forking has a side-effect.
        let _: u64 = self.inner.random();
        SeededRng::new(child_seed)
    }
}

// ─── HSV Helpers ──────────────────────────────────────────────────────────

/// Convert linear RGB to HSV. H is in [0, 1], S and V in [0, 1].
pub fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let v = max;

    if d.abs() < 1e-6 {
        return (0.0, 0.0, v);
    }

    let s = d / max.max(1e-6);

    let h = if (max - r).abs() < 1e-6 {
        let mut h = (g - b) / d;
        if g < b {
            h += 6.0;
        }
        h
    } else if (max - g).abs() < 1e-6 {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };

    (h / 6.0, s.clamp(0.0, 1.0), v.clamp(0.0, 1.0))
}

/// Convert HSV to linear RGB. H is in [0, 1], S and V in [0, 1].
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    if s < 1e-6 {
        return (v, v, v);
    }

    let h6 = (h.fract() + 1.0).fract() * 6.0;
    let i = h6.floor() as u32;
    let f = h6 - h6.floor();
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));

    match i % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

// ─── HSL Helpers ──────────────────────────────────────────────────────────

/// Convert linear RGB to HSL. H is in [0, 1], S and L in [0, 1].
fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;

    if (max - min).abs() < 1e-6 {
        return (0.0, 0.0, l);
    }

    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min).max(1e-6)
    } else {
        d / (max + min).max(1e-6)
    };

    let h = if (max - r).abs() < 1e-6 {
        let mut h = (g - b) / d;
        if g < b {
            h += 6.0;
        }
        h
    } else if (max - g).abs() < 1e-6 {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };

    (h / 6.0, s.clamp(0.0, 1.0), l.clamp(0.0, 1.0))
}

/// Convert HSL to linear RGB. H is in [0, 1], S and L in [0, 1].
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    if s < 1e-6 {
        return (l, l, l);
    }

    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;

    let r = hue_to_rgb(p, q, h + 1.0 / 3.0);
    let g = hue_to_rgb(p, q, h);
    let b = hue_to_rgb(p, q, h - 1.0 / 3.0);

    (r, g, b)
}

fn hue_to_rgb(p: f32, q: f32, t: f32) -> f32 {
    let t = if t < 0.0 {
        t + 1.0
    } else if t > 1.0 {
        t - 1.0
    } else {
        t
    };
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 0.5 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn determinism_same_seed_same_sequence() {
        let mut a = SeededRng::new(42);
        let mut b = SeededRng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_f64(), b.next_f64());
        }
    }

    #[test]
    fn different_seeds_differ() {
        let mut a = SeededRng::new(1);
        let mut b = SeededRng::new(2);
        // At least one of the first 10 values should differ.
        let differs = (0..10).any(|_| a.next_f64() != b.next_f64());
        assert!(differs);
    }

    #[test]
    fn next_f32_in_range() {
        let mut rng = SeededRng::new(99);
        for _ in 0..1000 {
            let v = rng.next_f32();
            assert!((0.0..1.0).contains(&v), "f32 out of range: {v}");
        }
    }

    #[test]
    fn next_range_bounds() {
        let mut rng = SeededRng::new(77);
        for _ in 0..1000 {
            let v = rng.next_range(5.0, 10.0);
            assert!(v >= 5.0 && v < 10.0, "range out of bounds: {v}");
        }
    }

    #[test]
    fn next_range_i32_inclusive() {
        let mut rng = SeededRng::new(55);
        let mut saw_min = false;
        let mut saw_max = false;
        for _ in 0..10_000 {
            let v = rng.next_range_i32(3, 5);
            assert!((3..=5).contains(&v));
            if v == 3 {
                saw_min = true;
            }
            if v == 5 {
                saw_max = true;
            }
        }
        assert!(saw_min, "never saw min bound");
        assert!(saw_max, "never saw max bound");
    }

    #[test]
    fn next_vec3_unit_length() {
        let mut rng = SeededRng::new(123);
        for _ in 0..100 {
            let v = rng.next_vec3_unit();
            let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-4, "unit vector length {len} != 1.0");
        }
    }

    #[test]
    fn color_variation_clamps() {
        let mut rng = SeededRng::new(0);
        let base = [0.0, 1.0, 0.5, 0.8];
        for _ in 0..100 {
            let c = rng.next_color_variation(base, 0.5);
            for i in 0..3 {
                assert!(
                    c[i] >= 0.0 && c[i] <= 1.0,
                    "channel {i} out of range: {}",
                    c[i]
                );
            }
            assert_eq!(c[3], 0.8, "alpha should be preserved");
        }
    }

    #[test]
    fn fork_determinism() {
        let mut a = SeededRng::new(42);
        let mut b = SeededRng::new(42);
        let child_a = a.fork("trees");
        let child_b = b.fork("trees");
        assert_eq!(child_a.seed(), child_b.seed());
    }

    #[test]
    fn fork_label_independence() {
        let mut a = SeededRng::new(42);
        let mut b = SeededRng::new(42);
        let child_a = a.fork("trees");
        let child_b = b.fork("rocks");
        assert_ne!(child_a.seed(), child_b.seed());
    }

    #[test]
    fn fork_advances_parent() {
        let mut a = SeededRng::new(42);
        let mut b = SeededRng::new(42);
        let _ = a.fork("test");
        // After forking, parent 'a' should produce different values than 'b'.
        let va = a.next_f64();
        let vb = b.next_f64();
        assert_ne!(va, vb);
    }

    #[test]
    fn seed_accessor() {
        let rng = SeededRng::new(12345);
        assert_eq!(rng.seed(), 12345);
    }

    #[test]
    fn hsl_roundtrip() {
        // Brown color: convert to HSL and back, verify roundtrip
        let r = 0.47_f32;
        let g = 0.28_f32;
        let b = 0.14_f32;
        let (h, s, l) = rgb_to_hsl(r, g, b);
        let (r2, g2, b2) = hsl_to_rgb(h, s, l);
        assert!((r - r2).abs() < 1e-4, "red roundtrip: {r} vs {r2}");
        assert!((g - g2).abs() < 1e-4, "green roundtrip: {g} vs {g2}");
        assert!((b - b2).abs() < 1e-4, "blue roundtrip: {b} vs {b2}");
    }

    #[test]
    fn hsl_variation_preserves_hue() {
        let base = [0.47, 0.28, 0.14, 1.0]; // brown
        let (base_h, _, _) = rgb_to_hsl(base[0], base[1], base[2]);

        let mut rng = SeededRng::new(42);
        for _ in 0..100 {
            let c = rng.next_color_variation_hsl(base, 0.15);
            let (h, _, _) = rgb_to_hsl(c[0], c[1], c[2]);
            // Hue should be preserved (within floating point tolerance)
            assert!(
                (h - base_h).abs() < 0.02,
                "hue drifted: base_h={base_h:.4} got h={h:.4}"
            );
            // All channels valid
            for i in 0..3 {
                assert!(c[i] >= 0.0 && c[i] <= 1.0, "channel {i} out of range: {}", c[i]);
            }
            assert_eq!(c[3], 1.0, "alpha preserved");
        }
    }

    #[test]
    fn hsv_roundtrip() {
        let r = 0.47_f32;
        let g = 0.28_f32;
        let b = 0.14_f32;
        let (h, s, v) = rgb_to_hsv(r, g, b);
        let (r2, g2, b2) = hsv_to_rgb(h, s, v);
        assert!((r - r2).abs() < 1e-4, "red roundtrip: {r} vs {r2}");
        assert!((g - g2).abs() < 1e-4, "green roundtrip: {g} vs {g2}");
        assert!((b - b2).abs() < 1e-4, "blue roundtrip: {b} vs {b2}");
    }

    #[test]
    fn hsl_variation_clamps() {
        let mut rng = SeededRng::new(0);
        let base = [0.0, 1.0, 0.5, 0.8];
        for _ in 0..100 {
            let c = rng.next_color_variation_hsl(base, 0.5);
            for i in 0..3 {
                assert!(c[i] >= 0.0 && c[i] <= 1.0, "channel {i} out of range: {}", c[i]);
            }
            assert_eq!(c[3], 0.8, "alpha should be preserved");
        }
    }
}
