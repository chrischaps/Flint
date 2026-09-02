//! Procedural bark normal map generation.
//!
//! Generates a tangent-space normal map from layered FBM noise with vertical
//! stretch to simulate elongated bark ridges. A Sobel-style filter converts
//! the heightfield to normals.

use crate::algorithms::noise::{Fbm, NoiseSource, PerlinNoise};
use crate::types::{ChannelSemantics, ImageData};
use crate::SeededRng;

/// Generate a procedural bark normal map.
///
/// The heightfield is built from two FBM layers:
/// - A primary layer with vertical stretch (v scaled by 0.3) for bark ridges
/// - A secondary higher-frequency layer for fine cracks
///
/// The heightfield is then converted to tangent-space normals via a Sobel filter.
pub fn generate_bark_normal_map(rng: &mut SeededRng, resolution: u32, strength: f32) -> ImageData {
    let res = resolution as usize;

    // Fork RNG for deterministic bark generation
    let mut bark_rng = rng.fork("bark_normal");

    // Primary FBM: elongated bark ridges
    let primary_seed = bark_rng.fork("primary").seed();
    let primary = Fbm::new(PerlinNoise::new(primary_seed))
        .with_octaves(4)
        .with_frequency(8.0)
        .with_persistence(0.5)
        .with_lacunarity(2.0);

    // Secondary FBM: fine cracks at higher frequency
    let secondary_seed = bark_rng.fork("secondary").seed();
    let secondary = Fbm::new(PerlinNoise::new(secondary_seed))
        .with_octaves(3)
        .with_frequency(24.0)
        .with_persistence(0.4)
        .with_lacunarity(2.2);

    // Build heightfield
    let mut heightfield = vec![0.0f32; res * res];
    for y in 0..res {
        for x in 0..res {
            let u = x as f64 / res as f64;
            let v = y as f64 / res as f64;

            // Vertical stretch: scale v by 0.3 to create elongated ridges
            let h_primary = primary.sample_2d(u, v * 0.3) as f32;
            let h_secondary = secondary.sample_2d(u, v) as f32 * 0.3;

            heightfield[y * res + x] = h_primary + h_secondary;
        }
    }

    // Convert heightfield to normal map via Sobel filter
    let mut image = ImageData::new(resolution, resolution, ChannelSemantics::Normal);

    for y in 0..res {
        for x in 0..res {
            // Sample neighbors with wrapping for seamless tiling
            let left = heightfield[y * res + (x + res - 1) % res];
            let right = heightfield[y * res + (x + 1) % res];
            let top = heightfield[((y + res - 1) % res) * res + x];
            let bottom = heightfield[((y + 1) % res) * res + x];

            // Sobel-style gradient
            let dx = left - right;
            let dy = top - bottom;

            // Tangent-space normal with configurable strength
            let inv_strength = if strength.abs() > 1e-10 {
                1.0 / strength
            } else {
                1.0
            };
            let nx = -dx;
            let ny = -dy;
            let nz = inv_strength;

            // Normalize
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            let nx = nx / len;
            let ny = ny / len;
            let nz = nz / len;

            // Encode to [0, 255] range (tangent-space: Z always positive)
            let r = ((nx * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8;
            let g = ((ny * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8;
            let b = ((nz * 0.5 + 0.5) * 255.0).clamp(0.0, 255.0) as u8;

            let offset = (y * res + x) * 4;
            image.pixels[offset] = r;
            image.pixels[offset + 1] = g;
            image.pixels[offset + 2] = b;
            image.pixels[offset + 3] = 255;
        }
    }

    image
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_map_dimensions() {
        let mut rng = SeededRng::new(42);
        let img = generate_bark_normal_map(&mut rng, 64, 1.0);
        assert_eq!(img.width, 64);
        assert_eq!(img.height, 64);
        assert!(img.validate().is_ok());
        assert_eq!(img.channel_semantics, ChannelSemantics::Normal);
    }

    #[test]
    fn normal_map_z_always_positive() {
        let mut rng = SeededRng::new(42);
        let img = generate_bark_normal_map(&mut rng, 64, 1.0);

        // Z channel (blue) should always be > 127 (positive hemisphere)
        for y in 0..64 {
            for x in 0..64 {
                let offset = (y * 64 + x) * 4;
                let b = img.pixels[offset + 2];
                assert!(b > 127, "Z channel at ({x}, {y}) = {b}, expected > 127");
            }
        }
    }

    #[test]
    fn normal_map_alpha_opaque() {
        let mut rng = SeededRng::new(42);
        let img = generate_bark_normal_map(&mut rng, 32, 1.0);

        for i in 0..img.pixels.len() / 4 {
            assert_eq!(img.pixels[i * 4 + 3], 255);
        }
    }

    #[test]
    fn normal_map_not_flat() {
        let mut rng = SeededRng::new(42);
        let img = generate_bark_normal_map(&mut rng, 64, 1.0);

        // Not all pixels should have the same R/G values (flat normal = 128,128,255)
        let has_variation = img.pixels.chunks(4).any(|px| px[0] != 128 || px[1] != 128);
        assert!(has_variation, "normal map should not be perfectly flat");
    }

    #[test]
    fn normal_map_determinism() {
        let mut rng1 = SeededRng::new(99);
        let mut rng2 = SeededRng::new(99);
        let img1 = generate_bark_normal_map(&mut rng1, 32, 1.0);
        let img2 = generate_bark_normal_map(&mut rng2, 32, 1.0);
        assert_eq!(img1.pixels, img2.pixels);
    }
}
