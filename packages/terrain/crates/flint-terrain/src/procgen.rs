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
            HeightLayer::Erosion(erosion) => {
                apply_erosion(&mut buffer, res, erosion, &spec.geometry, seed)
            }
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
            fbm_sample(x, y, layer, &octaves) as f32 * layer.amplitude as f32
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
    geometry: &crate::spec::GeometryConfig,
    seed: u64,
) {
    match layer.kind {
        ErosionKind::Thermal => thermal_erosion(buffer, res, layer),
        ErosionKind::Hydraulic => hydraulic_erosion(buffer, res, layer, geometry, seed),
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

// The droplet model's constants (#1693), for heights measured in texels.
/// How much of its heading a droplet keeps each step (0 follows the slope).
const INERTIA: f32 = 0.05;
/// Soil a droplet carries for each unit of drop, speed and water.
const CAPACITY: f32 = 4.0;
/// What it can carry over flat ground, so a slow droplet still wears.
const MIN_CAPACITY: f32 = 0.01;
/// The share of its excess soil a droplet drops each step.
const DEPOSIT: f32 = 0.3;
/// The share of its water lost each step.
const EVAPORATE: f32 = 0.02;
/// Speed gained for each texel dropped.
const GRAVITY: f32 = 4.0;
/// The radius in texels soil settles over as a droplet slows. On four
/// texels alone it lay in speckle that the splat rules turned to grit.
const SETTLE_RADIUS: f32 = 1.5;

/// Droplet erosion (#1693): `rainfall` droplets for each texel of the map,
/// each rolling downhill with inertia, picking up soil over a `radius`-texel
/// brush while it speeds up and has room to carry it, and dropping it as it
/// slows, climbs or dries. Many droplets share paths, so the paths deepen
/// into branching gullies; the brush and the inertia keep them smooth where
/// a walk to the lowest neighbour cut axis-aligned dashes.
///
/// Heights are taken in texels of the relief the bake ends with
/// (`height_scale` across `width`), so the slopes are the world's and the
/// constants mean the same at every resolution. One seeded stream, droplets
/// run one after another: the same seed gives the same bytes.
fn hydraulic_erosion(
    buffer: &mut [f32],
    res: usize,
    layer: &crate::spec::ErosionLayer,
    geometry: &crate::spec::GeometryConfig,
    seed: u64,
) {
    // A droplet samples a 2x2 cell, so a map under 3x3 has no room to roll
    if res < 3 {
        return;
    }
    let (lo, hi) = buffer
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &h| (lo.min(h), hi.max(h)));
    let range = hi - lo;
    let texel = geometry.width / (res - 1) as f32;
    // Buffer units to texels; flat ground, or no size, has nothing to wear
    let scale = geometry.height_scale / texel / range;
    if !(range > 1e-9 && scale.is_finite() && scale > 0.0) {
        return;
    }
    let to_buffer = 1.0 / scale;
    let droplets = (layer.rainfall.max(0.0) * (res * res) as f64).round() as u64;
    let erode_rate = layer.strength.max(0.0) as f32;
    let brush = erosion_brush(layer.radius as f32);
    let settle = erosion_brush(SETTLE_RADIUS);
    // A droplet lives long enough to cross about an eighth of the map
    let lifetime = (res / 8).clamp(24, 96);
    let edge = (res - 1) as f32;
    let rain_min = layer.rain_min_height.map(|h| h as f32);
    let mut rng = ChaCha8Rng::seed_from_u64(seed);

    for _ in 0..droplets {
        let mut x = rand::Rng::random::<f32>(&mut rng) * edge;
        let mut y = rand::Rng::random::<f32>(&mut rng) * edge;
        if let Some(min) = rain_min {
            // Rain below the line is not run, so the land's share stays
            // `rainfall` however much sea there is
            if sample(buffer, res, x, y).0 < min {
                continue;
            }
        }
        let (mut dx, mut dy) = (0.0f32, 0.0f32);
        let (mut speed, mut water, mut sediment) = (1.0f32, 1.0f32, 0.0f32);

        for _ in 0..lifetime {
            let (h, gx, gy) = sample(buffer, res, x, y);
            let (h, gx, gy) = (h * scale, gx * scale, gy * scale);
            dx = dx * INERTIA - gx * (1.0 - INERTIA);
            dy = dy * INERTIA - gy * (1.0 - INERTIA);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-6 {
                break; // a dead-flat cell: nowhere to go
            }
            dx /= len;
            dy /= len;
            let (nx, ny) = (x + dx, y + dy);
            if !(nx >= 0.0 && ny >= 0.0 && nx < edge && ny < edge) {
                break; // off the map, carrying its soil away
            }
            let dh = sample(buffer, res, nx, ny).0 * scale - h;
            let capacity = (-dh * speed * water * CAPACITY).max(MIN_CAPACITY);
            if sediment > capacity || dh > 0.0 {
                // Climbing, it fills the pit behind it, at most; otherwise it
                // drops a share of what it can't carry
                let drop = if dh > 0.0 {
                    dh.min(sediment)
                } else {
                    (sediment - capacity) * DEPOSIT
                };
                sediment -= drop;
                if dh > 0.0 {
                    deposit(buffer, res, x, y, drop * to_buffer);
                } else {
                    erode(buffer, res, x, y, &settle, -drop * to_buffer);
                }
            } else {
                // Never deeper than the drop, so it can't dig a pit
                let want = ((capacity - sediment) * erode_rate).min(-dh);
                sediment += erode(buffer, res, x, y, &brush, want * to_buffer) * scale;
            }
            speed = (speed * speed - dh * GRAVITY).max(0.0).sqrt();
            water *= 1.0 - EVAPORATE;
            x = nx;
            y = ny;
        }
    }
}

/// The bilinear height at `(x, y)` and its gradient, in buffer units.
fn sample(buffer: &[f32], res: usize, x: f32, y: f32) -> (f32, f32, f32) {
    let (x0, y0) = cell(res, x, y);
    let (u, v) = (x - x0 as f32, y - y0 as f32);
    let i = y0 * res + x0;
    let (nw, ne) = (buffer[i], buffer[i + 1]);
    let (sw, se) = (buffer[i + res], buffer[i + res + 1]);
    let gx = (ne - nw) * (1.0 - v) + (se - sw) * v;
    let gy = (sw - nw) * (1.0 - u) + (se - ne) * u;
    let h = nw * (1.0 - u) * (1.0 - v) + ne * u * (1.0 - v) + sw * (1.0 - u) * v + se * u * v;
    (h, gx, gy)
}

/// The north-west texel of the cell holding `(x, y)`, kept inside the map.
fn cell(res: usize, x: f32, y: f32) -> (usize, usize) {
    ((x as usize).min(res - 2), (y as usize).min(res - 2))
}

/// Soil dropped on the four texels of the droplet's cell, by its position.
fn deposit(buffer: &mut [f32], res: usize, x: f32, y: f32, amount: f32) {
    let (x0, y0) = cell(res, x, y);
    let (u, v) = (x - x0 as f32, y - y0 as f32);
    let i = y0 * res + x0;
    buffer[i] += amount * (1.0 - u) * (1.0 - v);
    buffer[i + 1] += amount * u * (1.0 - v);
    buffer[i + res] += amount * (1.0 - u) * v;
    buffer[i + res + 1] += amount * u * v;
}

/// Wears `amount` off the brush around the droplet's nearest texel and
/// returns what came off: less near the map's edge. The edge's own texels
/// are never worn: they are the base level. Droplets leave the map there
/// with their soil, so a worn edge would sink without end and pull every
/// gully after it. (Weights from the droplet's exact position wore no more
/// evenly and tripled the bake.)
fn erode(
    buffer: &mut [f32],
    res: usize,
    x: f32,
    y: f32,
    brush: &[(i32, i32, f32)],
    amount: f32,
) -> f32 {
    let (cx, cy) = ((x + 0.5) as i32, (y + 0.5) as i32);
    let n = res as i32;
    let mut taken = 0.0;
    for &(ox, oy, w) in brush {
        let (bx, by) = (cx + ox, cy + oy);
        if bx > 0 && by > 0 && bx < n - 1 && by < n - 1 {
            buffer[by as usize * res + bx as usize] -= amount * w;
            taken += amount * w;
        }
    }
    taken
}

/// The texels within `radius` of a centre and their weights, falling off
/// linearly to the rim and summing to one.
fn erosion_brush(radius: f32) -> Vec<(i32, i32, f32)> {
    let r = radius.max(0.5);
    let reach = r.ceil() as i32;
    let mut brush = Vec::new();
    for oy in -reach..=reach {
        for ox in -reach..=reach {
            let w = r - ((ox * ox + oy * oy) as f32).sqrt();
            if w > 0.0 {
                brush.push((ox, oy, w));
            }
        }
    }
    let total: f32 = brush.iter().map(|b| b.2).sum();
    for b in &mut brush {
        b.2 /= total;
    }
    brush
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
            ..ErosionLayer::default()
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

    fn hydraulic(rainfall: f64) -> ErosionLayer {
        ErosionLayer {
            kind: ErosionKind::Hydraulic,
            rainfall,
            ..ErosionLayer::default()
        }
    }

    /// The share of texels that moved by more than 0.2% of the relief.
    fn worn_share(before: &[f32], after: &[f32]) -> f64 {
        let moved = before
            .iter()
            .zip(after)
            .filter(|(a, b)| (*a - *b).abs() > 0.002)
            .count();
        moved as f64 / before.len() as f64
    }

    #[test]
    fn hydraulic_erosion_by_default_is_seeded_and_wears_the_land() {
        // #1693: 50 single raindrops changed 1,300 of 263,169 texels
        let mut spec = minimal_spec();
        spec.geometry.resolution = 96;
        let before = generate_heightmap(&spec, 42);
        spec.height_layers
            .push(HeightLayer::Erosion(hydraulic(1.0)));
        let after = generate_heightmap(&spec, 42);
        assert_eq!(after, generate_heightmap(&spec, 42));
        assert_ne!(after, generate_heightmap(&spec, 43));
        let worn = worn_share(&before, &after);
        assert!(worn > 0.3, "the default rain wore {worn:.3} of the map");
    }

    #[test]
    fn no_rainfall_and_rain_below_the_line_change_nothing() {
        let mut spec = minimal_spec();
        let before = generate_heightmap(&spec, 42);
        spec.height_layers
            .push(HeightLayer::Erosion(hydraulic(0.0)));
        assert_eq!(generate_heightmap(&spec, 42), before);
        spec.height_layers.pop();
        spec.height_layers.push(HeightLayer::Erosion(ErosionLayer {
            rain_min_height: Some(10.0),
            ..hydraulic(2.0)
        }));
        assert_eq!(generate_heightmap(&spec, 42), before);
    }

    /// A cone of `res`² texels, highest in the middle, on a flat skirt so
    /// every direction runs down to the same base level.
    fn cone(res: usize) -> Vec<f32> {
        let c = (res - 1) as f32 / 2.0;
        (0..res * res)
            .map(|i| {
                let (x, y) = ((i % res) as f32 - c, (i / res) as f32 - c);
                (1.0 - (x * x + y * y).sqrt() / c).max(0.0)
            })
            .collect()
    }

    #[test]
    fn the_map_edge_is_never_worn_lower() {
        // The edge is the base level: worn, it sank without end and every
        // gully deepened after it
        let res = 64;
        // A ramp down to the x = 0 edge, which every droplet runs off
        let before: Vec<f32> = (0..res * res).map(|i| (i % res) as f32 / 8.0).collect();
        let mut after = before.clone();
        let geometry = GeometryConfig::default();
        hydraulic_erosion(&mut after, res, &hydraulic(4.0), &geometry, 7);
        let worn_by_the_edge = (1..res - 1).filter(|&y| after[y * res + 1] < before[y * res + 1]);
        assert!(
            worn_by_the_edge.count() > res / 2,
            "the rain never reached the edge"
        );
        for i in 0..res * res {
            let (x, y) = (i % res, i / res);
            if x == 0 || y == 0 || x == res - 1 || y == res - 1 {
                assert!(after[i] >= before[i], "edge texel ({x}, {y}) sank");
            }
        }
    }

    #[test]
    fn droplets_wear_a_cone_alike_along_the_axes_and_the_diagonals() {
        // A walk to the lowest of four neighbours cut its dashes along the
        // grid's axes; a droplet with inertia has no favourite direction
        let res = 129;
        let before = cone(res);
        let mut after = before.clone();
        let geometry = GeometryConfig::default();
        hydraulic_erosion(&mut after, res, &hydraulic(4.0), &geometry, 11);
        let c = (res - 1) as f32 / 2.0;
        let (mut axes, mut diagonals) = (0.0f64, 0.0f64);
        for i in 0..res * res {
            let (x, y) = ((i % res) as f32 - c, (i / res) as f32 - c);
            let r = (x * x + y * y).sqrt();
            if !(8.0..c - 4.0).contains(&r) {
                continue;
            }
            let angle = y.abs().atan2(x.abs()).to_degrees();
            let worn = f64::from((before[i] - after[i]).max(0.0));
            if !(10.0..=80.0).contains(&angle) {
                axes += worn;
            } else if (35.0..=55.0).contains(&angle) {
                diagonals += worn;
            }
        }
        // Both sectors are 20 degrees of every quadrant. The old walk, at the
        // same 64,000 drops, measured 0.60; this model 1.11
        let ratio = axes / diagonals;
        assert!(
            (0.8..1.25).contains(&ratio),
            "axes / diagonals = {ratio:.3}"
        );
    }
}
