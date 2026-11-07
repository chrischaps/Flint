//! Shared Voronoi tessellation helpers.
//!
//! Provides seed-point generation, Lloyd's relaxation, spatial acceleration,
//! and toroidal distance utilities used by both the legacy `VoronoiBrickPattern`
//! and the pipeline `VoronoiGridOp`.

use crate::SeededRng;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Wrap a coordinate difference into `[-0.5, 0.5]` (the shortest way round
/// a unit torus).
#[inline]
pub fn wrap_delta(d: f32) -> f32 {
    d - d.round()
}

/// Squared distance between two points, with optional toroidal wrapping.
pub fn dist_sq(ax: f32, ay: f32, bx: f32, by: f32, seamless: bool) -> f32 {
    let (mut dx, mut dy) = (ax - bx, ay - by);
    if seamless {
        dx = wrap_delta(dx);
        dy = wrap_delta(dy);
    }
    dx * dx + dy * dy
}

/// Linear interpolation helper.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ---------------------------------------------------------------------------
// Seed point generation
// ---------------------------------------------------------------------------

pub fn generate_seed_points(rng: &mut SeededRng, count: usize) -> Vec<(f32, f32)> {
    let mut point_rng = rng.fork("voronoi_points");
    (0..count)
        .map(|_| (point_rng.next_f32(), point_rng.next_f32()))
        .collect()
}

// ---------------------------------------------------------------------------
// Lloyd's relaxation
// ---------------------------------------------------------------------------

/// Perform Lloyd's relaxation iterations on the seed points.
/// Moves each point toward the centroid of its Voronoi cell.
///
/// On a torus a cell can straddle the seam, so samples are accumulated as
/// offsets from the cell's own point (wrapped the short way round); averaging
/// raw coordinates would drag such a cell's point to the middle of the tile.
pub fn lloyds_relaxation(
    points: &[(f32, f32)],
    iterations: usize,
    seamless: bool,
) -> Vec<(f32, f32)> {
    let mut current = points.to_vec();
    let resolution = 64u32; // Sample grid for centroid estimation

    for _ in 0..iterations {
        let grid = SpatialGrid::new(&current, seamless);
        let mut sum_x = vec![0.0f64; current.len()];
        let mut sum_y = vec![0.0f64; current.len()];
        let mut count = vec![0u32; current.len()];

        for sy in 0..resolution {
            for sx in 0..resolution {
                let u = (sx as f32 + 0.5) / resolution as f32;
                let v = (sy as f32 + 0.5) / resolution as f32;
                let id = grid.nearest(u, v).id;
                let (px, py) = current[id];
                let (mut dx, mut dy) = (u - px, v - py);
                if seamless {
                    dx = wrap_delta(dx);
                    dy = wrap_delta(dy);
                }
                sum_x[id] += dx as f64;
                sum_y[id] += dy as f64;
                count[id] += 1;
            }
        }

        for i in 0..current.len() {
            if count[i] > 0 {
                let cx = current[i].0 + (sum_x[i] / count[i] as f64) as f32;
                let cy = current[i].1 + (sum_y[i] / count[i] as f64) as f32;
                current[i] = if seamless {
                    (cx.rem_euclid(1.0), cy.rem_euclid(1.0))
                } else {
                    (cx.clamp(0.0, 1.0), cy.clamp(0.0, 1.0))
                };
            }
        }
    }

    current
}

/// Move each point a fraction `t` of the way to its relaxed position. On a
/// torus it goes the short way round, so a point that relaxation carried
/// across the seam is not dragged back through the middle of the tile.
pub fn blend_points(points: &mut [(f32, f32)], relaxed: &[(f32, f32)], t: f32, seamless: bool) {
    for (p, r) in points.iter_mut().zip(relaxed) {
        if seamless {
            p.0 = (p.0 + wrap_delta(r.0 - p.0) * t).rem_euclid(1.0);
            p.1 = (p.1 + wrap_delta(r.1 - p.1) * t).rem_euclid(1.0);
        } else {
            p.0 = lerp(p.0, r.0, t);
            p.1 = lerp(p.1, r.1, t);
        }
    }
}

// ---------------------------------------------------------------------------
// Spatial acceleration grid
// ---------------------------------------------------------------------------

/// The answer to a nearest-site query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Nearest {
    /// Index of the nearest seed point.
    pub id: usize,
    /// Distance to the nearest site.
    pub d1: f32,
    /// Distance to the second-nearest site (another point, or on a torus
    /// another copy of the same one).
    pub d2: f32,
    /// Exact distance to the cell's boundary: the nearest bisector between
    /// the nearest site and any other. Unlike `d2 - d1`, this is the same at
    /// every point along an edge, so a groove of width `2w` is `w` either side.
    pub edge: f32,
}

/// A uniform bucket grid over `[0, 1)²` for nearest-site queries.
///
/// Each point sits once, in the bucket that holds it. A query searches square
/// rings of buckets outward from its own until no unvisited bucket could hold
/// anything closer, so it is exact however unevenly the points fall. On a
/// torus, buckets past the edge are the wrapped buckets with their points
/// shifted by a whole tile, so every periodic copy is a site of its own.
pub struct SpatialGrid {
    /// Buckets per axis.
    grid_size: usize,
    /// Point indices in each bucket, row-major.
    cells: Vec<Vec<usize>>,
    points: Vec<(f32, f32)>,
    seamless: bool,
}

impl SpatialGrid {
    pub fn new(points: &[(f32, f32)], seamless: bool) -> Self {
        let grid_size = (points.len() as f32).sqrt().ceil().max(1.0) as usize;
        let mut cells = vec![Vec::new(); grid_size * grid_size];
        for (idx, &(px, py)) in points.iter().enumerate() {
            let (gx, gy) = (bucket(px, grid_size), bucket(py, grid_size));
            cells[gy * grid_size + gx].push(idx);
        }
        Self {
            grid_size,
            cells,
            points: points.to_vec(),
            seamless,
        }
    }

    /// Visit every site in the ring of buckets at Chebyshev distance `r`
    /// from bucket `(gx, gy)`, as `(index, x, y)` with toroidal shifts applied.
    fn for_ring(&self, gx: i64, gy: i64, r: i64, mut f: impl FnMut(usize, f32, f32)) {
        let g = self.grid_size as i64;
        for dy in -r..=r {
            let step = if dy.abs() == r { 1 } else { 2 * r.max(1) };
            let mut dx = -r;
            while dx <= r {
                let (cx, cy) = (gx + dx, gy + dy);
                dx += step;
                let (bx, by, ox, oy) = if self.seamless {
                    (
                        cx.rem_euclid(g),
                        cy.rem_euclid(g),
                        cx.div_euclid(g) as f32,
                        cy.div_euclid(g) as f32,
                    )
                } else if (0..g).contains(&cx) && (0..g).contains(&cy) {
                    (cx, cy, 0.0, 0.0)
                } else {
                    continue;
                };
                for &idx in &self.cells[(by * g + bx) as usize] {
                    let (px, py) = self.points[idx];
                    f(idx, px + ox, py + oy);
                }
            }
        }
    }

    /// Largest ring worth searching. Rings up to `r` hold every site within
    /// `r / grid_size`; on a torus the second site is never more than ~1.12
    /// away (a copy of the nearest), so `grid_size + 2` covers it, and the
    /// distance tests above end the search long before that in practice.
    fn max_ring(&self) -> i64 {
        self.grid_size as i64 + 2
    }

    /// Nearest and second-nearest site to `(u, v)`, and the exact distance to
    /// the edge of the nearest site's cell.
    pub fn nearest(&self, u: f32, v: f32) -> Nearest {
        let g = self.grid_size as i64;
        let inv = 1.0 / self.grid_size as f32;
        let (gx, gy) = (
            bucket(u, self.grid_size) as i64,
            bucket(v, self.grid_size) as i64,
        );

        // Pass 1: the two nearest sites. A bucket in ring r is at least
        // (r - 1) bucket widths away, so stop once that exceeds d2.
        let (mut id, mut d1, mut d2) = (0usize, f32::INFINITY, f32::INFINITY);
        let mut site = (f32::NAN, f32::NAN);
        for r in 0..=self.max_ring() {
            if (r - 1) as f32 * inv > d2 {
                break;
            }
            self.for_ring(gx, gy, r, |idx, px, py| {
                let d = ((u - px).powi(2) + (v - py).powi(2)).sqrt();
                if d < d1 {
                    d2 = d1;
                    d1 = d;
                    id = idx;
                    site = (px, py);
                } else if d < d2 {
                    d2 = d;
                }
            });
            if !self.seamless && r >= g {
                break;
            }
        }

        // Pass 2: distance to the nearest bisector. A site at distance d is
        // at least (d - d1) / 2 from being the bisector's other side.
        let mut edge = f32::INFINITY;
        for r in 0..=self.max_ring() {
            if ((r - 1) as f32 * inv - d1) * 0.5 > edge {
                break;
            }
            self.for_ring(gx, gy, r, |_, px, py| {
                let (nx, ny) = (px - site.0, py - site.1);
                let len = (nx * nx + ny * ny).sqrt();
                if len < 1e-9 {
                    return; // the nearest site itself
                }
                // Signed distance from (u, v) to the perpendicular bisector.
                let (mx, my) = ((px + site.0) * 0.5, (py + site.1) * 0.5);
                let dist = ((mx - u) * nx + (my - v) * ny) / len;
                edge = edge.min(dist.max(0.0));
            });
            if !self.seamless && r >= g {
                break;
            }
        }

        Nearest { id, d1, d2, edge }
    }
}

fn bucket(x: f32, grid_size: usize) -> usize {
    ((x * grid_size as f32).floor().max(0.0) as usize).min(grid_size - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every site, with all nine toroidal copies on a torus.
    fn sites(pts: &[(f32, f32)], seamless: bool) -> Vec<(usize, f32, f32)> {
        let shifts: &[f32] = if seamless { &[-1.0, 0.0, 1.0] } else { &[0.0] };
        let mut out = Vec::new();
        for (i, &(x, y)) in pts.iter().enumerate() {
            for &ox in shifts {
                for &oy in shifts {
                    out.push((i, x + ox, y + oy));
                }
            }
        }
        out
    }

    fn brute(u: f32, v: f32, pts: &[(f32, f32)], seamless: bool) -> Nearest {
        let all = sites(pts, seamless);
        let mut d: Vec<(f32, usize, f32, f32)> = all
            .iter()
            .map(|&(i, x, y)| (((u - x).powi(2) + (v - y).powi(2)).sqrt(), i, x, y))
            .collect();
        d.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let (d1, id, sx, sy) = d[0];
        let edge = d[1..]
            .iter()
            .map(|&(_, _, x, y)| {
                let (nx, ny) = (x - sx, y - sy);
                let len = (nx * nx + ny * ny).sqrt();
                (((x + sx) * 0.5 - u) * nx + ((y + sy) * 0.5 - v) * ny) / len
            })
            .fold(f32::INFINITY, f32::min);
        Nearest {
            id,
            d1,
            d2: d[1].0,
            edge,
        }
    }

    /// The grid must agree with brute force at every pixel, for sparse and
    /// dense, flat and toroidal layouts. The old 3x3 search (with clamped
    /// neighbours that visited border buckets twice) got the second distance
    /// wrong at ~69% of pixels for 12 points, and the cell itself at ~1%.
    #[test]
    fn grid_matches_brute_force() {
        for &(n, seamless) in &[
            (1usize, true),
            (2, false),
            (12, true),
            (12, false),
            (64, true),
            (64, false),
        ] {
            for seed in 0..3u64 {
                let mut rng = SeededRng::new(seed);
                let pts = generate_seed_points(&mut rng, n);
                let grid = SpatialGrid::new(&pts, seamless);
                for y in 0..96 {
                    for x in 0..96 {
                        let u = (x as f32 + 0.5) / 96.0;
                        let v = (y as f32 + 0.5) / 96.0;
                        let g = grid.nearest(u, v);
                        let b = brute(u, v, &pts, seamless);
                        assert!((g.d1 - b.d1).abs() < 1e-5, "n={n} seamless={seamless} d1");
                        if n > 1 || seamless {
                            assert!(
                                (g.d2 - b.d2).abs() < 1e-5,
                                "n={n} seamless={seamless} d2 {} vs {}",
                                g.d2,
                                b.d2
                            );
                            assert!(
                                (g.edge - b.edge).abs() < 1e-5,
                                "n={n} seamless={seamless} edge {} vs {}",
                                g.edge,
                                b.edge
                            );
                        }
                        // Ties can pick either index; the distance decides.
                        if g.id != b.id {
                            assert!((g.d1 - b.d1).abs() < 1e-6);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn relaxation_keeps_a_seam_cell_at_the_seam() {
        // One point near the corner of a torus: its cell is the whole tile,
        // centred on the point itself, so relaxing must not move it.
        let pts = [(0.02f32, 0.98f32)];
        let out = lloyds_relaxation(&pts, 3, true);
        assert!(wrap_delta(out[0].0 - 0.02).abs() < 0.01, "{:?}", out);
        assert!(wrap_delta(out[0].1 - 0.98).abs() < 0.01, "{:?}", out);
    }
}
