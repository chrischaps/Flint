//! Cellular noise: [`WorleyNoise`] (a jittered feature point per cell, as a
//! [`NoiseSource`]) and the custom Voronoi that `voronoi_texture` samples
//! ([`voronoi_sample`]), with the distance metric they share.

use crate::SeededRng;

use super::{split, whole_cells, wrap_cell, NoiseSource};

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
pub(super) struct Nearest {
    pub(super) f1: f64,
    pub(super) f2: f64,
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
        let n = self.nearest_2d(x * self.frequency, y * self.frequency, (None, None));
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
    ///
    /// With `wrap = (Some(px), Some(py))` cell coordinates are hashed modulo
    /// the period, so the feature points (and the per-cell value) repeat
    /// every `px` × `py` cells; `None` leaves an axis unwrapped.
    pub(super) fn nearest_2d(&self, x: f64, y: f64, wrap: (Option<i64>, Option<i64>)) -> Nearest {
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
                    let (cx, cy) = (wrap_cell(ix + dx, wrap.0), wrap_cell(iy + dy, wrap.1));
                    let h = cell_hash(self.seed, &[cx, cy]);
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
        let n = self.nearest_2d(x * self.frequency, y * self.frequency, (None, None));
        self.to_signed(&n, worley_norm(self.metric))
    }

    fn sample_2d_tiled(&self, x: f64, y: f64, period_x: f64, period_y: f64) -> f64 {
        let f = self.frequency;
        let (px, sx) = whole_cells(period_x * f);
        let (py, sy) = whole_cells(period_y * f);
        let n = self.nearest_2d(x * f * sx, y * f * sy, (px, py));
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
pub(super) fn cell_hash(seed: u64, coords: &[i64]) -> u64 {
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
pub(super) fn unit(h: u64, lane: u64) -> f64 {
    (mix64(h.wrapping_add(lane)) >> 11) as f64 / (1u64 << 53) as f64
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
pub(super) fn voronoi_distance(dx: f64, dy: f64, metric: VoronoiMetric) -> f64 {
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
    voronoi_cells(
        x * scale,
        y * scale,
        None,
        seed,
        randomness,
        feature,
        metric,
    )
}

/// [`voronoi_sample`] tiling over the unit square: `scale` is rounded to a
/// whole number of cells per tile (at least 1) and cell coordinates are
/// hashed modulo it, so the feature points, values and cell ids repeat at
/// `x = 1` and `y = 1`.
pub fn voronoi_sample_tiled(
    x: f64,
    y: f64,
    seed: u64,
    scale: f64,
    randomness: f64,
    feature: VoronoiFeature,
    metric: VoronoiMetric,
) -> VoronoiResult {
    let cells = scale.round().max(1.0);
    let period = cells.min(i32::MAX as f64) as i32;
    let wrap = Some((period, period));
    voronoi_cells(
        x * cells,
        y * cells,
        wrap,
        seed,
        randomness,
        feature,
        metric,
    )
}

/// Voronoi over cell space (one cell per unit), optionally wrapping cell
/// coordinates every `wrap` cells.
fn voronoi_cells(
    sx: f64,
    sy: f64,
    wrap: Option<(i32, i32)>,
    seed: u64,
    randomness: f64,
    feature: VoronoiFeature,
    metric: VoronoiMetric,
) -> VoronoiResult {
    let hash_cell = |cx: i32, cy: i32| match wrap {
        Some((px, py)) => (cx.rem_euclid(px), cy.rem_euclid(py)),
        None => (cx, cy),
    };
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
            let (hx, hy) = hash_cell(cx, cy);
            let (ox, oy) = voronoi_hash(hx, hy, seed);
            let px = cx as f64 + 0.5 + (ox - 0.5) * randomness;
            let py = cy as f64 + 0.5 + (oy - 0.5) * randomness;
            let dist = voronoi_distance(sx - px, sy - py, metric);

            if dist < d1 {
                d2 = d1;
                d1 = dist;
                closest_cell = hash_cell(cx, cy);
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
