//! Gerstner ocean wave spectrum — the single source of truth for wave math.
//!
//! The CPU generates a deterministic, seeded spectrum of trochoidal (Gerstner)
//! waves with deep-water dispersion. The renderer uploads the *same* wave
//! array to the GPU (see `flint-render/src/ocean_shader.wgsl`), which only
//! sums it — so a `sample_height()` query and the rendered surface can never
//! disagree, which is what makes script-driven buoyancy believable.
//!
//! Per-frame wave phases are computed here in f64 and reduced mod 2π before
//! upload, so f32 `sin` precision never degrades over long sessions.
//!
//! Conventions: Y is up; waves displace in XZ. For wave *i* with direction
//! `d`, wavenumber `k = 2π/λ`, amplitude `A`, steepness `Q`, and phase
//! `θ = k·(d·p) − ω·t + φ₀`:
//!
//! ```text
//! offset.xz = Σ Q·A·d·cos θ        offset.y = Σ A·sin θ
//! normal    = normalize(−Σ d.x·k·A·cos θ,  1 − Σ Q·k·A·sin θ,  −Σ d.y·k·A·cos θ)
//! ```
//!
//! Steepness is normalized (`Qᵢ = choppiness / (kᵢ·Aᵢ·N)`) so the summed
//! steepness never exceeds `choppiness ≤ 1` — no self-intersecting loops.

use crate::toml_util::toml_f32;

fn toml_i64(v: &toml::Value) -> Option<i64> {
    match v {
        toml::Value::Integer(i) => Some(*i),
        toml::Value::Float(f) => Some(*f as i64),
        _ => None,
    }
}

/// Gravitational acceleration for deep-water dispersion ω = √(g·k).
pub const GRAVITY: f64 = 9.81;

/// Maximum waves in a spectrum (mirrors the WGSL uniform array size).
pub const MAX_WAVES: usize = 16;

/// Simulation parameters — everything that shapes the spectrum.
/// These map 1:1 to the `ocean` component fields (see schemas/components/ocean.toml).
#[derive(Debug, Clone, PartialEq)]
pub struct OceanParams {
    /// RNG seed; same seed + params → identical spectrum.
    pub seed: i64,
    /// Number of waves (clamped to 1..=MAX_WAVES).
    pub num_waves: usize,
    /// Shortest wavelength in meters.
    pub wavelength_min: f32,
    /// Longest wavelength in meters.
    pub wavelength_max: f32,
    /// Total wave amplitude in meters: Σ Aᵢ (max possible crest height).
    pub amplitude: f32,
    /// 0 = rolling swells, 1 = maximum-sharp trochoids (pre-cusp).
    pub choppiness: f32,
    /// Wind/primary travel direction in degrees (0 = +Z, 90 = +X).
    pub direction_deg: f32,
    /// Directional spread in degrees; small waves wander more than large.
    pub spread_deg: f32,
    /// Multiplier on time — a "becalmed" slider (1 = physical speed).
    pub speed_scale: f32,
}

impl Default for OceanParams {
    fn default() -> Self {
        Self {
            seed: 7,
            num_waves: 12,
            wavelength_min: 4.0,
            wavelength_max: 55.0,
            amplitude: 0.85,
            choppiness: 0.7,
            direction_deg: 25.0,
            spread_deg: 40.0,
            speed_scale: 1.0,
        }
    }
}

impl OceanParams {
    /// Read params from an `ocean` component TOML table; missing fields keep defaults.
    pub fn from_component(value: &toml::Value) -> Self {
        let d = Self::default();
        let f = |name: &str, dv: f32| value.get(name).and_then(toml_f32).unwrap_or(dv);
        Self {
            seed: value.get("seed").and_then(toml_i64).unwrap_or(d.seed),
            num_waves: value
                .get("num_waves")
                .and_then(toml_i64)
                .map(|n| n.clamp(1, MAX_WAVES as i64) as usize)
                .unwrap_or(d.num_waves),
            wavelength_min: f("wavelength_min", d.wavelength_min).max(0.5),
            wavelength_max: f("wavelength_max", d.wavelength_max).max(1.0),
            amplitude: f("amplitude", d.amplitude).max(0.0),
            choppiness: f("choppiness", d.choppiness).clamp(0.0, 1.0),
            direction_deg: f("direction_deg", d.direction_deg),
            spread_deg: f("spread_deg", d.spread_deg).clamp(0.0, 180.0),
            speed_scale: f("speed_scale", d.speed_scale).clamp(0.0, 8.0),
        }
    }
}

/// One Gerstner wave. All fields precomputed at spectrum generation.
#[derive(Debug, Clone, Copy)]
pub struct GerstnerWave {
    /// Unit travel direction in XZ.
    pub dir: [f32; 2],
    /// Wavenumber k = 2π/λ.
    pub k: f32,
    /// Amplitude in meters.
    pub amp: f32,
    /// Angular frequency ω = √(g·k) (rad/s), stored f64 for phase precision.
    pub omega: f64,
    /// Initial phase offset φ₀ in radians.
    pub phase0: f64,
    /// Steepness Q (already normalized against the whole spectrum).
    pub q: f32,
}

/// A generated spectrum plus the params that produced it.
#[derive(Debug, Clone)]
pub struct WaveSpectrum {
    pub params: OceanParams,
    pub waves: Vec<GerstnerWave>,
}

/// SplitMix64 — tiny deterministic RNG, no dependencies.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in [lo, hi).
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_f64()
    }
}

impl WaveSpectrum {
    /// Deterministically generate a spectrum from params.
    pub fn generate(params: &OceanParams) -> Self {
        let n = params.num_waves.clamp(1, MAX_WAVES);
        let mut rng = SplitMix64(params.seed as u64 ^ 0xF10A7);
        let lmin = params.wavelength_min.min(params.wavelength_max) as f64;
        let lmax = params.wavelength_max.max(params.wavelength_min) as f64;
        let wind = (params.direction_deg as f64).to_radians();
        let spread = (params.spread_deg as f64).to_radians();

        let mut waves = Vec::with_capacity(n);
        let mut amp_sum = 0.0_f64;
        for i in 0..n {
            // Log-spaced wavelengths with jitter: even coverage of the octaves.
            let u = if n == 1 { 0.5 } else { i as f64 / (n - 1) as f64 };
            let jitter = rng.range(-0.5, 0.5) / n.max(2) as f64;
            let lambda = lmin * (lmax / lmin).powf((u + jitter).clamp(0.0, 1.0));
            let k = std::f64::consts::TAU / lambda;

            // Longer waves hew to the wind; short chop wanders across the
            // full spread. `u` runs small→large wavelength, so weight by it.
            let wander = 1.0 - 0.65 * u;
            let angle = wind + rng.range(-1.0, 1.0) * spread * wander;
            let dir = [angle.sin() as f32, angle.cos() as f32]; // 0° = +Z

            // Amplitude proportional to wavelength (constant slope), then the
            // whole set is normalized to Σ A = params.amplitude below.
            let amp = lambda * rng.range(0.75, 1.25);

            // omega derives from the f32-rounded k so stored fields are
            // exactly consistent with what the GPU receives.
            let k32 = k as f32;
            waves.push(GerstnerWave {
                dir,
                k: k32,
                amp: amp as f32,
                omega: (GRAVITY * k32 as f64).sqrt(),
                phase0: rng.range(0.0, std::f64::consts::TAU),
                q: 0.0,
            });
            amp_sum += amp;
        }

        // Normalize amplitudes to the requested total, then distribute
        // steepness so Σ Qᵢ·kᵢ·Aᵢ = choppiness (≤ 1 ⇒ no cusps/loops).
        let amp_scale = if amp_sum > 0.0 {
            params.amplitude as f64 / amp_sum
        } else {
            0.0
        };
        for w in &mut waves {
            w.amp = (w.amp as f64 * amp_scale) as f32;
            let ka = w.k * w.amp;
            w.q = if ka > 1e-6 {
                params.choppiness / (ka * n as f32)
            } else {
                0.0
            };
        }

        Self {
            params: params.clone(),
            waves,
        }
    }

    /// Per-wave phase `ωᵢ·t − φ₀ᵢ`, computed in f64 and wrapped to [0, 2π).
    /// The shader (and the CPU sampler) evaluate θ = k·(d·p) − phase_t.
    pub fn phases_at(&self, time: f64) -> Vec<f32> {
        let t = time * self.params.speed_scale as f64;
        self.waves
            .iter()
            .map(|w| ((w.omega * t - w.phase0).rem_euclid(std::f64::consts::TAU)) as f32)
            .collect()
    }

    /// Lagrangian Gerstner offset of the parameter point (x, z) at the given
    /// pre-computed phases. Returns [dx, dy, dz].
    pub fn displacement(&self, x: f32, z: f32, phases: &[f32]) -> [f32; 3] {
        let mut off = [0.0_f32; 3];
        for (w, &ph) in self.waves.iter().zip(phases) {
            let theta = w.k * (w.dir[0] * x + w.dir[1] * z) - ph;
            let (s, c) = theta.sin_cos();
            off[0] += w.q * w.amp * w.dir[0] * c;
            off[1] += w.amp * s;
            off[2] += w.q * w.amp * w.dir[1] * c;
        }
        off
    }

    /// Surface normal at parameter point (x, z).
    pub fn normal(&self, x: f32, z: f32, phases: &[f32]) -> [f32; 3] {
        let mut nx = 0.0_f32;
        let mut ny = 1.0_f32;
        let mut nz = 0.0_f32;
        for (w, &ph) in self.waves.iter().zip(phases) {
            let theta = w.k * (w.dir[0] * x + w.dir[1] * z) - ph;
            let (s, c) = theta.sin_cos();
            let ka = w.k * w.amp;
            nx -= w.dir[0] * ka * c;
            ny -= w.q * ka * s;
            nz -= w.dir[1] * ka * c;
        }
        let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-6);
        [nx / len, ny / len, nz / len]
    }

    /// Eulerian surface height at *world* (x, z) — what buoyancy wants.
    ///
    /// Gerstner is Lagrangian (it moves particles horizontally), so we invert
    /// the horizontal displacement by fixed-point iteration: find the
    /// parameter point p whose displaced position lands on (x, z). Converges
    /// because Σ Q·k·A ≤ 1 keeps the map a contraction.
    pub fn sample_height(&self, x: f32, z: f32, time: f64) -> f32 {
        let phases = self.phases_at(time);
        self.sample_height_with_phases(x, z, &phases)
    }

    /// Like [`sample_height`], reusing precomputed phases (batch queries).
    pub fn sample_height_with_phases(&self, x: f32, z: f32, phases: &[f32]) -> f32 {
        let (px, pz) = self.invert_displacement(x, z, phases);
        self.displacement(px, pz, phases)[1]
    }

    /// Eulerian surface normal at world (x, z).
    pub fn sample_normal(&self, x: f32, z: f32, time: f64) -> [f32; 3] {
        let phases = self.phases_at(time);
        let (px, pz) = self.invert_displacement(x, z, phases.as_slice());
        self.normal(px, pz, &phases)
    }

    fn invert_displacement(&self, x: f32, z: f32, phases: &[f32]) -> (f32, f32) {
        let mut px = x;
        let mut pz = z;
        for _ in 0..4 {
            let d = self.displacement(px, pz, phases);
            px = x - d[0];
            pz = z - d[2];
        }
        (px, pz)
    }

    /// Pack waves for the GPU: two vec4s per wave slot, MAX_WAVES slots.
    /// Slot layout: `[dir.x, dir.y, k, amp]`, `[phase_t, q, 0, 0]`.
    /// Unused slots are zero (amp 0 contributes nothing).
    pub fn to_gpu(&self, time: f64) -> [[f32; 4]; MAX_WAVES * 2] {
        let phases = self.phases_at(time);
        let mut out = [[0.0_f32; 4]; MAX_WAVES * 2];
        for (i, (w, &ph)) in self.waves.iter().zip(&phases).enumerate().take(MAX_WAVES) {
            out[i * 2] = [w.dir[0], w.dir[1], w.k, w.amp];
            out[i * 2 + 1] = [ph, w.q, 0.0, 0.0];
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spectrum() -> WaveSpectrum {
        WaveSpectrum::generate(&OceanParams::default())
    }

    #[test]
    fn dispersion_relation_holds() {
        for w in &spectrum().waves {
            let expected = (GRAVITY * w.k as f64).sqrt();
            assert!(
                (w.omega - expected).abs() < 1e-9,
                "omega {} != sqrt(g*k) {}",
                w.omega,
                expected
            );
        }
    }

    #[test]
    fn steepness_sum_bounded_by_choppiness() {
        let params = OceanParams {
            choppiness: 1.0,
            ..Default::default()
        };
        let spec = WaveSpectrum::generate(&params);
        let sum: f32 = spec.waves.iter().map(|w| w.q * w.k * w.amp).sum();
        assert!(sum <= 1.0 + 1e-4, "steepness sum {} exceeds 1", sum);
        assert!(sum >= 0.99, "steepness sum {} should reach choppiness", sum);
    }

    #[test]
    fn amplitude_sum_matches_param() {
        let spec = spectrum();
        let sum: f32 = spec.waves.iter().map(|w| w.amp).sum();
        assert!(
            (sum - spec.params.amplitude).abs() < 1e-4,
            "amp sum {} != requested {}",
            sum,
            spec.params.amplitude
        );
    }

    #[test]
    fn deterministic_for_seed() {
        let a = spectrum();
        let b = spectrum();
        for (wa, wb) in a.waves.iter().zip(&b.waves) {
            assert_eq!(wa.dir, wb.dir);
            assert_eq!(wa.amp, wb.amp);
            assert_eq!(wa.phase0, wb.phase0);
        }
        let c = WaveSpectrum::generate(&OceanParams {
            seed: 8,
            ..Default::default()
        });
        assert!(a.waves[0].phase0 != c.waves[0].phase0);
    }

    #[test]
    fn height_bounded_by_total_amplitude() {
        let spec = spectrum();
        let bound = spec.params.amplitude + 1e-3;
        for i in 0..200 {
            let x = (i as f32 * 7.3) % 300.0 - 150.0;
            let z = (i as f32 * 3.9) % 300.0 - 150.0;
            let h = spec.sample_height(x, z, i as f64 * 0.37);
            assert!(h.abs() <= bound, "height {} out of bound at ({x},{z})", h);
        }
    }

    #[test]
    fn eulerian_inversion_converges() {
        // The displaced parameter point must land back on the queried column.
        let spec = spectrum();
        let phases = spec.phases_at(11.7);
        for i in 0..100 {
            let x = (i as f32 * 5.1) % 200.0 - 100.0;
            let z = (i as f32 * 9.7) % 200.0 - 100.0;
            let (px, pz) = spec.invert_displacement(x, z, &phases);
            let d = spec.displacement(px, pz, &phases);
            let err = ((px + d[0] - x).powi(2) + (pz + d[2] - z).powi(2)).sqrt();
            assert!(err < 0.02, "inversion residual {} at ({x},{z})", err);
        }
    }

    #[test]
    fn gpu_packing_matches_cpu_evaluation() {
        // Transliterate the WGSL summation against the packed array and
        // compare with the CPU displacement/normal — the parity contract.
        let spec = spectrum();
        let t = 42.31;
        let gpu = spec.to_gpu(t);
        let phases = spec.phases_at(t);

        let eval_gpu = |x: f32, z: f32| -> ([f32; 3], [f32; 3]) {
            let mut off = [0.0_f32; 3];
            let mut n = [0.0_f32, 1.0, 0.0];
            for i in 0..MAX_WAVES {
                let a = gpu[i * 2];
                let b = gpu[i * 2 + 1];
                let (dx, dy, k, amp) = (a[0], a[1], a[2], a[3]);
                let (ph, q) = (b[0], b[1]);
                if amp <= 0.0 {
                    continue;
                }
                let theta = k * (dx * x + dy * z) - ph;
                let (s, c) = theta.sin_cos();
                off[0] += q * amp * dx * c;
                off[1] += amp * s;
                off[2] += q * amp * dy * c;
                n[0] -= dx * k * amp * c;
                n[1] -= q * k * amp * s;
                n[2] -= dy * k * amp * c;
            }
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            (off, [n[0] / len, n[1] / len, n[2] / len])
        };

        for i in 0..100 {
            let x = (i as f32 * 13.7) % 400.0 - 200.0;
            let z = (i as f32 * 6.1) % 400.0 - 200.0;
            let (goff, gn) = eval_gpu(x, z);
            let coff = spec.displacement(x, z, &phases);
            let cn = spec.normal(x, z, &phases);
            for a in 0..3 {
                assert!(
                    (goff[a] - coff[a]).abs() < 1e-4,
                    "offset[{a}] diverges: {} vs {}",
                    goff[a],
                    coff[a]
                );
                assert!(
                    (gn[a] - cn[a]).abs() < 1e-4,
                    "normal[{a}] diverges: {} vs {}",
                    gn[a],
                    cn[a]
                );
            }
        }
    }

    #[test]
    fn phases_wrap_and_stay_finite_over_long_sessions() {
        let spec = spectrum();
        // 100 hours of session time: phases must remain in [0, 2π).
        for &t in &[0.0, 60.0, 3600.0, 360_000.0] {
            for p in spec.phases_at(t) {
                assert!(p.is_finite());
                assert!((0.0..std::f32::consts::TAU + 1e-3).contains(&p));
            }
        }
    }
}
