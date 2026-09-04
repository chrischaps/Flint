//! Value-over-lifetime curves.
//!
//! A [`Curve`] is a small sorted key list over normalised age `t ∈ [0, 1]`
//! with linear, smoothstep or stepped interpolation between keys. Size,
//! colour, alpha and speed all sample one of these each frame (ADR 0068).

use serde::{Deserialize, Serialize};

/// Linear interpolation between two floats
pub fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Linear interpolation between two RGBA colors
pub fn lerp_color(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        lerp_f32(a[0], b[0], t),
        lerp_f32(a[1], b[1], t),
        lerp_f32(a[2], b[2], t),
        lerp_f32(a[3], b[3], t),
    ]
}

/// Types a [`Curve`] can interpolate.
pub trait Lerp: Copy {
    fn lerp(a: Self, b: Self, t: f32) -> Self;
}

impl Lerp for f32 {
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        lerp_f32(a, b, t)
    }
}

impl Lerp for [f32; 2] {
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        [lerp_f32(a[0], b[0], t), lerp_f32(a[1], b[1], t)]
    }
}

impl Lerp for [f32; 3] {
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        [
            lerp_f32(a[0], b[0], t),
            lerp_f32(a[1], b[1], t),
            lerp_f32(a[2], b[2], t),
        ]
    }
}

impl Lerp for [f32; 4] {
    fn lerp(a: Self, b: Self, t: f32) -> Self {
        lerp_color(a, b, t)
    }
}

/// How a curve moves between neighbouring keys.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Interp {
    /// Straight lines between keys.
    #[default]
    Linear,
    /// Smoothstep eased segments (zero slope at each key).
    Smooth,
    /// Hold each key's value until the next key.
    Step,
}

/// A sorted set of `(t, value)` keys sampled over normalised age.
#[derive(Clone, Debug, PartialEq)]
pub struct Curve<T: Lerp> {
    keys: Vec<(f32, T)>,
    interp: Interp,
}

impl<T: Lerp> Curve<T> {
    /// A curve that returns `v` everywhere.
    pub fn constant(v: T) -> Self {
        Self {
            keys: vec![(0.0, v)],
            interp: Interp::Linear,
        }
    }

    /// The classic two-point start → end ramp.
    pub fn start_end(start: T, end: T) -> Self {
        Self {
            keys: vec![(0.0, start), (1.0, end)],
            interp: Interp::Linear,
        }
    }

    /// Build from arbitrary keys. Keys are sorted by `t`; every `t` must be
    /// finite and within `[0, 1]`, and at least one key is required.
    pub fn from_keys(mut keys: Vec<(f32, T)>, interp: Interp) -> Result<Self, String> {
        if keys.is_empty() {
            return Err("curve needs at least one key".into());
        }
        for (t, _) in &keys {
            if !t.is_finite() || *t < 0.0 || *t > 1.0 {
                return Err(format!("curve key t={t} is outside [0, 1]"));
            }
        }
        keys.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        Ok(Self { keys, interp })
    }

    pub fn keys(&self) -> &[(f32, T)] {
        &self.keys
    }

    pub fn interp(&self) -> Interp {
        self.interp
    }

    /// Value at the first key (birth).
    pub fn first(&self) -> T {
        self.keys[0].1
    }

    /// Value at the last key (death).
    pub fn last(&self) -> T {
        self.keys[self.keys.len() - 1].1
    }

    pub fn is_constant(&self) -> bool {
        self.keys.len() == 1
    }

    /// Sample the curve at normalised age `t` (clamped to the key range).
    pub fn sample(&self, t: f32) -> T {
        let n = self.keys.len();
        if n == 1 || t <= self.keys[0].0 {
            return self.keys[0].1;
        }
        if t >= self.keys[n - 1].0 {
            return self.keys[n - 1].1;
        }
        // Find the segment [i, i+1] containing t.
        let mut i = 0;
        while i + 1 < n && self.keys[i + 1].0 <= t {
            i += 1;
        }
        let (t0, a) = self.keys[i];
        let (t1, b) = self.keys[(i + 1).min(n - 1)];
        let span = t1 - t0;
        if span <= 1e-6 {
            return b;
        }
        let u = ((t - t0) / span).clamp(0.0, 1.0);
        match self.interp {
            Interp::Linear => T::lerp(a, b, u),
            Interp::Smooth => T::lerp(a, b, u * u * (3.0 - 2.0 * u)),
            Interp::Step => a,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_f32_endpoints() {
        assert!((lerp_f32(0.0, 10.0, 0.0) - 0.0).abs() < 1e-6);
        assert!((lerp_f32(0.0, 10.0, 1.0) - 10.0).abs() < 1e-6);
        assert!((lerp_f32(0.0, 10.0, 0.5) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn lerp_color_midpoint() {
        let white = [1.0, 1.0, 1.0, 1.0];
        let black = [0.0, 0.0, 0.0, 0.0];
        let mid = lerp_color(white, black, 0.5);
        for c in &mid {
            assert!((*c - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn curve_samples_at_between_and_outside_keys() {
        let c =
            Curve::from_keys(vec![(1.0, 10.0f32), (0.0, 0.0), (0.5, 4.0)], Interp::Linear).unwrap();
        // Sorted on construction.
        assert_eq!(c.keys()[1].0, 0.5);
        assert!((c.sample(-1.0) - 0.0).abs() < 1e-6);
        assert!((c.sample(0.0) - 0.0).abs() < 1e-6);
        assert!((c.sample(0.25) - 2.0).abs() < 1e-6);
        assert!((c.sample(0.5) - 4.0).abs() < 1e-6);
        assert!((c.sample(0.75) - 7.0).abs() < 1e-6);
        assert!((c.sample(1.0) - 10.0).abs() < 1e-6);
        assert!((c.sample(2.0) - 10.0).abs() < 1e-6);
    }

    #[test]
    fn smooth_hits_endpoints_and_eases_midpoint() {
        let c = Curve::start_end(0.0f32, 1.0);
        let s = Curve::from_keys(c.keys().to_vec(), Interp::Smooth).unwrap();
        assert!((s.sample(0.0)).abs() < 1e-6);
        assert!((s.sample(1.0) - 1.0).abs() < 1e-6);
        assert!((s.sample(0.5) - 0.5).abs() < 1e-6);
        // Eased: below linear before the midpoint.
        assert!(s.sample(0.25) < 0.25);
    }

    #[test]
    fn step_holds_previous_key() {
        let s =
            Curve::from_keys(vec![(0.0, 1.0f32), (0.5, 2.0), (1.0, 3.0)], Interp::Step).unwrap();
        assert_eq!(s.sample(0.49), 1.0);
        assert_eq!(s.sample(0.5), 2.0);
        assert_eq!(s.sample(0.99), 2.0);
        assert_eq!(s.sample(1.0), 3.0);
    }

    #[test]
    fn invalid_keys_rejected() {
        assert!(Curve::<f32>::from_keys(vec![], Interp::Linear).is_err());
        assert!(Curve::from_keys(vec![(1.5, 0.0f32)], Interp::Linear).is_err());
        assert!(Curve::from_keys(vec![(f32::NAN, 0.0f32)], Interp::Linear).is_err());
    }

    #[test]
    fn constant_curve_is_flat() {
        let c = Curve::constant([1.0f32, 2.0]);
        assert!(c.is_constant());
        assert_eq!(c.sample(0.0), [1.0, 2.0]);
        assert_eq!(c.sample(0.7), [1.0, 2.0]);
    }
}
