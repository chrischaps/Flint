//! Pattern generators: noise, checkers, gradients, waves, Voronoi and
//! Musgrave fields written into a channel (`noise_layer`,
//! `checker_texture`, `gradient_texture`, `wave_texture`, `white_noise`,
//! `voronoi_texture`, `musgrave_texture`).

use crate::algorithms::noise::{
    Fbm, MusgraveType, NoiseSource, PerlinNoise, SimplexNoise, ValueNoise, VoronoiFeature,
    VoronoiMetric, WorleyNoise,
};
use crate::SeededRng;

use super::cells::cell_noise_offset;
use super::TextureOp;
use crate::generators::texture::cell_memo::CellMemo;
use crate::generators::texture::descriptor::{OpDescriptor, ParamDesc as P};
use crate::generators::texture::field::TextureField;

// ─── NoiseType ────────────────────────────────────────────────────────────

/// The names a `noise_type` parameter takes, in [`NoiseType`]'s order.
pub const NOISE_TYPES: &[&str] = &["perlin", "simplex", "value", "voronoi"];

/// Selects the base noise algorithm for [`NoiseLayerOp`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoiseType {
    Perlin,
    Simplex,
    Value,
    Voronoi,
}

impl NoiseType {
    pub fn from_str(s: &str) -> Self {
        match s {
            "simplex" => NoiseType::Simplex,
            "value" => NoiseType::Value,
            "voronoi" => NoiseType::Voronoi,
            _ => NoiseType::Perlin,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            NoiseType::Perlin => "perlin",
            NoiseType::Simplex => "simplex",
            NoiseType::Value => "value",
            NoiseType::Voronoi => "voronoi",
        }
    }
}

// ─── NoiseLayerOp ─────────────────────────────────────────────────────────

/// Generate FBM noise into a named output channel.
///
/// Writes a single-channel signal (0–1) that can be blended into any channel
/// downstream via a [`BlendOp`].
pub struct NoiseLayerOp {
    /// Channel name to write the noise signal to (e.g. `"noise"`, `"micro_detail"`).
    pub output: String,
    /// Base noise frequency (cycles across the texture).
    pub frequency: f32,
    /// Number of FBM octaves.
    pub octaves: u32,
    /// Base noise algorithm.
    pub noise_type: NoiseType,
    /// Horizontal scale multiplier (>1 compresses horizontally → vertical streaks).
    pub scale_x: f32,
    /// Vertical scale multiplier (>1 compresses vertically → horizontal streaks).
    pub scale_y: f32,
    /// When true, offset noise UVs per cell using `cell_id` channel for decorrelation.
    pub cell_offset: bool,
}

/// The [`NoiseLayerOp`] op, as the pipeline knows it.
pub const NOISE_LAYER: OpDescriptor = OpDescriptor {
    type_name: "noise_layer",
    label: "Noise Layer",
    summary: "Write FBM noise (0–1) into a channel, for a blend to mix in downstream.",
    reads: &[],
    writes: &["<output>"],
    modifies: &[],
    params: &[
        P::channel("output", "noise").help("Channel the noise is written to"),
        P::choice("noise_type", NOISE_TYPES, "perlin").help("The base noise: perlin, simplex, value, or voronoi (Worley cells)"),
        P::float("frequency", 10.0).min(0.1).help("Noise cells across the texture in the first octave; rounded to whole cycles when seamless"),
        P::int("octaves", 4).range(1.0, 16.0).help("FBM octaves, each twice as fine"),
        P::float("scale_x", 1.0).min(0.01).help("Horizontal squeeze of the noise: above 1 packs it tighter across, for vertical streaks"),
        P::float("scale_y", 1.0).min(0.01).help("Vertical squeeze of the noise: above 1 packs it tighter down, for horizontal streaks"),
        P::flag("cell_offset", false).help("Offset the noise per cell (reads the cell id), so neighbouring stones don't share a pattern"),
    ],
    build: |p| {
        Ok(Box::new(NoiseLayerOp {
            output: p.text("output")?,
            frequency: p.f32("frequency")?,
            octaves: p.u32("octaves")?,
            noise_type: NoiseType::from_str(p.choice("noise_type")?),
            scale_x: p.f32("scale_x")?,
            scale_y: p.f32("scale_y")?,
            cell_offset: p.flag("cell_offset")?,
        }))
    },
};

impl TextureOp for NoiseLayerOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &NOISE_LAYER
    }

    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let noise_rng = rng.fork("noise_layer");
        let seed = noise_rng.seed();
        let freq = self.frequency as f64;

        // Build the FBM noise source with the selected algorithm.
        // Frequency is set on the FBM combinator; sample coordinates are
        // normalized to [0, 1] texture space.
        let noise: Box<dyn NoiseSource> = match self.noise_type {
            NoiseType::Perlin => Box::new(
                Fbm::new(PerlinNoise::new(seed))
                    .with_octaves(self.octaves)
                    .with_frequency(freq),
            ),
            NoiseType::Simplex => Box::new(
                Fbm::new(SimplexNoise::new(seed))
                    .with_octaves(self.octaves)
                    .with_frequency(freq),
            ),
            NoiseType::Value => Box::new(
                Fbm::new(ValueNoise::new(seed))
                    .with_octaves(self.octaves)
                    .with_frequency(freq),
            ),
            NoiseType::Voronoi => Box::new(
                Fbm::new(WorleyNoise::new(seed))
                    .with_octaves(self.octaves)
                    .with_frequency(freq),
            ),
        };

        field.ensure_channel(&self.output);

        let use_cell_offset = self.cell_offset && field.has_channel("cell_id");
        let mut offsets = CellMemo::new(|id| cell_noise_offset(id, seed));

        let tile = field.seamless;

        for y in 0..h {
            for x in 0..w {
                let mut nx = x as f64 / w as f64 * self.scale_x as f64;
                let mut ny = y as f64 / h as f64 * self.scale_y as f64;
                if use_cell_offset {
                    let cid = field.get("cell_id", x, y) as u32;
                    let (ox, oy) = offsets.get(cid);
                    nx += ox;
                    ny += oy;
                }
                let raw = if tile {
                    // One tile spans scale_x x scale_y in noise space.
                    noise.sample_2d_tiled(nx, ny, self.scale_x as f64, self.scale_y as f64)
                } else {
                    noise.sample_2d(nx, ny)
                };
                let val = raw as f32 * 0.5 + 0.5;
                field.set(&self.output, x, y, val);
            }
        }
    }
}

// ─── CheckerTextureOp ────────────────────────────────────────────────────

/// Checker pattern generator.
///
/// Writes `<output>`.
pub struct CheckerTextureOp {
    pub output: String,
    pub scale_x: f32,
    pub scale_y: f32,
}

/// The [`CheckerTextureOp`] op, as the pipeline knows it.
pub const CHECKER_TEXTURE: OpDescriptor = OpDescriptor {
    type_name: "checker_texture",
    label: "Checker Texture",
    summary: "Write a checkerboard of 0s and 1s.",
    reads: &[],
    writes: &["<output>"],
    modifies: &[],
    params: &[
        P::channel("output", "output").help("Channel the checker is written to"),
        P::float("scale_x", 8.0)
            .min(0.1)
            .help("Squares across; an even count when seamless"),
        P::float("scale_y", 8.0)
            .min(0.1)
            .help("Squares down; an even count when seamless"),
    ],
    build: |p| {
        Ok(Box::new(CheckerTextureOp {
            output: p.text("output")?,
            scale_x: p.f32("scale_x")?,
            scale_y: p.f32("scale_y")?,
        }))
    },
};

impl TextureOp for CheckerTextureOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &CHECKER_TEXTURE
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        // A checker repeats every two squares, so it tiles with an even
        // number of squares across.
        let even = |s: f32| ((s * 0.5).round() * 2.0).max(2.0);
        let (sx, sy) = if field.seamless {
            (even(self.scale_x), even(self.scale_y))
        } else {
            (self.scale_x, self.scale_y)
        };

        for y in 0..h {
            for x in 0..w {
                let u = x as f32 / w as f32;
                let v = y as f32 / h as f32;
                let check = ((u * sx).floor() as i32 + (v * sy).floor() as i32) % 2;
                field.set(&self.output, x, y, check.abs() as f32);
            }
        }
    }
}

// ─── GradientTextureOp ───────────────────────────────────────────────────

/// Directional gradient pattern generator.
///
/// Writes `<output>`.
pub struct GradientTextureOp {
    pub output: String,
    pub gradient_type: GradientType,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GradientType {
    Linear,
    Quadratic,
    Easing,
    Diagonal,
    Spherical,
    QuadraticSphere,
    Radial,
}

impl GradientType {
    pub fn from_str(s: &str) -> Self {
        match s {
            "quadratic" => GradientType::Quadratic,
            "easing" => GradientType::Easing,
            "diagonal" => GradientType::Diagonal,
            "spherical" => GradientType::Spherical,
            "quadratic_sphere" => GradientType::QuadraticSphere,
            "radial" => GradientType::Radial,
            _ => GradientType::Linear,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            GradientType::Linear => "linear",
            GradientType::Quadratic => "quadratic",
            GradientType::Easing => "easing",
            GradientType::Diagonal => "diagonal",
            GradientType::Spherical => "spherical",
            GradientType::QuadraticSphere => "quadratic_sphere",
            GradientType::Radial => "radial",
        }
    }
}

/// The [`GradientTextureOp`] op, as the pipeline knows it.
pub const GRADIENT_TEXTURE: OpDescriptor = OpDescriptor {
    type_name: "gradient_texture",
    label: "Gradient Texture",
    summary: "Write a gradient across the texture (a ramp: it never tiles).",
    reads: &[],
    writes: &["<output>"],
    modifies: &[],
    params: &[
        P::channel("output", "output").help("Channel the gradient is written to"),
        P::choice("gradient_type", &[
            "linear", "quadratic", "easing", "diagonal", "spherical", "quadratic_sphere",
            "radial",
        ], "linear").help("Its shape: left to right (linear, quadratic, easing), corner to corner (diagonal), out from the centre (spherical, quadratic_sphere) or around it (radial)"),
    ],
    build: |p| {
        Ok(Box::new(GradientTextureOp {
            output: p.text("output")?,
            gradient_type: GradientType::from_str(p.choice("gradient_type")?),
        }))
    },
};

impl TextureOp for GradientTextureOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &GRADIENT_TEXTURE
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        for y in 0..h {
            for x in 0..w {
                let u = x as f32 / w as f32;
                let v = y as f32 / h as f32;
                let val = match self.gradient_type {
                    GradientType::Linear => u,
                    GradientType::Quadratic => u * u,
                    GradientType::Easing => {
                        let t = u.clamp(0.0, 1.0);
                        t * t * (3.0 - 2.0 * t)
                    }
                    GradientType::Diagonal => (u + v) * 0.5,
                    GradientType::Spherical => {
                        let dx = u - 0.5;
                        let dy = v - 0.5;
                        (1.0 - (dx * dx + dy * dy).sqrt() * 2.0).max(0.0)
                    }
                    GradientType::QuadraticSphere => {
                        let dx = u - 0.5;
                        let dy = v - 0.5;
                        let d = (dx * dx + dy * dy).sqrt() * 2.0;
                        (1.0 - d * d).max(0.0)
                    }
                    GradientType::Radial => {
                        let dx = u - 0.5;
                        let dy = v - 0.5;
                        dy.atan2(dx) / std::f32::consts::TAU + 0.5
                    }
                };
                field.set(&self.output, x, y, val.clamp(0.0, 1.0));
            }
        }
    }
}

// ─── WaveTextureOp ───────────────────────────────────────────────────────

/// Sine/saw/triangle wave bands.
///
/// Writes `<output>`.
pub struct WaveTextureOp {
    pub output: String,
    pub wave_type: WaveType,
    pub direction: WaveDirection,
    pub scale: f32,
    pub distortion: f32,
    pub detail: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WaveType {
    Sine,
    Saw,
    Triangle,
}

impl WaveType {
    pub fn from_str(s: &str) -> Self {
        match s {
            "saw" => WaveType::Saw,
            "triangle" => WaveType::Triangle,
            _ => WaveType::Sine,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WaveDirection {
    X,
    Y,
    Diagonal,
}

impl WaveDirection {
    pub fn from_str(s: &str) -> Self {
        match s {
            "y" => WaveDirection::Y,
            "diagonal" => WaveDirection::Diagonal,
            _ => WaveDirection::X,
        }
    }
}

/// The [`WaveTextureOp`] op, as the pipeline knows it.
pub const WAVE_TEXTURE: OpDescriptor = OpDescriptor {
    type_name: "wave_texture",
    label: "Wave Texture",
    summary:
        "Write bands of sine, saw or triangle waves, optionally bent by noise: wood grain, ripples.",
    reads: &[],
    writes: &["<output>"],
    modifies: &[],
    params: &[
        P::channel("output", "output").help("Channel the waves are written to"),
        P::choice("wave_type", &["sine", "saw", "triangle"], "sine")
            .help("The profile of each band"),
        P::choice("direction", &["x", "y", "diagonal"], "x")
            .help("The way the bands run across: x varies left to right, y top to bottom"),
        P::float("scale", 4.0)
            .min(0.1)
            .help("Bands across the texture; whole when seamless (even along diagonal)"),
        P::float("distortion", 0.0)
            .min(0.0)
            .help("How far noise bends the bands; needs detail above 0"),
        P::int("detail", 0)
            .range(0.0, 16.0)
            .help("Octaves of the bending noise; 0 turns distortion off"),
    ],
    build: |p| {
        Ok(Box::new(WaveTextureOp {
            output: p.text("output")?,
            wave_type: WaveType::from_str(p.choice("wave_type")?),
            direction: WaveDirection::from_str(p.choice("direction")?),
            scale: p.f32("scale")?,
            distortion: p.f32("distortion")?,
            detail: p.u32("detail")?,
        }))
    },
};

impl TextureOp for WaveTextureOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &WAVE_TEXTURE
    }

    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        // Build optional distortion noise
        let distortion_noise: Option<Box<dyn NoiseSource>> =
            if self.distortion > 0.0 && self.detail > 0 {
                let noise_rng = rng.fork("wave_distortion");
                Some(Box::new(
                    Fbm::new(PerlinNoise::new(noise_rng.seed()))
                        .with_octaves(self.detail)
                        .with_frequency(self.scale as f64),
                ))
            } else {
                None
            };

        // Tiling needs a whole number of bands per tile. Along the diagonal,
        // coord = (u + v) / 2 steps by 1/2 per tile, so the count must be even.
        let tile = field.seamless;
        let scale = self.scale as f64;
        let bands = if !tile {
            scale
        } else if self.direction == WaveDirection::Diagonal {
            ((scale * 0.5).round() * 2.0).max(2.0)
        } else {
            scale.round().max(1.0)
        };

        for y in 0..h {
            for x in 0..w {
                let u = x as f64 / w as f64;
                let v = y as f64 / h as f64;

                let coord = match self.direction {
                    WaveDirection::X => u,
                    WaveDirection::Y => v,
                    WaveDirection::Diagonal => (u + v) * 0.5,
                };

                let distorted = if let Some(ref noise) = distortion_noise {
                    let n = if tile {
                        noise.sample_2d_tiled(u * scale, v * scale, scale, scale)
                    } else {
                        noise.sample_2d(u * scale, v * scale)
                    };
                    coord + n * self.distortion as f64
                } else {
                    coord
                };

                let phase = distorted * bands;
                let val = match self.wave_type {
                    WaveType::Sine => (phase * std::f64::consts::TAU).sin() * 0.5 + 0.5,
                    WaveType::Saw => phase.rem_euclid(1.0),
                    WaveType::Triangle => {
                        let t = phase.rem_euclid(1.0);
                        if t < 0.5 {
                            t * 2.0
                        } else {
                            2.0 - t * 2.0
                        }
                    }
                };
                field.set(&self.output, x, y, val as f32);
            }
        }
    }
}

// ─── WhiteNoiseOp ────────────────────────────────────────────────────────

/// Per-pixel random noise.
///
/// Writes `<output>`.
pub struct WhiteNoiseOp {
    pub output: String,
}

/// The [`WhiteNoiseOp`] op, as the pipeline knows it.
pub const WHITE_NOISE: OpDescriptor = OpDescriptor {
    type_name: "white_noise",
    label: "White Noise",
    summary: "Write an independent random value (0–1) per pixel: grit, sparkle.",
    reads: &[],
    writes: &["<output>"],
    modifies: &[],
    params: &[P::channel("output", "output").help("Channel the noise is written to")],
    build: |p| {
        Ok(Box::new(WhiteNoiseOp {
            output: p.text("output")?,
        }))
    },
};

impl TextureOp for WhiteNoiseOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &WHITE_NOISE
    }

    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        for y in 0..h {
            for x in 0..w {
                field.set(&self.output, x, y, rng.next_f32());
            }
        }
    }
}

// ─── VoronoiTextureOp ────────────────────────────────────────────────────

/// Full Voronoi texture generator with multiple features and metrics.
///
/// Writes `<output>` and optional `<cell_output>`.
pub struct VoronoiTextureOp {
    pub output: String,
    pub cell_output: Option<String>,
    pub scale: f32,
    pub randomness: f32,
    pub feature: VoronoiFeature,
    pub metric: VoronoiMetric,
}

/// The [`VoronoiTextureOp`] op, as the pipeline knows it.
pub const VORONOI_TEXTURE: OpDescriptor = OpDescriptor {
    type_name: "voronoi_texture",
    label: "Voronoi Texture",
    summary: "Write a Voronoi (Worley) distance field, and optionally each pixel's cell.",
    reads: &[],
    writes: &["<output>"],
    modifies: &[],
    params: &[
        P::channel("output", "output").help("Channel the distance (0–1) is written to"),
        P::opt_channel("cell_output").help("Channel each pixel's cell number is written to; unset, none is"),
        P::float("scale", 4.0).min(0.1).help("Cells across the texture"),
        P::float("randomness", 1.0).range(0.0, 1.0).help("How far each cell's point strays from a regular grid: 0 a grid, 1 fully random"),
        P::choice("feature", &["f1", "f2", "smooth_f1", "distance_to_edge"], "f1").help("What is measured: distance to the nearest point (f1), the second nearest (f2), a rounded f1, or distance to the cell's edge"),
        P::choice("metric", &["euclidean", "manhattan", "chebyshev", "minkowski"], "euclidean").help("How distance is measured: round cells (euclidean), diamonds (manhattan), squares (chebyshev), or minkowski with exponent"),
        P::float("exponent", 2.0).min(0.1).help("The minkowski metric's exponent: 1 is manhattan, 2 euclidean; read only with metric = \"minkowski\""),
    ],
    build: |p| {
        use crate::algorithms::noise::{VoronoiFeature, VoronoiMetric};
        let feature = match p.choice("feature")? {
            "f2" => VoronoiFeature::F2,
            "smooth_f1" => VoronoiFeature::SmoothF1,
            "distance_to_edge" => VoronoiFeature::DistanceToEdge,
            _ => VoronoiFeature::F1,
        };
        let exponent = p.f32("exponent")?;
        let metric = match p.choice("metric")? {
            "manhattan" => VoronoiMetric::Manhattan,
            "chebyshev" => VoronoiMetric::Chebyshev,
            "minkowski" => VoronoiMetric::Minkowski(exponent as f64),
            _ => VoronoiMetric::Euclidean,
        };
        Ok(Box::new(VoronoiTextureOp {
            output: p.text("output")?,
            cell_output: p.opt_text("cell_output")?,
            scale: p.f32("scale")?,
            randomness: p.f32("randomness")?,
            feature,
            metric,
        }))
    },
};

impl TextureOp for VoronoiTextureOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &VORONOI_TEXTURE
    }

    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let seed = rng.fork("voronoi").seed();
        field.ensure_channel(&self.output);
        if let Some(ref cell_ch) = self.cell_output {
            field.ensure_channel(cell_ch);
        }
        let tile = field.seamless;

        for y in 0..h {
            for x in 0..w {
                let u = x as f64 / w as f64;
                let v = y as f64 / h as f64;
                let sample = if tile {
                    crate::algorithms::noise::voronoi_sample_tiled
                } else {
                    crate::algorithms::noise::voronoi_sample
                };
                let result = sample(
                    u,
                    v,
                    seed,
                    self.scale as f64,
                    self.randomness as f64,
                    self.feature,
                    self.metric,
                );
                // Normalize the value to 0-1 range
                let val = result.value.clamp(0.0, 1.0) as f32;
                field.set(&self.output, x, y, val);
                if let Some(ref cell_ch) = self.cell_output {
                    field.set(cell_ch, x, y, result.cell_id as f32);
                }
            }
        }
    }
}

// ─── MusgraveTextureOp ───────────────────────────────────────────────────

/// Musgrave FBM variant texture generator.
///
/// Writes `<output>`.
pub struct MusgraveTextureOp {
    pub output: String,
    pub noise_type: NoiseType,
    pub musgrave_type: MusgraveType,
    pub frequency: f32,
    pub octaves: u32,
    pub lacunarity: f32,
    pub dimension: f32,
    pub offset: f32,
    pub gain: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    /// When true, offset noise UVs per cell using `cell_id` channel for decorrelation.
    pub cell_offset: bool,
}

/// The [`MusgraveTextureOp`] op, as the pipeline knows it.
pub const MUSGRAVE_TEXTURE: OpDescriptor = OpDescriptor {
    type_name: "musgrave_texture",
    label: "Musgrave Texture",
    summary: "Write fractal noise of Musgrave's kinds: fBm, multifractal, ridged, hybrid, hetero terrain.",
    reads: &[],
    writes: &["<output>"],
    modifies: &[],
    params: &[
        P::channel("output", "output").help("Channel the noise (about 0–1) is written to"),
        P::choice("noise_type", NOISE_TYPES, "perlin").help("The base noise: perlin, simplex, value, or voronoi (Worley cells)"),
        P::choice("musgrave_type", &[
            "fbm", "multifractal", "ridged_multifractal", "hybrid_multifractal",
            "hetero_terrain",
        ], "fbm").help("How octaves combine: summed (fbm), multiplied (multifractal), as sharp ridges (ridged_multifractal), or rougher where higher (hybrid_multifractal, hetero_terrain)"),
        P::float("frequency", 4.0).min(0.1).help("Noise cells across the texture in the first octave; rounded to whole cycles when seamless"),
        P::int("octaves", 4).range(1.0, 16.0).help("Octaves summed"),
        P::float("lacunarity", 2.0).min(0.1).help("How much finer each octave is than the last"),
        P::float("dimension", 1.0).help("Fractal dimension (H): higher weakens the fine octaves, for a smoother result"),
        P::float("offset", 1.0).help("Added to each octave before combining (ridged, hybrid and hetero terrain)"),
        P::float("gain", 2.0).help("How strongly one octave drives the next (ridged and hybrid)"),
        P::float("scale_x", 1.0).min(0.01).help("Horizontal squeeze of the noise: above 1 packs it tighter across"),
        P::float("scale_y", 1.0).min(0.01).help("Vertical squeeze of the noise: above 1 packs it tighter down"),
        P::flag("cell_offset", false).help("Offset the noise per cell (reads the cell id), so neighbouring stones don't share a pattern"),
    ],
    build: |p| {
        use crate::algorithms::noise::MusgraveType;
        let musgrave_type = match p.choice("musgrave_type")? {
            "multifractal" => MusgraveType::Multifractal,
            "ridged_multifractal" => MusgraveType::RidgedMultifractal,
            "hybrid_multifractal" => MusgraveType::HybridMultifractal,
            "hetero_terrain" => MusgraveType::HeteroTerrain,
            _ => MusgraveType::Fbm,
        };
        Ok(Box::new(MusgraveTextureOp {
            output: p.text("output")?,
            noise_type: NoiseType::from_str(p.choice("noise_type")?),
            musgrave_type,
            frequency: p.f32("frequency")?,
            octaves: p.u32("octaves")?,
            lacunarity: p.f32("lacunarity")?,
            dimension: p.f32("dimension")?,
            offset: p.f32("offset")?,
            gain: p.f32("gain")?,
            scale_x: p.f32("scale_x")?,
            scale_y: p.f32("scale_y")?,
            cell_offset: p.flag("cell_offset")?,
        }))
    },
};

impl TextureOp for MusgraveTextureOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &MUSGRAVE_TEXTURE
    }

    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let noise_rng = rng.fork("musgrave");
        let seed = noise_rng.seed();
        field.ensure_channel(&self.output);

        let source: Box<dyn NoiseSource> = match self.noise_type {
            NoiseType::Perlin => Box::new(PerlinNoise::new(seed)),
            NoiseType::Simplex => Box::new(SimplexNoise::new(seed)),
            NoiseType::Value => Box::new(ValueNoise::new(seed)),
            NoiseType::Voronoi => Box::new(WorleyNoise::new(seed)),
        };

        let use_cell_offset = self.cell_offset && field.has_channel("cell_id");
        let mut offsets = CellMemo::new(|id| cell_noise_offset(id, seed));

        let tile = field.seamless;
        let (sx, sy) = (self.scale_x as f64, self.scale_y as f64);
        let (freq, lac) = (self.frequency as f64, self.lacunarity as f64);
        let (dim, off, gain) = (self.dimension as f64, self.offset as f64, self.gain as f64);
        let (src, mt, oct) = (source.as_ref(), self.musgrave_type, self.octaves);

        for y in 0..h {
            for x in 0..w {
                let mut nx = x as f64 / w as f64 * self.scale_x as f64;
                let mut ny = y as f64 / h as f64 * self.scale_y as f64;
                if use_cell_offset {
                    let cid = field.get("cell_id", x, y) as u32;
                    let (ox, oy) = offsets.get(cid);
                    nx += ox;
                    ny += oy;
                }
                let val = if tile {
                    crate::algorithms::noise::musgrave_sample_tiled(
                        src, nx, ny, sx, sy, mt, freq, oct, lac, dim, off, gain,
                    )
                } else {
                    crate::algorithms::noise::musgrave_sample(
                        src, nx, ny, mt, freq, oct, lac, dim, off, gain,
                    )
                };
                // Remap roughly to 0-1
                let val = (val as f32 * 0.5 + 0.5).clamp(0.0, 1.0);
                field.set(&self.output, x, y, val);
            }
        }
    }
}
