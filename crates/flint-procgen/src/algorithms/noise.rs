//! Noise functions for procedural generation.
//!
//! Provides a unified [`NoiseSource`] trait over Perlin, simplex, value and
//! Worley (cellular) noise, plus a generic [`Fbm`] combinator for fractal
//! Brownian motion. Everything here is implemented from scratch — no external
//! noise crate — so the output is fully under our control and cannot drift
//! when a dependency is upgraded. All types are `Send + Sync` for safe use in
//! parallel generation pipelines.
//!
//! Gradient noises (Perlin, simplex, value) are driven by a 256-entry
//! [`PermutationTable`] shuffled by [`SeededRng`]; Worley feature points come
//! from a stateless integer hash of the cell coordinates and the seed.

use crate::{ChannelSemantics, ImageData, SeededRng};

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// A deterministic noise source that can be sampled in 2D and 3D.
///
/// All implementations must be `Send + Sync` so they can be shared across
/// threads. Perlin, simplex and value noise are guaranteed to stay within
/// \[-1, 1\]; Worley in its signed mode does too.
pub trait NoiseSource: Send + Sync {
    /// Sample the noise field at a 2D point. Returns a value in \[-1, 1\].
    fn sample_2d(&self, x: f64, y: f64) -> f64;

    /// Sample the noise field at a 3D point. Returns a value in \[-1, 1\].
    fn sample_3d(&self, x: f64, y: f64, z: f64) -> f64;

    /// Convenience: remap [`sample_2d`](NoiseSource::sample_2d) to \[0, 1\].
    fn sample_2d_01(&self, x: f64, y: f64) -> f64 {
        self.sample_2d(x, y) * 0.5 + 0.5
    }

    /// Convenience: remap [`sample_3d`](NoiseSource::sample_3d) to \[0, 1\].
    fn sample_3d_01(&self, x: f64, y: f64, z: f64) -> f64 {
        self.sample_3d(x, y, z) * 0.5 + 0.5
    }
}

// ---------------------------------------------------------------------------
// Permutation table
// ---------------------------------------------------------------------------

/// Ken Perlin's permutation table: the bytes 0..=255 shuffled by a seeded
/// Fisher–Yates pass, stored twice so `perm[perm[x] + y]` never needs to wrap.
///
/// Lattice coordinates are reduced modulo 256, so every noise built on the
/// table repeats with a period of 256 units along each axis.
#[derive(Clone)]
pub struct PermutationTable {
    perm: [u8; 512],
}

impl PermutationTable {
    /// Build a table from a seed (via a fresh [`SeededRng`]).
    pub fn new(seed: u64) -> Self {
        Self::from_rng(&mut SeededRng::new(seed))
    }

    /// Build a table by shuffling with the given RNG (advances it 255 times).
    pub fn from_rng(rng: &mut SeededRng) -> Self {
        let mut p: [u8; 256] = std::array::from_fn(|i| i as u8);
        for i in (1..256).rev() {
            let j = rng.next_range_i32(0, i as i32) as usize;
            p.swap(i, j);
        }
        Self {
            perm: std::array::from_fn(|i| p[i & 255]),
        }
    }

    #[inline]
    fn hash2(&self, x: i64, y: i64) -> u8 {
        let a = self.perm[(x & 255) as usize] as usize;
        self.perm[a + (y & 255) as usize]
    }

    #[inline]
    fn hash3(&self, x: i64, y: i64, z: i64) -> u8 {
        let a = self.hash2(x, y) as usize;
        self.perm[a + (z & 255) as usize]
    }
}

/// Perlin's quintic fade curve `6t⁵ − 15t⁴ + 10t³` (C² continuous).
#[inline]
fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline]
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + t * (b - a)
}

/// Floor to a lattice coordinate plus the fractional offset within the cell.
#[inline]
fn split(v: f64) -> (i64, f64) {
    let f = v.floor();
    (f as i64, v - f)
}

/// Dot product with one of eight 2D gradients, all of length √2: the four
/// diagonals and the four axes. Equal lengths keep the field isotropic and put
/// the 2D Perlin extremum (reached at a cell centre) at exactly ±1.
#[inline]
fn grad2(hash: u8, x: f64, y: f64) -> f64 {
    const S: f64 = std::f64::consts::SQRT_2;
    match hash & 7 {
        0 => x + y,
        1 => -x + y,
        2 => x - y,
        3 => -x - y,
        4 => S * x,
        5 => -S * x,
        6 => S * y,
        _ => -S * y,
    }
}

/// Dot product with one of Perlin's twelve cube-edge gradients (padded to 16
/// so the hash can be masked rather than taken modulo 12).
#[inline]
fn grad3(hash: u8, x: f64, y: f64, z: f64) -> f64 {
    match hash & 15 {
        0 | 12 => x + y,
        1 | 13 => -x + y,
        2 => x - y,
        3 => -x - y,
        4 => x + z,
        5 => -x + z,
        6 => x - z,
        7 => -x - z,
        8 => y + z,
        9 | 14 => -y + z,
        10 => y - z,
        _ => -y - z,
    }
}

// ---------------------------------------------------------------------------
// Perlin
// ---------------------------------------------------------------------------

/// Classic (improved, 2002) Perlin gradient noise.
///
/// Zero at every integer lattice point; output in \[-1, 1\].
#[derive(Clone)]
pub struct PerlinNoise {
    perm: PermutationTable,
}

impl PerlinNoise {
    pub fn new(seed: u64) -> Self {
        Self {
            perm: PermutationTable::new(seed),
        }
    }

    pub fn from_rng(rng: &mut SeededRng) -> Self {
        let child = rng.fork("perlin");
        Self::new(child.seed())
    }

    /// 2D Perlin that repeats every `period_x` × `period_y` lattice cells.
    ///
    /// The lattice corners are wrapped before hashing, so the field and its
    /// derivatives match exactly across the seam: sampling `[0, period)` on
    /// each axis gives a texture that tiles (or wraps around a cylinder).
    /// Periods below 1 are treated as 1.
    pub fn sample_2d_periodic(&self, x: f64, y: f64, period_x: i64, period_y: i64) -> f64 {
        let (px, py) = (period_x.max(1), period_y.max(1));
        let (xi, xf) = split(x);
        let (yi, yf) = split(y);
        let (x0, x1) = (xi.rem_euclid(px), (xi + 1).rem_euclid(px));
        let (y0, y1) = (yi.rem_euclid(py), (yi + 1).rem_euclid(py));
        let p = &self.perm;

        let n00 = grad2(p.hash2(x0, y0), xf, yf);
        let n10 = grad2(p.hash2(x1, y0), xf - 1.0, yf);
        let n01 = grad2(p.hash2(x0, y1), xf, yf - 1.0);
        let n11 = grad2(p.hash2(x1, y1), xf - 1.0, yf - 1.0);

        let u = fade(xf);
        let v = fade(yf);
        lerp(lerp(n00, n10, u), lerp(n01, n11, u), v).clamp(-1.0, 1.0)
    }

    /// FBM that tiles over the unit square: `(u, v)` in `[0, 1)` is one tile.
    ///
    /// Each octave samples [`Self::sample_2d_periodic`] with a whole number of
    /// lattice cells per tile, `round(frequency · lacunarityᵏ)` (at least 1),
    /// so every octave repeats exactly at the tile edge. A fractional base
    /// frequency or lacunarity is rounded per octave rather than refused.
    /// Normalised by the amplitude sum, so the output stays in \[-1, 1\].
    pub fn fbm_2d_periodic(
        &self,
        u: f64,
        v: f64,
        frequency: f64,
        octaves: u32,
        lacunarity: f64,
        persistence: f64,
    ) -> f64 {
        let (mut sum, mut amp, mut norm, mut freq) = (0.0, 1.0, 0.0, frequency);
        for _ in 0..octaves.max(1) {
            let period = (freq.round() as i64).max(1);
            let p = period as f64;
            sum += amp * self.sample_2d_periodic(u * p, v * p, period, period);
            norm += amp;
            amp *= persistence;
            freq *= lacunarity;
        }
        sum / norm
    }
}

/// 3D Perlin with the cube-edge gradients peaks slightly above 1 (1.0025 over
/// 24M random samples); dividing by this keeps it inside ±1, and the clamp
/// catches any rarer excursion.
const PERLIN_3D_PEAK: f64 = 1.01;

impl NoiseSource for PerlinNoise {
    fn sample_2d(&self, x: f64, y: f64) -> f64 {
        let (xi, xf) = split(x);
        let (yi, yf) = split(y);
        let p = &self.perm;

        let n00 = grad2(p.hash2(xi, yi), xf, yf);
        let n10 = grad2(p.hash2(xi + 1, yi), xf - 1.0, yf);
        let n01 = grad2(p.hash2(xi, yi + 1), xf, yf - 1.0);
        let n11 = grad2(p.hash2(xi + 1, yi + 1), xf - 1.0, yf - 1.0);

        let u = fade(xf);
        let v = fade(yf);
        lerp(lerp(n00, n10, u), lerp(n01, n11, u), v).clamp(-1.0, 1.0)
    }

    fn sample_3d(&self, x: f64, y: f64, z: f64) -> f64 {
        let (xi, xf) = split(x);
        let (yi, yf) = split(y);
        let (zi, zf) = split(z);
        let p = &self.perm;

        let corner = |dx: i64, dy: i64, dz: i64| {
            grad3(
                p.hash3(xi + dx, yi + dy, zi + dz),
                xf - dx as f64,
                yf - dy as f64,
                zf - dz as f64,
            )
        };

        let u = fade(xf);
        let v = fade(yf);
        let w = fade(zf);
        let x00 = lerp(corner(0, 0, 0), corner(1, 0, 0), u);
        let x10 = lerp(corner(0, 1, 0), corner(1, 1, 0), u);
        let x01 = lerp(corner(0, 0, 1), corner(1, 0, 1), u);
        let x11 = lerp(corner(0, 1, 1), corner(1, 1, 1), u);
        let n = lerp(lerp(x00, x10, v), lerp(x01, x11, v), w);
        (n / PERLIN_3D_PEAK).clamp(-1.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// Simplex
// ---------------------------------------------------------------------------

/// Simplex noise (Perlin 2001, after Gustavson's reference formulation).
///
/// Sums radial kernels over the corners of a triangle (2D) or tetrahedron
/// (3D) instead of a square/cube, so it costs fewer gradient evaluations and
/// shows less axis-aligned structure than Perlin. Kernels use the exact
/// radius² = 0.5 so the field is continuous across simplex boundaries.
#[derive(Clone)]
pub struct SimplexNoise {
    perm: PermutationTable,
}

impl SimplexNoise {
    pub fn new(seed: u64) -> Self {
        Self {
            perm: PermutationTable::new(seed),
        }
    }

    pub fn from_rng(rng: &mut SeededRng) -> Self {
        let child = rng.fork("simplex");
        Self::new(child.seed())
    }
}

/// Skew/unskew factors between Cartesian space and the simplex lattice.
const F2: f64 = 0.366_025_403_784_438_6; // (√3 − 1) / 2
const G2: f64 = 0.211_324_865_405_187_1; // (3 − √3) / 6
const F3: f64 = 1.0 / 3.0;
const G3: f64 = 1.0 / 6.0;

/// Output scales that map the summed kernels onto \[-1, 1\]: just under the
/// reciprocal of each field's peak, measured over 24M random samples (2D peak
/// 0.014256 → 70.15, 3D peak 0.013006 → 76.89). The clamp absorbs any rarer,
/// higher peak.
const SIMPLEX_2D_SCALE: f64 = 70.0;
const SIMPLEX_3D_SCALE: f64 = 76.5;

/// One simplex corner: `(0.5 − r²)⁴ · (g · d)`, zero outside the radius.
#[inline]
fn kernel(r2: f64, dot: f64) -> f64 {
    let t = 0.5 - r2;
    if t <= 0.0 {
        0.0
    } else {
        let t2 = t * t;
        t2 * t2 * dot
    }
}

impl NoiseSource for SimplexNoise {
    fn sample_2d(&self, x: f64, y: f64) -> f64 {
        let p = &self.perm;

        // Skew into the lattice and find the containing triangle.
        let s = (x + y) * F2;
        let i = (x + s).floor();
        let j = (y + s).floor();
        let t = (i + j) * G2;
        let x0 = x - (i - t);
        let y0 = y - (j - t);
        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };

        let x1 = x0 - i1 as f64 + G2;
        let y1 = y0 - j1 as f64 + G2;
        let x2 = x0 - 1.0 + 2.0 * G2;
        let y2 = y0 - 1.0 + 2.0 * G2;

        let (i, j) = (i as i64, j as i64);
        let n0 = kernel(x0 * x0 + y0 * y0, grad2(p.hash2(i, j), x0, y0));
        let n1 = kernel(x1 * x1 + y1 * y1, grad2(p.hash2(i + i1, j + j1), x1, y1));
        let n2 = kernel(x2 * x2 + y2 * y2, grad2(p.hash2(i + 1, j + 1), x2, y2));

        ((n0 + n1 + n2) * SIMPLEX_2D_SCALE).clamp(-1.0, 1.0)
    }

    fn sample_3d(&self, x: f64, y: f64, z: f64) -> f64 {
        let p = &self.perm;

        let s = (x + y + z) * F3;
        let i = (x + s).floor();
        let j = (y + s).floor();
        let k = (z + s).floor();
        let t = (i + j + k) * G3;
        let x0 = x - (i - t);
        let y0 = y - (j - t);
        let z0 = z - (k - t);

        // Which of the six tetrahedra in the skewed cube are we in?
        let (o1, o2) = if x0 >= y0 {
            if y0 >= z0 {
                ((1, 0, 0), (1, 1, 0))
            } else if x0 >= z0 {
                ((1, 0, 0), (1, 0, 1))
            } else {
                ((0, 0, 1), (1, 0, 1))
            }
        } else if y0 < z0 {
            ((0, 0, 1), (0, 1, 1))
        } else if x0 < z0 {
            ((0, 1, 0), (0, 1, 1))
        } else {
            ((0, 1, 0), (1, 1, 0))
        };

        let (i, j, k) = (i as i64, j as i64, k as i64);
        let corner = |o: (i64, i64, i64), g: f64| {
            let dx = x0 - o.0 as f64 + g;
            let dy = y0 - o.1 as f64 + g;
            let dz = z0 - o.2 as f64 + g;
            kernel(
                dx * dx + dy * dy + dz * dz,
                grad3(p.hash3(i + o.0, j + o.1, k + o.2), dx, dy, dz),
            )
        };

        let n = corner((0, 0, 0), 0.0)
            + corner(o1, G3)
            + corner(o2, 2.0 * G3)
            + corner((1, 1, 1), 3.0 * G3);
        (n * SIMPLEX_3D_SCALE).clamp(-1.0, 1.0)
    }
}

// ---------------------------------------------------------------------------
// Value noise
// ---------------------------------------------------------------------------

/// Value noise — random values at grid points with smooth interpolation.
///
/// Blockier than gradient noise and not zero on the lattice; output in
/// \[-1, 1\].
#[derive(Clone)]
pub struct ValueNoise {
    perm: PermutationTable,
}

impl ValueNoise {
    pub fn new(seed: u64) -> Self {
        Self {
            perm: PermutationTable::new(seed),
        }
    }

    pub fn from_rng(rng: &mut SeededRng) -> Self {
        let child = rng.fork("value");
        Self::new(child.seed())
    }
}

#[inline]
fn byte_to_signed(b: u8) -> f64 {
    b as f64 / 127.5 - 1.0
}

impl NoiseSource for ValueNoise {
    fn sample_2d(&self, x: f64, y: f64) -> f64 {
        let (xi, xf) = split(x);
        let (yi, yf) = split(y);
        let v = |dx, dy| byte_to_signed(self.perm.hash2(xi + dx, yi + dy));
        let u = fade(xf);
        lerp(
            lerp(v(0, 0), v(1, 0), u),
            lerp(v(0, 1), v(1, 1), u),
            fade(yf),
        )
    }

    fn sample_3d(&self, x: f64, y: f64, z: f64) -> f64 {
        let (xi, xf) = split(x);
        let (yi, yf) = split(y);
        let (zi, zf) = split(z);
        let v = |dx, dy, dz| byte_to_signed(self.perm.hash3(xi + dx, yi + dy, zi + dz));
        let u = fade(xf);
        let w = fade(yf);
        let a = lerp(
            lerp(v(0, 0, 0), v(1, 0, 0), u),
            lerp(v(0, 1, 0), v(1, 1, 0), u),
            w,
        );
        let b = lerp(
            lerp(v(0, 0, 1), v(1, 0, 1), u),
            lerp(v(0, 1, 1), v(1, 1, 1), u),
            w,
        );
        lerp(a, b, fade(zf))
    }
}

// ---------------------------------------------------------------------------
// Worley (cellular)
// ---------------------------------------------------------------------------

/// Distances to the nearest (`f1`) and second-nearest (`f2`) Worley feature
/// points, normalised to approximately \[0, 1\]. `f1 <= f2` always holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorleySample {
    pub f1: f32,
    pub f2: f32,
}

/// Worley (cellular) noise: one jittered feature point per unit cell.
///
/// [`f1_f2`](WorleyNoise::f1_f2) returns the raw cellular distances. As a
/// [`NoiseSource`] it returns either a per-cell random value (the default,
/// flat-shaded "cracked tile" look) or the signed F1 distance, both in
/// \[-1, 1\]. Distances use any [`VoronoiMetric`]; the default is Euclidean.
#[derive(Clone)]
pub struct WorleyNoise {
    seed: u64,
    frequency: f64,
    metric: VoronoiMetric,
    return_distance: bool,
}

/// Raw nearest-two search result, before normalisation.
struct Nearest {
    f1: f64,
    f2: f64,
    /// Hash of the cell owning the nearest feature point.
    cell: u64,
}

impl WorleyNoise {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            frequency: 1.0,
            metric: VoronoiMetric::Euclidean,
            return_distance: false,
        }
    }

    pub fn from_rng(rng: &mut SeededRng) -> Self {
        let child = rng.fork("worley");
        Self::new(child.seed())
    }

    /// Set the frequency of seed-point distribution.
    pub fn with_frequency(mut self, frequency: f64) -> Self {
        self.frequency = frequency;
        self
    }

    /// Set the distance metric used for F1/F2.
    pub fn with_metric(mut self, metric: VoronoiMetric) -> Self {
        self.metric = metric;
        self
    }

    /// If `true`, returns distance to the nearest seed point instead of a
    /// hashed cell value.
    pub fn with_return_distance(mut self, distance: bool) -> Self {
        self.return_distance = distance;
        self
    }

    /// Distances to the nearest and second-nearest feature points at a 2D
    /// point, normalised to approximately \[0, 1\] (clamped, which preserves
    /// `f1 <= f2`).
    pub fn f1_f2(&self, x: f64, y: f64) -> WorleySample {
        let n = self.nearest_2d(x * self.frequency, y * self.frequency);
        let norm = worley_norm(self.metric);
        WorleySample {
            f1: (n.f1 / norm).clamp(0.0, 1.0) as f32,
            f2: (n.f2 / norm).clamp(0.0, 1.0) as f32,
        }
    }

    /// Exact F1/F2 search. Cells are visited in square rings around the
    /// sample's cell; a ring at Chebyshev radius `r` is at least `r − 1` away
    /// (a lower bound under every Minkowski metric), so the search stops as
    /// soon as that bound reaches F2. In practice it ends after ring 1 or 2.
    fn nearest_2d(&self, x: f64, y: f64) -> Nearest {
        let (ix, fx) = split(x);
        let (iy, fy) = split(y);
        let mut best = Nearest {
            f1: f64::INFINITY,
            f2: f64::INFINITY,
            cell: 0,
        };

        let mut r: i64 = 0;
        while r == 0 || ((r - 1) as f64) < best.f2 {
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs().max(dy.abs()) != r {
                        continue;
                    }
                    // Skip cells whose nearest edge is already beyond F2.
                    let gap_x = axis_gap(dx, fx);
                    let gap_y = axis_gap(dy, fy);
                    if gap_x.max(gap_y) >= best.f2 {
                        continue;
                    }
                    let h = cell_hash(self.seed, &[ix + dx, iy + dy]);
                    let px = dx as f64 + unit(h, 0);
                    let py = dy as f64 + unit(h, 1);
                    let d = voronoi_distance(px - fx, py - fy, self.metric);
                    best.offer(d, h);
                }
            }
            r += 1;
        }
        best
    }

    /// 3D F1 search over the 27 surrounding cells (Euclidean only — the 3D
    /// path exists for [`NoiseSource::sample_3d`]).
    fn nearest_3d(&self, x: f64, y: f64, z: f64) -> Nearest {
        let (ix, fx) = split(x);
        let (iy, fy) = split(y);
        let (iz, fz) = split(z);
        let mut best = Nearest {
            f1: f64::INFINITY,
            f2: f64::INFINITY,
            cell: 0,
        };
        for dz in -1..=1 {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let h = cell_hash(self.seed, &[ix + dx, iy + dy, iz + dz]);
                    let ddx = dx as f64 + unit(h, 0) - fx;
                    let ddy = dy as f64 + unit(h, 1) - fy;
                    let ddz = dz as f64 + unit(h, 2) - fz;
                    best.offer((ddx * ddx + ddy * ddy + ddz * ddz).sqrt(), h);
                }
            }
        }
        best
    }

    fn to_signed(&self, n: &Nearest, norm: f64) -> f64 {
        if self.return_distance {
            ((n.f1 / norm).clamp(0.0, 1.0) * 2.0 - 1.0).clamp(-1.0, 1.0)
        } else {
            unit(n.cell, 3) * 2.0 - 1.0
        }
    }
}

impl Nearest {
    #[inline]
    fn offer(&mut self, d: f64, cell: u64) {
        if d < self.f1 {
            self.f2 = self.f1;
            self.f1 = d;
            self.cell = cell;
        } else if d < self.f2 {
            self.f2 = d;
        }
    }
}

impl NoiseSource for WorleyNoise {
    fn sample_2d(&self, x: f64, y: f64) -> f64 {
        let n = self.nearest_2d(x * self.frequency, y * self.frequency);
        self.to_signed(&n, worley_norm(self.metric))
    }

    fn sample_3d(&self, x: f64, y: f64, z: f64) -> f64 {
        let f = self.frequency;
        let n = self.nearest_3d(x * f, y * f, z * f);
        self.to_signed(&n, WORLEY_NORM_3D)
    }
}

/// Distance that maps to 1.0 after normalisation, per metric, chosen just
/// above the 99.9th percentile of F2 measured over 1.6M samples (Euclidean
/// 1.20, Manhattan 1.52), so F2 clamps only rarely and F1 (99.9th percentile
/// 0.97 / 1.27) spans most of \[0, 1\]. 3D F1's 99.9th percentile is 0.99.
fn worley_norm(metric: VoronoiMetric) -> f64 {
    match metric {
        VoronoiMetric::Euclidean => 1.25,
        VoronoiMetric::Manhattan => 1.6,
        VoronoiMetric::Chebyshev => 1.1,
        VoronoiMetric::Minkowski(_) => 1.25,
    }
}

const WORLEY_NORM_3D: f64 = 1.0;

/// Distance along one axis from the sample (at fractional offset `f` in cell
/// 0) to the nearest edge of the cell at offset `d`.
#[inline]
fn axis_gap(d: i64, f: f64) -> f64 {
    match d {
        0 => 0.0,
        d if d > 0 => d as f64 - f,
        d => f - (d + 1) as f64,
    }
}

/// Stateless 64-bit hash of a lattice cell (splitmix64 finaliser per
/// coordinate), so Worley needs no table and has no period.
#[inline]
fn cell_hash(seed: u64, coords: &[i64]) -> u64 {
    let mut h = seed ^ 0x9E37_79B9_7F4A_7C15;
    for &c in coords {
        h = mix64(h ^ (c as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9));
    }
    h
}

#[inline]
fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The `lane`-th uniform value in \[0, 1) derived from a cell hash.
#[inline]
fn unit(h: u64, lane: u64) -> f64 {
    (mix64(h.wrapping_add(lane)) >> 11) as f64 / (1u64 << 53) as f64
}

// ---------------------------------------------------------------------------
// FBM combinator
// ---------------------------------------------------------------------------

/// Fractal Brownian Motion — layers multiple octaves of a base noise source.
///
/// Works over any [`NoiseSource`] (including [`WorleyNoise`] and other
/// `Fbm`s). Each octave multiplies frequency by `lacunarity` and amplitude by
/// `persistence`; the sum is divided by the total amplitude so the result
/// stays within the base noise's range, then scaled by `amplitude`. With one
/// octave, frequency 1 and amplitude 1 the output is identical to the source.
pub struct Fbm<S: NoiseSource> {
    source: S,
    octaves: u32,
    frequency: f64,
    lacunarity: f64,
    persistence: f64,
    amplitude: f64,
}

impl<S: NoiseSource> Fbm<S> {
    pub fn new(source: S) -> Self {
        Self {
            source,
            octaves: 6,
            frequency: 1.0,
            lacunarity: 2.0,
            persistence: 0.5,
            amplitude: 1.0,
        }
    }

    pub fn with_octaves(mut self, octaves: u32) -> Self {
        self.octaves = octaves;
        self
    }

    pub fn with_frequency(mut self, frequency: f64) -> Self {
        self.frequency = frequency;
        self
    }

    pub fn with_lacunarity(mut self, lacunarity: f64) -> Self {
        self.lacunarity = lacunarity;
        self
    }

    pub fn with_persistence(mut self, persistence: f64) -> Self {
        self.persistence = persistence;
        self
    }

    /// Overall output scale applied after normalisation (default 1.0).
    pub fn with_amplitude(mut self, amplitude: f64) -> Self {
        self.amplitude = amplitude;
        self
    }

    fn layer(&self, mut octave: impl FnMut(f64) -> f64) -> f64 {
        let mut value = 0.0;
        let mut amplitude = 1.0;
        let mut freq = self.frequency;
        let mut max_amplitude = 0.0;

        for _ in 0..self.octaves {
            value += octave(freq) * amplitude;
            max_amplitude += amplitude;
            amplitude *= self.persistence;
            freq *= self.lacunarity;
        }

        if max_amplitude == 0.0 {
            return 0.0;
        }
        value / max_amplitude * self.amplitude
    }
}

impl<S: NoiseSource> NoiseSource for Fbm<S> {
    fn sample_2d(&self, x: f64, y: f64) -> f64 {
        self.layer(|f| self.source.sample_2d(x * f, y * f))
    }

    fn sample_3d(&self, x: f64, y: f64, z: f64) -> f64 {
        self.layer(|f| self.source.sample_3d(x * f, y * f, z * f))
    }
}

// ---------------------------------------------------------------------------
// Custom Voronoi
// ---------------------------------------------------------------------------

/// Distance metric for Voronoi cells.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VoronoiMetric {
    Euclidean,
    Manhattan,
    Chebyshev,
    Minkowski(f64),
}

/// Feature output for Voronoi texture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VoronoiFeature {
    F1,
    F2,
    SmoothF1,
    DistanceToEdge,
}

/// Result of a Voronoi sample — contains both the feature value and cell ID.
pub struct VoronoiResult {
    pub value: f64,
    pub cell_id: f64,
}

/// Hash a 2D grid cell to produce a pseudo-random feature point offset.
fn voronoi_hash(ix: i32, iy: i32, seed: u64) -> (f64, f64) {
    // Mix coordinates with seed using integer hashing
    let mut h = seed
        .wrapping_add(ix as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(iy as u64)
        .wrapping_mul(0x6C62_272E_07BB_0142);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    let x = (h & 0xFFFF_FFFF) as f64 / u32::MAX as f64;
    h = h.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    h ^= h >> 33;
    let y = (h & 0xFFFF_FFFF) as f64 / u32::MAX as f64;
    (x, y)
}

/// Compute distance with the given metric.
fn voronoi_distance(dx: f64, dy: f64, metric: VoronoiMetric) -> f64 {
    match metric {
        VoronoiMetric::Euclidean => (dx * dx + dy * dy).sqrt(),
        VoronoiMetric::Manhattan => dx.abs() + dy.abs(),
        VoronoiMetric::Chebyshev => dx.abs().max(dy.abs()),
        VoronoiMetric::Minkowski(e) => (dx.abs().powf(e) + dy.abs().powf(e)).powf(1.0 / e),
    }
}

/// Sample the custom Voronoi at a 2D point.
///
/// `scale` multiplies the input coordinates. `randomness` in \[0, 1\] controls
/// how far feature points deviate from grid centers (0 = grid, 1 = fully random).
pub fn voronoi_sample(
    x: f64,
    y: f64,
    seed: u64,
    scale: f64,
    randomness: f64,
    feature: VoronoiFeature,
    metric: VoronoiMetric,
) -> VoronoiResult {
    let sx = x * scale;
    let sy = y * scale;
    let ix = sx.floor() as i32;
    let iy = sy.floor() as i32;

    let mut d1 = f64::MAX;
    let mut d2 = f64::MAX;
    let mut closest_cell = (0i32, 0i32);

    // Search 3x3 neighborhood
    for dy in -1..=1 {
        for dx in -1..=1 {
            let cx = ix + dx;
            let cy = iy + dy;
            let (ox, oy) = voronoi_hash(cx, cy, seed);
            let px = cx as f64 + 0.5 + (ox - 0.5) * randomness;
            let py = cy as f64 + 0.5 + (oy - 0.5) * randomness;
            let dist = voronoi_distance(sx - px, sy - py, metric);

            if dist < d1 {
                d2 = d1;
                d1 = dist;
                closest_cell = (cx, cy);
            } else if dist < d2 {
                d2 = dist;
            }
        }
    }

    // Cell ID: hash the closest cell to a 0-1 value
    let (cell_hash, _) = voronoi_hash(closest_cell.0, closest_cell.1, seed.wrapping_add(1));

    let value = match feature {
        VoronoiFeature::F1 => d1,
        VoronoiFeature::F2 => d2,
        VoronoiFeature::SmoothF1 => {
            // Smooth minimum approximation
            let k = 0.1_f64;
            let a = (-d1 / k).exp();
            let b = (-d2 / k).exp();
            -(a + b).ln() * k
        }
        VoronoiFeature::DistanceToEdge => {
            // Approximate distance to the nearest cell edge
            // by finding the midpoint between closest and second-closest
            // and computing distance to that bisector plane
            (d2 - d1) * 0.5
        }
    };

    VoronoiResult {
        value,
        cell_id: cell_hash,
    }
}

// ---------------------------------------------------------------------------
// Musgrave FBM variants
// ---------------------------------------------------------------------------

/// Musgrave noise type selector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MusgraveType {
    Fbm,
    Multifractal,
    RidgedMultifractal,
    HybridMultifractal,
    HeteroTerrain,
}

/// Sample a Musgrave-type FBM variant from a noise source.
///
/// Parameters:
/// - `dimension`: fractal dimension (controls how fast amplitude drops per octave)
/// - `offset`: used by ridged/hybrid/hetero variants
/// - `gain`: used by ridged/hybrid variants
///
/// Returns a value roughly centered on 0 but range depends on variant.
#[allow(clippy::too_many_arguments)]
pub fn musgrave_sample(
    source: &dyn NoiseSource,
    x: f64,
    y: f64,
    musgrave_type: MusgraveType,
    frequency: f64,
    octaves: u32,
    lacunarity: f64,
    dimension: f64,
    offset: f64,
    gain: f64,
) -> f64 {
    match musgrave_type {
        MusgraveType::Fbm => musgrave_fbm(source, x, y, frequency, octaves, lacunarity, dimension),
        MusgraveType::Multifractal => musgrave_multifractal(
            source, x, y, frequency, octaves, lacunarity, dimension, offset,
        ),
        MusgraveType::RidgedMultifractal => musgrave_ridged(
            source, x, y, frequency, octaves, lacunarity, dimension, offset, gain,
        ),
        MusgraveType::HybridMultifractal => musgrave_hybrid(
            source, x, y, frequency, octaves, lacunarity, dimension, offset, gain,
        ),
        MusgraveType::HeteroTerrain => musgrave_hetero(
            source, x, y, frequency, octaves, lacunarity, dimension, offset,
        ),
    }
}

fn musgrave_fbm(
    source: &dyn NoiseSource,
    x: f64,
    y: f64,
    frequency: f64,
    octaves: u32,
    lacunarity: f64,
    dimension: f64,
) -> f64 {
    let mut value = 0.0;
    let mut freq = frequency;
    for i in 0..octaves {
        let amp = freq.powf(-dimension);
        value += source.sample_2d(x * freq, y * freq) * amp;
        freq *= lacunarity;
        // Small offset per octave to decorrelate
        let _ = i;
    }
    value
}

#[allow(clippy::too_many_arguments)]
fn musgrave_multifractal(
    source: &dyn NoiseSource,
    x: f64,
    y: f64,
    frequency: f64,
    octaves: u32,
    lacunarity: f64,
    dimension: f64,
    offset: f64,
) -> f64 {
    let mut value = 1.0;
    let mut freq = frequency;
    for _ in 0..octaves {
        let amp = freq.powf(-dimension);
        value *= (source.sample_2d(x * freq, y * freq) + offset) * amp;
        freq *= lacunarity;
    }
    value
}

#[allow(clippy::too_many_arguments)]
fn musgrave_ridged(
    source: &dyn NoiseSource,
    x: f64,
    y: f64,
    frequency: f64,
    octaves: u32,
    lacunarity: f64,
    dimension: f64,
    offset: f64,
    gain: f64,
) -> f64 {
    let mut value = 0.0;
    let mut weight = 1.0;
    let mut freq = frequency;
    for _ in 0..octaves {
        let amp = freq.powf(-dimension);
        let signal = offset - source.sample_2d(x * freq, y * freq).abs();
        let signal = signal * signal * weight;
        weight = (signal * gain).clamp(0.0, 1.0);
        value += signal * amp;
        freq *= lacunarity;
    }
    value
}

#[allow(clippy::too_many_arguments)]
fn musgrave_hybrid(
    source: &dyn NoiseSource,
    x: f64,
    y: f64,
    frequency: f64,
    octaves: u32,
    lacunarity: f64,
    dimension: f64,
    offset: f64,
    gain: f64,
) -> f64 {
    let mut freq = frequency;
    let amp0 = freq.powf(-dimension);
    let mut value = (source.sample_2d(x * freq, y * freq) + offset) * amp0;
    let mut weight = value;
    freq *= lacunarity;

    for _ in 1..octaves {
        weight = (weight * gain).clamp(0.0, 1.0);
        let amp = freq.powf(-dimension);
        let signal = (source.sample_2d(x * freq, y * freq) + offset) * amp;
        value += weight * signal;
        freq *= lacunarity;
    }
    value
}

#[allow(clippy::too_many_arguments)]
fn musgrave_hetero(
    source: &dyn NoiseSource,
    x: f64,
    y: f64,
    frequency: f64,
    octaves: u32,
    lacunarity: f64,
    dimension: f64,
    offset: f64,
) -> f64 {
    let mut freq = frequency;
    let amp0 = freq.powf(-dimension);
    let mut value = (source.sample_2d(x * freq, y * freq) + offset) * amp0;
    freq *= lacunarity;

    for _ in 1..octaves {
        let amp = freq.powf(-dimension);
        let increment = (source.sample_2d(x * freq, y * freq) + offset) * amp * value;
        value += increment;
        freq *= lacunarity;
    }
    value
}

// ---------------------------------------------------------------------------
// Convenience constructors
// ---------------------------------------------------------------------------

/// Create a Perlin-based FBM noise source.
pub fn perlin_fbm(seed: u64, octaves: u32, frequency: f64) -> Fbm<PerlinNoise> {
    Fbm::new(PerlinNoise::new(seed))
        .with_octaves(octaves)
        .with_frequency(frequency)
}

/// Create a Simplex-based FBM noise source.
pub fn simplex_fbm(seed: u64, octaves: u32, frequency: f64) -> Fbm<SimplexNoise> {
    Fbm::new(SimplexNoise::new(seed))
        .with_octaves(octaves)
        .with_frequency(frequency)
}

// ---------------------------------------------------------------------------
// Image helper
// ---------------------------------------------------------------------------

/// Evaluate a noise source over a 2D grid and produce a grayscale RGBA8 image.
///
/// The `scale` parameter controls the zoom level — higher values reveal more
/// detail (as if zooming out). Each pixel maps noise \[-1, 1\] → grayscale
/// \[0, 255\]. The resulting [`ImageData`] uses [`ChannelSemantics::Height`].
pub fn noise_to_image(source: &dyn NoiseSource, width: u32, height: u32, scale: f64) -> ImageData {
    let mut img = ImageData::new(width, height, ChannelSemantics::Height);

    for y in 0..height {
        for x in 0..width {
            let nx = x as f64 / width as f64 * scale;
            let ny = y as f64 / height as f64 * scale;
            let v = source.sample_2d(nx, ny);
            let byte = ((v * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            let offset = ((y * width + x) * 4) as usize;
            img.pixels[offset] = byte;
            img.pixels[offset + 1] = byte;
            img.pixels[offset + 2] = byte;
            img.pixels[offset + 3] = 255;
        }
    }

    img
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 100 deterministic sample points spread over several lattice cells,
    /// including negative coordinates.
    fn points() -> impl Iterator<Item = (f64, f64, f64)> {
        (0..100).map(|i| {
            let t = i as f64;
            (t * 0.173 - 8.0, t * 0.071 + 0.3, t * -0.113)
        })
    }

    fn assert_deterministic(make: impl Fn() -> Box<dyn NoiseSource>) {
        let a = make();
        let b = make();
        for (x, y, z) in points() {
            assert_eq!(a.sample_2d(x, y).to_bits(), b.sample_2d(x, y).to_bits());
            assert_eq!(
                a.sample_3d(x, y, z).to_bits(),
                b.sample_3d(x, y, z).to_bits()
            );
        }
    }

    // -- Determinism ----------------------------------------------------------

    #[test]
    fn perlin_determinism() {
        assert_deterministic(|| Box::new(PerlinNoise::new(42)));
    }

    #[test]
    fn simplex_determinism() {
        assert_deterministic(|| Box::new(SimplexNoise::new(42)));
    }

    #[test]
    fn value_determinism() {
        assert_deterministic(|| Box::new(ValueNoise::new(42)));
    }

    #[test]
    fn worley_determinism() {
        assert_deterministic(|| Box::new(WorleyNoise::new(42)));
        assert_deterministic(|| Box::new(WorleyNoise::new(42).with_return_distance(true)));
        let a = WorleyNoise::new(42);
        let b = WorleyNoise::new(42);
        for (x, y, _) in points() {
            assert_eq!(a.f1_f2(x, y), b.f1_f2(x, y));
        }
    }

    #[test]
    fn fbm_determinism() {
        assert_deterministic(|| Box::new(perlin_fbm(42, 6, 1.0)));
    }

    #[test]
    fn permutation_table_is_a_permutation() {
        let t = PermutationTable::new(7);
        let mut seen = [false; 256];
        for &b in &t.perm[..256] {
            seen[b as usize] = true;
        }
        assert!(
            seen.iter().all(|&s| s),
            "table must contain every byte once"
        );
        assert_eq!(
            t.perm[..256],
            t.perm[256..],
            "second half mirrors the first"
        );
    }

    // -- Different seeds differ -----------------------------------------------

    #[test]
    fn different_seeds_differ() {
        let pairs: Vec<(Box<dyn NoiseSource>, Box<dyn NoiseSource>)> = vec![
            (Box::new(PerlinNoise::new(1)), Box::new(PerlinNoise::new(2))),
            (
                Box::new(SimplexNoise::new(1)),
                Box::new(SimplexNoise::new(2)),
            ),
            (Box::new(ValueNoise::new(1)), Box::new(ValueNoise::new(2))),
            (Box::new(WorleyNoise::new(1)), Box::new(WorleyNoise::new(2))),
        ];
        for (a, b) in &pairs {
            let differs = points().any(|(x, y, _)| a.sample_2d(x, y) != b.sample_2d(x, y));
            assert!(differs, "different seeds should produce different output");
        }
    }

    // -- Output range ---------------------------------------------------------

    /// 10,000 points across many cells, both signs.
    fn range_points() -> impl Iterator<Item = (f64, f64, f64)> {
        (0..10_000).map(|i| {
            let t = i as f64;
            (
                t * 0.037 - 150.0,
                t * 0.023 - 90.0,
                (t * 0.719).sin() * 40.0,
            )
        })
    }

    fn assert_unit_range(name: &str, n: &dyn NoiseSource) {
        for (x, y, z) in range_points() {
            let v = n.sample_2d(x, y);
            assert!(
                (-1.0..=1.0).contains(&v),
                "{name} 2d out of range: {v} at ({x}, {y})"
            );
            let v3 = n.sample_3d(x, y, z);
            assert!(
                (-1.0..=1.0).contains(&v3),
                "{name} 3d out of range: {v3} at ({x}, {y}, {z})"
            );
        }
    }

    #[test]
    fn perlin_output_range() {
        assert_unit_range("perlin", &PerlinNoise::new(99));
    }

    #[test]
    fn simplex_output_range() {
        assert_unit_range("simplex", &SimplexNoise::new(99));
    }

    #[test]
    fn value_output_range() {
        assert_unit_range("value", &ValueNoise::new(99));
    }

    #[test]
    fn worley_output_range() {
        assert_unit_range("worley", &WorleyNoise::new(99));
        assert_unit_range(
            "worley distance",
            &WorleyNoise::new(99).with_return_distance(true),
        );
    }

    #[test]
    fn fbm_normalization() {
        assert_unit_range("fbm", &perlin_fbm(99, 8, 2.0));
    }

    /// The fields should actually use their range, not hug zero.
    #[test]
    fn gradient_noise_spans_its_range() {
        let sources: Vec<(&str, Box<dyn NoiseSource>)> = vec![
            ("perlin", Box::new(PerlinNoise::new(5))),
            ("simplex", Box::new(SimplexNoise::new(5))),
        ];
        for (name, n) in &sources {
            let (mut lo, mut hi) = (0.0f64, 0.0f64);
            for (x, y, _) in range_points() {
                let v = n.sample_2d(x, y);
                lo = lo.min(v);
                hi = hi.max(v);
            }
            assert!(
                lo < -0.6 && hi > 0.6,
                "{name} range too narrow: [{lo}, {hi}]"
            );
        }
    }

    // -- Structural properties ------------------------------------------------

    #[test]
    fn perlin_is_zero_on_the_lattice() {
        let n = PerlinNoise::new(3);
        for i in -5..5 {
            for j in -5..5 {
                let (x, y) = (i as f64, j as f64);
                assert_eq!(n.sample_2d(x, y), 0.0);
                assert_eq!(n.sample_3d(x, y, (i + j) as f64), 0.0);
            }
        }
    }

    /// Shifting by a whole period on either axis lands on the same value, and
    /// inside one period the field is the ordinary Perlin it was built from.
    #[test]
    fn periodic_perlin_repeats_on_its_period() {
        let n = PerlinNoise::new(11);
        for (x, y, _) in points() {
            let a = n.sample_2d_periodic(x, y, 6, 3);
            for (dx, dy) in [(6.0, 0.0), (0.0, -3.0), (-12.0, 9.0)] {
                let b = n.sample_2d_periodic(x + dx, y + dy, 6, 3);
                assert!(
                    (a - b).abs() < 1e-9,
                    "({x}, {y}) + ({dx}, {dy}): {a} vs {b}"
                );
            }
        }
        assert_eq!(n.sample_2d_periodic(2.3, 1.7, 8, 8), n.sample_2d(2.3, 1.7));
    }

    /// Small steps produce small changes: catches seams at cell or simplex
    /// boundaries (e.g. a simplex kernel radius that doesn't fall to zero).
    #[test]
    fn noise_is_continuous() {
        let sources: Vec<(&str, Box<dyn NoiseSource>)> = vec![
            ("perlin", Box::new(PerlinNoise::new(11))),
            ("simplex", Box::new(SimplexNoise::new(11))),
            ("value", Box::new(ValueNoise::new(11))),
        ];
        let h = 1e-4;
        for (name, n) in &sources {
            for (x, y, z) in range_points() {
                let d2 = (n.sample_2d(x + h, y + h) - n.sample_2d(x, y)).abs();
                let d3 = (n.sample_3d(x + h, y + h, z + h) - n.sample_3d(x, y, z)).abs();
                assert!(d2 < 0.01, "{name} 2d jumps by {d2} at ({x}, {y})");
                assert!(d3 < 0.01, "{name} 3d jumps by {d3} at ({x}, {y}, {z})");
            }
        }
    }

    #[test]
    fn noise_tiles_every_256_units() {
        let n = PerlinNoise::new(8);
        for (x, y, z) in points() {
            approx_eq(n.sample_2d(x, y), n.sample_2d(x + 256.0, y - 256.0));
            approx_eq(n.sample_3d(x, y, z), n.sample_3d(x, y + 256.0, z));
        }
    }

    // -- Worley ---------------------------------------------------------------

    #[test]
    fn worley_f1_never_exceeds_f2() {
        for metric in [VoronoiMetric::Euclidean, VoronoiMetric::Manhattan] {
            let n = WorleyNoise::new(17).with_metric(metric);
            for (x, y, _) in range_points() {
                let s = n.f1_f2(x, y);
                assert!(
                    s.f1 <= s.f2,
                    "{metric:?}: f1 {} > f2 {} at ({x}, {y})",
                    s.f1,
                    s.f2
                );
                assert!((0.0..=1.0).contains(&s.f1) && (0.0..=1.0).contains(&s.f2));
            }
        }
    }

    /// The ring search must agree with a brute-force scan of a wide window.
    #[test]
    fn worley_matches_brute_force() {
        for metric in [VoronoiMetric::Euclidean, VoronoiMetric::Manhattan] {
            let n = WorleyNoise::new(23).with_metric(metric);
            for (x, y, _) in range_points().step_by(10) {
                let (ix, fx) = split(x);
                let (iy, fy) = split(y);
                let mut d: Vec<f64> = Vec::new();
                for dy in -4..=4 {
                    for dx in -4..=4 {
                        let h = cell_hash(23, &[ix + dx, iy + dy]);
                        let px = dx as f64 + unit(h, 0);
                        let py = dy as f64 + unit(h, 1);
                        d.push(voronoi_distance(px - fx, py - fy, metric));
                    }
                }
                d.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let got = n.nearest_2d(x, y);
                assert_eq!(got.f1, d[0], "{metric:?} f1 at ({x}, {y})");
                assert_eq!(got.f2, d[1], "{metric:?} f2 at ({x}, {y})");
            }
        }
    }

    #[test]
    fn worley_metrics_differ() {
        let e = WorleyNoise::new(4);
        let m = WorleyNoise::new(4).with_metric(VoronoiMetric::Manhattan);
        assert!(points().any(|(x, y, _)| e.f1_f2(x, y) != m.f1_f2(x, y)));
    }

    #[test]
    fn worley_f1_is_zero_at_a_feature_point() {
        let n = WorleyNoise::new(31);
        let h = cell_hash(31, &[3, -2]);
        let (px, py) = (3.0 + unit(h, 0), -2.0 + unit(h, 1));
        assert!(n.f1_f2(px, py).f1 < 1e-6);
    }

    // -- FBM ------------------------------------------------------------------

    #[test]
    fn fbm_one_octave_matches_base() {
        let sources: Vec<(Box<dyn NoiseSource>, Box<dyn NoiseSource>)> = vec![
            (
                Box::new(PerlinNoise::new(9)),
                Box::new(Fbm::new(PerlinNoise::new(9)).with_octaves(1)),
            ),
            (
                Box::new(SimplexNoise::new(9)),
                Box::new(Fbm::new(SimplexNoise::new(9)).with_octaves(1)),
            ),
            (
                Box::new(WorleyNoise::new(9)),
                Box::new(Fbm::new(WorleyNoise::new(9)).with_octaves(1)),
            ),
        ];
        for (base, fbm) in &sources {
            for (x, y, z) in points() {
                assert_eq!(base.sample_2d(x, y), fbm.sample_2d(x, y));
                assert_eq!(base.sample_3d(x, y, z), fbm.sample_3d(x, y, z));
            }
        }
    }

    #[test]
    fn fbm_more_octaves_adds_detail() {
        let one = Fbm::new(PerlinNoise::new(9)).with_octaves(1);
        let four = Fbm::new(PerlinNoise::new(9)).with_octaves(4);
        let differs = points().filter(|&(x, y, _)| one.sample_2d(x, y) != four.sample_2d(x, y));
        assert!(
            differs.count() > 90,
            "4 octaves should differ almost everywhere"
        );

        // Detail = wiggles: count turning points along a line. Each octave
        // adds features at twice the frequency of the last.
        let turns = |n: &dyn NoiseSource| {
            let v: Vec<f64> = (0..4000)
                .map(|i| n.sample_2d(i as f64 * 0.005, 0.37))
                .collect();
            v.windows(3)
                .filter(|w| (w[1] - w[0]) * (w[2] - w[1]) < 0.0)
                .count()
        };
        let (t1, t4) = (turns(&one), turns(&four));
        assert!(
            t4 > t1 * 3,
            "4 octaves should turn far more often: {t4} vs {t1}"
        );
    }

    #[test]
    fn fbm_amplitude_scales_output() {
        let base = Fbm::new(SimplexNoise::new(2)).with_octaves(3);
        let half = Fbm::new(SimplexNoise::new(2))
            .with_octaves(3)
            .with_amplitude(0.5);
        for (x, y, _) in points() {
            approx_eq(base.sample_2d(x, y) * 0.5, half.sample_2d(x, y));
        }
    }

    #[test]
    fn fbm_zero_octaves_is_silent() {
        let n = Fbm::new(PerlinNoise::new(1)).with_octaves(0);
        assert_eq!(n.sample_2d(0.3, 0.7), 0.0);
    }

    // -- from_rng determinism -------------------------------------------------

    #[test]
    fn from_rng_determinism() {
        let mut rng_a = SeededRng::new(100);
        let mut rng_b = SeededRng::new(100);
        let pa = PerlinNoise::from_rng(&mut rng_a);
        let pb = PerlinNoise::from_rng(&mut rng_b);
        assert_eq!(pa.sample_2d(1.5, 2.5), pb.sample_2d(1.5, 2.5));

        let sa = SimplexNoise::from_rng(&mut rng_a);
        let sb = SimplexNoise::from_rng(&mut rng_b);
        assert_eq!(sa.sample_2d(1.5, 2.5), sb.sample_2d(1.5, 2.5));

        let wa = WorleyNoise::from_rng(&mut rng_a);
        let wb = WorleyNoise::from_rng(&mut rng_b);
        assert_eq!(wa.sample_2d(1.5, 2.5), wb.sample_2d(1.5, 2.5));

        let ta = PermutationTable::from_rng(&mut rng_a);
        let tb = PermutationTable::from_rng(&mut rng_b);
        assert_eq!(ta.perm, tb.perm);
    }

    // -- Known-value snapshots ------------------------------------------------

    /// Pinned outputs for seed 42. These change only if the algorithms, the
    /// permutation shuffle or the Worley hash change — any of which alters
    /// every procedural texture and mesh built on them, so update deliberately.
    #[test]
    fn known_value_snapshots() {
        let perlin = PerlinNoise::new(42);
        let simplex = SimplexNoise::new(42);
        let value = ValueNoise::new(42);
        let worley = WorleyNoise::new(42);

        let cases: [(&str, f64, f64); 9] = [
            ("perlin 2d", perlin.sample_2d(0.5, 0.5), SNAP_PERLIN_2D),
            (
                "perlin 3d",
                perlin.sample_3d(1.25, -3.5, 7.75),
                SNAP_PERLIN_3D,
            ),
            ("simplex 2d", simplex.sample_2d(0.5, 0.5), SNAP_SIMPLEX_2D),
            (
                "simplex 3d",
                simplex.sample_3d(1.25, -3.5, 7.75),
                SNAP_SIMPLEX_3D,
            ),
            ("value 2d", value.sample_2d(0.5, 0.5), SNAP_VALUE_2D),
            ("value 3d", value.sample_3d(1.25, -3.5, 7.75), SNAP_VALUE_3D),
            ("worley 2d", worley.sample_2d(0.5, 0.5), SNAP_WORLEY_2D),
            (
                "worley f1",
                worley.f1_f2(2.3, -1.7).f1 as f64,
                SNAP_WORLEY_F1,
            ),
            (
                "worley f2",
                worley.f1_f2(2.3, -1.7).f2 as f64,
                SNAP_WORLEY_F2,
            ),
        ];
        let drift: Vec<String> = cases
            .iter()
            .filter(|(_, got, want)| (got - want).abs() >= 1e-9)
            .map(|(name, got, want)| format!("{name}: got {got:.12}, pinned {want:.12}"))
            .collect();
        assert!(
            drift.is_empty(),
            "noise output drifted:
{}",
            drift.join(
                "
"
            )
        );
    }

    const SNAP_PERLIN_2D: f64 = -0.353553390593;
    const SNAP_PERLIN_3D: f64 = 0.227206768376;
    const SNAP_SIMPLEX_2D: f64 = -0.434384907343;
    const SNAP_SIMPLEX_3D: f64 = 0.115576850043;
    const SNAP_VALUE_2D: f64 = 0.260784313725;
    const SNAP_VALUE_3D: f64 = -0.026770842309;
    const SNAP_WORLEY_2D: f64 = -0.673696482294;
    const SNAP_WORLEY_F1: f64 = 0.277442604303;
    const SNAP_WORLEY_F2: f64 = 0.392840534449;

    fn approx_eq(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-12, "values differ: {a} vs {b}");
    }

    // -- noise_to_image -------------------------------------------------------

    #[test]
    fn noise_to_image_dimensions() {
        let perlin = PerlinNoise::new(1);
        let img = noise_to_image(&perlin, 64, 32, 4.0);
        assert_eq!(img.width, 64);
        assert_eq!(img.height, 32);
        assert_eq!(img.channel_semantics, ChannelSemantics::Height);
        assert!(img.validate().is_ok());
    }

    #[test]
    fn noise_to_image_not_uniform() {
        let perlin = PerlinNoise::new(7);
        let img = noise_to_image(&perlin, 32, 32, 4.0);
        // Not all pixels should be the same value.
        let first = img.pixels[0];
        let has_variation = img.pixels.iter().step_by(4).any(|&p| p != first);
        assert!(has_variation, "noise image should not be uniform");
    }

    // -- 01 remappers ---------------------------------------------------------

    #[test]
    fn remap_01_bounds() {
        let n = PerlinNoise::new(42);
        for i in 0..1000 {
            let x = i as f64 * 0.05;
            let y = i as f64 * 0.03;
            let v = n.sample_2d_01(x, y);
            assert!((0.0..=1.0).contains(&v), "01 remap out of range: {v}");
        }
    }

    // -- Visual debug ---------------------------------------------------------

    /// F1, F2 or F2 − F1 of a Worley field as a [`NoiseSource`] in \[-1, 1\],
    /// for the debug images only.
    struct WorleyFeature(WorleyNoise, fn(WorleySample) -> f32);

    impl NoiseSource for WorleyFeature {
        fn sample_2d(&self, x: f64, y: f64) -> f64 {
            (self.1)(self.0.f1_f2(x, y)) as f64 * 2.0 - 1.0
        }
        fn sample_3d(&self, x: f64, y: f64, _z: f64) -> f64 {
            self.sample_2d(x, y)
        }
    }

    /// Writes a 256×256 PNG of every noise type to `target/test-output/noise/`
    /// for manual inspection. Not an assertion — a visual aid.
    #[test]
    fn noise_visual_debug_pngs() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-output/noise");
        std::fs::create_dir_all(&dir).unwrap();

        let size = 256;
        let scale = 8.0;
        let manhattan = || WorleyNoise::new(42).with_metric(VoronoiMetric::Manhattan);

        let cases: Vec<(&str, Box<dyn NoiseSource>)> = vec![
            ("perlin", Box::new(PerlinNoise::new(42))),
            ("simplex", Box::new(SimplexNoise::new(42))),
            ("value", Box::new(ValueNoise::new(42))),
            ("worley_value", Box::new(WorleyNoise::new(42))),
            (
                "worley_distance",
                Box::new(WorleyNoise::new(42).with_return_distance(true)),
            ),
            (
                "worley_f1",
                Box::new(WorleyFeature(WorleyNoise::new(42), |s| s.f1)),
            ),
            (
                "worley_f2",
                Box::new(WorleyFeature(WorleyNoise::new(42), |s| s.f2)),
            ),
            (
                "worley_f2_minus_f1",
                Box::new(WorleyFeature(WorleyNoise::new(42), |s| s.f2 - s.f1)),
            ),
            (
                "worley_f1_manhattan",
                Box::new(WorleyFeature(manhattan(), |s| s.f1)),
            ),
            ("perlin_fbm_1", Box::new(perlin_fbm(42, 1, 2.0))),
            ("perlin_fbm", Box::new(perlin_fbm(42, 6, 2.0))),
            ("simplex_fbm", Box::new(simplex_fbm(42, 6, 2.0))),
        ];

        for (name, source) in &cases {
            let img = noise_to_image(source.as_ref(), size, size, scale);
            let path = dir.join(format!("{name}.png"));
            image::save_buffer(
                &path,
                &img.pixels,
                img.width,
                img.height,
                image::ColorType::Rgba8,
            )
            .unwrap();
        }
    }
}
