//! Voronoi-based brick/stone pattern generator.
//!
//! Produces irregular stone and cobblestone layouts via Voronoi tessellation
//! with optional Lloyd's relaxation for regularity control. Each cell gets a
//! unique ID, height variation, and edge distance suitable for mortar grooves.
//!
//! This is a custom implementation (not `noise::Worley`) because we need cell
//! IDs and precise edge distances, not just distance values.

use crate::algorithms::noise::{Fbm, NoiseSource, PerlinNoise};
use crate::generators::util::{toml_f32, toml_u32};
use crate::{ProcGenError, Result, SeededRng};

use super::pattern::{Pattern, PatternField};

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

/// Parameters for the Voronoi brick/stone pattern.
#[derive(Debug, Clone)]
pub struct VoronoiBrickParams {
    /// Number of Voronoi cells (seed points). ~12 for a stone wall.
    pub cell_count: u32,
    /// Width of the mortar groove as a fraction of the texture. 0.02 typical.
    pub mortar_width: f32,
    /// Per-cell height randomness in \[0, 1\]. 0 = flat, 1 = max variation.
    pub height_variation: f32,
    /// Regularity: 0 = random Voronoi, 1 = fully grid-relaxed (Lloyd's).
    /// Intermediate values blend between random and relaxed positions.
    pub regularity: f32,
}

impl Default for VoronoiBrickParams {
    fn default() -> Self {
        Self {
            cell_count: 12,
            mortar_width: 0.02,
            height_variation: 0.3,
            regularity: 0.0,
        }
    }
}

impl VoronoiBrickParams {
    /// Parse voronoi brick parameters from a TOML `params` value.
    ///
    /// Missing fields fall back to defaults. Invalid values produce errors.
    pub fn from_toml(params: &toml::Value) -> Result<Self> {
        let table = params
            .as_table()
            .ok_or_else(|| ProcGenError::InvalidParameter {
                name: "params".into(),
                reason: "expected a TOML table".into(),
            })?;

        let mut p = Self::default();

        if let Some(v) = table.get("cell_count") {
            p.cell_count = toml_u32(v, "cell_count")?;
            if p.cell_count == 0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "cell_count".into(),
                    reason: "must be positive".into(),
                });
            }
        }
        if let Some(v) = table.get("mortar_width") {
            p.mortar_width = toml_f32(v, "mortar_width")?;
            if p.mortar_width < 0.0 || p.mortar_width > 1.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "mortar_width".into(),
                    reason: "must be in [0.0, 1.0]".into(),
                });
            }
        }
        if let Some(v) = table.get("height_variation") {
            p.height_variation = toml_f32(v, "height_variation")?;
            if p.height_variation < 0.0 || p.height_variation > 1.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "height_variation".into(),
                    reason: "must be in [0.0, 1.0]".into(),
                });
            }
        }
        if let Some(v) = table.get("regularity") {
            p.regularity = toml_f32(v, "regularity")?;
            if p.regularity < 0.0 || p.regularity > 1.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "regularity".into(),
                    reason: "must be in [0.0, 1.0]".into(),
                });
            }
        }

        Ok(p)
    }
}

// ---------------------------------------------------------------------------
// Pattern implementation
// ---------------------------------------------------------------------------

/// Voronoi-based stone/brick pattern generator.
///
/// Scatters seed points and computes per-pixel Voronoi cell membership, edge
/// distance, and height. Supports seamless tiling via toroidal distance.
pub struct VoronoiBrickPattern {
    pub params: VoronoiBrickParams,
}

impl VoronoiBrickPattern {
    pub fn new(params: VoronoiBrickParams) -> Self {
        Self { params }
    }
}

impl Pattern for VoronoiBrickPattern {
    fn generate(
        &self,
        width: u32,
        height: u32,
        seamless: bool,
        rng: &mut SeededRng,
    ) -> PatternField {
        let p = &self.params;
        let cell_count = p.cell_count.max(1) as usize;

        // --- Step 1: Generate seed points in [0, 1)² ---
        let mut points = generate_seed_points(rng, cell_count);

        // --- Step 2: Apply Lloyd's relaxation for regularity ---
        if p.regularity > 0.0 {
            let iterations = (p.regularity * 10.0).ceil() as usize;
            let relaxed = lloyds_relaxation(&points, iterations, seamless);
            // Blend between random and relaxed based on regularity
            for i in 0..points.len() {
                points[i].0 = lerp(points[i].0, relaxed[i].0, p.regularity);
                points[i].1 = lerp(points[i].1, relaxed[i].1, p.regularity);
            }
        }

        // --- Step 3: Build spatial acceleration grid ---
        let grid = SpatialGrid::new(&points, seamless);

        // --- Step 4: Per-cell heights via hash + noise ---
        let mut height_rng = rng.fork("cell_heights");
        let cell_base_heights: Vec<f32> = (0..cell_count)
            .map(|_| {
                let base = 0.5 + (height_rng.next_f32() - 0.5) * p.height_variation;
                base.clamp(0.0, 1.0)
            })
            .collect();

        // Surface variation noise (subtle per-cell bumps)
        let noise_rng = rng.fork("surface_noise");
        let surface_noise = Fbm::new(PerlinNoise::new(noise_rng.seed()))
            .with_octaves(4)
            .with_frequency(8.0)
            .with_persistence(0.4);

        // --- Step 5: Fill the pattern field ---
        let mut field = PatternField::new(width, height);

        for y in 0..height {
            for x in 0..width {
                let u = (x as f32 + 0.5) / width as f32;
                let v = (y as f32 + 0.5) / height as f32;

                // Find closest and second-closest seed points
                let (closest_id, dist1, dist2) = grid.find_two_nearest(u, v, &points, seamless);

                // Edge distance: difference between closest and second-closest
                let raw_edge_dist = dist2 - dist1;
                // Normalize: mortar_width defines the 0→1 transition zone
                let mortar_w = p.mortar_width.max(1e-6);
                let edge_normalized = (raw_edge_dist / mortar_w).min(1.0);

                // Height: base cell height + subtle surface noise
                let base_h = cell_base_heights[closest_id];
                let noise_val = surface_noise.sample_2d(u as f64 * 16.0, v as f64 * 16.0) as f32;
                let surface_h = base_h + noise_val * 0.05;

                // In mortar regions, lower the height
                let final_height = if edge_normalized < 1.0 {
                    // Smooth blend into mortar groove
                    let mortar_depth = 0.15;
                    let t = edge_normalized; // 0 = on edge, 1 = fully inside cell
                    let mortar_h = (surface_h - mortar_depth).max(0.0);
                    // Smooth step for nice transition
                    let s = t * t * (3.0 - 2.0 * t);
                    lerp(mortar_h, surface_h, s)
                } else {
                    surface_h
                };

                let cell = field.get_mut(x, y);
                cell.cell_id = closest_id as u32;
                cell.height = final_height.clamp(0.0, 1.0);
                cell.distance_to_edge = edge_normalized;
            }
        }

        field
    }
}

// ---------------------------------------------------------------------------
// Seed point generation
// ---------------------------------------------------------------------------

fn generate_seed_points(rng: &mut SeededRng, count: usize) -> Vec<(f32, f32)> {
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
fn lloyds_relaxation(points: &[(f32, f32)], iterations: usize, seamless: bool) -> Vec<(f32, f32)> {
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
struct SpatialGrid {
    /// Number of grid cells per axis.
    grid_size: usize,
    /// For each grid cell, a list of (point_index, wrapped_x, wrapped_y).
    cells: Vec<Vec<(usize, f32, f32)>>,
}

impl SpatialGrid {
    fn new(points: &[(f32, f32)], seamless: bool) -> Self {
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
    fn find_two_nearest(
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Squared distance between two points, with optional toroidal wrapping.
fn dist_sq(ax: f32, ay: f32, bx: f32, by: f32, seamless: bool) -> f32 {
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

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn default_pattern() -> VoronoiBrickPattern {
        VoronoiBrickPattern::new(VoronoiBrickParams::default())
    }

    #[test]
    fn voronoi_params_from_toml_defaults() {
        let toml_val: toml::Value = toml::toml! {
            cell_count = 12
        }
        .into();
        let p = VoronoiBrickParams::from_toml(&toml_val).unwrap();
        assert_eq!(p.cell_count, 12);
        assert!((p.mortar_width - 0.02).abs() < 1e-5);
        assert!((p.height_variation - 0.3).abs() < 1e-5);
        assert!((p.regularity - 0.0).abs() < 1e-5);
    }

    #[test]
    fn voronoi_params_from_toml_custom() {
        let toml_val: toml::Value = toml::toml! {
            cell_count = 24
            mortar_width = 0.05
            height_variation = 0.6
            regularity = 0.8
        }
        .into();
        let p = VoronoiBrickParams::from_toml(&toml_val).unwrap();
        assert_eq!(p.cell_count, 24);
        assert!((p.mortar_width - 0.05).abs() < 1e-5);
        assert!((p.height_variation - 0.6).abs() < 1e-5);
        assert!((p.regularity - 0.8).abs() < 1e-5);
    }

    #[test]
    fn voronoi_params_from_toml_invalid_cell_count() {
        let toml_val: toml::Value = toml::toml! {
            cell_count = 0
        }
        .into();
        assert!(VoronoiBrickParams::from_toml(&toml_val).is_err());
    }

    #[test]
    fn voronoi_params_from_toml_invalid_regularity() {
        let toml_val: toml::Value = toml::toml! {
            regularity = 1.5
        }
        .into();
        assert!(VoronoiBrickParams::from_toml(&toml_val).is_err());
    }

    #[test]
    fn voronoi_determinism() {
        let pattern = default_pattern();
        let mut rng1 = SeededRng::new(42);
        let mut rng2 = SeededRng::new(42);
        let f1 = pattern.generate(64, 64, false, &mut rng1);
        let f2 = pattern.generate(64, 64, false, &mut rng2);

        assert_eq!(f1.cells.len(), f2.cells.len());
        for (a, b) in f1.cells.iter().zip(f2.cells.iter()) {
            assert_eq!(a.cell_id, b.cell_id);
            assert_eq!(a.height, b.height);
            assert_eq!(a.distance_to_edge, b.distance_to_edge);
        }
    }

    #[test]
    fn voronoi_validity() {
        let pattern = default_pattern();
        let mut rng = SeededRng::new(99);
        let field = pattern.generate(64, 64, false, &mut rng);

        for cell in &field.cells {
            assert!(
                cell.height >= 0.0 && cell.height <= 1.0,
                "height out of range: {}",
                cell.height
            );
            assert!(
                cell.distance_to_edge >= 0.0 && cell.distance_to_edge <= 1.0,
                "distance_to_edge out of range: {}",
                cell.distance_to_edge
            );
        }
    }

    #[test]
    fn voronoi_cell_count() {
        let pattern = VoronoiBrickPattern::new(VoronoiBrickParams {
            cell_count: 20,
            ..Default::default()
        });
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(128, 128, false, &mut rng);

        let unique_ids: std::collections::HashSet<u32> =
            field.cells.iter().map(|c| c.cell_id).collect();
        assert!(
            unique_ids.len() >= 15 && unique_ids.len() <= 20,
            "expected ~20 unique cell IDs, got {}",
            unique_ids.len()
        );
    }

    #[test]
    fn voronoi_mortar_regions_exist() {
        let pattern = VoronoiBrickPattern::new(VoronoiBrickParams {
            cell_count: 12,
            mortar_width: 0.05, // generous mortar for test visibility
            ..Default::default()
        });
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(128, 128, false, &mut rng);

        let mortar_pixels = field
            .cells
            .iter()
            .filter(|c| c.distance_to_edge < 0.5)
            .count();
        assert!(
            mortar_pixels > 0,
            "no mortar-region pixels found (distance_to_edge < 0.5)"
        );
    }

    #[test]
    fn voronoi_seamless_continuity() {
        let pattern = default_pattern();
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(64, 64, true, &mut rng);

        // Compare top row vs bottom row
        let tolerance = 0.15; // Allow some tolerance due to pixel discretization
        let mut top_bottom_matches = 0;
        for x in 0..field.width {
            let top = field.get(x, 0);
            let bottom = field.get(x, field.height - 1);
            if top.cell_id == bottom.cell_id {
                top_bottom_matches += 1;
            }
            // Height should be close at boundaries
            assert!(
                (top.height - bottom.height).abs() < tolerance || top.cell_id != bottom.cell_id,
                "seamless height mismatch at x={}: top={}, bottom={}",
                x,
                top.height,
                bottom.height
            );
        }

        // Compare left column vs right column
        let mut left_right_matches = 0;
        for y in 0..field.height {
            let left = field.get(0, y);
            let right = field.get(field.width - 1, y);
            if left.cell_id == right.cell_id {
                left_right_matches += 1;
            }
        }

        // At least some boundary pixels should share cell IDs for seamless tiling
        assert!(
            top_bottom_matches > 0 || left_right_matches > 0,
            "no seamless continuity found at boundaries"
        );
    }

    #[test]
    fn voronoi_regularity() {
        let regular = VoronoiBrickPattern::new(VoronoiBrickParams {
            cell_count: 9,
            regularity: 1.0,
            ..Default::default()
        });
        let mut rng = SeededRng::new(42);
        let field = regular.generate(64, 64, false, &mut rng);

        // With high regularity, cells should be roughly equal-sized.
        let mut cell_sizes: std::collections::HashMap<u32, u32> = Default::default();
        for cell in &field.cells {
            *cell_sizes.entry(cell.cell_id).or_default() += 1;
        }
        let sizes: Vec<u32> = cell_sizes.values().copied().collect();
        let mean = sizes.iter().sum::<u32>() as f32 / sizes.len() as f32;
        let max_deviation = sizes
            .iter()
            .map(|&s| (s as f32 - mean).abs() / mean)
            .fold(0.0f32, f32::max);

        // With Lloyd's relaxation, max deviation from mean should be moderate
        assert!(
            max_deviation < 1.0,
            "high regularity should produce more uniform cells, max deviation: {}",
            max_deviation
        );
    }

    #[test]
    #[ignore]
    fn voronoi_visual_debug_pngs() {
        use std::path::PathBuf;

        let dir = PathBuf::from(std::env::temp_dir()).join("flint_pattern_debug");
        std::fs::create_dir_all(&dir).unwrap();

        let pattern = VoronoiBrickPattern::new(VoronoiBrickParams {
            cell_count: 16,
            mortar_width: 0.03,
            height_variation: 0.4,
            regularity: 0.3,
        });
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(256, 256, true, &mut rng);

        for (name, img) in [
            ("voronoi_height", field.to_height_image()),
            ("voronoi_cells", field.to_cell_id_image()),
            ("voronoi_edges", field.to_edge_distance_image()),
        ] {
            let path = dir.join(format!("{name}.png"));
            image::save_buffer(
                &path,
                &img.pixels,
                img.width,
                img.height,
                image::ColorType::Rgba8,
            )
            .unwrap();
            println!("wrote {}", path.display());
        }
        println!("\nVoronoi debug PNGs written to: {}", dir.display());
    }
}
