//! Scripts' `random()`, `random_range(a, b)` and `seed_random(n)` (#580).
//!
//! One SplitMix64 per [`ScriptEngine`](crate::engine::ScriptEngine), shared by
//! every script in the order they call it. It starts from the wall clock once,
//! when the engine is made, so a plain run differs from the last one while
//! consecutive calls still never repeat; `--seed`, `--frame-step` and
//! `flint render --scripts` reseed it from Rust ([`ScriptRandom::seed`]), and a
//! script from Rhai with `seed_random(n)`. The same seed, whichever way it
//! arrives, gives the same numbers.

use crate::api::builder::kind::{Int, Num};
use crate::api::ScriptApi;
use std::sync::{Arc, Mutex};

/// SplitMix64's increment (the golden ratio in 64 bits).
const GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

/// The engine's random sequence: a SplitMix64 state behind a shared lock, so
/// the Rhai functions and the host reach the same one.
#[derive(Clone)]
pub struct ScriptRandom(Arc<Mutex<u64>>);

impl ScriptRandom {
    /// A sequence started at `seed`.
    pub fn seeded(seed: u64) -> Self {
        Self(Arc::new(Mutex::new(seed)))
    }

    /// A sequence started from the wall clock, read once. The nanoseconds
    /// since the epoch go through one SplitMix64 step first, so two engines
    /// made a few nanoseconds apart start far apart.
    pub fn from_clock() -> Self {
        let nanos = web_time::SystemTime::now()
            .duration_since(web_time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        Self::seeded(mix(nanos.wrapping_add(GAMMA)))
    }

    /// Restart the sequence at `seed`: the next draw is the first of the
    /// sequence any other start at `seed` gives.
    pub fn seed(&self, seed: u64) {
        *crate::lock_or_recover(&self.0) = seed;
    }

    /// The next value, in `[0, 1)`.
    pub fn next_f64(&self) -> f64 {
        let z = {
            let mut s = crate::lock_or_recover(&self.0);
            *s = s.wrapping_add(GAMMA);
            mix(*s)
        };
        // The top 53 bits, as a float in [0, 1).
        (z >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Declare `random()`, `random_range(min, max)` and `seed_random(n)`
    /// over this sequence, in the `math` module (ADR 0116).
    pub(crate) fn declare(&self, api: &mut ScriptApi) {
        let mut m = api.module("math");
        let random = self.clone();
        m.func(
            "random",
            "The next number of the engine's one random sequence, every script's draws in turn.",
        )
        .returns(Num, "in [0, 1)")
        .run(move |_, ()| random.next_f64());
        let random = self.clone();
        m.func(
            "random_range",
            "A random number between two, from the same sequence.",
        )
        .arg("min", Num)
        .arg("max", Num)
        .returns(Num, "in [min, max)")
        .run(move |_, (min, max)| min + (max - min) * random.next_f64());
        // An i64 seed's bits, as the host's u64 has them: seed_random(-1) is
        // --seed 18446744073709551615
        let random = self.clone();
        m.func(
            "seed_random",
            "Restart every script's random sequence at a seed, as --seed does; call it in on_init, not at the top.",
        )
        .arg("seed", Int)
        .run(move |_, (seed,)| random.seed(seed as u64));
    }
}

/// SplitMix64's output function.
fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_is_the_same_sequence_and_reseeding_restarts_it() {
        let a = ScriptRandom::seeded(42);
        let first: Vec<f64> = (0..8).map(|_| a.next_f64()).collect();
        let b = ScriptRandom::seeded(7);
        b.seed(42);
        let again: Vec<f64> = (0..8).map(|_| b.next_f64()).collect();
        assert_eq!(first, again);
        assert!(first.iter().all(|v| (0.0..1.0).contains(v)));
    }

    #[test]
    fn consecutive_draws_never_repeat() {
        // The wall-clock hash this replaced returned one value for every call
        // inside the timer's resolution
        let r = ScriptRandom::from_clock();
        let draws: Vec<u64> = (0..10_000).map(|_| r.next_f64().to_bits()).collect();
        let mut unique = draws.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), draws.len());
        let mean = draws.iter().map(|b| f64::from_bits(*b)).sum::<f64>() / 10_000.0;
        assert!((mean - 0.5).abs() < 0.02, "{mean}");
    }
}
