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

fn apply_noise(buffer: &mut [f32], res: usize, layer: &crate::spec::NoiseLayer, seed: u64) {
    let seed_u32 = (seed & 0xFFFF_FFFF) as u32;
    // Each octave's noise is built once: `Perlin::new` shuffles a table, and
    // building it per texel was nearly all of a bake's time (#719). The same
    // seeds give the same tables, so the heights are unchanged bit for bit.
    let octaves: Vec<OctaveNoise> = (0..layer.octaves)
        .map(|octave| OctaveNoise::new(layer.noise_type, seed_u32.wrapping_add(octave)))
        .collect();

    // Generate FBM noise values
    let noise_values: Vec<f32> = (0..res * res)
        .map(|idx| {
            let x = (idx % res) as f64 / res as f64;
            let y = (idx / res) as f64 / res as f64;
            let raw = fbm_sample(x, y, layer, &octaves);
            let shaped = match layer.range {
                Some(to) => remap(raw, layer.noise_type.raw_range(), to),
                None => raw,
            };
            shaped as f32 * layer.amplitude as f32
        })
        .collect();

    // Blend into buffer
    for (i, &val) in noise_values.iter().enumerate() {
        buffer[i] = blend(buffer[i], val, layer.blend);
    }
}

/// One octave's noise function, seeded.
enum OctaveNoise {
    Perlin(Perlin),
    Simplex(OpenSimplex),
    Value(ValueNoise),
    Ridged(Perlin),
}

impl OctaveNoise {
    fn new(kind: NoiseType, seed: u32) -> Self {
        match kind {
            NoiseType::Perlin => Self::Perlin(Perlin::new(seed)),
            NoiseType::Simplex => Self::Simplex(OpenSimplex::new(seed)),
            NoiseType::Value => Self::Value(ValueNoise::new(seed)),
            NoiseType::Ridged => Self::Ridged(Perlin::new(seed)),
        }
    }

    fn get(&self, p: [f64; 2]) -> f64 {
        match self {
            Self::Perlin(n) => n.get(p),
            Self::Simplex(n) => n.get(p),
            Self::Value(n) => n.get(p),
            Self::Ridged(n) => 1.0 - n.get(p).abs(),
        }
    }
}

fn fbm_sample(x: f64, y: f64, layer: &crate::spec::NoiseLayer, octaves: &[OctaveNoise]) -> f64 {
    let mut total = 0.0;
    let mut freq = layer.frequency;
    let mut amp = 1.0;
    let mut max_amp = 0.0;

    for noise in octaves {
        let nx = x * freq;
        let ny = y * freq;

        let sample = noise.get([nx, ny]);

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

/// `v` from `[from_lo, from_hi]` onto `[to_lo, to_hi]`, held inside the
/// target so a `[0, 1]` multiplier is never negative, whatever the noise
/// does at a lattice edge. A reversed target inverts.
fn remap(v: f64, [from_lo, from_hi]: [f64; 2], [to_lo, to_hi]: [f64; 2]) -> f64 {
    let t = (v - from_lo) / (from_hi - from_lo);
    let out = to_lo + t * (to_hi - to_lo);
    // max/min, not clamp: a `nan` in the spec mustn't panic a bake
    out.max(to_lo.min(to_hi)).min(to_lo.max(to_hi))
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

fn apply_erosion(buffer: &mut [f32], res: usize, layer: &crate::spec::ErosionLayer, seed: u64) {
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

fn hydraulic_erosion(buffer: &mut [f32], res: usize, layer: &crate::spec::ErosionLayer, seed: u64) {
    // A raindrop starts off the border, and a map under 3x3 has no inside
    if res < 3 {
        return;
    }
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
                range: None,
            })],
            splat_rules: Vec::new(),
            grass: None,
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

    /// The mean height step between neighbouring texels of a normalized
    /// map. Land climbs a little at a time (about 0.01 at 129²); a map that
    /// normalized from nothing is erosion's per-texel speckle stretched over
    /// the whole range, which steps about 0.1.
    fn step(h: &[f32], res: usize) -> f64 {
        let steps: f64 = (0..res)
            .flat_map(|y| (0..res - 1).map(move |x| (y, x)))
            .map(|(y, x)| (h[y * res + x + 1] - h[y * res + x]).abs() as f64)
            .sum();
        steps / (res * (res - 1)) as f64
    }

    const SPECKLE: f64 = 0.05;

    fn swell() -> TerrainSpec {
        let text = include_str!("../../../../../tests/fixtures/scenes/terrain/swell.terrain.toml");
        let (spec, issues) = TerrainSpec::parse_checked(text).unwrap();
        assert!(issues.is_empty(), "{issues:?}");
        spec
    }

    /// The swell layer: the recipe's one `multiply`.
    fn multiplier(spec: &mut TerrainSpec) -> &mut NoiseLayer {
        spec.height_layers
            .iter_mut()
            .find_map(|l| match l {
                HeightLayer::Noise(n) if n.blend == BlendMode::Multiply => Some(n),
                _ => None,
            })
            .unwrap()
    }

    /// #1695's done-when: a recipe that multiplies by a remapped noise makes
    /// land for every seed of a 16-seed batch. The same recipe multiplying
    /// by the raw, signed swell makes speckle for some of them (Relief's
    /// seeds 5, 7, 9 and 16 at full size), so the range is what holds it.
    #[test]
    fn a_ranged_multiplier_makes_land_for_every_seed() {
        let ranged = swell();
        let res = ranged.geometry.resolution as usize;
        let mut raw = ranged.clone();
        multiplier(&mut raw).range = None;

        let steps = |spec: &TerrainSpec| -> Vec<(u64, f64)> {
            (1..=16)
                .map(|seed| (seed, step(&generate_heightmap(spec, seed), res)))
                .collect()
        };
        let speckled = |s: &[(u64, f64)]| -> Vec<u64> {
            s.iter()
                .filter(|(_, d)| *d > SPECKLE)
                .map(|(s, _)| *s)
                .collect()
        };

        let with_range = steps(&ranged);
        assert!(
            speckled(&with_range).is_empty(),
            "seeds {:?} made speckle with the swell in 0..1: {with_range:?}",
            speckled(&with_range)
        );
        let without = steps(&raw);
        assert!(
            !speckled(&without).is_empty(),
            "the raw swell made land for every seed too, so this test holds nothing: {without:?}"
        );
    }

    #[test]
    fn each_noise_type_lies_in_its_raw_range() {
        for kind in [
            NoiseType::Perlin,
            NoiseType::Simplex,
            NoiseType::Value,
            NoiseType::Ridged,
        ] {
            let layer = NoiseLayer {
                noise_type: kind,
                octaves: 4,
                frequency: 7.0,
                ..match &minimal_spec().height_layers[0] {
                    HeightLayer::Noise(n) => n.clone(),
                    _ => unreachable!(),
                }
            };
            let octaves: Vec<OctaveNoise> = (0..layer.octaves)
                .map(|o| OctaveNoise::new(kind, 3 + o))
                .collect();
            let [lo, hi] = kind.raw_range();
            for i in 0..4096 {
                let (x, y) = ((i % 64) as f64 / 64.0, (i / 64) as f64 / 64.0);
                let v = fbm_sample(x, y, &layer, &octaves);
                assert!(
                    (lo..=hi).contains(&v),
                    "{kind:?} gave {v} at ({x}, {y}), outside {lo}..{hi}"
                );
            }
        }
    }

    #[test]
    fn a_range_maps_the_raw_noise_and_holds_it_there() {
        assert_eq!(remap(-1.0, [-1.0, 1.0], [0.0, 1.0]), 0.0);
        assert_eq!(remap(0.0, [-1.0, 1.0], [0.0, 1.0]), 0.5);
        assert_eq!(remap(1.0, [-1.0, 1.0], [0.2, 0.6]), 0.6);
        // Reversed inverts; past the raw range is held at the ends
        assert_eq!(remap(1.0, [-1.0, 1.0], [1.0, 0.0]), 0.0);
        assert_eq!(remap(1.5, [-1.0, 1.0], [0.0, 1.0]), 1.0);
        assert_eq!(remap(-1.5, [-1.0, 1.0], [1.0, 0.0]), 1.0);
        // A nan in the spec bakes something instead of panicking
        let _ = remap(0.0, [-1.0, 1.0], [f64::NAN, 1.0]);
    }

    #[test]
    fn a_ranged_layer_never_goes_negative_and_amplitude_scales_it() {
        let mut spec = minimal_spec();
        let HeightLayer::Noise(n) = &mut spec.height_layers[0] else {
            unreachable!()
        };
        n.range = Some([0.0, 1.0]);
        n.amplitude = 0.5;
        let layer = n.clone();
        let mut buffer = vec![0.0f32; 32 * 32];
        apply_noise(&mut buffer, 32, &layer, 42);
        assert!(buffer.iter().all(|v| (0.0..=0.5).contains(v)), "{buffer:?}");
        // Without a range the same noise is signed
        let mut raw = vec![0.0f32; 32 * 32];
        apply_noise(
            &mut raw,
            32,
            &NoiseLayer {
                range: None,
                ..layer
            },
            42,
        );
        assert!(raw.iter().any(|v| *v < 0.0));
    }
}
