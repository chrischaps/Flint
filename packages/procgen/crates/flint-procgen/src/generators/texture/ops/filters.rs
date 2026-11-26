//! Spatial filters: ops that read a pixel's neighbours (`derive_normal`,
//! `blur`, `sharpen`, `edge_detect`, `edge_erode`).

use crate::algorithms::noise::{
    Fbm, NoiseSource, PerlinNoise, SimplexNoise, ValueNoise, WorleyNoise,
};
use crate::SeededRng;

use super::patterns::{NoiseType, NOISE_TYPES};
use super::TextureOp;
use crate::generators::texture::descriptor::{OpDescriptor, ParamDesc as P};
use crate::generators::texture::field::TextureField;

// ─── DeriveNormalOp ───────────────────────────────────────────────────────

/// Derive a tangent-space normal map from the `height` channel.
///
/// Reads `height`, writes `normal_x`, `normal_y`, `normal_z`.
pub struct DeriveNormalOp {
    /// Normal map strength multiplier.
    pub strength: f32,
}

/// The [`DeriveNormalOp`] op, as the pipeline knows it.
pub const DERIVE_NORMAL: OpDescriptor = OpDescriptor {
    type_name: "derive_normal",
    label: "Derive Normal",
    summary: "Derive the tangent-space normal map from the height channel.",
    reads: &["height"],
    writes: &["normal_x", "normal_y", "normal_z"],
    modifies: &[],
    params: &[P::float("strength", 1.0)
        .min(0.0)
        .help("Depth of the relief; 0 is flat")],
    build: |p| {
        Ok(Box::new(DeriveNormalOp {
            strength: p.f32("strength")?,
        }))
    },
};

impl TextureOp for DeriveNormalOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &DERIVE_NORMAL
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let wi = w as i32;
        let hi = h as i32;
        field.ensure_channel("normal_x");
        field.ensure_channel("normal_y");
        field.ensure_channel("normal_z");

        // Snapshot heights to avoid borrow conflicts during write
        let heights: Vec<f32> = (0..(w * h))
            .map(|i| field.get("height", i % w, i / w))
            .collect();

        for y in 0..h {
            for x in 0..w {
                let ix = x as i32;
                let iy = y as i32;

                let sample = |sx: i32, sy: i32| -> f32 {
                    let wx = ((sx % wi) + wi) % wi;
                    let wy = ((sy % hi) + hi) % hi;
                    heights[(wy * wi + wx) as usize]
                };

                let h_left = sample(ix - 1, iy);
                let h_right = sample(ix + 1, iy);
                let h_up = sample(ix, iy - 1);
                let h_down = sample(ix, iy + 1);

                let dx = (h_right - h_left) * self.strength;
                let dy = (h_down - h_up) * self.strength;

                let nx = -dx;
                let ny = -dy;
                let nz = 1.0_f32;
                let len = (nx * nx + ny * ny + nz * nz).sqrt();

                field.set("normal_x", x, y, nx / len);
                field.set("normal_y", x, y, ny / len);
                field.set("normal_z", x, y, nz / len);
            }
        }
    }
}

/// Separable 1D Gaussian blur pass (horizontal or vertical).
///
/// Wraps at edges for seamless texture support.
fn gaussian_blur_1d(
    input: &[f32],
    output: &mut [f32],
    w: u32,
    h: u32,
    radius: u32,
    sigma: f32,
    horizontal: bool,
) {
    if radius == 0 || sigma <= 0.0 {
        output.copy_from_slice(input);
        return;
    }

    // Build kernel
    let r = radius as i32;
    let mut kernel = Vec::with_capacity((2 * r + 1) as usize);
    let mut sum = 0.0_f32;
    for i in -r..=r {
        let w = (-0.5 * (i as f32 / sigma).powi(2)).exp();
        kernel.push(w);
        sum += w;
    }
    // Normalize
    for k in &mut kernel {
        *k /= sum;
    }

    let wi = w as i32;
    let hi = h as i32;

    for y in 0..h {
        for x in 0..w {
            let mut val = 0.0_f32;
            for (ki, i) in (-r..=r).enumerate() {
                let (sx, sy) = if horizontal {
                    (((x as i32 + i) % wi + wi) % wi, y as i32)
                } else {
                    (x as i32, ((y as i32 + i) % hi + hi) % hi)
                };
                val += input[(sy * wi + sx) as usize] * kernel[ki];
            }
            output[(y * w + x) as usize] = val;
        }
    }
}

// ─── BlurOp ──────────────────────────────────────────────────────────────

/// Gaussian blur (separable two-pass).
///
/// Reads `<input>`, writes `<output>`.
pub struct BlurOp {
    pub input: String,
    pub output: String,
    pub radius: u32,
    pub sigma: f32,
}

/// The [`BlurOp`] op, as the pipeline knows it.
pub const BLUR: OpDescriptor = OpDescriptor {
    type_name: "blur",
    label: "Blur",
    summary: "Gaussian-blur a channel (two separable passes, wrapping at the edges).",
    reads: &["<input>"],
    writes: &["<output>"],
    modifies: &[],
    params: &[
        P::channel("input", "input").help("Channel to blur"),
        P::channel("output", "output").help("Channel the result goes to"),
        P::int("radius", 2)
            .range(0.0, 64.0)
            .help("Pixels the kernel reaches each way; 0 copies the channel unchanged"),
        P::float("sigma", 1.0)
            .range(0.1, 32.0)
            .help("The Gaussian's spread, in pixels: larger is softer within the radius"),
    ],
    build: |p| {
        Ok(Box::new(BlurOp {
            input: p.text("input")?,
            output: p.text("output")?,
            radius: p.u32("radius")?,
            sigma: p.f32("sigma")?,
        }))
    },
};

impl TextureOp for BlurOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &BLUR
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let len = (w * h) as usize;

        // Snapshot input
        let input: Vec<f32> = (0..len)
            .map(|i| field.get(&self.input, i as u32 % w, i as u32 / w))
            .collect();

        let mut temp = vec![0.0_f32; len];
        let mut result = vec![0.0_f32; len];

        // Horizontal pass
        gaussian_blur_1d(&input, &mut temp, w, h, self.radius, self.sigma, true);
        // Vertical pass
        gaussian_blur_1d(&temp, &mut result, w, h, self.radius, self.sigma, false);

        field.ensure_channel(&self.output);
        for (i, &val) in result.iter().enumerate() {
            field.set(&self.output, i as u32 % w, i as u32 / w, val);
        }
    }
}

// ─── SharpenOp ───────────────────────────────────────────────────────────

/// Unsharp mask sharpening.
///
/// Reads `<input>`, writes `<output>`.
pub struct SharpenOp {
    pub input: String,
    pub output: String,
    pub strength: f32,
    pub radius: u32,
}

/// The [`SharpenOp`] op, as the pipeline knows it.
pub const SHARPEN: OpDescriptor = OpDescriptor {
    type_name: "sharpen",
    label: "Sharpen",
    summary: "Sharpen a channel with an unsharp mask.",
    reads: &["<input>"],
    writes: &["<output>"],
    modifies: &[],
    params: &[
        P::channel("input", "input").help("Channel to sharpen"),
        P::channel("output", "output").help("Channel the result goes to"),
        P::float("strength", 1.0)
            .range(0.0, 10.0)
            .help("How much of the difference from the blurred copy is added back"),
        P::int("radius", 2)
            .range(1.0, 64.0)
            .help("Radius of the blur it compares against, in pixels"),
    ],
    build: |p| {
        Ok(Box::new(SharpenOp {
            input: p.text("input")?,
            output: p.text("output")?,
            strength: p.f32("strength")?,
            radius: p.u32("radius")?,
        }))
    },
};

impl TextureOp for SharpenOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &SHARPEN
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let len = (w * h) as usize;

        // Snapshot input
        let input: Vec<f32> = (0..len)
            .map(|i| field.get(&self.input, i as u32 % w, i as u32 / w))
            .collect();

        // Compute blurred version
        let sigma = self.radius as f32 * 0.5;
        let mut temp = vec![0.0_f32; len];
        let mut blurred = vec![0.0_f32; len];
        gaussian_blur_1d(&input, &mut temp, w, h, self.radius, sigma, true);
        gaussian_blur_1d(&temp, &mut blurred, w, h, self.radius, sigma, false);

        // Unsharp mask: v + (v - blurred) * strength
        field.ensure_channel(&self.output);
        for i in 0..len {
            let v = input[i];
            let result = v + (v - blurred[i]) * self.strength;
            field.set(
                &self.output,
                i as u32 % w,
                i as u32 / w,
                result.clamp(0.0, 1.0),
            );
        }
    }
}

// ─── EdgeDetectOp ────────────────────────────────────────────────────────

/// Sobel edge detection.
///
/// Reads `<input>`, writes `<output>`.
pub struct EdgeDetectOp {
    pub input: String,
    pub output: String,
}

/// The [`EdgeDetectOp`] op, as the pipeline knows it.
pub const EDGE_DETECT: OpDescriptor = OpDescriptor {
    type_name: "edge_detect",
    label: "Edge Detect",
    summary: "Write the strength of a channel's edges (Sobel): high where it changes fast.",
    reads: &["<input>"],
    writes: &["<output>"],
    modifies: &[],
    params: &[
        P::channel("input", "input").help("Channel whose edges are found"),
        P::channel("output", "output").help("Channel the edge strength goes to"),
    ],
    build: |p| {
        Ok(Box::new(EdgeDetectOp {
            input: p.text("input")?,
            output: p.text("output")?,
        }))
    },
};

impl TextureOp for EdgeDetectOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &EDGE_DETECT
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let wi = w as i32;
        let hi = h as i32;

        // Snapshot input
        let input: Vec<f32> = (0..(w * h))
            .map(|i| field.get(&self.input, i % w, i / w))
            .collect();

        field.ensure_channel(&self.output);

        for y in 0..h {
            for x in 0..w {
                let ix = x as i32;
                let iy = y as i32;

                let sample = |sx: i32, sy: i32| -> f32 {
                    let wx = ((sx % wi) + wi) % wi;
                    let wy = ((sy % hi) + hi) % hi;
                    input[(wy * wi + wx) as usize]
                };

                // Sobel kernels
                let gx =
                    -sample(ix - 1, iy - 1) - 2.0 * sample(ix - 1, iy) - sample(ix - 1, iy + 1)
                        + sample(ix + 1, iy - 1)
                        + 2.0 * sample(ix + 1, iy)
                        + sample(ix + 1, iy + 1);
                let gy =
                    -sample(ix - 1, iy - 1) - 2.0 * sample(ix, iy - 1) - sample(ix + 1, iy - 1)
                        + sample(ix - 1, iy + 1)
                        + 2.0 * sample(ix, iy + 1)
                        + sample(ix + 1, iy + 1);

                let magnitude = (gx * gx + gy * gy).sqrt();
                field.set(&self.output, x, y, magnitude.min(1.0));
            }
        }
    }
}

// ─── EdgeErodeOp ──────────────────────────────────────────────────────────

/// Erode cell boundaries with high-frequency noise displacement.
///
/// Snapshots `cell_id`, `edge_dist`, and `mask`, then for pixels near edges
/// (where `edge_dist` < `edge_width`) samples noise-displaced positions via
/// nearest-neighbor lookup, effectively nibbling irregular chips into cell
/// boundaries. Strength fades linearly toward the interior.
///
/// A low-frequency mask noise gates the erosion so that only some regions
/// along each edge are affected (`threshold` controls sparsity). This
/// prevents the "puzzle piece" look of continuous edge displacement.
pub struct EdgeErodeOp {
    /// Noise frequency — higher = more granular/chippy edges.
    pub noise_frequency: f32,
    /// Displacement strength in texture-space fraction.
    pub strength: f32,
    /// Only affects pixels with `edge_dist` < this value.
    pub edge_width: f32,
    /// Base noise algorithm.
    pub noise_type: NoiseType,
    /// FBM octaves (1 = sharpest grain).
    pub octaves: u32,
    /// Sparsity gate: 0.0 = erode everywhere, 1.0 = no erosion. A separate
    /// low-frequency noise is sampled and erosion only occurs where that
    /// noise exceeds this threshold, creating intermittent chips.
    pub threshold: f32,
}

/// The [`EdgeErodeOp`] op, as the pipeline knows it.
pub const EDGE_ERODE: OpDescriptor = OpDescriptor {
    type_name: "edge_erode",
    label: "Edge Erode",
    summary: "Nibble irregular chips into cell edges by displacing them with noise, here and there along each edge.",
    reads: &[],
    writes: &[],
    modifies: &["cell_id", "edge_dist", "mask"],
    params: &[
        P::float("noise_frequency", 40.0).min(0.1).help("Frequency of the displacing noise: higher is finer, chippier edges"),
        P::float("strength", 0.02).range(0.0, 0.5).help("How far an edge is pushed, in texture widths; fades toward the cell's interior"),
        P::float("edge_width", 0.06).range(0.0, 1.0).help("Only pixels with edge_dist below this are moved"),
        P::choice("noise_type", NOISE_TYPES, "perlin").help("The displacing noise: perlin, simplex, value, or voronoi (Worley cells)"),
        P::int("octaves", 1).range(1.0, 8.0).help("FBM octaves of the displacing noise; 1 is the sharpest grain"),
        P::float("threshold", 0.3).range(0.0, 1.0).help("How sparse the chips are: a slow noise gates the erosion, which happens only where it's above this; 0 erodes everywhere, 1 nowhere"),
    ],
    build: |p| {
        Ok(Box::new(EdgeErodeOp {
            noise_frequency: p.f32("noise_frequency")?,
            strength: p.f32("strength")?,
            edge_width: p.f32("edge_width")?,
            noise_type: NoiseType::from_str(p.choice("noise_type")?),
            octaves: p.u32("octaves")?,
            threshold: p.f32("threshold")?,
        }))
    },
};

impl TextureOp for EdgeErodeOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &EDGE_ERODE
    }

    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let noise_rng = rng.fork("edge_erode");
        let seed = noise_rng.seed();
        let freq = self.noise_frequency as f64;

        // Helper: build FBM noise source with given seed and frequency
        let make_noise = |s: u64, f: f64| -> Box<dyn NoiseSource> {
            match self.noise_type {
                NoiseType::Perlin => Box::new(
                    Fbm::new(PerlinNoise::new(s))
                        .with_octaves(self.octaves)
                        .with_frequency(f),
                ),
                NoiseType::Simplex => Box::new(
                    Fbm::new(SimplexNoise::new(s))
                        .with_octaves(self.octaves)
                        .with_frequency(f),
                ),
                NoiseType::Value => Box::new(
                    Fbm::new(ValueNoise::new(s))
                        .with_octaves(self.octaves)
                        .with_frequency(f),
                ),
                NoiseType::Voronoi => Box::new(
                    Fbm::new(WorleyNoise::new(s))
                        .with_octaves(self.octaves)
                        .with_frequency(f),
                ),
            }
        };

        let noise_x = make_noise(seed, freq);
        let noise_y = make_noise(seed.wrapping_add(7919), freq);

        // Low-frequency mask noise for sparsity gating (~1/4 displacement freq)
        let mask_freq = (self.noise_frequency * 0.25).max(1.0) as f64;
        let noise_mask: Box<dyn NoiseSource> = Box::new(
            Fbm::new(PerlinNoise::new(seed.wrapping_add(15937)))
                .with_octaves(1)
                .with_frequency(mask_freq),
        );

        // Snapshot the three channels we'll resample
        let snap_cell_id: Vec<f32> = (0..(w * h))
            .map(|i| field.get("cell_id", i % w, i / w))
            .collect();
        let snap_edge_dist: Vec<f32> = (0..(w * h))
            .map(|i| field.get("edge_dist", i % w, i / w))
            .collect();
        let snap_mask: Vec<f32> = (0..(w * h))
            .map(|i| field.get("mask", i % w, i / w))
            .collect();

        let wf = w as f32;
        let hf = h as f32;
        let tile = field.seamless;

        for y in 0..h {
            for x in 0..w {
                let idx = (y * w + x) as usize;
                let ed = snap_edge_dist[idx];

                if ed >= self.edge_width {
                    continue;
                }

                let u = (x as f32 + 0.5) / wf;
                let v = (y as f32 + 0.5) / hf;

                // Sparsity gate: remap noise from ~[-1,1] to [0,1], skip if below threshold
                let sample = |n: &dyn NoiseSource| {
                    let (u, v) = (u as f64, v as f64);
                    if tile {
                        n.sample_2d_tiled(u, v, 1.0, 1.0) as f32
                    } else {
                        n.sample_2d(u, v) as f32
                    }
                };
                let mask_val = sample(noise_mask.as_ref()) * 0.5 + 0.5;
                if mask_val < self.threshold {
                    continue;
                }

                // Fade strength: full at edge (ed=0), zero at edge_width
                let factor = 1.0 - (ed / self.edge_width);

                // Noise returns ~[-1, 1]; scale by strength and fade factor
                let dx = sample(noise_x.as_ref()) * self.strength * factor;
                let dy = sample(noise_y.as_ref()) * self.strength * factor;

                // Displaced pixel coordinates (nearest-neighbor, wrapping)
                let sx = ((u + dx) * wf).round() as i32;
                let sy = ((v + dy) * hf).round() as i32;
                let wx = ((sx % w as i32) + w as i32) as u32 % w;
                let wy = ((sy % h as i32) + h as i32) as u32 % h;
                let src_idx = (wy * w + wx) as usize;

                field.set("cell_id", x, y, snap_cell_id[src_idx]);
                field.set("edge_dist", x, y, snap_edge_dist[src_idx]);
                field.set("mask", x, y, snap_mask[src_idx]);
            }
        }
    }
}
