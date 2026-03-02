//! Regular grid/tiling pattern generator.
//!
//! Produces manufactured-looking surfaces like floor tiles, brick patterns, and
//! panels. Supports per-cell jitter for a less mechanical look, gap/mortar
//! regions, and per-tile height variation.

use crate::algorithms::noise::{Fbm, NoiseSource, PerlinNoise};
use crate::SeededRng;

use super::pattern::{Pattern, PatternField};

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

/// Parameters for the tiling grid pattern.
#[derive(Debug, Clone)]
pub struct TilingGridParams {
    /// Number of grid columns.
    pub columns: u32,
    /// Number of grid rows.
    pub rows: u32,
    /// Gap between tiles as a fraction of a cell size. 0.05 typical.
    pub gap_width: f32,
    /// Per-tile height randomness in \[0, 1\].
    pub height_variation: f32,
    /// Jitter: 0 = perfect grid, up to 0.5 = moderate randomness.
    pub jitter: f32,
}

impl Default for TilingGridParams {
    fn default() -> Self {
        Self {
            columns: 4,
            rows: 4,
            gap_width: 0.05,
            height_variation: 0.2,
            jitter: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Pattern implementation
// ---------------------------------------------------------------------------

/// Regular tiling grid pattern generator.
///
/// Divides the texture into a regular grid of columns x rows, with optional
/// per-cell jitter and gap regions. Each tile gets a unique cell ID and
/// randomized height.
pub struct TilingGridPattern {
    pub params: TilingGridParams,
}

impl TilingGridPattern {
    pub fn new(params: TilingGridParams) -> Self {
        Self { params }
    }
}

impl Pattern for TilingGridPattern {
    fn generate(
        &self,
        width: u32,
        height: u32,
        seamless: bool,
        rng: &mut SeededRng,
    ) -> PatternField {
        let p = &self.params;
        let cols = p.columns.max(1);
        let rows = p.rows.max(1);

        // --- Per-cell jitter offsets (deterministic hash of cell coords) ---
        let jitter_offsets = if p.jitter > 0.0 {
            let mut jitter_rng = rng.fork("grid_jitter");
            let mut offsets = Vec::with_capacity((cols * rows) as usize);
            for _ in 0..(cols * rows) {
                let jx = (jitter_rng.next_f32() - 0.5) * 2.0 * p.jitter;
                let jy = (jitter_rng.next_f32() - 0.5) * 2.0 * p.jitter;
                offsets.push((jx, jy));
            }
            Some(offsets)
        } else {
            None
        };

        // --- Per-cell base heights ---
        let mut height_rng = rng.fork("grid_heights");
        let cell_heights: Vec<f32> = (0..(cols * rows))
            .map(|_| {
                let h = 0.5 + (height_rng.next_f32() - 0.5) * p.height_variation;
                h.clamp(0.0, 1.0)
            })
            .collect();

        // --- Surface variation noise ---
        let noise_rng = rng.fork("grid_surface");
        let surface_noise = Fbm::new(PerlinNoise::new(noise_rng.seed()))
            .with_octaves(3)
            .with_frequency(12.0)
            .with_persistence(0.3);

        let cell_w = 1.0 / cols as f32;
        let cell_h = 1.0 / rows as f32;

        let mut field = PatternField::new(width, height);

        for y in 0..height {
            for x in 0..width {
                let u = (x as f32 + 0.5) / width as f32;
                let v = (y as f32 + 0.5) / height as f32;

                if p.jitter > 0.0 {
                    // With jitter, find the nearest cell center
                    let geom = GridGeom {
                        cols,
                        rows,
                        cell_w,
                        cell_h,
                        gap_width: p.gap_width,
                        jitter_offsets: jitter_offsets.as_deref().unwrap(),
                        seamless,
                    };
                    let (cell_id, dist_to_edge) = find_nearest_jittered_cell(u, v, &geom);

                    let base_h = cell_heights[cell_id as usize];
                    let noise_val =
                        surface_noise.sample_2d(u as f64 * 16.0, v as f64 * 16.0) as f32;
                    let surface_h = base_h + noise_val * 0.03;

                    let gap_w = p.gap_width.max(1e-6);
                    let edge_normalized = (dist_to_edge / gap_w).min(1.0);

                    // In gap regions, lower the height
                    let final_height = if edge_normalized < 1.0 {
                        let gap_depth = 0.12;
                        let t = edge_normalized;
                        let s = t * t * (3.0 - 2.0 * t); // smoothstep
                        let gap_h = (surface_h - gap_depth).max(0.0);
                        lerp(gap_h, surface_h, s)
                    } else {
                        surface_h
                    };

                    let cell = field.get_mut(x, y);
                    cell.cell_id = cell_id;
                    cell.height = final_height.clamp(0.0, 1.0);
                    cell.distance_to_edge = edge_normalized;
                } else {
                    // No jitter: simple grid lookup
                    let col = ((u / cell_w).floor() as u32).min(cols - 1);
                    let row = ((v / cell_h).floor() as u32).min(rows - 1);
                    let cell_id = if seamless {
                        (row % rows) * cols + (col % cols)
                    } else {
                        row * cols + col
                    };

                    // Position within the cell [0, 1]
                    let local_u = (u - col as f32 * cell_w) / cell_w;
                    let local_v = (v - row as f32 * cell_h) / cell_h;

                    // Distance to nearest cell edge
                    let dist_u = local_u.min(1.0 - local_u);
                    let dist_v = local_v.min(1.0 - local_v);
                    let raw_edge_dist = dist_u.min(dist_v) * cell_w.min(cell_h);

                    let gap_w = p.gap_width.max(1e-6) * cell_w.min(cell_h);
                    let edge_normalized = (raw_edge_dist / gap_w).min(1.0);

                    let base_h = cell_heights[cell_id as usize];
                    let noise_val =
                        surface_noise.sample_2d(u as f64 * 16.0, v as f64 * 16.0) as f32;
                    let surface_h = base_h + noise_val * 0.03;

                    // Gap groove
                    let final_height = if edge_normalized < 1.0 {
                        let gap_depth = 0.12;
                        let t = edge_normalized;
                        let s = t * t * (3.0 - 2.0 * t);
                        let gap_h = (surface_h - gap_depth).max(0.0);
                        lerp(gap_h, surface_h, s)
                    } else {
                        surface_h
                    };

                    let cell = field.get_mut(x, y);
                    cell.cell_id = cell_id;
                    cell.height = final_height.clamp(0.0, 1.0);
                    cell.distance_to_edge = edge_normalized;
                }
            }
        }

        field
    }
}

// ---------------------------------------------------------------------------
// Jittered cell lookup
// ---------------------------------------------------------------------------

/// Grid geometry passed to jittered cell lookup.
struct GridGeom<'a> {
    cols: u32,
    rows: u32,
    cell_w: f32,
    cell_h: f32,
    gap_width: f32,
    jitter_offsets: &'a [(f32, f32)],
    seamless: bool,
}

/// Find the nearest cell center for a jittered grid, returning (cell_id, distance_to_edge).
fn find_nearest_jittered_cell(u: f32, v: f32, g: &GridGeom<'_>) -> (u32, f32) {
    let cols = g.cols;
    let rows = g.rows;
    let cell_w = g.cell_w;
    let cell_h = g.cell_h;
    let gap_width = g.gap_width;
    let jitter_offsets = g.jitter_offsets;
    let seamless = g.seamless;
    // Approximate which grid cell this pixel is in
    let approx_col = (u / cell_w).floor() as i32;
    let approx_row = (v / cell_h).floor() as i32;

    let mut best_id = 0u32;
    let mut best_dist = f32::INFINITY;
    let mut second_dist = f32::INFINITY;

    // Search the 3x3 neighborhood
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            let nc = approx_col + dx;
            let nr = approx_row + dy;

            // Wrap for seamless tiling
            let wc = if seamless {
                nc.rem_euclid(cols as i32) as u32
            } else if nc < 0 || nc >= cols as i32 {
                continue;
            } else {
                nc as u32
            };

            let wr = if seamless {
                nr.rem_euclid(rows as i32) as u32
            } else if nr < 0 || nr >= rows as i32 {
                continue;
            } else {
                nr as u32
            };

            let cell_id = wr * cols + wc;
            let (jx, jy) = jitter_offsets[cell_id as usize];

            // Cell center with jitter (in UV space)
            let cx = (nc as f32 + 0.5) * cell_w + jx * cell_w;
            let cy = (nr as f32 + 0.5) * cell_h + jy * cell_h;

            let dx_uv = u - cx;
            let dy_uv = v - cy;
            let dist = (dx_uv * dx_uv + dy_uv * dy_uv).sqrt();

            if dist < best_dist {
                second_dist = best_dist;
                best_dist = dist;
                best_id = cell_id;
            } else if dist < second_dist {
                second_dist = dist;
            }
        }
    }

    // Edge distance from Voronoi-style nearest/second-nearest
    let raw_edge = second_dist - best_dist;
    let gap_w = gap_width.max(1e-6) * cell_w.min(cell_h);
    let edge_normalized = (raw_edge / gap_w).min(1.0);

    (best_id, edge_normalized)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn default_pattern() -> TilingGridPattern {
        TilingGridPattern::new(TilingGridParams::default())
    }

    #[test]
    fn grid_determinism() {
        let pattern = default_pattern();
        let mut rng1 = SeededRng::new(42);
        let mut rng2 = SeededRng::new(42);
        let f1 = pattern.generate(64, 64, false, &mut rng1);
        let f2 = pattern.generate(64, 64, false, &mut rng2);

        for (a, b) in f1.cells.iter().zip(f2.cells.iter()) {
            assert_eq!(a.cell_id, b.cell_id);
            assert_eq!(a.height, b.height);
            assert_eq!(a.distance_to_edge, b.distance_to_edge);
        }
    }

    #[test]
    fn grid_validity() {
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
    fn grid_cell_count() {
        let pattern = TilingGridPattern::new(TilingGridParams {
            columns: 3,
            rows: 3,
            ..Default::default()
        });
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(128, 128, false, &mut rng);

        let unique_ids: std::collections::HashSet<u32> =
            field.cells.iter().map(|c| c.cell_id).collect();
        assert_eq!(
            unique_ids.len(),
            9,
            "expected 9 unique cell IDs for 3x3 grid, got {}",
            unique_ids.len()
        );
    }

    #[test]
    fn grid_seamless_continuity() {
        let pattern = default_pattern();
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(64, 64, true, &mut rng);

        // For a 4x4 grid, seamless means the grid structure wraps correctly.
        // Column assignment should be consistent across top/bottom rows.
        for x in 0..field.width {
            let top = field.get(x, 0);
            let bottom = field.get(x, field.height - 1);
            // Same column: cell_id % cols should match
            assert_eq!(
                top.cell_id % 4,
                bottom.cell_id % 4,
                "seamless column mismatch at x={}",
                x
            );
        }

        // Left column and right column should be in the same row
        for y in 0..field.height {
            let left = field.get(0, y);
            let right = field.get(field.width - 1, y);
            // Same row: cell_id / cols should match
            assert_eq!(
                left.cell_id / 4,
                right.cell_id / 4,
                "seamless row mismatch at y={}",
                y
            );
        }

        // Edge distance structure should be symmetric at boundaries:
        // pixels near the top edge and bottom edge of the texture should
        // both be near a cell edge (low distance_to_edge), forming the
        // gap/mortar that makes the tile join seamless.
        let top_avg_edge: f32 = (0..field.width)
            .map(|x| field.get(x, 0).distance_to_edge)
            .sum::<f32>()
            / field.width as f32;
        let bottom_avg_edge: f32 = (0..field.width)
            .map(|x| field.get(x, field.height - 1).distance_to_edge)
            .sum::<f32>()
            / field.width as f32;
        // Both boundary rows should have similar edge-distance profiles
        assert!(
            (top_avg_edge - bottom_avg_edge).abs() < 0.3,
            "boundary edge distance asymmetry: top_avg={:.3} bottom_avg={:.3}",
            top_avg_edge,
            bottom_avg_edge
        );
    }

    #[test]
    fn grid_gap_regions_exist() {
        let pattern = TilingGridPattern::new(TilingGridParams {
            columns: 4,
            rows: 4,
            gap_width: 0.1, // generous for test visibility
            ..Default::default()
        });
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(128, 128, false, &mut rng);

        let gap_pixels = field
            .cells
            .iter()
            .filter(|c| c.distance_to_edge < 0.5)
            .count();
        assert!(
            gap_pixels > 0,
            "no gap-region pixels found (distance_to_edge < 0.5)"
        );
    }

    #[test]
    fn grid_with_jitter() {
        let pattern = TilingGridPattern::new(TilingGridParams {
            columns: 4,
            rows: 4,
            jitter: 0.3,
            ..Default::default()
        });
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(64, 64, false, &mut rng);

        // Should still produce valid output
        for cell in &field.cells {
            assert!(cell.height >= 0.0 && cell.height <= 1.0);
            assert!(cell.distance_to_edge >= 0.0 && cell.distance_to_edge <= 1.0);
        }

        let unique_ids: std::collections::HashSet<u32> =
            field.cells.iter().map(|c| c.cell_id).collect();
        // With jitter, most cells should still be represented
        assert!(
            unique_ids.len() >= 8,
            "jittered grid should have most cells present, got {}",
            unique_ids.len()
        );
    }

    #[test]
    fn grid_jitter_determinism() {
        let pattern = TilingGridPattern::new(TilingGridParams {
            jitter: 0.4,
            ..Default::default()
        });
        let mut rng1 = SeededRng::new(42);
        let mut rng2 = SeededRng::new(42);
        let f1 = pattern.generate(32, 32, false, &mut rng1);
        let f2 = pattern.generate(32, 32, false, &mut rng2);

        for (a, b) in f1.cells.iter().zip(f2.cells.iter()) {
            assert_eq!(a.cell_id, b.cell_id);
            assert_eq!(a.height, b.height);
        }
    }

    #[test]
    #[ignore]
    fn grid_visual_debug_pngs() {
        use std::path::PathBuf;

        let dir = PathBuf::from(std::env::temp_dir()).join("flint_pattern_debug");
        std::fs::create_dir_all(&dir).unwrap();

        let pattern = TilingGridPattern::new(TilingGridParams {
            columns: 5,
            rows: 4,
            gap_width: 0.06,
            height_variation: 0.3,
            jitter: 0.15,
        });
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(256, 256, true, &mut rng);

        for (name, img) in [
            ("grid_height", field.to_height_image()),
            ("grid_cells", field.to_cell_id_image()),
            ("grid_edges", field.to_edge_distance_image()),
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
        println!("\nGrid debug PNGs written to: {}", dir.display());
    }
}
