//! Noise-based organic pattern generator.
//!
//! Produces natural-looking surfaces like layered rock, dirt, bark, and organic
//! materials using FBM noise with configurable octaves and frequency. Height
//! values are quantized into bands for cell IDs, simulating geological strata
//! or natural layering.

use std::f64::consts::PI;

use crate::algorithms::noise::{Fbm, NoiseSource, PerlinNoise, SimplexNoise};
use crate::SeededRng;

use super::pattern::{Pattern, PatternField};

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

/// Parameters for the noise-based organic pattern.
#[derive(Debug, Clone)]
pub struct PerlinOrganicParams {
    /// Noise type: `"perlin"` or `"simplex"`.
    pub noise_type: String,
    /// Number of FBM octaves (default 6).
    pub octaves: u32,
    /// Base frequency (default 4.0).
    pub frequency: f64,
    /// Frequency multiplier per octave (default 2.0).
    pub lacunarity: f64,
    /// Amplitude decay per octave (default 0.5).
    pub persistence: f64,
    /// Number of discrete height bands for cell_id assignment (default 8).
    pub height_bands: u32,
}

impl Default for PerlinOrganicParams {
    fn default() -> Self {
        Self {
            noise_type: "perlin".into(),
            octaves: 6,
            frequency: 4.0,
            lacunarity: 2.0,
            persistence: 0.5,
            height_bands: 8,
        }
    }
}

// ---------------------------------------------------------------------------
// Pattern implementation
// ---------------------------------------------------------------------------

/// Noise-based organic pattern generator.
///
/// Uses FBM noise to produce natural terrain-like height fields. Heights are
/// quantized into bands to generate cell IDs for material layering.
pub struct PerlinOrganicPattern {
    pub params: PerlinOrganicParams,
}

impl PerlinOrganicPattern {
    pub fn new(params: PerlinOrganicParams) -> Self {
        Self { params }
    }
}

impl Pattern for PerlinOrganicPattern {
    fn generate(
        &self,
        width: u32,
        height: u32,
        seamless: bool,
        rng: &mut SeededRng,
    ) -> PatternField {
        let p = &self.params;
        let bands = p.height_bands.max(1);

        // Build the noise source
        let noise_seed = rng.fork("organic_noise").seed();
        let noise: Box<dyn NoiseSource> = match p.noise_type.as_str() {
            "simplex" => Box::new(
                Fbm::new(SimplexNoise::new(noise_seed))
                    .with_octaves(p.octaves)
                    .with_frequency(p.frequency)
                    .with_lacunarity(p.lacunarity)
                    .with_persistence(p.persistence),
            ),
            _ => Box::new(
                Fbm::new(PerlinNoise::new(noise_seed))
                    .with_octaves(p.octaves)
                    .with_frequency(p.frequency)
                    .with_lacunarity(p.lacunarity)
                    .with_persistence(p.persistence),
            ),
        };

        let mut field = PatternField::new(width, height);

        for y in 0..height {
            for x in 0..width {
                let u = x as f64 / width as f64;
                let v = y as f64 / height as f64;

                let raw = if seamless {
                    // 3D projection for seamless tiling:
                    // Map the 2D torus into 3D space using cylindrical coordinates.
                    // This gives near-seamless results with our 3D noise.
                    let u_angle = u * 2.0 * PI;
                    let v_angle = v * 2.0 * PI;
                    let nx = u_angle.cos();
                    let ny = u_angle.sin();
                    let nz = v_angle.cos();
                    // We use sample_3d which gives seamless-ish results.
                    // The 4th dimension (v_angle.sin()) is implicit in the 3D projection.
                    noise.sample_3d(nx, ny, nz)
                } else {
                    noise.sample_2d(u, v)
                };

                // Map noise [-1, 1] → [0, 1]
                let h = (raw * 0.5 + 0.5).clamp(0.0, 1.0) as f32;

                // Quantize into bands for cell_id
                let band_f = h * bands as f32;
                let cell_id = (band_f.floor() as u32).min(bands - 1);

                // Distance to nearest band threshold
                let band_frac = band_f.fract();
                // Distance to nearest edge: either the top or bottom of this band
                let dist_to_lower = band_frac;
                let dist_to_upper = 1.0 - band_frac;
                let distance_to_edge = dist_to_lower.min(dist_to_upper);
                // Scale so that 0.5 band-widths → 1.0 (center of band = max distance)
                let edge_normalized = (distance_to_edge * 2.0).min(1.0);

                let cell = field.get_mut(x, y);
                cell.cell_id = cell_id;
                cell.height = h;
                cell.distance_to_edge = edge_normalized;
            }
        }

        field
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn default_pattern() -> PerlinOrganicPattern {
        PerlinOrganicPattern::new(PerlinOrganicParams::default())
    }

    #[test]
    fn organic_determinism() {
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
    fn organic_validity() {
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
            assert!(
                cell.cell_id < 8,
                "cell_id {} exceeds band count",
                cell.cell_id
            );
        }
    }

    #[test]
    fn organic_band_count() {
        let pattern = PerlinOrganicPattern::new(PerlinOrganicParams {
            height_bands: 4,
            ..Default::default()
        });
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(128, 128, false, &mut rng);

        let unique_ids: std::collections::HashSet<u32> =
            field.cells.iter().map(|c| c.cell_id).collect();
        // With 4 bands and enough pixels, we should see most bands
        assert!(
            unique_ids.len() >= 2 && unique_ids.len() <= 4,
            "expected 2-4 unique bands, got {}",
            unique_ids.len()
        );
    }

    #[test]
    fn organic_seamless_continuity() {
        let pattern = default_pattern();
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(64, 64, true, &mut rng);

        // With 3D projection seamless tiling, top/bottom and left/right should match
        let tolerance = 0.1;
        let mut height_matches = 0;
        let total = field.width;

        for x in 0..field.width {
            let top = field.get(x, 0);
            let bottom = field.get(x, field.height - 1);
            if (top.height - bottom.height).abs() < tolerance {
                height_matches += 1;
            }
        }

        // At least 50% of boundary pixels should be close for a good seamless result
        assert!(
            height_matches > total / 2,
            "seamless: only {}/{} top-bottom pixel heights match within tolerance",
            height_matches,
            total
        );
    }

    #[test]
    fn organic_simplex_variant() {
        let pattern = PerlinOrganicPattern::new(PerlinOrganicParams {
            noise_type: "simplex".into(),
            ..Default::default()
        });
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(32, 32, false, &mut rng);

        // Should produce valid output
        for cell in &field.cells {
            assert!(cell.height >= 0.0 && cell.height <= 1.0);
        }

        // Should differ from perlin
        let perlin_pattern = default_pattern();
        let mut rng2 = SeededRng::new(42);
        let perlin_field = perlin_pattern.generate(32, 32, false, &mut rng2);

        let differs = field
            .cells
            .iter()
            .zip(perlin_field.cells.iter())
            .any(|(a, b)| a.height != b.height);
        assert!(
            differs,
            "simplex and perlin should produce different output"
        );
    }

    #[test]
    fn organic_height_has_variation() {
        let pattern = default_pattern();
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(64, 64, false, &mut rng);

        let first = field.cells[0].height;
        let has_variation = field.cells.iter().any(|c| c.height != first);
        assert!(has_variation, "organic pattern should not be uniform");
    }

    #[test]
    #[ignore]
    fn organic_visual_debug_pngs() {
        use std::path::PathBuf;

        let dir = PathBuf::from(std::env::temp_dir()).join("flint_pattern_debug");
        std::fs::create_dir_all(&dir).unwrap();

        let pattern = default_pattern();
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(256, 256, true, &mut rng);

        for (name, img) in [
            ("organic_height", field.to_height_image()),
            ("organic_cells", field.to_cell_id_image()),
            ("organic_edges", field.to_edge_distance_image()),
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
        println!("\nOrganic debug PNGs written to: {}", dir.display());
    }
}
