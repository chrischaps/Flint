//! Automatic splat map generation from height/slope rules

use crate::spec::SplatRule;
use noise::{NoiseFn, Perlin};

/// Generate an RGBA8 splat map from heightmap data and splat rules.
///
/// Each pixel's R/G/B/A channels correspond to layers 0-3.
/// Channel values are normalized so they sum to 255 per pixel.
///
/// Returns `splat_res * splat_res * 4` bytes.
pub fn generate_splat_map(
    heights: &[f32],
    height_res: u32,
    splat_res: u32,
    rules: &[SplatRule],
    width: f32,
    depth: f32,
    height_scale: f32,
    seed: u64,
) -> Vec<u8> {
    let splat_sz = splat_res as usize;
    let height_sz = height_res as usize;
    let mut data = vec![0u8; splat_sz * splat_sz * 4];

    let noise_gen = Perlin::new((seed & 0xFFFF_FFFF) as u32);

    for py in 0..splat_sz {
        for px in 0..splat_sz {
            let u = px as f32 / (splat_sz - 1).max(1) as f32;
            let v = py as f32 / (splat_sz - 1).max(1) as f32;

            // Bilinear sample height
            let h = sample_bilinear(heights, height_sz, u, v);

            // Compute slope via finite difference
            let slope = compute_slope(heights, height_sz, u, v, width, depth, height_scale);

            // Accumulate weights per layer
            let mut weights = [0.0f32; 4];

            for rule in rules {
                if rule.layer > 3 {
                    continue;
                }

                // Height criterion (in normalized [0..1] space)
                let h_norm = h;
                if h_norm < rule.height_min || h_norm > rule.height_max {
                    continue;
                }

                // Slope criterion
                if slope > rule.slope_max {
                    continue;
                }

                // Height-based weight (smooth edges)
                let h_range = rule.height_max - rule.height_min;
                let h_weight = if h_range > 0.0 {
                    let edge = h_range * 0.1;
                    let low_fade = ((h_norm - rule.height_min) / edge.max(0.001)).clamp(0.0, 1.0);
                    let high_fade = ((rule.height_max - h_norm) / edge.max(0.001)).clamp(0.0, 1.0);
                    low_fade.min(high_fade)
                } else {
                    1.0
                };

                // Slope weight
                let slope_weight = if rule.slope_max > 0.0 {
                    (1.0 - slope / rule.slope_max).clamp(0.0, 1.0)
                } else {
                    1.0
                };

                // Noise modulation
                let noise_mod = if rule.noise_amount > 0.0 {
                    let nx = u as f64 * rule.noise_scale as f64;
                    let ny = v as f64 * rule.noise_scale as f64;
                    let n = noise_gen.get([nx, ny]) as f32;
                    1.0 + n * rule.noise_amount
                } else {
                    1.0
                };

                let w = (h_weight * slope_weight * noise_mod).max(0.0);
                weights[rule.layer as usize] += w;
            }

            // Normalize weights to sum = 255
            let total: f32 = weights.iter().sum();
            let idx = (py * splat_sz + px) * 4;

            if total > 0.0 {
                for c in 0..4 {
                    data[idx + c] = ((weights[c] / total) * 255.0).round() as u8;
                }
            } else {
                // Default: layer 0 = full
                data[idx] = 255;
            }
        }
    }

    data
}

fn sample_bilinear(heights: &[f32], res: usize, u: f32, v: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    let v = v.clamp(0.0, 1.0);

    let fx = u * (res - 1) as f32;
    let fy = v * (res - 1) as f32;

    let x0 = (fx as usize).min(res - 2);
    let y0 = (fy as usize).min(res - 2);
    let x1 = x0 + 1;
    let y1 = y0 + 1;

    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;

    let h00 = heights[y0 * res + x0];
    let h10 = heights[y0 * res + x1];
    let h01 = heights[y1 * res + x0];
    let h11 = heights[y1 * res + x1];

    let h0 = h00 * (1.0 - tx) + h10 * tx;
    let h1 = h01 * (1.0 - tx) + h11 * tx;

    h0 * (1.0 - ty) + h1 * ty
}

fn compute_slope(
    heights: &[f32],
    res: usize,
    u: f32,
    v: f32,
    width: f32,
    depth: f32,
    height_scale: f32,
) -> f32 {
    let eps_u = 1.0 / res as f32;
    let eps_v = 1.0 / res as f32;

    let h_left = sample_bilinear(heights, res, (u - eps_u).max(0.0), v) * height_scale;
    let h_right = sample_bilinear(heights, res, (u + eps_u).min(1.0), v) * height_scale;
    let h_down = sample_bilinear(heights, res, u, (v - eps_v).max(0.0)) * height_scale;
    let h_up = sample_bilinear(heights, res, u, (v + eps_v).min(1.0)) * height_scale;

    let dx = (h_right - h_left) / (2.0 * eps_u * width);
    let dz = (h_up - h_down) / (2.0 * eps_v * depth);

    // Slope = sin(angle) ≈ |gradient| / sqrt(1 + |gradient|²)
    let grad_sq = dx * dx + dz * dz;
    grad_sq.sqrt() / (1.0 + grad_sq).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::SplatRule;

    #[test]
    fn height_based_rules() {
        let res = 32;
        // Gradient from 0 to 1 along Y
        let heights: Vec<f32> = (0..res * res)
            .map(|i| (i / res) as f32 / (res - 1) as f32)
            .collect();

        let rules = vec![
            SplatRule {
                layer: 0,
                height_min: 0.0,
                height_max: 0.4,
                slope_max: 1.0,
                noise_amount: 0.0,
                noise_scale: 1.0,
            },
            SplatRule {
                layer: 1,
                height_min: 0.6,
                height_max: 1.0,
                slope_max: 1.0,
                noise_amount: 0.0,
                noise_scale: 1.0,
            },
        ];

        let splat = generate_splat_map(
            &heights, res as u32, res as u32, &rules, 100.0, 100.0, 50.0, 42,
        );

        // Row 2 (height ~0.065): should be layer 0 (R channel)
        let low_row = 2;
        let low_idx = (low_row * res) * 4;
        assert!(splat[low_idx] > 0, "Low rows should have layer 0 weight");

        // Row 30 (height ~0.97): should be layer 1 (G channel)
        let high_row = 30;
        let high_idx = (high_row * res) * 4;
        assert!(
            splat[high_idx + 1] > 0,
            "High rows should have layer 1 weight"
        );
    }
}
