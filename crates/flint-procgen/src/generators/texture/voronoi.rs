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
use super::voronoi_util::{self, SpatialGrid};

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

/// Parameters for the Voronoi brick/stone pattern.
#[derive(Debug, Clone)]
pub struct VoronoiBrickParams {
    /// Number of Voronoi cells (seed points). ~12 for a stone wall.
    pub cell_count: u32,
    /// Typical cell size in pixels. When set it overrides `cell_count`:
    /// a `w × h` texture gets `round(w·h / cell_size²)` cells, so
    /// `cell_size = 32` on 256 × 256 gives 64.
    pub cell_size: Option<f32>,
    /// Full width of the mortar groove as a fraction of the texture width,
    /// measured across the edge (half on each side). 0.02 typical.
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
            cell_size: None,
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
        if let Some(v) = table.get("irregularity") {
            if table.contains_key("regularity") {
                return Err(ProcGenError::InvalidParameter {
                    name: "irregularity".into(),
                    reason: "give regularity or irregularity, not both".into(),
                });
            }
            let irr = toml_f32(v, "irregularity")?;
            if !(0.0..=1.0).contains(&irr) {
                return Err(ProcGenError::InvalidParameter {
                    name: "irregularity".into(),
                    reason: "must be in [0.0, 1.0]".into(),
                });
            }
            p.regularity = 1.0 - irr;
        }
        if let Some(v) = table.get("cell_size") {
            let size = toml_f32(v, "cell_size")?;
            if size.is_nan() || size <= 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "cell_size".into(),
                    reason: "must be positive (pixels)".into(),
                });
            }
            p.cell_size = Some(size);
        }

        Ok(p)
    }

    /// Cells to scatter on a `width × height` texture: from `cell_size` when
    /// it is set, otherwise `cell_count`.
    pub fn cell_count_for(&self, width: u32, height: u32) -> usize {
        match self.cell_size {
            Some(size) => ((width as f32 * height as f32) / (size * size))
                .round()
                .max(1.0) as usize,
            None => self.cell_count.max(1) as usize,
        }
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
        let cell_count = p.cell_count_for(width, height);

        // --- Step 1: Generate seed points in [0, 1)² ---
        let mut points = voronoi_util::generate_seed_points(rng, cell_count);

        // --- Step 2: Apply Lloyd's relaxation for regularity ---
        if p.regularity > 0.0 {
            let iterations = (p.regularity * 10.0).ceil() as usize;
            let relaxed = voronoi_util::lloyds_relaxation(&points, iterations, seamless);
            voronoi_util::blend_points(&mut points, &relaxed, p.regularity, seamless);
        }

        self.fill(&points, width, height, seamless, rng)
    }
}

impl VoronoiBrickPattern {
    /// Steps 3-5 of [`Pattern::generate`] for seed points already placed in
    /// `[0, 1)²`: one stone per point.
    pub(crate) fn fill(
        &self,
        points: &[(f32, f32)],
        width: u32,
        height: u32,
        seamless: bool,
        rng: &mut SeededRng,
    ) -> PatternField {
        let p = &self.params;

        // --- Step 3: Build spatial acceleration grid ---
        let grid = SpatialGrid::new(points, seamless);

        // --- Step 4: Per-cell heights via hash + noise ---
        let mut height_rng = rng.fork("cell_heights");
        let cell_base_heights: Vec<f32> = (0..points.len())
            .map(|_| {
                let base = 0.5 + (height_rng.next_f32() - 0.5) * p.height_variation;
                base.clamp(0.0, 1.0)
            })
            .collect();

        // Surface variation noise (subtle per-cell bumps): 128 lattice cells
        // per tile at the base octave, periodic when the tile must repeat.
        let noise_rng = rng.fork("surface_noise");
        let perlin = PerlinNoise::new(noise_rng.seed());
        let surface_noise = Fbm::new(perlin.clone())
            .with_octaves(4)
            .with_frequency(8.0)
            .with_persistence(0.4);
        let surface = |u: f32, v: f32| -> f32 {
            if seamless {
                perlin.fbm_2d_periodic(u as f64, v as f64, 128.0, 4, 2.0, 0.4) as f32
            } else {
                surface_noise.sample_2d(u as f64 * 16.0, v as f64 * 16.0) as f32
            }
        };

        // --- Step 5: Fill the pattern field ---
        let mut field = PatternField::new(width, height);
        // The groove is mortar_width across, so half of it on each side.
        let half_mortar = (p.mortar_width * 0.5).max(1e-6);

        for y in 0..height {
            for x in 0..width {
                let u = (x as f32 + 0.5) / width as f32;
                let v = (y as f32 + 0.5) / height as f32;

                let near = grid.nearest(u, v);
                let closest_id = near.id;
                // 0 on the cell's edge, 1 at half a groove in and beyond.
                let edge_normalized = (near.edge / half_mortar).min(1.0);

                // Height: base cell height + subtle surface noise
                let base_h = cell_base_heights[closest_id];
                let surface_h = base_h + surface(u, v) * 0.05;

                // In mortar regions, lower the height
                let final_height = if edge_normalized < 1.0 {
                    // Smooth blend into mortar groove
                    let mortar_depth = 0.15;
                    let t = edge_normalized; // 0 = on edge, 1 = fully inside cell
                    let mortar_h = (surface_h - mortar_depth).max(0.0);
                    // Smooth step for nice transition
                    let s = t * t * (3.0 - 2.0 * t);
                    voronoi_util::lerp(mortar_h, surface_h, s)
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

// Shared helpers (SpatialGrid, lloyds_relaxation, dist_sq, generate_seed_points, lerp)
// are in super::voronoi_util.

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators::texture::pattern::PatternCell;

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
    fn voronoi_seamless_has_no_seam() {
        // Once the tile repeats, the last column and the first are neighbours:
        // the step across the wrap must look like any interior step.
        for seed in [1u64, 42, 7] {
            let mut rng = SeededRng::new(seed);
            let field = default_pattern().generate(128, 128, true, &mut rng);
            let s = field.seam_steps();
            assert!(s.ratio() < 1.6, "seed {seed}: seam {s:?}");
        }
    }

    #[test]
    fn voronoi_seamless_cells_continue_across_the_wrap() {
        // Away from mortar, a pixel and its wrap-around neighbour are in the
        // same stone.
        let mut rng = SeededRng::new(42);
        let field = default_pattern().generate(128, 128, true, &mut rng);
        let (w, h) = (field.width, field.height);
        let mut checked = 0;
        for y in 0..h {
            let (a, b) = (field.get(w - 1, y), field.get(0, y));
            if a.distance_to_edge >= 1.0 && b.distance_to_edge >= 1.0 {
                assert_eq!(a.cell_id, b.cell_id, "row {y}");
                checked += 1;
            }
        }
        for x in 0..w {
            let (a, b) = (field.get(x, h - 1), field.get(x, 0));
            if a.distance_to_edge >= 1.0 && b.distance_to_edge >= 1.0 {
                assert_eq!(a.cell_id, b.cell_id, "column {x}");
                checked += 1;
            }
        }
        assert!(checked > 128, "only {checked} interior pairs on the seams");
    }

    #[test]
    fn voronoi_border_is_not_all_mortar() {
        // The old grid search counted border buckets twice, so d2 == d1 and
        // the whole outer ring of the tile came out as mortar.
        let mut rng = SeededRng::new(42);
        let field = VoronoiBrickPattern::new(VoronoiBrickParams {
            cell_count: 18,
            mortar_width: 0.04,
            ..Default::default()
        })
        .generate(256, 256, true, &mut rng);
        let mortar = |c: &PatternCell| c.distance_to_edge < 1.0;
        let overall =
            field.cells.iter().filter(|c| mortar(c)).count() as f32 / field.cells.len() as f32;
        let ring: Vec<&PatternCell> = (0..256u32)
            .flat_map(|y| (0..256u32).map(move |x| (x, y)))
            .filter(|&(x, y)| x < 26 || y < 26 || x >= 230 || y >= 230)
            .map(|(x, y)| field.get(x, y))
            .collect();
        let in_ring = ring.iter().filter(|c| mortar(c)).count() as f32 / ring.len() as f32;
        assert!(overall < 0.35, "mortar covers {overall}");
        assert!(
            in_ring < overall * 1.5,
            "border ring {in_ring} vs overall {overall}"
        );
    }

    #[test]
    fn voronoi_cell_size_sets_the_count() {
        // cell_size = 32 on 256 x 256: 64 cells, all of them visible.
        let params = VoronoiBrickParams {
            cell_size: Some(32.0),
            ..Default::default()
        };
        assert_eq!(params.cell_count_for(256, 256), 64);
        for seed in 0..4u64 {
            let mut rng = SeededRng::new(seed);
            let field = VoronoiBrickPattern::new(params.clone()).generate(256, 256, true, &mut rng);
            let ids: std::collections::HashSet<u32> =
                field.cells.iter().map(|c| c.cell_id).collect();
            assert!(
                (32..=96).contains(&ids.len()),
                "seed {seed}: {} cells",
                ids.len()
            );
            assert!(ids.iter().all(|&id| id < 64));
        }
    }

    #[test]
    fn voronoi_cell_size_and_irregularity_from_toml() {
        let v: toml::Value = toml::toml! { cell_size = 32 irregularity = 0.25 }.into();
        let p = VoronoiBrickParams::from_toml(&v).unwrap();
        assert_eq!(p.cell_size, Some(32.0));
        assert!((p.regularity - 0.75).abs() < 1e-6);
        let both: toml::Value = toml::toml! { regularity = 0.5 irregularity = 0.5 }.into();
        assert!(VoronoiBrickParams::from_toml(&both).is_err());
        let zero: toml::Value = toml::toml! { cell_size = 0 }.into();
        assert!(VoronoiBrickParams::from_toml(&zero).is_err());
    }

    #[test]
    fn voronoi_mortar_width_matches_spec() {
        // Two stones side by side: the edge is the vertical line u = 0.5, and
        // the groove should be mortar_width wide, centred on it.
        let pattern = VoronoiBrickPattern::new(VoronoiBrickParams {
            mortar_width: 0.0625, // 16 px of 256
            height_variation: 0.0,
            ..Default::default()
        });
        let mut rng = SeededRng::new(3);
        let field = pattern.fill(&[(0.25, 0.5), (0.75, 0.5)], 256, 64, false, &mut rng);
        let groove: Vec<u32> = (0..256)
            .filter(|&x| field.get(x, 32).distance_to_edge < 1.0)
            .collect();
        assert_eq!(groove.len(), 16, "groove {groove:?}");
        assert_eq!((groove[0], groove[15]), (120, 135));
        // The deepest point is on the edge itself.
        assert!(field.get(127, 32).distance_to_edge < 0.07);
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
        use crate::generators::texture::pattern::write_debug_png;
        let pattern = VoronoiBrickPattern::new(VoronoiBrickParams {
            cell_count: 16,
            mortar_width: 0.03,
            height_variation: 0.4,
            regularity: 0.3,
            ..Default::default()
        });
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(256, 256, true, &mut rng);
        write_debug_png("voronoi_height", &field.to_height_image());
        write_debug_png("voronoi_cells", &field.to_cell_id_image());
        write_debug_png("voronoi_edges", &field.to_edge_distance_image());
    }
}
