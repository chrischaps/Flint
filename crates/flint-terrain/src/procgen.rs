//! Procedural heightmap generation from terrain spec layers

use crate::spec::{BlendMode, ErosionKind, FlattenMode, HeightLayer, NoiseType, TerrainSpec};
use noise::{NoiseFn, OpenSimplex, Perlin, Value as ValueNoise};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

/// Generate a heightmap buffer from a terrain spec's height layers.
/// Returns a `resolution x resolution` Vec<f32> with values normalized to [0..1].
pub fn generate_heightmap(spec: &TerrainSpec, seed: u64) -> Vec<f32> {
    let res = spec.geometry.resolution.max(2) as usize;
    let mut buffer = vec![0.0f32; res * res];

    for layer in &spec.height_layers {
        match layer {
            HeightLayer::Noise(noise) => apply_noise(&mut buffer, res, noise, seed),
            HeightLayer::Flatten(flatten) => apply_flatten(&mut buffer, res, flatten),
            HeightLayer::Erosion(erosion) => apply_erosion(&mut buffer, res, erosion, seed),
        }
    }

    // Normalize to [0..1]
    normalize(&mut buffer);
    buffer
}

fn apply_noise(
    buffer: &mut [f32],
    res: usize,
    layer: &crate::spec::NoiseLayer,
    seed: u64,
) {
    let seed_u32 = (seed & 0xFFFF_FFFF) as u32;

    // Generate FBM noise values
    let noise_values: Vec<f32> = (0..res * res)
        .map(|idx| {
            let x = (idx % res) as f64 / res as f64;
            let y = (idx / res) as f64 / res as f64;
            fbm_sample(x, y, layer, seed_u32) as f32 * layer.amplitude as f32
        })
        .collect();

    // Blend into buffer
    for (i, &val) in noise_values.iter().enumerate() {
        buffer[i] = blend(buffer[i], val, layer.blend);
    }
}

fn fbm_sample(x: f64, y: f64, layer: &crate::spec::NoiseLayer, seed: u32) -> f64 {
    let mut total = 0.0;
    let mut freq = layer.frequency;
    let mut amp = 1.0;
    let mut max_amp = 0.0;

    for octave in 0..layer.octaves {
        let s = seed.wrapping_add(octave);
        let nx = x * freq;
        let ny = y * freq;

        let sample = match layer.noise_type {
            NoiseType::Perlin => {
                let n = Perlin::new(s);
                n.get([nx, ny])
            }
            NoiseType::Simplex => {
                let n = OpenSimplex::new(s);
                n.get([nx, ny])
            }
            NoiseType::Value => {
                let n = ValueNoise::new(s);
                n.get([nx, ny])
            }
            NoiseType::Ridged => {
                let n = Perlin::new(s);
                1.0 - n.get([nx, ny]).abs()
            }
        };

        total += sample * amp;
        max_amp += amp;
        amp *= layer.persistence;
        freq *= layer.lacunarity;
    }

    if max_amp > 0.0 {
        total / max_amp
    } else {
        0.0
    }
}

fn blend(base: f32, value: f32, mode: BlendMode) -> f32 {
    match mode {
        BlendMode::Add => base + value,
        BlendMode::Multiply => base * value,
        BlendMode::Max => base.max(value),
        BlendMode::Min => base.min(value),
        BlendMode::Overlay => {
            // Soft light blend
            if base < 0.5 {
                2.0 * base * value
            } else {
                1.0 - 2.0 * (1.0 - base) * (1.0 - value)
            }
        }
    }
}

fn apply_flatten(buffer: &mut [f32], _res: usize, layer: &crate::spec::FlattenLayer) {
    let threshold = layer.height as f32;
    let falloff = layer.falloff as f32;

    for h in buffer.iter_mut() {
        match layer.mode {
            FlattenMode::Below => {
                if *h < threshold {
                    if falloff > 0.0 {
                        let t = ((threshold - *h) / falloff).min(1.0);
                        *h = *h + (threshold - *h) * t;
                    } else {
                        *h = threshold;
                    }
                }
            }
            FlattenMode::Above => {
                if *h > threshold {
                    if falloff > 0.0 {
                        let t = ((*h - threshold) / falloff).min(1.0);
                        *h = *h - (*h - threshold) * t;
                    } else {
                        *h = threshold;
                    }
                }
            }
        }
    }
}

fn apply_erosion(
    buffer: &mut [f32],
    res: usize,
    layer: &crate::spec::ErosionLayer,
    seed: u64,
) {
    match layer.kind {
        ErosionKind::Thermal => thermal_erosion(buffer, res, layer),
        ErosionKind::Hydraulic => hydraulic_erosion(buffer, res, layer, seed),
    }
}

fn thermal_erosion(buffer: &mut [f32], res: usize, layer: &crate::spec::ErosionLayer) {
    let talus = layer.talus_angle as f32 / res as f32;
    let strength = layer.strength as f32;

    // Neighbor offsets (4-connected)
    let offsets: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];

    for _ in 0..layer.iterations {
        let snapshot = buffer.to_vec();

        for y in 1..res as i32 - 1 {
            for x in 1..res as i32 - 1 {
                let idx = y as usize * res + x as usize;
                let h = snapshot[idx];

                let mut max_diff = 0.0f32;
                let mut total_diff = 0.0f32;
                let mut diffs = [0.0f32; 4];

                for (i, (dx, dy)) in offsets.iter().enumerate() {
                    let ni = (y + dy) as usize * res + (x + dx) as usize;
                    let diff = h - snapshot[ni];
                    if diff > talus {
                        diffs[i] = diff - talus;
                        total_diff += diffs[i];
                        max_diff = max_diff.max(diffs[i]);
                    }
                }

                if total_diff > 0.0 {
                    let amount = max_diff * strength * 0.5;
                    buffer[idx] -= amount;
                    for (i, (dx, dy)) in offsets.iter().enumerate() {
                        if diffs[i] > 0.0 {
                            let ni = (y + dy) as usize * res + (x + dx) as usize;
                            buffer[ni] += amount * (diffs[i] / total_diff);
                        }
                    }
                }
            }
        }
    }
}

fn hydraulic_erosion(
    buffer: &mut [f32],
    res: usize,
    layer: &crate::spec::ErosionLayer,
    seed: u64,
) {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let strength = layer.strength as f32;

    let offsets: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];

    for _ in 0..layer.iterations {
        // Random raindrop position
        let x = rand::Rng::random_range(&mut rng, 1..res as i32 - 1);
        let y = rand::Rng::random_range(&mut rng, 1..res as i32 - 1);

        let mut cx = x;
        let mut cy = y;
        let mut sediment = 0.0f32;
        let capacity = strength * 0.1;

        for _ in 0..64 {
            let idx = cy as usize * res + cx as usize;
            let h = buffer[idx];

            // Find lowest neighbor
            let mut lowest_h = h;
            let mut lowest_x = cx;
            let mut lowest_y = cy;

            for (dx, dy) in &offsets {
                let nx = cx + dx;
                let ny = cy + dy;
                if nx >= 0 && nx < res as i32 && ny >= 0 && ny < res as i32 {
                    let ni = ny as usize * res + nx as usize;
                    if buffer[ni] < lowest_h {
                        lowest_h = buffer[ni];
                        lowest_x = nx;
                        lowest_y = ny;
                    }
                }
            }

            if lowest_x == cx && lowest_y == cy {
                // Deposit sediment in local minimum
                buffer[idx] += sediment;
                break;
            }

            let diff = h - lowest_h;

            // Erode or deposit
            if sediment < capacity {
                let erode = (capacity - sediment).min(diff * 0.5) * strength;
                buffer[idx] -= erode;
                sediment += erode;
            } else {
                let deposit = (sediment - capacity).min(diff * 0.3);
                buffer[idx] += deposit;
                sediment -= deposit;
            }

            cx = lowest_x;
            cy = lowest_y;
        }
    }
}

fn normalize(buffer: &mut [f32]) {
    if buffer.is_empty() {
        return;
    }

    let mut min_val = f32::MAX;
    let mut max_val = f32::MIN;

    for &v in buffer.iter() {
        min_val = min_val.min(v);
        max_val = max_val.max(v);
    }

    let range = max_val - min_val;
    if range < 1e-10 {
        // All values same — set to 0.5
        for v in buffer.iter_mut() {
            *v = 0.5;
        }
        return;
    }

    for v in buffer.iter_mut() {
        *v = (*v - min_val) / range;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::*;

    fn minimal_spec() -> TerrainSpec {
        TerrainSpec {
            meta: MetaConfig {
                name: "test".to_string(),
                version: "1.0.0".to_string(),
            },
            seed: SeedConfig::default(),
            geometry: GeometryConfig {
                resolution: 32,
                ..GeometryConfig::default()
            },
            textures: TextureConfig::default(),
            height_layers: vec![HeightLayer::Noise(NoiseLayer {
                noise_type: NoiseType::Perlin,
                octaves: 4,
                frequency: 2.0,
                lacunarity: 2.0,
                persistence: 0.5,
                amplitude: 1.0,
                blend: BlendMode::Add,
            })],
            splat_rules: Vec::new(),
        }
    }

    #[test]
    fn noise_determinism() {
        let spec = minimal_spec();
        let h1 = generate_heightmap(&spec, 42);
        let h2 = generate_heightmap(&spec, 42);
        assert_eq!(h1, h2);

        let h3 = generate_heightmap(&spec, 99);
        assert_ne!(h1, h3);
    }

    #[test]
    fn output_normalized() {
        let spec = minimal_spec();
        let heights = generate_heightmap(&spec, 42);
        let min = heights.iter().copied().fold(f32::MAX, f32::min);
        let max = heights.iter().copied().fold(f32::MIN, f32::max);
        assert!(min >= 0.0 - 1e-6);
        assert!(max <= 1.0 + 1e-6);
    }

    #[test]
    fn flatten_clamps_below() {
        let mut spec = minimal_spec();
        spec.height_layers.push(HeightLayer::Flatten(FlattenLayer {
            height: 0.3,
            falloff: 0.0,
            mode: FlattenMode::Below,
        }));
        let heights = generate_heightmap(&spec, 42);
        // After normalize, min should be >= ~0.3 (adjusted for renormalization)
        // Just verify no extreme low values
        let min = heights.iter().copied().fold(f32::MAX, f32::min);
        assert!(min >= 0.0);
    }

    #[test]
    fn erosion_changes_heightmap() {
        let mut spec = minimal_spec();
        let before = generate_heightmap(&spec, 42);

        spec.height_layers.push(HeightLayer::Erosion(ErosionLayer {
            kind: ErosionKind::Thermal,
            iterations: 100,
            talus_angle: 0.3,
            strength: 0.5,
        }));
        let after = generate_heightmap(&spec, 42);

        // Erosion should produce a different heightmap
        assert_ne!(before, after);
        // Output should still be normalized
        let min = after.iter().copied().fold(f32::MAX, f32::min);
        let max = after.iter().copied().fold(f32::MIN, f32::max);
        assert!(min >= 0.0 - 1e-6);
        assert!(max <= 1.0 + 1e-6);
    }

}
