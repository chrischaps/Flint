//! Regular grid/tiling pattern generator.
//!
//! Produces manufactured-looking surfaces like floor tiles, brick patterns, and
//! panels. Supports per-cell jitter for a less mechanical look, gap/mortar
//! regions, and per-tile height variation.

use crate::algorithms::noise::{Fbm, NoiseSource, PerlinNoise};
use crate::generators::util::{toml_f32, toml_u32};
use crate::{ProcGenError, Result, SeededRng};

use super::pattern::{Pattern, PatternField};
use super::voronoi_util::SpatialGrid;

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
    /// Amplitude of the fine surface noise on each tile's height (0.03 default).
    pub noise_strength: f32,
}

impl Default for TilingGridParams {
    fn default() -> Self {
        Self {
            columns: 4,
            rows: 4,
            gap_width: 0.05,
            height_variation: 0.2,
            jitter: 0.0,
            noise_strength: 0.03,
        }
    }
}

impl TilingGridParams {
    /// Parse tiling grid parameters from a TOML `params` value.
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

        if let Some(v) = table.get("columns") {
            p.columns = toml_u32(v, "columns")?;
            if p.columns == 0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "columns".into(),
                    reason: "must be positive".into(),
                });
            }
        }
        if let Some(v) = table.get("rows") {
            p.rows = toml_u32(v, "rows")?;
            if p.rows == 0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "rows".into(),
                    reason: "must be positive".into(),
                });
            }
        }
        if let Some(v) = table.get("gap_width") {
            p.gap_width = toml_f32(v, "gap_width")?;
            if p.gap_width < 0.0 || p.gap_width > 1.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "gap_width".into(),
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
        if let Some(v) = table.get("jitter") {
            p.jitter = toml_f32(v, "jitter")?;
            if p.jitter < 0.0 || p.jitter > 0.5 {
                return Err(ProcGenError::InvalidParameter {
                    name: "jitter".into(),
                    reason: "must be in [0.0, 0.5]".into(),
                });
            }
        }

        if let Some(v) = table.get("noise_strength") {
            p.noise_strength = toml_f32(v, "noise_strength")?;
            if !(0.0..=1.0).contains(&p.noise_strength) {
                return Err(ProcGenError::InvalidParameter {
                    name: "noise_strength".into(),
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
        let cell_w = 1.0 / cols as f32;
        let cell_h = 1.0 / rows as f32;

        // --- Jittered tile centres: one Voronoi site per tile, found exactly
        // by the shared grid search. Offsets stay within the tile (|j| ≤ 0.5),
        // so the site's index is still its tile's row * cols + col. ---
        let jitter_grid = if p.jitter > 0.0 {
            let mut jitter_rng = rng.fork("grid_jitter");
            let mut centres = Vec::with_capacity((cols * rows) as usize);
            for r in 0..rows {
                for c in 0..cols {
                    let jx = (jitter_rng.next_f32() - 0.5) * 2.0 * p.jitter;
                    let jy = (jitter_rng.next_f32() - 0.5) * 2.0 * p.jitter;
                    let x = (c as f32 + 0.5 + jx) * cell_w;
                    let y = (r as f32 + 0.5 + jy) * cell_h;
                    centres.push((x.rem_euclid(1.0), y.rem_euclid(1.0)));
                }
            }
            Some(SpatialGrid::new(&centres, seamless))
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

        // --- Surface variation noise: 192 lattice cells per tile at the base
        // octave, periodic when the texture must tile. ---
        let noise_rng = rng.fork("grid_surface");
        let perlin = PerlinNoise::new(noise_rng.seed());
        let surface_noise = Fbm::new(perlin.clone())
            .with_octaves(3)
            .with_frequency(12.0)
            .with_persistence(0.3);
        let surface = |u: f32, v: f32| -> f32 {
            if seamless {
                perlin.fbm_2d_periodic(u as f64, v as f64, 192.0, 3, 2.0, 0.3) as f32
            } else {
                surface_noise.sample_2d(u as f64 * 16.0, v as f64 * 16.0) as f32
            }
        };

        // Gap half-width in UV: gap_width of the smaller cell side on each side
        // of every edge.
        let gap_w = p.gap_width.max(1e-6) * cell_w.min(cell_h);
        let mut field = PatternField::new(width, height);

        for y in 0..height {
            for x in 0..width {
                let u = (x as f32 + 0.5) / width as f32;
                let v = (y as f32 + 0.5) / height as f32;

                let (cell_id, raw_edge_dist) = match &jitter_grid {
                    Some(grid) => {
                        let near = grid.nearest(u, v);
                        (near.id as u32, near.edge)
                    }
                    None => {
                        let col = ((u / cell_w).floor() as u32).min(cols - 1);
                        let row = ((v / cell_h).floor() as u32).min(rows - 1);
                        // Distance to the tile's nearest side, in UV.
                        let lu = u - col as f32 * cell_w;
                        let lv = v - row as f32 * cell_h;
                        let dist_u = lu.min(cell_w - lu);
                        let dist_v = lv.min(cell_h - lv);
                        (row * cols + col, dist_u.min(dist_v))
                    }
                };
                let edge_normalized = (raw_edge_dist / gap_w).min(1.0);

                let base_h = cell_heights[cell_id as usize];
                let surface_h = base_h + surface(u, v) * p.noise_strength;

                // Gap groove
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
            }
        }

        field
    }
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
    fn grid_params_from_toml_defaults() {
        let toml_val: toml::Value = toml::toml! {
            columns = 4
        }
        .into();
        let p = TilingGridParams::from_toml(&toml_val).unwrap();
        assert_eq!(p.columns, 4);
        assert_eq!(p.rows, 4);
        assert!((p.gap_width - 0.05).abs() < 1e-5);
        assert!((p.height_variation - 0.2).abs() < 1e-5);
        assert!((p.jitter - 0.0).abs() < 1e-5);
    }

    #[test]
    fn grid_params_from_toml_custom() {
        let toml_val: toml::Value = toml::toml! {
            columns = 6
            rows = 3
            gap_width = 0.08
            height_variation = 0.5
            jitter = 0.3
        }
        .into();
        let p = TilingGridParams::from_toml(&toml_val).unwrap();
        assert_eq!(p.columns, 6);
        assert_eq!(p.rows, 3);
        assert!((p.gap_width - 0.08).abs() < 1e-5);
        assert!((p.height_variation - 0.5).abs() < 1e-5);
        assert!((p.jitter - 0.3).abs() < 1e-5);
    }

    #[test]
    fn grid_params_from_toml_invalid_columns() {
        let toml_val: toml::Value = toml::toml! {
            columns = 0
        }
        .into();
        assert!(TilingGridParams::from_toml(&toml_val).is_err());
    }

    #[test]
    fn grid_params_from_toml_invalid_jitter() {
        let toml_val: toml::Value = toml::toml! {
            jitter = 0.8
        }
        .into();
        assert!(TilingGridParams::from_toml(&toml_val).is_err());
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
    fn grid_seamless_has_no_seam() {
        for jitter in [0.0f32, 0.3] {
            let pattern = TilingGridPattern::new(TilingGridParams {
                jitter,
                ..Default::default()
            });
            let field = pattern.generate(128, 128, true, &mut SeededRng::new(42));
            let s = field.seam_steps();
            assert!(s.ratio() < 1.6, "jitter {jitter}: {s:?}");
        }
    }

    #[test]
    fn grid_tiles_continue_across_the_wrap() {
        // With jitter a tile can spill over the texture's edge; it must come
        // back on the far side as the same tile.
        let pattern = TilingGridPattern::new(TilingGridParams {
            jitter: 0.4,
            ..Default::default()
        });
        let field = pattern.generate(128, 128, true, &mut SeededRng::new(7));
        let (w, h) = (field.width, field.height);
        for y in 0..h {
            let (a, b) = (field.get(w - 1, y), field.get(0, y));
            if a.distance_to_edge >= 1.0 && b.distance_to_edge >= 1.0 {
                assert_eq!(a.cell_id, b.cell_id, "row {y}");
            }
        }
        for x in 0..w {
            let (a, b) = (field.get(x, h - 1), field.get(x, 0));
            if a.distance_to_edge >= 1.0 && b.distance_to_edge >= 1.0 {
                assert_eq!(a.cell_id, b.cell_id, "column {x}");
            }
        }
    }

    #[test]
    fn grid_gap_is_as_wide_with_jitter_as_without() {
        // A jittered grid used d2 - d1 (about twice the edge distance), so
        // its grooves were half as wide as a plain grid's at the same
        // gap_width. Both now measure the true distance to the edge.
        let gap_fraction = |jitter: f32| {
            let pattern = TilingGridPattern::new(TilingGridParams {
                jitter,
                gap_width: 0.08,
                ..Default::default()
            });
            let f = pattern.generate(256, 256, true, &mut SeededRng::new(42));
            f.cells.iter().filter(|c| c.distance_to_edge < 1.0).count() as f32
                / f.cells.len() as f32
        };
        let (plain, nudged) = (gap_fraction(0.0), gap_fraction(0.001));
        assert!(
            (plain - nudged).abs() < 0.02,
            "plain {plain} vs barely jittered {nudged}"
        );
    }

    #[test]
    fn grid_gap_is_even_on_wide_tiles() {
        // 2 x 8 tiles are four times wider than tall; the vertical and
        // horizontal grooves should both be gap_width of the short side.
        let pattern = TilingGridPattern::new(TilingGridParams {
            columns: 2,
            rows: 8,
            gap_width: 0.1,
            ..Default::default()
        });
        let f = pattern.generate(256, 256, true, &mut SeededRng::new(1));
        let row: usize = (0..256)
            .filter(|&x| f.get(x, 48).distance_to_edge < 1.0)
            .count();
        let col: usize = (0..256)
            .filter(|&y| f.get(64, y).distance_to_edge < 1.0)
            .count();
        // Short side = 32 px, gap 3.2 px each side of an edge.
        assert_eq!(
            row / 2,
            col / 8,
            "vertical groove {row} px over 2 edges, horizontal {col} over 8"
        );
    }

    #[test]
    fn grid_noise_strength_from_toml() {
        let v: toml::Value = toml::toml! { noise_strength = 0.0 }.into();
        assert_eq!(TilingGridParams::from_toml(&v).unwrap().noise_strength, 0.0);
        let bad: toml::Value = toml::toml! { noise_strength = 2.0 }.into();
        assert!(TilingGridParams::from_toml(&bad).is_err());
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
        use crate::generators::texture::pattern::write_debug_png;
        let pattern = TilingGridPattern::new(TilingGridParams {
            columns: 5,
            rows: 4,
            gap_width: 0.06,
            height_variation: 0.3,
            jitter: 0.15,
            ..Default::default()
        });
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(256, 256, true, &mut rng);
        write_debug_png("grid_height", &field.to_height_image());
        write_debug_png("grid_cells", &field.to_cell_id_image());
        write_debug_png("grid_edges", &field.to_edge_distance_image());
    }
}
