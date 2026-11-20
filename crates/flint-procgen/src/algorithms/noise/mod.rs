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
//!
//! Cellular noise ([`WorleyNoise`] and [`voronoi_sample`]) is in `cellular.rs`,
//! the Musgrave variants ([`musgrave_sample`]) in `musgrave.rs`; both are
//! re-exported here, so every type keeps its `noise::` path.

use crate::{ChannelSemantics, ImageData, SeededRng};

mod cellular;
mod musgrave;

pub use cellular::*;
pub use musgrave::*;

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

    /// Sample a version of the field that repeats every `period_x` along x
    /// and `period_y` along y, so `[0, period)` on each axis is one tile.
    ///
    /// Lattice noises (Perlin, value, Worley) wrap their lattice: a period is
    /// rounded to a whole number of cells (at least 1) and the coordinate is
    /// stretched to match, so the tile still spans `[0, period)` and the
    /// field is unchanged in character. [`Fbm`] passes each octave its own
    /// period, so every octave rounds for itself. The default, used by
    /// simplex (whose skewed lattice has no square period), cross-fades the
    /// plain field with copies shifted one period; see [`crossfade_tiled`].
    /// A period that isn't positive leaves that axis unwrapped.
    fn sample_2d_tiled(&self, x: f64, y: f64, period_x: f64, period_y: f64) -> f64 {
        crossfade_tiled(|x, y| self.sample_2d(x, y), x, y, period_x, period_y)
    }

    /// Convenience: remap [`sample_2d`](NoiseSource::sample_2d) to \[0, 1\].
    fn sample_2d_01(&self, x: f64, y: f64) -> f64 {
        self.sample_2d(x, y) * 0.5 + 0.5
    }

    /// Convenience: remap [`sample_3d`](NoiseSource::sample_3d) to \[0, 1\].
    fn sample_3d_01(&self, x: f64, y: f64, z: f64) -> f64 {
        self.sample_3d(x, y, z) * 0.5 + 0.5
    }
}

/// Make any field tile by cross-fading it with copies of itself shifted one
/// period left, up, and both: at `x = period_x` the blend is exactly the
/// `x = 0` value. Averaging lowers the variance mid-tile, so the blend is
/// divided by the RMS of its weights to keep the contrast even across the
/// tile. Coordinates outside `[0, period)` are wrapped first. A period that
/// isn't positive leaves that axis unblended.
pub fn crossfade_tiled(
    f: impl Fn(f64, f64) -> f64,
    x: f64,
    y: f64,
    period_x: f64,
    period_y: f64,
) -> f64 {
    // (wrapped coordinate, its copy one period back, weight of the copy)
    let axis = |c: f64, p: f64| {
        if p > 0.0 && p.is_finite() {
            let m = c.rem_euclid(p);
            (m, m - p, m / p)
        } else {
            (c, c, 0.0)
        }
    };
    let (x0, x1, s) = axis(x, period_x);
    let (y0, y1, t) = axis(y, period_y);
    let (ws, wt) = (1.0 - s, 1.0 - t);
    // Copies with zero weight are skipped, not sampled.
    let a = f(x0, y0);
    let b = if s > 0.0 { f(x1, y0) } else { 0.0 };
    let c = if t > 0.0 { f(x0, y1) } else { 0.0 };
    let d = if s > 0.0 && t > 0.0 { f(x1, y1) } else { 0.0 };
    let blend = wt * (ws * a + s * b) + t * (ws * c + s * d);
    let rms = ((ws * ws + s * s) * (wt * wt + t * t)).sqrt();
    (blend / rms).clamp(-1.0, 1.0)
}

/// Whole lattice cells per tile for a requested period, and the factor that
/// stretches a coordinate in `[0, period)` onto `[0, cells)`. A period that
/// isn't positive gets no wrap (`None`) and no stretch.
#[inline]
fn whole_cells(period: f64) -> (Option<i64>, f64) {
    if period > 0.0 && period.is_finite() {
        let cells = period.round().max(1.0);
        (Some(cells as i64), cells / period)
    } else {
        (None, 1.0)
    }
}

/// A lattice period for the permutation-table noises. `None` (no wrap)
/// becomes a multiple of the table's 256, which wraps nothing the table
/// doesn't already repeat.
#[inline]
fn table_period(cells: Option<i64>) -> i64 {
    cells.unwrap_or(1 << 40)
}

/// A cell coordinate wrapped into `[0, period)`, or left as it is.
#[inline]
fn wrap_cell(c: i64, period: Option<i64>) -> i64 {
    period.map_or(c, |p| c.rem_euclid(p))
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

    fn sample_2d_tiled(&self, x: f64, y: f64, period_x: f64, period_y: f64) -> f64 {
        let (px, sx) = whole_cells(period_x);
        let (py, sy) = whole_cells(period_y);
        self.sample_2d_periodic(x * sx, y * sy, table_period(px), table_period(py))
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

    fn sample_2d_tiled(&self, x: f64, y: f64, period_x: f64, period_y: f64) -> f64 {
        let (px, sx) = whole_cells(period_x);
        let (py, sy) = whole_cells(period_y);
        let (xi, xf) = split(x * sx);
        let (yi, yf) = split(y * sy);
        let v = |dx: i64, dy: i64| {
            byte_to_signed(self.perm.hash2(
                (xi + dx).rem_euclid(table_period(px)),
                (yi + dy).rem_euclid(table_period(py)),
            ))
        };
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

    fn sample_2d_tiled(&self, x: f64, y: f64, period_x: f64, period_y: f64) -> f64 {
        self.layer(|f| {
            self.source
                .sample_2d_tiled(x * f, y * f, period_x * f, period_y * f)
        })
    }

    fn sample_3d(&self, x: f64, y: f64, z: f64) -> f64 {
        self.layer(|f| self.source.sample_3d(x * f, y * f, z * f))
    }
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

#[cfg(test)]
mod tests;
