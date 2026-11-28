//! Automatic splat map generation from height/slope rules

use crate::spec::{SplatRule, TerrainSpec};
use noise::{NoiseFn, Perlin};

/// A splat map, and how much of it no rule covered.
#[derive(Debug, Clone, PartialEq)]
pub struct SplatMap {
    /// RGBA8 layer weights (R = layer 0 .. A = layer 3), `splat_res² * 4`
    /// bytes. A covered texel's four sum to about 255; an uncovered one is
    /// the fallback's (255 in its layer, or all 0 for `"none"`).
    pub rgba: Vec<u8>,
    /// Texels no rule covered, which took the spec's `[splat] fallback`.
    pub uncovered: usize,
}

/// Generate an RGBA8 splat map from heightmap data and the spec's splat
/// rules, at the spec's geometry and fallback.
///
/// Each pixel's R/G/B/A channels correspond to layers 0-3. Every rule a
/// texel fits adds its weight to its layer, and the four are normalized to
/// sum to 255. A texel no rule gives any weight is the spec's fallback
/// (#1696): one layer at full weight (layer 0 unless the spec says), or no
/// layer at all.
pub fn generate_splat_map(
    heights: &[f32],
    height_res: u32,
    splat_res: u32,
    spec: &TerrainSpec,
    seed: u64,
) -> SplatMap {
    let splat_sz = splat_res as usize;
    let height_sz = height_res as usize;
    let geo = &spec.geometry;
    let mut data = vec![0u8; splat_sz * splat_sz * 4];
    let mut uncovered = 0;

    let noise_gen = Perlin::new((seed & 0xFFFF_FFFF) as u32);

    for py in 0..splat_sz {
        for px in 0..splat_sz {
            let u = px as f32 / (splat_sz - 1).max(1) as f32;
            let v = py as f32 / (splat_sz - 1).max(1) as f32;

            // Bilinear sample height
            let h = sample_bilinear(heights, height_sz, u, v);

            // Rise over run, by finite difference
            let slope = compute_slope(
                heights,
                height_sz,
                u,
                v,
                geo.width,
                geo.depth,
                geo.height_scale,
            );

            // Accumulate weights per layer
            let mut weights = [0.0f32; 4];
            for rule in &spec.splat_rules {
                if rule.layer > 3 {
                    continue;
                }
                weights[rule.layer as usize] += rule_weight(rule, h, slope, u, v, &noise_gen);
            }

            // Normalize weights to sum = 255
            let total: f32 = weights.iter().sum();
            let idx = (py * splat_sz + px) * 4;

            if total > 0.0 {
                for c in 0..4 {
                    data[idx + c] = ((weights[c] / total) * 255.0).round() as u8;
                }
            } else {
                uncovered += 1;
                if let Some(layer) = spec.splat.fallback.layer().filter(|&l| l <= 3) {
                    data[idx + layer as usize] = 255;
                }
            }
        }
    }

    SplatMap {
        rgba: data,
        uncovered,
    }
}

/// One rule's weight at a texel of normalized height `h` and slope `slope`
/// (rise over run): 0 outside its bands, fading toward their edges.
fn rule_weight(rule: &SplatRule, h: f32, slope: f32, u: f32, v: f32, noise: &Perlin) -> f32 {
    // Height criterion (in normalized [0..1] space)
    if h < rule.height_min || h > rule.height_max {
        return 0.0;
    }
    // Height-based weight (smooth edges). A band reaching 0 or 1 has no
    // edge there to fade at: heights are normalized, so the lowest texel
    // is exactly 0 and the summit exactly 1, and until #1696 a fade to 0
    // at them left both to the fallback (a grassy summit).
    let h_range = rule.height_max - rule.height_min;
    let h_weight = if h_range > 0.0 {
        let edge = (h_range * 0.1).max(0.001);
        let low_fade = if rule.height_min <= 0.0 {
            1.0
        } else {
            ((h - rule.height_min) / edge).clamp(0.0, 1.0)
        };
        let high_fade = if rule.height_max >= 1.0 {
            1.0
        } else {
            ((rule.height_max - h) / edge).clamp(0.0, 1.0)
        };
        low_fade.min(high_fade)
    } else {
        1.0
    };

    // Slope ceiling: full on the flat, fading out to 0 at slope_max
    let max_weight = match rule.slope_max {
        Some(max) if slope > max => return 0.0,
        Some(max) if max > 0.0 => (1.0 - slope / max).clamp(0.0, 1.0),
        _ => 1.0,
    };
    // Slope floor, the mirror of the ceiling: 0 at slope_min, fading in
    // toward full on a cliff (#1696)
    let min_weight = match rule.slope_min {
        Some(min) if min > 0.0 && slope <= min => return 0.0,
        Some(min) if min > 0.0 => (1.0 - min / slope).clamp(0.0, 1.0),
        _ => 1.0,
    };

    // Noise modulation
    let noise_mod = if rule.noise_amount > 0.0 {
        let nx = u as f64 * rule.noise_scale as f64;
        let ny = v as f64 * rule.noise_scale as f64;
        let n = noise.get([nx, ny]) as f32;
        1.0 + n * rule.noise_amount
    } else {
        1.0
    };

    (h_weight * max_weight * min_weight * noise_mod).max(0.0)
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

/// The ground's slope at (u, v) as rise over run: the length of the height
/// gradient in metres a metre, so 1 is 45 degrees.
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

    let (u0, u1) = ((u - eps_u).max(0.0), (u + eps_u).min(1.0));
    let (v0, v1) = ((v - eps_v).max(0.0), (v + eps_v).min(1.0));
    let h_left = sample_bilinear(heights, res, u0, v) * height_scale;
    let h_right = sample_bilinear(heights, res, u1, v) * height_scale;
    let h_down = sample_bilinear(heights, res, u, v0) * height_scale;
    let h_up = sample_bilinear(heights, res, u, v1) * height_scale;

    // Divided by the distance actually spanned, which is one step, not two,
    // at the map's edge
    let dx = (h_right - h_left) / ((u1 - u0) * width).max(f32::EPSILON);
    let dz = (h_up - h_down) / ((v1 - v0) * depth).max(f32::EPSILON);

    (dx * dx + dz * dz).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::SplatFallback;

    fn spec_with(rules: &str) -> TerrainSpec {
        toml::from_str(&format!(
            "[meta]\nname = \"t\"\n[geometry]\nwidth = 100.0\ndepth = 100.0\nheight_scale = 50.0\n{rules}"
        ))
        .unwrap()
    }

    #[test]
    fn height_based_rules() {
        let res = 32;
        // Gradient from 0 to 1 along Y
        let heights: Vec<f32> = (0..res * res)
            .map(|i| (i / res) as f32 / (res - 1) as f32)
            .collect();

        let spec = spec_with(
            "[[splat_rules]]\nlayer = 0\nheight_max = 0.4\n\
             [[splat_rules]]\nlayer = 1\nheight_min = 0.6\n",
        );
        let splat = generate_splat_map(&heights, res as u32, res as u32, &spec, 42).rgba;

        // Row 2 (height ~0.065): should be layer 0 (R channel)
        let low_idx = (2 * res) * 4;
        assert!(splat[low_idx] > 0, "Low rows should have layer 0 weight");

        // Row 30 (height ~0.97): should be layer 1 (G channel)
        let high_idx = (30 * res) * 4;
        assert!(
            splat[high_idx + 1] > 0,
            "High rows should have layer 1 weight"
        );
    }

    /// A ramp whose slope grows steadily from flat at x = 0 to rise over
    /// run 2 at the far edge: heights `x² / 64` metres on a 64 m square,
    /// so the slope at x is `x / 32` and 45 degrees is x = 32 m.
    fn ramp(res: usize) -> (Vec<f32>, TerrainSpec, impl Fn(usize) -> f32) {
        let size = 64.0f32;
        let step = size / (res - 1) as f32;
        let heights = (0..res * res)
            .map(|i| {
                let x = (i % res) as f32 * step;
                x * x / size / size // normalized: height_scale is 64 m
            })
            .collect();
        let spec: TerrainSpec = toml::from_str(
            "[meta]\nname = \"ramp\"\n[geometry]\nwidth = 64.0\ndepth = 64.0\nheight_scale = 64.0\n",
        )
        .unwrap();
        (heights, spec, move |px| px as f32 * step / 32.0)
    }

    /// Done-when of #1696: `slope_min = 1.0` paints only ground steeper
    /// than 45 degrees.
    #[test]
    fn slope_min_one_paints_only_what_is_steeper_than_45_degrees() {
        let res = 129;
        let (heights, mut spec, slope_at) = ramp(res);
        spec.splat_rules = vec![toml::from_str("layer = 2\nslope_min = 1.0").unwrap()];
        spec.splat.fallback = SplatFallback::None;
        let map = generate_splat_map(&heights, res as u32, res as u32, &spec, 1);

        let mut steep = 0;
        // The middle row; the columns either side of the 45 degree line
        // (and the edge, which differences one-sided) are held exactly
        for px in 1..res - 1 {
            let rock = map.rgba[((res / 2) * res + px) * 4 + 2];
            let slope = slope_at(px);
            if slope > 1.01 {
                assert_eq!(rock, 255, "column {px}, slope {slope:.3}: not rock");
                steep += 1;
            } else if slope < 0.99 {
                assert_eq!(rock, 0, "column {px}, slope {slope:.3}: rock below 45");
            }
        }
        assert!(steep > 50, "only {steep} steep columns");
        // Everything gentler than 45 degrees took the fallback: half the map
        let share = map.uncovered as f64 / (res * res) as f64;
        assert!((share - 0.5).abs() < 0.02, "uncovered {share}");
    }

    /// The mirror pair: rock on the steep, grass on the gentle, and between
    /// them no texel left to the fallback.
    #[test]
    fn slope_min_and_slope_max_split_the_ramp_at_45_degrees() {
        let res = 65;
        let (heights, mut spec, slope_at) = ramp(res);
        spec.splat_rules = vec![
            toml::from_str("layer = 0\nslope_max = 1.0").unwrap(),
            toml::from_str("layer = 2\nslope_min = 1.0").unwrap(),
        ];
        let map = generate_splat_map(&heights, res as u32, res as u32, &spec, 1);
        for px in 1..res - 1 {
            let t = &map.rgba[((res / 2) * res + px) * 4..][..4];
            let slope = slope_at(px);
            if slope < 0.98 {
                assert_eq!(t, [255, 0, 0, 0], "column {px}, slope {slope:.3}");
            } else if slope > 1.02 {
                assert_eq!(t, [0, 0, 255, 0], "column {px}, slope {slope:.3}");
            }
        }
    }

    /// The slope is rise over run, as the docs have always said: a
    /// straight 45 degree ramp measures 1, not its sine (0.707).
    #[test]
    fn a_45_degree_ramp_measures_one() {
        let res = 33;
        let heights: Vec<f32> = (0..res * res)
            .map(|i| (i % res) as f32 / (res - 1) as f32)
            .collect();
        for u in [0.0, 0.3, 0.5, 1.0] {
            let s = compute_slope(&heights, res, u, 0.5, 50.0, 50.0, 50.0);
            assert!((s - 1.0).abs() < 1e-4, "u {u}: {s}");
        }
    }

    /// Done-when of #1696: a band no rule covers goes to the fallback, a
    /// layer or none, and the map counts it.
    #[test]
    fn a_band_no_rule_covers_takes_the_fallback() {
        let res = 32;
        let heights: Vec<f32> = (0..res * res)
            .map(|i| (i / res) as f32 / (res - 1) as f32)
            .collect();
        // Only the top half is painted: the sea bed below it is the band
        let mut spec = spec_with("[[splat_rules]]\nlayer = 1\nheight_min = 0.5\n");
        let mut bake = |fallback| {
            spec.splat.fallback = fallback;
            generate_splat_map(&heights, res as u32, res as u32, &spec, 3)
        };
        let low = 3 * res * 4; // a texel of row 3, height ~0.1
        let old = bake(SplatFallback::default());
        assert_eq!(
            &old.rgba[low..low + 4],
            [255, 0, 0, 0],
            "layer 0 by default"
        );
        let sand = bake(SplatFallback::Layer(3));
        assert_eq!(&sand.rgba[low..low + 4], [0, 0, 0, 255]);
        let none = bake(SplatFallback::None);
        assert_eq!(&none.rgba[low..low + 4], [0, 0, 0, 0]);
        // Rows 0..=15 are under 0.5: half the map, whichever fallback
        for map in [&old, &sand, &none] {
            assert_eq!(map.uncovered, 16 * res);
        }
        // The painted half is the same bytes in all three
        let top = 16 * res * 4;
        assert_eq!(old.rgba[top..], none.rgba[top..]);
    }

    /// The summit is exactly 1 and the lowest texel exactly 0; a rule
    /// whose band reaches them paints them.
    #[test]
    fn a_band_to_the_top_paints_the_summit() {
        let heights = [0.0, 0.5, 0.5, 1.0];
        let spec = spec_with(
            "[[splat_rules]]\nlayer = 1\nheight_max = 0.6\n\
             [[splat_rules]]\nlayer = 2\nheight_min = 0.4\n",
        );
        let map = generate_splat_map(&heights, 2, 2, &spec, 0);
        assert_eq!(map.uncovered, 0);
        assert_eq!(&map.rgba[..4], [0, 255, 0, 0], "the lowest texel");
        assert_eq!(&map.rgba[12..], [0, 0, 255, 0], "the summit");
    }

    #[test]
    fn no_rules_is_all_fallback() {
        let heights = vec![0.5; 16];
        let mut spec = spec_with("");
        let map = generate_splat_map(&heights, 4, 4, &spec, 0);
        assert_eq!(map.uncovered, 16);
        assert!(map.rgba.chunks(4).all(|t| t == [255, 0, 0, 0]));
        spec.splat.fallback = SplatFallback::None;
        let map = generate_splat_map(&heights, 4, 4, &spec, 0);
        assert!(map.rgba.iter().all(|&b| b == 0));
    }
}
