//! Noise-based organic pattern generator.
//!
//! Produces natural-looking surfaces like layered rock, dirt, bark, and organic
//! materials using FBM noise with configurable octaves and frequency. Height
//! values are quantized into bands for cell IDs, simulating geological strata
//! or natural layering.

use crate::algorithms::noise::{Fbm, NoiseSource, PerlinNoise, SimplexNoise};
use crate::generators::util::{toml_f64, toml_string, toml_u32};
use crate::{ProcGenError, Result, SeededRng};

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

impl PerlinOrganicParams {
    /// Parse organic pattern parameters from a TOML `params` value.
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

        if let Some(v) = table.get("noise_type") {
            let s = toml_string(v, "noise_type")?;
            match s.as_str() {
                "perlin" | "simplex" => p.noise_type = s,
                other => {
                    return Err(ProcGenError::InvalidParameter {
                        name: "noise_type".into(),
                        reason: format!(
                            "unknown noise type \"{other}\"; expected \"perlin\" or \"simplex\""
                        ),
                    });
                }
            }
        }
        if let Some(v) = table.get("octaves") {
            p.octaves = toml_u32(v, "octaves")?;
            if p.octaves == 0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "octaves".into(),
                    reason: "must be positive".into(),
                });
            }
        }
        if let Some(v) = table.get("frequency") {
            p.frequency = toml_f64(v, "frequency")?;
            if p.frequency <= 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "frequency".into(),
                    reason: "must be positive".into(),
                });
            }
        }
        if let Some(v) = table.get("lacunarity") {
            p.lacunarity = toml_f64(v, "lacunarity")?;
            if p.lacunarity <= 0.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "lacunarity".into(),
                    reason: "must be positive".into(),
                });
            }
        }
        if let Some(v) = table.get("persistence") {
            p.persistence = toml_f64(v, "persistence")?;
            if p.persistence <= 0.0 || p.persistence > 1.0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "persistence".into(),
                    reason: "must be in (0.0, 1.0]".into(),
                });
            }
        }
        if let Some(v) = table.get("height_bands") {
            p.height_bands = toml_u32(v, "height_bands")?;
            if p.height_bands == 0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "height_bands".into(),
                    reason: "must be positive".into(),
                });
            }
        }

        Ok(p)
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

        let perlin = PerlinNoise::new(noise_seed);
        let mut field = PatternField::new(width, height);

        for y in 0..height {
            for x in 0..width {
                let u = x as f64 / width as f64;
                let v = y as f64 / height as f64;

                let raw = if !seamless {
                    noise.sample_2d(u, v)
                } else if p.noise_type == "simplex" {
                    crossfade_tile(noise.as_ref(), u, v)
                } else {
                    // Perlin tiles exactly: every octave gets a whole number
                    // of lattice cells per tile (frequency rounded per octave).
                    perlin.fbm_2d_periodic(
                        u,
                        v,
                        p.frequency,
                        p.octaves,
                        p.lacunarity,
                        p.persistence,
                    )
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

/// Make any noise tile by cross-fading it with copies of itself shifted one
/// tile left, up, and both: at `u = 1` the blend is exactly the `u = 0`
/// value. Averaging lowers the variance mid-tile, so the blend is divided by
/// the RMS of its weights to keep the contrast even across the tile.
///
/// Used for simplex, which has no periodic lattice; Perlin uses
/// [`PerlinNoise::fbm_2d_periodic`], which tiles without blending.
fn crossfade_tile(noise: &dyn NoiseSource, u: f64, v: f64) -> f64 {
    let (a, b) = (noise.sample_2d(u, v), noise.sample_2d(u - 1.0, v));
    let (c, d) = (
        noise.sample_2d(u, v - 1.0),
        noise.sample_2d(u - 1.0, v - 1.0),
    );
    let (wu, wv) = (1.0 - u, 1.0 - v);
    let blend = wv * (wu * a + u * b) + v * (wu * c + u * d);
    let rms = ((wu * wu + u * u) * (wv * wv + v * v)).sqrt();
    (blend / rms).clamp(-1.0, 1.0)
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
    fn organic_params_from_toml_defaults() {
        let toml_val: toml::Value = toml::toml! {
            noise_type = "perlin"
        }
        .into();
        let p = PerlinOrganicParams::from_toml(&toml_val).unwrap();
        assert_eq!(p.noise_type, "perlin");
        assert_eq!(p.octaves, 6);
        assert!((p.frequency - 4.0).abs() < 1e-5);
        assert!((p.lacunarity - 2.0).abs() < 1e-5);
        assert!((p.persistence - 0.5).abs() < 1e-5);
        assert_eq!(p.height_bands, 8);
    }

    #[test]
    fn organic_params_from_toml_custom() {
        let toml_val: toml::Value = toml::toml! {
            noise_type = "simplex"
            octaves = 4
            frequency = 8.0
            lacunarity = 2.5
            persistence = 0.4
            height_bands = 12
        }
        .into();
        let p = PerlinOrganicParams::from_toml(&toml_val).unwrap();
        assert_eq!(p.noise_type, "simplex");
        assert_eq!(p.octaves, 4);
        assert!((p.frequency - 8.0).abs() < 1e-5);
        assert!((p.lacunarity - 2.5).abs() < 1e-5);
        assert!((p.persistence - 0.4).abs() < 1e-5);
        assert_eq!(p.height_bands, 12);
    }

    #[test]
    fn organic_params_from_toml_invalid_noise_type() {
        let toml_val: toml::Value = toml::toml! {
            noise_type = "worley"
        }
        .into();
        assert!(PerlinOrganicParams::from_toml(&toml_val).is_err());
    }

    #[test]
    fn organic_params_from_toml_invalid_persistence() {
        let toml_val: toml::Value = toml::toml! {
            persistence = 0.0
        }
        .into();
        assert!(PerlinOrganicParams::from_toml(&toml_val).is_err());
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

    fn seamless(noise_type: &str, frequency: f64) -> PatternField {
        let pattern = PerlinOrganicPattern::new(PerlinOrganicParams {
            noise_type: noise_type.into(),
            frequency,
            ..Default::default()
        });
        pattern.generate(128, 128, true, &mut SeededRng::new(42))
    }

    #[test]
    fn organic_seamless_has_no_seam() {
        // Perlin's periodic lattice and simplex's cross-fade both make the
        // step across the wrap look like any step between neighbours.
        for (noise, freq) in [
            ("perlin", 4.0),
            ("perlin", 0.7),
            ("simplex", 4.0),
            ("simplex", 0.7),
        ] {
            let s = seamless(noise, freq).seam_steps();
            assert!(s.ratio() < 1.6, "{noise} @ {freq}: {s:?}");
        }
    }

    #[test]
    fn organic_seamless_is_not_a_mirror() {
        // The old torus-in-3D trick fed cos(2πv) to the noise, so row y and
        // row (h - y) were the same row: every tile was mirrored top to
        // bottom. Real tiling noise has no such symmetry.
        for noise in ["perlin", "simplex"] {
            let f = seamless(noise, 4.0);
            let (w, h) = (f.width, f.height);
            let mut diff = 0.0f32;
            for y in 1..h / 2 {
                for x in 0..w {
                    diff += (f.get(x, y).height - f.get(x, h - y).height).abs();
                }
            }
            let mean = diff / ((h / 2 - 1) * w) as f32;
            assert!(
                mean > 0.03,
                "{noise}: rows mirror each other (mean diff {mean})"
            );
        }
    }

    #[test]
    fn organic_octaves_add_detail() {
        // More octaves, more fine structure. The mean absolute second
        // difference (curvature) weighs octave k by its frequency squared
        // times its amplitude, so each octave added should raise it.
        let detail = |octaves: u32| {
            let pattern = PerlinOrganicPattern::new(PerlinOrganicParams {
                octaves,
                ..Default::default()
            });
            let f = pattern.generate(128, 128, true, &mut SeededRng::new(42));
            let h = |x: u32, y: u32| f.get(x, y).height;
            let mut sum = 0.0f32;
            for y in 0..128 {
                for x in 1..127 {
                    sum += (h(x + 1, y) - 2.0 * h(x, y) + h(x - 1, y)).abs();
                }
            }
            sum / (128.0 * 126.0)
        };
        let steps: Vec<f32> = [1, 2, 4, 6].iter().map(|&o| detail(o)).collect();
        for pair in steps.windows(2) {
            assert!(pair[1] > pair[0], "detail by octave {steps:?}");
        }
        assert!(steps[3] > steps[0] * 1.5, "detail by octave {steps:?}");
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
        use crate::generators::texture::pattern::write_debug_png;
        let pattern = default_pattern();
        let mut rng = SeededRng::new(42);
        let field = pattern.generate(256, 256, true, &mut rng);
        write_debug_png("organic_height", &field.to_height_image());
        write_debug_png("organic_cells", &field.to_cell_id_image());
        write_debug_png("organic_edges", &field.to_edge_distance_image());
        for octaves in [1u32, 2, 4, 6] {
            let p = PerlinOrganicPattern::new(PerlinOrganicParams {
                octaves,
                ..Default::default()
            });
            let f = p.generate(256, 256, true, &mut SeededRng::new(42));
            write_debug_png(&format!("organic_octaves_{octaves}"), &f.to_height_image());
        }
    }
}
