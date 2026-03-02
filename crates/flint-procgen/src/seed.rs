//! Deterministic seeding for procedural generators.
//!
//! Every generator receives a [`Seed`] that controls its random output.
//! Seeds can be fixed for reproducibility, derived from a string key for
//! content-addressed determinism, or generated randomly.

use flint_core::ContentHash;
use serde::{Deserialize, Serialize};

/// A seed value that controls procedural generation output.
///
/// # Variants
///
/// - `Fixed(u64)` — always resolves to the given value.
/// - `Random` — resolves to a different value each time (system-time entropy).
/// - `Derived(String)` — hashes the key string to produce a stable `u64`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Seed {
    /// A specific seed value, guaranteed to reproduce the same output.
    Fixed(u64),
    /// A fresh random seed on every call (uses system-time entropy).
    Random,
    /// A seed derived from a string key via SHA-256, stable across runs.
    Derived(String),
}

impl Seed {
    /// Resolve this seed to a concrete `u64` value.
    ///
    /// - `Fixed(n)` → `n`
    /// - `Random` → a value derived from the current system time
    /// - `Derived(key)` → the first 8 bytes of `SHA-256(key)` as a `u64`
    pub fn resolve(&self) -> u64 {
        match self {
            Seed::Fixed(n) => *n,
            Seed::Random => {
                let duration = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default();
                // Combine seconds and nanos for sub-second uniqueness
                duration.as_secs()
                    ^ (duration.subsec_nanos() as u64).wrapping_mul(0x9E3779B97F4A7C15)
            }
            Seed::Derived(key) => {
                let hash = ContentHash::from_str(key);
                let bytes = hash.as_bytes();
                u64::from_le_bytes([
                    bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
                ])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_seed_returns_value() {
        assert_eq!(Seed::Fixed(42).resolve(), 42);
    }

    #[test]
    fn fixed_seed_is_deterministic() {
        let a = Seed::Fixed(12345).resolve();
        let b = Seed::Fixed(12345).resolve();
        assert_eq!(a, b);
    }

    #[test]
    fn derived_seed_is_stable() {
        let a = Seed::Derived("oak_tree".into()).resolve();
        let b = Seed::Derived("oak_tree".into()).resolve();
        assert_eq!(a, b);
    }

    #[test]
    fn derived_seed_differs_for_different_keys() {
        let a = Seed::Derived("oak_tree".into()).resolve();
        let b = Seed::Derived("pine_tree".into()).resolve();
        assert_ne!(a, b);
    }

    #[test]
    fn random_seed_varies() {
        // Two calls should almost certainly differ (sub-nanosecond collisions
        // are theoretically possible but practically impossible).
        let a = Seed::Random.resolve();
        std::thread::sleep(std::time::Duration::from_millis(1));
        let b = Seed::Random.resolve();
        assert_ne!(a, b);
    }

    #[test]
    fn seed_serde_roundtrip() {
        let seeds = vec![
            Seed::Fixed(99),
            Seed::Random,
            Seed::Derived("test_key".into()),
        ];
        for seed in &seeds {
            let json = serde_json::to_string(seed).unwrap();
            let parsed: Seed = serde_json::from_str(&json).unwrap();
            assert_eq!(*seed, parsed);
        }
    }
}
