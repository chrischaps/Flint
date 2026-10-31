//! Procedural bark normal map generation.
//!
//! Bark is a heightfield of plates split by vertical furrows (the primary
//! pattern) and crossed by short horizontal cracks (the secondary detail),
//! with a little fine grain on top. Every layer is built from periodic noise,
//! so the map tiles in both directions: `u` wraps around the trunk and `v`
//! repeats up it. Central differences (wrapping at the edges) turn the
//! heightfield into tangent-space normals.

use crate::algorithms::noise::PerlinNoise;
use crate::types::{ChannelSemantics, ImageData};
use crate::SeededRng;

/// Furrows across one tile, i.e. once around the base of the trunk.
const FURROWS: f64 = 14.0;

/// Depth of a furrow in tile units (one tile is the base's circumference),
/// which sets how steep the normals get at `strength = 1`.
const RELIEF: f64 = 0.03;

/// Steepest slope a normal may take, so its Z stays well above zero even at
/// high `strength` (Z ≥ 1/√(1 + 6²) ≈ 0.16).
const MAX_SLOPE: f64 = 6.0;

/// Sum of periodic Perlin octaves over the unit tile, `cells` lattice cells
/// across on each axis at the first octave and twice as many per octave after.
fn periodic_fbm(noise: &PerlinNoise, u: f64, v: f64, cells: (i64, i64), octaves: u32) -> f64 {
    let (mut cx, mut cy) = cells;
    let (mut sum, mut amp, mut norm) = (0.0, 1.0, 0.0);
    for _ in 0..octaves {
        sum += amp * noise.sample_2d_periodic(u * cx as f64, v * cy as f64, cx, cy);
        norm += amp;
        amp *= 0.5;
        cx *= 2;
        cy *= 2;
    }
    sum / norm
}

fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The noise behind each layer of the bark heightfield.
struct BarkLayers {
    warp: PerlinNoise,
    plate: PerlinNoise,
    crack: PerlinNoise,
    crack_mask: PerlinNoise,
    grain: PerlinNoise,
}

impl BarkLayers {
    fn new(rng: &mut SeededRng) -> Self {
        let mut bark_rng = rng.fork("bark_normal");
        let mut layer = |label: &str| PerlinNoise::new(bark_rng.fork(label).seed());
        Self {
            warp: layer("warp"),
            plate: layer("plate"),
            crack: layer("crack"),
            crack_mask: layer("crack_mask"),
            grain: layer("grain"),
        }
    }

    /// Bark height at `(u, v)`, periodic with period 1 on both axes.
    fn height(&self, u: f64, v: f64) -> f64 {
        // Furrows: distance to the nearest groove line, 0 in the groove and
        // 1 mid-plate, with flattened plate tops.
        let phase = FURROWS * u + 1.6 * periodic_fbm(&self.warp, u, v, (3, 2), 3);
        let across = 1.0 - (2.0 * phase.rem_euclid(1.0) - 1.0).abs();
        let plates = smoothstep(0.0, 0.6, across);
        let swell = 0.8 + 0.2 * periodic_fbm(&self.plate, u, v, (7, 5), 2);

        // Cracks: thin lines where horizontally stretched noise crosses zero,
        // kept only where the mask is positive and on plate tops.
        let c = periodic_fbm(&self.crack, u, v, (5, 24), 2);
        let lines = 1.0 - smoothstep(0.0, 0.08, c.abs());
        let mask = smoothstep(0.0, 0.25, periodic_fbm(&self.crack_mask, u, v, (6, 6), 1));
        let cracks = lines * mask * plates;

        let fine = periodic_fbm(&self.grain, u, v, (48, 32), 2);

        plates * swell - 0.45 * cracks + 0.04 * fine
    }
}

/// Generate a procedural bark normal map.
///
/// The heightfield has three layers, all periodic over the tile:
/// - **Furrows** (primary): `FURROWS` vertical grooves, their line wandered
///   sideways by low-frequency noise so plates widen, narrow and merge.
/// - **Cracks** (secondary): zero-crossings of noise stretched horizontally,
///   cut into the plate tops only and broken up by a mask.
/// - **Grain**: faint high-frequency noise.
///
/// `strength` scales the relief; Z stays positive at any strength.
pub fn generate_bark_normal_map(rng: &mut SeededRng, resolution: u32, strength: f32) -> ImageData {
    let res = resolution as usize;

    let layers = BarkLayers::new(rng);
    let mut heightfield = vec![0.0f64; res * res];
    for y in 0..res {
        for x in 0..res {
            heightfield[y * res + x] = layers.height(x as f64 / res as f64, y as f64 / res as f64);
        }
    }

    // Slopes are measured per tile (res pixels), so the look does not change
    // with resolution.
    let relief = RELIEF * strength as f64 * res as f64 * 0.5;
    let mut image = ImageData::new(resolution, resolution, ChannelSemantics::Normal);
    for y in 0..res {
        for x in 0..res {
            let left = heightfield[y * res + (x + res - 1) % res];
            let right = heightfield[y * res + (x + 1) % res];
            let prev_row = heightfield[((y + res - 1) % res) * res + x];
            let next_row = heightfield[((y + 1) % res) * res + x];

            // A heightfield's normal leans away from its slope, (-∂h/∂u,
            // -∂h/∂v, 1) with v running down the image. glTF's +Y points up
            // the image (the renderer's bitangent is N × T, which is -v), so
            // Y is stored as +∂h/∂v.
            let mut sx = -(right - left) * relief;
            let mut sy = (next_row - prev_row) * relief;
            let slope = (sx * sx + sy * sy).sqrt();
            if slope > MAX_SLOPE {
                sx *= MAX_SLOPE / slope;
                sy *= MAX_SLOPE / slope;
            }
            let len = (sx * sx + sy * sy + 1.0).sqrt();
            let n = [sx / len, sy / len, 1.0 / len];

            let offset = (y * res + x) * 4;
            for (c, value) in n.iter().enumerate() {
                image.pixels[offset + c] = ((value * 0.5 + 0.5) * 255.0).round() as u8;
            }
            image.pixels[offset + 3] = 255;
        }
    }

    image
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(img: &ImageData, x: usize, y: usize) -> [f64; 3] {
        let o = (y * img.width as usize + x) * 4;
        std::array::from_fn(|c| img.pixels[o + c] as f64 / 255.0 * 2.0 - 1.0)
    }

    #[test]
    fn normal_map_dimensions() {
        let mut rng = SeededRng::new(42);
        let img = generate_bark_normal_map(&mut rng, 64, 1.0);
        assert_eq!(img.width, 64);
        assert_eq!(img.height, 64);
        assert!(img.validate().is_ok());
        assert_eq!(img.channel_semantics, ChannelSemantics::Normal);
    }

    /// Z (blue) is in the positive hemisphere everywhere, even with relief
    /// far steeper than any spec would ask for.
    #[test]
    fn normal_map_z_always_positive() {
        for strength in [1.0, 4.0, 50.0] {
            let mut rng = SeededRng::new(42);
            let img = generate_bark_normal_map(&mut rng, 64, strength);
            for px in img.pixels.chunks(4) {
                assert!(px[2] > 127, "strength {strength}: Z = {}", px[2]);
            }
        }
    }

    /// Decoded normals are unit length, to within 8-bit quantisation.
    #[test]
    fn normals_are_unit_length() {
        let img = generate_bark_normal_map(&mut SeededRng::new(8), 64, 1.5);
        for y in 0..64 {
            for x in 0..64 {
                let [nx, ny, nz] = decode(&img, x, y);
                let len = (nx * nx + ny * ny + nz * nz).sqrt();
                assert!((len - 1.0).abs() < 0.02, "({x},{y}) length {len:.3}");
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

    /// The heightfield repeats with period 1 on both axes, so the map wraps
    /// around the trunk and repeats up it without a seam.
    #[test]
    fn bark_height_is_periodic() {
        let layers = BarkLayers::new(&mut SeededRng::new(21));
        for i in 0..200 {
            let (u, v) = ((i as f64 * 0.137) % 1.0, (i as f64 * 0.291) % 1.0);
            let h = layers.height(u, v);
            for (du, dv) in [(1.0, 0.0), (0.0, 1.0), (-1.0, 2.0)] {
                let shifted = layers.height(u + du, v + dv);
                assert!(
                    (h - shifted).abs() < 1e-9,
                    "({u:.3},{v:.3}) +({du},{dv}): {h} vs {shifted}"
                );
            }
        }
    }

    /// Furrows run up the trunk: normals tilt sideways (X) far more than
    /// along the trunk (Y), while the cracks still give Y some relief.
    #[test]
    fn furrows_run_vertically_and_cracks_horizontally() {
        let img = generate_bark_normal_map(&mut SeededRng::new(4), 128, 1.0);
        let (mut sx, mut sy) = (0.0, 0.0);
        for px in img.pixels.chunks(4) {
            sx += (px[0] as f64 / 255.0 * 2.0 - 1.0).abs();
            sy += (px[1] as f64 / 255.0 * 2.0 - 1.0).abs();
        }
        assert!(sx > 1.5 * sy, "sideways tilt {sx:.0} vs vertical {sy:.0}");
        assert!(sy > 0.1 * sx, "no horizontal detail: {sy:.0} vs {sx:.0}");
    }
}
