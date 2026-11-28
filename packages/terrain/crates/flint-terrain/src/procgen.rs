//! Procedural heightmap generation from terrain spec layers

use crate::spec::{
    BlendMode, ErosionKind, FalloffBlend, FalloffLayer, FalloffShape, FlattenMode, HeightLayer,
    NoiseType, TerrainSpec,
};
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
            HeightLayer::Falloff(falloff) => apply_falloff(&mut buffer, res, falloff),
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

/// Hermite smoothstep from `lo` to `hi`; a zero-wide step is 0 up to `lo`
/// and 1 past it.
fn smoothstep(lo: f64, hi: f64, x: f64) -> f64 {
    if hi <= lo {
        return if x > lo { 1.0 } else { 0.0 };
    }
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A falloff's shape at map point `(u, v)`, both 0 to 1: 1 where the land is
/// kept whole, 0 where it goes (#1694).
pub fn falloff_mask(layer: &FalloffLayer, u: f64, v: f64) -> f64 {
    let [cu, cv] = layer.center;
    let width = layer.width.max(0.0);
    let disc = || {
        let d = ((u - cu).powi(2) + (v - cv).powi(2)).sqrt();
        1.0 - smoothstep(layer.radius, layer.radius + width, d)
    };
    // 0 on the border itself, whole `width` in from it
    let border = || smoothstep(0.0, width, u.min(1.0 - u).min(v).min(1.0 - v));
    let shape = match layer.shape {
        FalloffShape::Island => disc() * border(),
        FalloffShape::Edge => border(),
        FalloffShape::Radial => disc(),
        FalloffShape::Gradient => {
            let [du, dv] = layer.direction;
            let len = (du * du + dv * dv).sqrt();
            let (du, dv) = if len > 1e-12 {
                (du / len, dv / len)
            } else {
                (0.0, -1.0)
            };
            smoothstep(0.0, width, (u - cu) * du + (v - cv) * dv)
        }
    };
    let shaped = shape.powf(layer.curve.max(0.0));
    if layer.invert {
        1.0 - shaped
    } else {
        shaped
    }
}

fn apply_falloff(buffer: &mut [f32], res: usize, layer: &FalloffLayer) {
    // The border's texels sit at exactly 0 and 1, so `edge` and `island`
    // zero them whatever the resolution
    let span = (res - 1).max(1) as f64;
    // `sink`'s floor: the lowest height so far, which normalizing makes 0
    let floor = buffer.iter().copied().fold(f32::MAX, f32::min);
    for (idx, h) in buffer.iter_mut().enumerate() {
        let u = (idx % res) as f64 / span;
        let v = (idx / res) as f64 / span;
        let m = falloff_mask(layer, u, v) as f32;
        *h = match layer.blend {
            FalloffBlend::Sink => floor + (*h - floor) * m,
            FalloffBlend::Multiply => *h * m,
            FalloffBlend::Add => *h + m * layer.amplitude as f32,
        };
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
                range: None,
            })],
            splat_rules: Vec::new(),
            splat: Default::default(),
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

    /// An island recipe: rolling noise, an island falloff, thermal erosion
    /// after it, as Relief's would be (#1694).
    fn island_spec(falloff: Option<FalloffLayer>) -> TerrainSpec {
        let mut spec = minimal_spec();
        spec.geometry.resolution = 64;
        spec.height_layers = vec![HeightLayer::Noise(NoiseLayer {
            noise_type: NoiseType::Perlin,
            octaves: 5,
            frequency: 3.0,
            lacunarity: 2.0,
            persistence: 0.5,
            amplitude: 1.0,
            blend: BlendMode::Add,
            range: None,
        })];
        spec.height_layers.extend(falloff.map(HeightLayer::Falloff));
        spec.height_layers.push(HeightLayer::Erosion(ErosionLayer {
            kind: ErosionKind::Thermal,
            iterations: 20,
            talus_angle: 0.6,
            strength: 0.3,
            ..ErosionLayer::default()
        }));
        spec
    }

    /// The highest texel on the map's border.
    fn border_max(heights: &[f32], res: usize) -> f32 {
        (0..res * res)
            .filter(|i| {
                let (x, y) = (i % res, i / res);
                x == 0 || y == 0 || x == res - 1 || y == res - 1
            })
            .map(|i| heights[i])
            .fold(f32::MIN, f32::max)
    }

    /// The level a border must stay under: a sea's.
    const SEA: f32 = 0.05;

    /// Done-when (#1694): an island falloff keeps the land clear of the
    /// border for any seed, and there is land.
    #[test]
    fn an_island_falloff_keeps_every_seeds_border_under_the_sea() {
        let spec = island_spec(Some(FalloffLayer::default()));
        for seed in 0..20 {
            let heights = generate_heightmap(&spec, seed);
            let border = border_max(&heights, 64);
            assert!(border < SEA, "seed {seed}: the border reaches {border}");
            let land = heights.iter().filter(|&&h| h > 0.3).count();
            assert!(
                land > 64 * 64 / 50,
                "seed {seed}: only {land} texels of land"
            );
        }
    }

    /// The same recipe with no falloff is the lottery the ticket describes:
    /// most seeds' land meets the border.
    #[test]
    fn without_the_falloff_most_borders_are_land() {
        let spec = island_spec(None);
        let reaching = (0..20)
            .filter(|&seed| border_max(&generate_heightmap(&spec, seed), 64) > SEA)
            .count();
        assert!(
            reaching >= 18,
            "only {reaching} of 20 borders reach the sea level"
        );
    }

    /// Wherever the disc is put, `island` still clears the border; `radial`
    /// doesn't.
    #[test]
    fn island_clears_the_border_wherever_its_center_is() {
        let corner = FalloffLayer {
            center: [0.1, 0.1],
            radius: 0.4,
            ..FalloffLayer::default()
        };
        for (u, v) in [(0.0, 0.1), (0.1, 0.0), (0.0, 0.0), (1.0, 0.5), (0.5, 1.0)] {
            assert_eq!(falloff_mask(&corner, u, v), 0.0, "({u}, {v})");
        }
        assert_eq!(falloff_mask(&corner, 0.3, 0.3), 1.0);
        let radial = FalloffLayer {
            shape: FalloffShape::Radial,
            ..corner
        };
        assert_eq!(falloff_mask(&radial, 0.0, 0.1), 1.0);
    }

    #[test]
    fn the_shapes_keep_where_they_say() {
        let edge = FalloffLayer {
            shape: FalloffShape::Edge,
            width: 0.1,
            ..FalloffLayer::default()
        };
        assert_eq!(falloff_mask(&edge, 0.0, 0.5), 0.0);
        assert_eq!(falloff_mask(&edge, 0.15, 0.9), 1.0);
        assert!((falloff_mask(&edge, 0.05, 0.5) - 0.5).abs() < 1e-9);

        // Mountains toward -Z: kept at v = 0, gone at v = 1
        let north = FalloffLayer {
            shape: FalloffShape::Gradient,
            width: 0.3,
            ..FalloffLayer::default()
        };
        assert_eq!(falloff_mask(&north, 0.5, 0.1), 1.0);
        assert_eq!(falloff_mask(&north, 0.5, 0.9), 0.0);
        let east = FalloffLayer {
            direction: [2.0, 0.0],
            ..north.clone()
        };
        assert_eq!(falloff_mask(&east, 0.9, 0.5), 1.0);
        assert_eq!(falloff_mask(&east, 0.1, 0.5), 0.0);

        // A crater: inverted, the middle goes and the rest stays
        let crater = FalloffLayer {
            shape: FalloffShape::Radial,
            invert: true,
            ..FalloffLayer::default()
        };
        assert_eq!(falloff_mask(&crater, 0.5, 0.5), 0.0);
        assert_eq!(falloff_mask(&crater, 0.0, 0.0), 1.0);

        // A steeper curve keeps less of the fade
        let soft = FalloffLayer::default();
        let steep = FalloffLayer {
            curve: 3.0,
            ..FalloffLayer::default()
        };
        assert!(falloff_mask(&steep, 0.5, 0.8) < falloff_mask(&soft, 0.5, 0.8));
    }

    /// `sink` lands the border on the floor whatever the noise's sign:
    /// noise runs -1..1, so `multiply` leaves the border mid-height wherever
    /// the land around it dips below 0.
    #[test]
    fn sink_floors_the_border_where_multiply_cant() {
        let floored = |blend| {
            let spec = island_spec(Some(FalloffLayer {
                blend,
                ..FalloffLayer::default()
            }));
            (0..20)
                .filter(|&seed| border_max(&generate_heightmap(&spec, seed), 64) < SEA)
                .count()
        };
        assert_eq!(floored(FalloffBlend::Sink), 20);
        assert!(floored(FalloffBlend::Multiply) < 20);
    }

    #[test]
    fn add_raises_the_shape() {
        let mut spec = minimal_spec();
        spec.height_layers = vec![HeightLayer::Falloff(FalloffLayer {
            shape: FalloffShape::Radial,
            blend: FalloffBlend::Add,
            ..FalloffLayer::default()
        })];
        let heights = generate_heightmap(&spec, 1);
        let res = 32;
        assert_eq!(heights[res / 2 * res + res / 2], 1.0, "the plateau");
        assert_eq!(heights[0], 0.0, "the plain");
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
