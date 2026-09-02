//! Shared Voronoi tessellation helpers.
//!
//! Provides seed-point generation, Lloyd's relaxation, spatial acceleration,
//! and toroidal distance utilities used by both the legacy `VoronoiBrickPattern`
//! and the pipeline `VoronoiGridOp`.

use crate::SeededRng;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Squared distance between two points, with optional toroidal wrapping.
pub fn dist_sq(ax: f32, ay: f32, bx: f32, by: f32, seamless: bool) -> f32 {
    if seamless {
        let mut dx = (ax - bx).abs();
        let mut dy = (ay - by).abs();
        if dx > 0.5 {
            dx = 1.0 - dx;
        }
        if dy > 0.5 {
            dy = 1.0 - dy;
        }
        dx * dx + dy * dy
    } else {
        let dx = ax - bx;
        let dy = ay - by;
        dx * dx + dy * dy
    }
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
pub fn lloyds_relaxation(
    points: &[(f32, f32)],
    iterations: usize,
    seamless: bool,
) -> Vec<(f32, f32)> {
    let mut current = points.to_vec();
    let resolution = 64u32; // Sample grid for centroid estimation

    for _ in 0..iterations {
        // Accumulate centroid sums per cell
        let mut sum_x = vec![0.0f64; current.len()];
        let mut sum_y = vec![0.0f64; current.len()];
        let mut count = vec![0u32; current.len()];

        for sy in 0..resolution {
            for sx in 0..resolution {
                let u = (sx as f32 + 0.5) / resolution as f32;
                let v = (sy as f32 + 0.5) / resolution as f32;

                let mut best_id = 0;
                let mut best_dist = f32::INFINITY;

                for (id, &(px, py)) in current.iter().enumerate() {
                    let d = dist_sq(u, v, px, py, seamless);
                    if d < best_dist {
                        best_dist = d;
                        best_id = id;
                    }
                }

                sum_x[best_id] += u as f64;
                sum_y[best_id] += v as f64;
                count[best_id] += 1;
            }
        }

        for i in 0..current.len() {
            if count[i] > 0 {
                let cx = (sum_x[i] / count[i] as f64) as f32;
                let cy = (sum_y[i] / count[i] as f64) as f32;
                // Keep in [0, 1) for seamless wrapping
                current[i] = (cx.rem_euclid(1.0), cy.rem_euclid(1.0));
            }
        }
    }

    current
}

// ---------------------------------------------------------------------------
// Spatial acceleration grid
// ---------------------------------------------------------------------------

/// A spatial hash grid for accelerating nearest-neighbor queries on seed points.
///
/// For seamless tiling, the grid stores references to all 9 toroidal copies
/// of each seed point so boundary queries are handled naturally.
pub struct SpatialGrid {
    /// Number of grid cells per axis.
    grid_size: usize,
    /// For each grid cell, a list of (point_index, wrapped_x, wrapped_y).
    cells: Vec<Vec<(usize, f32, f32)>>,
}

impl SpatialGrid {
    pub fn new(points: &[(f32, f32)], seamless: bool) -> Self {
        let grid_size = (points.len() as f32).sqrt().ceil().max(2.0) as usize;
        let mut cells = vec![Vec::new(); grid_size * grid_size];
        let inv = grid_size as f32;

        let offsets: &[(f32, f32)] = if seamless {
            &[
                (0.0, 0.0),
                (-1.0, 0.0),
                (1.0, 0.0),
                (0.0, -1.0),
                (0.0, 1.0),
                (-1.0, -1.0),
                (-1.0, 1.0),
                (1.0, -1.0),
                (1.0, 1.0),
            ]
        } else {
            &[(0.0, 0.0)]
        };

        for (idx, &(px, py)) in points.iter().enumerate() {
            for &(ox, oy) in offsets {
                let wx = px + ox;
                let wy = py + oy;
                // Only insert if the wrapped point could be relevant (near [0,1]²)
                if (-0.5..=1.5).contains(&wx) && (-0.5..=1.5).contains(&wy) {
                    let gx =
                        ((wx * inv).floor() as isize).clamp(0, grid_size as isize - 1) as usize;
                    let gy =
                        ((wy * inv).floor() as isize).clamp(0, grid_size as isize - 1) as usize;
                    cells[gy * grid_size + gx].push((idx, wx, wy));
                }
            }
        }

        Self { grid_size, cells }
    }

    /// Find the nearest and second-nearest seed point to `(u, v)`.
    /// Returns `(closest_id, distance_to_closest, distance_to_second_closest)`.
    pub fn find_two_nearest(
        &self,
        u: f32,
        v: f32,
        _points: &[(f32, f32)],
        _seamless: bool,
    ) -> (usize, f32, f32) {
        let inv = self.grid_size as f32;
        let gx = ((u * inv).floor() as isize).clamp(0, self.grid_size as isize - 1) as usize;
        let gy = ((v * inv).floor() as isize).clamp(0, self.grid_size as isize - 1) as usize;

        let mut best_id = 0;
        let mut best_dist = f32::INFINITY;
        let mut second_dist = f32::INFINITY;

        // Search the 3x3 neighborhood of grid cells
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let nx = (gx as i32 + dx).clamp(0, self.grid_size as i32 - 1) as usize;
                let ny = (gy as i32 + dy).clamp(0, self.grid_size as i32 - 1) as usize;

                for &(idx, wx, wy) in &self.cells[ny * self.grid_size + nx] {
                    let dx = u - wx;
                    let dy = v - wy;
                    let d = (dx * dx + dy * dy).sqrt();

                    if d < best_dist {
                        second_dist = best_dist;
                        best_dist = d;
                        best_id = idx;
                    } else if d < second_dist {
                        second_dist = d;
                    }
                }
            }
        }

        (best_id, best_dist, second_dist)
    }
}
