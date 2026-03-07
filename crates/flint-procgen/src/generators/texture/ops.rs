//! Composable texture operations for the pipeline pattern.
//!
//! Each [`TextureOp`] is a small, reusable stage that reads and writes named
//! channels on a [`TextureField`]. Ops are chained sequentially to build up
//! PBR texture maps from simple building blocks.
//!
//! # Op categories
//!
//! - **Grid ops**: `BrickGridOp` — write `cell_id` and `edge_dist`
//! - **Height ops**: `CellHeightOp`, `CellBulgeOp`, `NoiseLayerOp`, `MortarGrooveOp` — write/modify `height`
//! - **Color ops**: `CellColorOp`, `MortarColorOp` — write `r`, `g`, `b`
//! - **Output ops**: `DeriveNormalOp`, `CellRoughnessOp` — write final map channels

use crate::algorithms::noise::{
    Fbm, MusgraveType, NoiseSource, PerlinNoise, SimplexNoise, ValueNoise, VoronoiFeature,
    VoronoiMetric, WorleyNoise,
};
use crate::rng::{rgb_to_hsv, hsv_to_rgb};
use crate::SeededRng;

use super::field::TextureField;
use super::voronoi_util::{self, SpatialGrid};

/// Linear interpolation helper.
#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

// ─── Op Port Metadata ────────────────────────────────────────────────────

/// Describes which channels an op reads, writes, and modifies.
///
/// Used by the node editor to draw typed ports and validate connections.
#[derive(Debug, Clone)]
pub struct OpPortInfo {
    /// The op type identifier (e.g. `"brick_grid"`).
    pub op_type: &'static str,
    /// Human-readable label for the node (e.g. `"Brick Grid"`).
    pub label: &'static str,
    /// Channels this op reads (must exist from a prior op).
    pub reads: &'static [&'static str],
    /// Channels this op creates/overwrites.
    pub writes: &'static [&'static str],
    /// Channels this op reads AND writes (in-place modification).
    pub modifies: &'static [&'static str],
}

// ─── TextureOp trait ──────────────────────────────────────────────────────

/// A composable texture generation stage.
///
/// Each op reads channels from the field, performs some computation, and
/// writes results back. Ops are applied sequentially; later ops can build
/// on earlier ops' output.
pub trait TextureOp: Send + Sync {
    /// Apply this operation to the field, using `rng` for any randomness.
    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng);

    /// Return port metadata describing this op's channel I/O.
    fn port_info(&self) -> OpPortInfo;
}

// ─── BrickGridOp ──────────────────────────────────────────────────────────

/// Lay out rectangular cells in staggered rows (running bond pattern).
///
/// Writes `cell_id` and `edge_dist` channels.
pub struct BrickGridOp {
    pub columns: u32,
    pub rows: u32,
    /// Additive horizontal offset per row as a fraction (0.5 = standard running bond, each row shifts by 0.5/cols).
    pub stagger: f32,
    /// Fraction of cell size that is gap/mortar. Controls edge_dist normalization.
    pub gap_width: f32,
    /// Per-row column width randomization (0.0 = uniform, 1.0 = max variation).
    pub width_variation: f32,
    /// Row height randomization (0.0 = uniform, 1.0 = max variation).
    pub row_height_variation: f32,
    /// Optional channel names for structural domain warp (UV displacement before cell lookup).
    pub warp_x: Option<String>,
    pub warp_y: Option<String>,
    pub warp_strength: f32,
}

impl TextureOp for BrickGridOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "brick_grid",
            label: "Brick Grid",
            reads: &[],
            writes: &["cell_id", "edge_dist", "mask"],
            modifies: &[],
        }
    }

    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let cols = self.columns.max(1);
        let rows = self.rows.max(1);
        field.ensure_channel("cell_id");
        field.ensure_channel("edge_dist");
        field.ensure_channel("mask");

        // Pre-compute per-row column boundaries when width_variation > 0
        let seed = rng.seed();
        let col_boundaries: Vec<Vec<f32>> = if self.width_variation > 0.0 {
            (0..rows)
                .map(|row| {
                    let row_seed = seed
                        .wrapping_mul(0x6C62_272E_07BB_0142)
                        .wrapping_add(row as u64);
                    let mut row_rng = SeededRng::new(row_seed);
                    // Generate random multipliers per column
                    let raw: Vec<f32> = (0..cols)
                        .map(|_| {
                            let r = row_rng.next_f32(); // 0..1
                            1.0 + (r - 0.5) * self.width_variation * 2.0
                        })
                        .collect();
                    let sum: f32 = raw.iter().sum();
                    // Cumulative boundaries: [0.0, w0, w0+w1, ..., 1.0]
                    let mut bounds = Vec::with_capacity(cols as usize + 1);
                    bounds.push(0.0);
                    let mut accum = 0.0;
                    for &r in &raw {
                        accum += r / sum;
                        bounds.push(accum);
                    }
                    // Ensure last boundary is exactly 1.0
                    if let Some(last) = bounds.last_mut() {
                        *last = 1.0;
                    }
                    bounds
                })
                .collect()
        } else {
            Vec::new()
        };

        // Pre-compute row boundaries when row_height_variation > 0
        let row_boundaries: Vec<f32> = if self.row_height_variation > 0.0 {
            let row_h_seed = seed
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(0);
            let mut row_rng = SeededRng::new(row_h_seed);
            let raw: Vec<f32> = (0..rows)
                .map(|_| {
                    let r = row_rng.next_f32();
                    1.0 + (r - 0.5) * self.row_height_variation * 2.0
                })
                .collect();
            let sum: f32 = raw.iter().sum();
            let mut bounds = Vec::with_capacity(rows as usize + 1);
            bounds.push(0.0);
            let mut accum = 0.0;
            for &r in &raw {
                accum += r / sum;
                bounds.push(accum);
            }
            if let Some(last) = bounds.last_mut() {
                *last = 1.0;
            }
            bounds
        } else {
            Vec::new()
        };

        for y in 0..h {
            for x in 0..w {
                let mut u = (x as f32 + 0.5) / w as f32;
                let mut v = (y as f32 + 0.5) / h as f32;

                // Optional structural domain warp
                if let (Some(wx_ch), Some(wy_ch)) = (&self.warp_x, &self.warp_y) {
                    if self.warp_strength > 0.0 {
                        let dx = field.get(wx_ch, x, y) - 0.5;
                        let dy = field.get(wy_ch, x, y) - 0.5;
                        u = (u + dx * self.warp_strength).rem_euclid(1.0);
                        v = (v + dy * self.warp_strength).rem_euclid(1.0);
                    }
                }

                // Row computation
                let (row, row_frac) = if !row_boundaries.is_empty() {
                    let mut r = 0u32;
                    for i in 1..row_boundaries.len() {
                        if v < row_boundaries[i] {
                            r = (i - 1) as u32;
                            break;
                        }
                    }
                    let r = r.min(rows - 1);
                    let lo = row_boundaries[r as usize];
                    let hi = row_boundaries[r as usize + 1];
                    let frac = if (hi - lo).abs() < 1e-10 {
                        0.5
                    } else {
                        (v - lo) / (hi - lo)
                    };
                    (r, frac)
                } else {
                    let row_f = v * rows as f32;
                    ((row_f.floor() as u32) % rows, row_f.fract())
                };

                // Column computation with additive stagger per row
                let stagger_offset = (row as f32 * self.stagger) / cols as f32;
                let u_shifted = (u + stagger_offset).fract();

                let (col, col_frac) = if !col_boundaries.is_empty() {
                    // Variable-width columns
                    let bounds = &col_boundaries[row as usize];
                    let mut c = 0u32;
                    for i in 1..bounds.len() {
                        if u_shifted < bounds[i] {
                            c = (i - 1) as u32;
                            break;
                        }
                    }
                    let c = c.min(cols - 1);
                    let lo = bounds[c as usize];
                    let hi = bounds[c as usize + 1];
                    let frac = if (hi - lo).abs() < 1e-10 {
                        0.5
                    } else {
                        (u_shifted - lo) / (hi - lo)
                    };
                    (c, frac)
                } else {
                    // Uniform columns
                    let col_f = u_shifted * cols as f32;
                    let c = (col_f.floor() as u32) % cols;
                    (c, col_f.fract())
                };

                // Unique cell ID
                let cell_id = row * cols + col;

                // Edge distance: min fractional distance to any cell boundary.
                // Range [0, 0.5] where 0 = on boundary, 0.5 = cell center.
                let dist = col_frac
                    .min(1.0 - col_frac)
                    .min(row_frac.min(1.0 - row_frac));

                // Binary mask: 1.0 inside cell, 0.0 in mortar/gap zone
                let mask = if dist > self.gap_width { 1.0 } else { 0.0 };

                field.set("cell_id", x, y, cell_id as f32);
                field.set("edge_dist", x, y, dist);
                field.set("mask", x, y, mask);
            }
        }
    }
}

// ─── VoronoiGridOp ────────────────────────────────────────────────────────

/// Voronoi tessellation cell layout for irregular stone patterns.
///
/// Writes `cell_id`, `edge_dist`, and `mask` channels — compatible with all
/// cell ops (`cell_height`, `cell_color`, `cell_roughness`, `mortar_groove`,
/// `mortar_color`).
pub struct VoronoiGridOp {
    /// Number of Voronoi cells (seed points).
    pub cell_count: u32,
    /// Regularity: 0 = random, 1 = fully Lloyd's-relaxed.
    pub regularity: f32,
    /// Width of mortar as a fraction of texture space.
    pub mortar_width: f32,
    /// Optional channel names for structural domain warp.
    pub warp_x: Option<String>,
    pub warp_y: Option<String>,
    pub warp_strength: f32,
}

impl TextureOp for VoronoiGridOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "voronoi_grid",
            label: "Voronoi Grid",
            reads: &[],
            writes: &["cell_id", "edge_dist", "mask"],
            modifies: &[],
        }
    }

    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let cell_count = self.cell_count.max(1) as usize;
        field.ensure_channel("cell_id");
        field.ensure_channel("edge_dist");
        field.ensure_channel("mask");

        // Generate seed points in [0, 1)²
        let mut points = voronoi_util::generate_seed_points(rng, cell_count);

        // Apply Lloyd's relaxation for regularity
        if self.regularity > 0.0 {
            let iterations = (self.regularity * 10.0).ceil() as usize;
            let relaxed = voronoi_util::lloyds_relaxation(&points, iterations, true);
            for i in 0..points.len() {
                points[i].0 = voronoi_util::lerp(points[i].0, relaxed[i].0, self.regularity);
                points[i].1 = voronoi_util::lerp(points[i].1, relaxed[i].1, self.regularity);
            }
        }

        // Build spatial acceleration grid (always seamless for tiling)
        let grid = SpatialGrid::new(&points, true);

        let mortar_w = self.mortar_width.max(1e-6);

        for y in 0..h {
            for x in 0..w {
                let mut u = (x as f32 + 0.5) / w as f32;
                let mut v = (y as f32 + 0.5) / h as f32;

                // Optional structural domain warp
                if let (Some(wx_ch), Some(wy_ch)) = (&self.warp_x, &self.warp_y) {
                    if self.warp_strength > 0.0 {
                        let dx = field.get(wx_ch, x, y) - 0.5;
                        let dy = field.get(wy_ch, x, y) - 0.5;
                        u = (u + dx * self.warp_strength).rem_euclid(1.0);
                        v = (v + dy * self.warp_strength).rem_euclid(1.0);
                    }
                }

                let (closest_id, dist1, dist2) = grid.find_two_nearest(u, v, &points, true);

                // Edge distance: normalized gap between nearest and second-nearest
                let raw_edge_dist = dist2 - dist1;
                let edge_normalized = (raw_edge_dist / mortar_w).min(1.0);

                // Binary mask: 1.0 inside cell, 0.0 in mortar zone
                let mask = if edge_normalized > 1e-3 { 1.0 } else { 0.0 };

                field.set("cell_id", x, y, closest_id as f32);
                field.set("edge_dist", x, y, edge_normalized);
                field.set("mask", x, y, mask);
            }
        }
    }
}

// ─── DomainWarpOp ─────────────────────────────────────────────────────────

/// General-purpose coordinate displacement op.
///
/// Reads displacement from two channels and warps a target channel using
/// bilinear interpolation.
pub struct DomainWarpOp {
    /// Channel to warp.
    pub input: String,
    /// Output channel (can be same as input for in-place).
    pub output: String,
    /// Channel providing horizontal displacement.
    pub warp_x: String,
    /// Channel providing vertical displacement.
    pub warp_y: String,
    /// Displacement strength in texture-space units.
    pub strength: f32,
}

impl TextureOp for DomainWarpOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "domain_warp",
            label: "Domain Warp",
            reads: &["<input>", "<warp_x>", "<warp_y>"],
            writes: &["<output>"],
            modifies: &[],
        }
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        // Snapshot input channel for reading during write
        let snapshot: Vec<f32> = (0..(w * h))
            .map(|i| field.get(&self.input, i % w, i / w))
            .collect();

        for y in 0..h {
            for x in 0..w {
                let u = (x as f32 + 0.5) / w as f32;
                let v = (y as f32 + 0.5) / h as f32;

                let dx = (field.get(&self.warp_x, x, y) - 0.5) * self.strength;
                let dy = (field.get(&self.warp_y, x, y) - 0.5) * self.strength;

                // Displaced coordinates with wrapping
                let su = (u + dx).rem_euclid(1.0) * w as f32 - 0.5;
                let sv = (v + dy).rem_euclid(1.0) * h as f32 - 0.5;

                // Bilinear interpolation
                let x0 = su.floor() as i32;
                let y0 = sv.floor() as i32;
                let fx = su - x0 as f32;
                let fy = sv - y0 as f32;

                let sample = |sx: i32, sy: i32| -> f32 {
                    let wx = ((sx % w as i32) + w as i32) as u32 % w;
                    let wy = ((sy % h as i32) + h as i32) as u32 % h;
                    snapshot[(wy * w + wx) as usize]
                };

                let v00 = sample(x0, y0);
                let v10 = sample(x0 + 1, y0);
                let v01 = sample(x0, y0 + 1);
                let v11 = sample(x0 + 1, y0 + 1);

                let result = lerp(
                    lerp(v00, v10, fx),
                    lerp(v01, v11, fx),
                    fy,
                );

                field.set(&self.output, x, y, result);
            }
        }
    }
}

// ─── CellHeightOp ─────────────────────────────────────────────────────────

/// Assign a random base height per cell.
///
/// Reads `cell_id`, writes `height`.
pub struct CellHeightOp {
    /// Height randomness magnitude in [0, 1].
    pub variation: f32,
}

impl TextureOp for CellHeightOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "cell_height",
            label: "Cell Height",
            reads: &["cell_id"],
            writes: &["height"],
            modifies: &[],
        }
    }

    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let seed = rng.seed();
        field.ensure_channel("height");

        for y in 0..h {
            for x in 0..w {
                let cell_id = field.get("cell_id", x, y) as u32;
                let cell_h = cell_height_for_id(cell_id, self.variation, seed);
                field.set("height", x, y, cell_h);
            }
        }
    }
}

/// Deterministic per-cell height from cell ID.
fn cell_height_for_id(cell_id: u32, variation: f32, rng_seed: u64) -> f32 {
    let cell_seed = rng_seed
        .wrapping_mul(0x6C62_272E_07BB_0142)
        .wrapping_add(cell_id as u64);
    let mut cell_rng = SeededRng::new(cell_seed);
    let h = 0.5 + (cell_rng.next_f32() - 0.5) * variation;
    h.clamp(0.0, 1.0)
}

/// Deterministic per-cell UV offset for noise decorrelation.
fn cell_noise_offset(cell_id: u32, rng_seed: u64) -> (f64, f64) {
    let cell_seed = rng_seed
        .wrapping_mul(0xA076_1D64_78BD_642F)
        .wrapping_add(cell_id as u64);
    let mut cell_rng = SeededRng::new(cell_seed);
    (cell_rng.next_f64() * 1000.0, cell_rng.next_f64() * 1000.0)
}

// ─── CellBulgeOp ──────────────────────────────────────────────────────────

/// Falloff curve for dome profile within each cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BulgeFalloff {
    /// `t` — cone shape.
    Linear,
    /// `1 - (1-t)^2` — convex dome, flat top.
    Parabolic,
    /// `(1 - cos(t*π)) / 2` — smooth S-curve, zero derivative at edges and center.
    Cosine,
}

impl BulgeFalloff {
    pub fn from_str(s: &str) -> Self {
        match s {
            "linear" => Self::Linear,
            "parabolic" => Self::Parabolic,
            _ => Self::Cosine,
        }
    }

    /// Map `t` in [0,1] (0=boundary, 1=center) to bulge factor in [0,1].
    #[inline]
    pub fn apply(self, t: f32) -> f32 {
        match self {
            Self::Linear => t,
            Self::Parabolic => 1.0 - (1.0 - t) * (1.0 - t),
            Self::Cosine => (1.0 - (t * std::f32::consts::PI).cos()) * 0.5,
        }
    }
}

/// Add dome-shaped height curvature within each cell.
///
/// Reads `cell_id` and `edge_dist`, modifies `height`. Uses a two-pass
/// algorithm to normalize `edge_dist` per cell before applying the falloff,
/// ensuring consistent dome profiles regardless of cell size/shape.
pub struct CellBulgeOp {
    /// Maximum height added at cell center (default 0.3).
    pub strength: f32,
    /// Falloff curve (default Cosine).
    pub falloff: BulgeFalloff,
}

impl TextureOp for CellBulgeOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "cell_bulge",
            label: "Cell Bulge",
            reads: &["cell_id", "edge_dist"],
            writes: &[],
            modifies: &["height"],
        }
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel("height");

        // Pass 1: find max edge_dist per cell
        let mut max_dist: std::collections::HashMap<u32, f32> =
            std::collections::HashMap::new();
        for y in 0..h {
            for x in 0..w {
                let cell_id = field.get("cell_id", x, y) as u32;
                let dist = field.get("edge_dist", x, y);
                let entry = max_dist.entry(cell_id).or_insert(0.0);
                if dist > *entry {
                    *entry = dist;
                }
            }
        }

        // Pass 2: apply normalized bulge
        for y in 0..h {
            for x in 0..w {
                let cell_id = field.get("cell_id", x, y) as u32;
                let dist = field.get("edge_dist", x, y);
                let max = max_dist.get(&cell_id).copied().unwrap_or(1.0);
                if max <= 0.0 {
                    continue;
                }
                let t = (dist / max).clamp(0.0, 1.0);
                let bulge = self.strength * self.falloff.apply(t);
                let cur = field.get("height", x, y);
                field.set("height", x, y, cur + bulge);
            }
        }
    }
}

// ─── NoiseType ────────────────────────────────────────────────────────────

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

impl TextureOp for NoiseLayerOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "noise_layer",
            label: "Noise Layer",
            reads: &[],
            writes: &["<output>"],
            modifies: &[],
        }
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

        for y in 0..h {
            for x in 0..w {
                let mut nx = x as f64 / w as f64 * self.scale_x as f64;
                let mut ny = y as f64 / h as f64 * self.scale_y as f64;
                if use_cell_offset {
                    let cid = field.get("cell_id", x, y) as u32;
                    let (ox, oy) = cell_noise_offset(cid, seed);
                    nx += ox;
                    ny += oy;
                }
                let val = noise.sample_2d(nx, ny) as f32 * 0.5 + 0.5;
                field.set(&self.output, x, y, val);
            }
        }
    }
}

// ─── BlendOp ──────────────────────────────────────────────────────────────

/// Blend a source channel into a target channel.
///
/// Reads `<source>`, modifies `<target>`.
pub struct BlendOp {
    /// Channel to read from (e.g. `"noise"`).
    pub source: String,
    /// Channel to modify (e.g. `"height"`).
    pub target: String,
    /// Blend mode.
    pub mode: BlendMode,
    /// Blend strength / amplitude.
    pub strength: f32,
    /// Optional mask channel – blend is scaled by the mask value at each pixel.
    pub mask: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BlendMode {
    /// target += (source - 0.5) * strength
    Add,
    /// target += source * strength (no centering)
    AddRaw,
    /// target *= lerp(1.0, source, strength)
    Multiply,
    /// target = lerp(target, source, strength)
    Mix,
    /// 1 - (1-a)(1-b)
    Screen,
    /// a<0.5 ? 2ab : 1-2(1-a)(1-b)
    Overlay,
    /// Pegtop soft light
    SoftLight,
    /// 2a + b - 1
    LinearLight,
    /// abs(a - b)
    Difference,
    /// min(a, b)
    Darken,
    /// max(a, b)
    Lighten,
    /// a / (1 - b)
    ColorDodge,
    /// 1 - (1-a) / b
    ColorBurn,
    /// a - b
    Subtract,
}

impl BlendMode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "multiply" => BlendMode::Multiply,
            "mix" => BlendMode::Mix,
            "screen" => BlendMode::Screen,
            "overlay" => BlendMode::Overlay,
            "soft_light" => BlendMode::SoftLight,
            "linear_light" => BlendMode::LinearLight,
            "difference" => BlendMode::Difference,
            "darken" => BlendMode::Darken,
            "lighten" => BlendMode::Lighten,
            "color_dodge" => BlendMode::ColorDodge,
            "color_burn" => BlendMode::ColorBurn,
            "subtract" => BlendMode::Subtract,
            "add_raw" => BlendMode::AddRaw,
            _ => BlendMode::Add,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            BlendMode::Add => "add",
            BlendMode::AddRaw => "add_raw",
            BlendMode::Multiply => "multiply",
            BlendMode::Mix => "mix",
            BlendMode::Screen => "screen",
            BlendMode::Overlay => "overlay",
            BlendMode::SoftLight => "soft_light",
            BlendMode::LinearLight => "linear_light",
            BlendMode::Difference => "difference",
            BlendMode::Darken => "darken",
            BlendMode::Lighten => "lighten",
            BlendMode::ColorDodge => "color_dodge",
            BlendMode::ColorBurn => "color_burn",
            BlendMode::Subtract => "subtract",
        }
    }
}

impl TextureOp for BlendOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "blend",
            label: "Blend",
            reads: &["<source>"],
            writes: &[],
            modifies: &["<target>"],
        }
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.target);

        for y in 0..h {
            for x in 0..w {
                let src = field.get(&self.source, x, y);
                let dst = field.get(&self.target, x, y);
                let blended = match self.mode {
                    BlendMode::Add => dst + (src - 0.5) * self.strength,
                    BlendMode::AddRaw => dst + src * self.strength,
                    BlendMode::Multiply => dst * (1.0 + (src - 1.0) * self.strength),
                    BlendMode::Mix => dst + (src - dst) * self.strength,
                    BlendMode::Screen => {
                        let b = 1.0 - (1.0 - dst) * (1.0 - src);
                        lerp(dst, b, self.strength)
                    }
                    BlendMode::Overlay => {
                        let b = if dst < 0.5 {
                            2.0 * dst * src
                        } else {
                            1.0 - 2.0 * (1.0 - dst) * (1.0 - src)
                        };
                        lerp(dst, b, self.strength)
                    }
                    BlendMode::SoftLight => {
                        // Pegtop formula
                        let b = (1.0 - 2.0 * src) * dst * dst + 2.0 * src * dst;
                        lerp(dst, b, self.strength)
                    }
                    BlendMode::LinearLight => {
                        let b = 2.0 * src + dst - 1.0;
                        lerp(dst, b, self.strength)
                    }
                    BlendMode::Difference => {
                        let b = (dst - src).abs();
                        lerp(dst, b, self.strength)
                    }
                    BlendMode::Darken => {
                        let b = dst.min(src);
                        lerp(dst, b, self.strength)
                    }
                    BlendMode::Lighten => {
                        let b = dst.max(src);
                        lerp(dst, b, self.strength)
                    }
                    BlendMode::ColorDodge => {
                        let b = if src >= 1.0 { 1.0 } else { (dst / (1.0 - src)).min(1.0) };
                        lerp(dst, b, self.strength)
                    }
                    BlendMode::ColorBurn => {
                        let b = if src <= 0.0 { 0.0 } else { (1.0 - (1.0 - dst) / src).max(0.0) };
                        lerp(dst, b, self.strength)
                    }
                    BlendMode::Subtract => {
                        let b = dst - src;
                        lerp(dst, b, self.strength)
                    }
                };
                let blended = match &self.mask {
                    Some(ch) => {
                        let m = field.get(ch, x, y);
                        lerp(dst, blended, m)
                    }
                    None => blended,
                };
                field.set(&self.target, x, y, blended.clamp(0.0, 1.0));
            }
        }
    }
}

// ─── MortarGrooveOp ───────────────────────────────────────────────────────

/// Lower the height in mortar zones to create visible grooves.
///
/// Reads `edge_dist`, modifies `height`.
pub struct MortarGrooveOp {
    /// How much to lower the height at the deepest point of the groove.
    pub depth: f32,
    /// Edge distance threshold below which the groove applies.
    pub width: f32,
}

impl TextureOp for MortarGrooveOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "mortar_groove",
            label: "Mortar Groove",
            reads: &["edge_dist"],
            writes: &[],
            modifies: &["height"],
        }
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel("height");

        for y in 0..h {
            for x in 0..w {
                let e = field.get("edge_dist", x, y);
                if e < self.width {
                    let t = e / self.width.max(1e-6); // 0 at boundary, 1 at threshold
                    let s = t * t * (3.0 - 2.0 * t); // smoothstep
                    let cur = field.get("height", x, y);
                    field.set("height", x, y, cur - self.depth * (1.0 - s));
                }
            }
        }
    }
}

// ─── CellColorOp ──────────────────────────────────────────────────────────

/// Assign per-cell color with HSL-space variation (hue-preserving).
///
/// Reads `cell_id`, writes `r`, `g`, `b`.
pub struct CellColorOp {
    /// Base color in linear RGBA.
    pub base_color: [f32; 4],
    /// Variation magnitude (HSL lightness ± this, saturation ± half this).
    pub variation: f32,
}

impl TextureOp for CellColorOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "cell_color",
            label: "Cell Color",
            reads: &["cell_id"],
            writes: &["r", "g", "b"],
            modifies: &[],
        }
    }

    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let seed = rng.seed();
        field.ensure_channel("r");
        field.ensure_channel("g");
        field.ensure_channel("b");

        for y in 0..h {
            for x in 0..w {
                let cell_id = field.get("cell_id", x, y) as u32;
                let color = cell_color_hsl(cell_id, self.base_color, self.variation, seed);
                field.set("r", x, y, color[0]);
                field.set("g", x, y, color[1]);
                field.set("b", x, y, color[2]);
            }
        }
    }
}

/// Deterministic per-cell color using HSL-space variation.
fn cell_color_hsl(
    cell_id: u32,
    base_color: [f32; 4],
    variation: f32,
    rng_seed: u64,
) -> [f32; 4] {
    if variation <= 0.0 {
        return base_color;
    }
    let cell_seed = rng_seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(cell_id as u64);
    let mut cell_rng = SeededRng::new(cell_seed);
    cell_rng.next_color_variation_hsl(base_color, variation)
}

// ─── MortarColorOp ────────────────────────────────────────────────────────

/// Blend mortar color near cell edges.
///
/// Reads `edge_dist`, `r`, `g`, `b`; modifies `r`, `g`, `b`.
pub struct MortarColorOp {
    /// Mortar color in linear RGBA.
    pub color: [f32; 4],
    /// Edge distance threshold for mortar blending.
    pub threshold: f32,
}

impl TextureOp for MortarColorOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "mortar_color",
            label: "Mortar Color",
            reads: &["edge_dist"],
            writes: &[],
            modifies: &["r", "g", "b"],
        }
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;

        for y in 0..h {
            for x in 0..w {
                let e = field.get("edge_dist", x, y);
                if e < self.threshold && self.threshold > 0.0 {
                    let t = e / self.threshold; // 0 at edge, 1 at threshold
                    let r = field.get("r", x, y);
                    let g = field.get("g", x, y);
                    let b = field.get("b", x, y);
                    field.set("r", x, y, self.color[0] * (1.0 - t) + r * t);
                    field.set("g", x, y, self.color[1] * (1.0 - t) + g * t);
                    field.set("b", x, y, self.color[2] * (1.0 - t) + b * t);
                }
            }
        }
    }
}

// ─── DeriveNormalOp ───────────────────────────────────────────────────────

/// Derive a tangent-space normal map from the `height` channel.
///
/// Reads `height`, writes `normal_x`, `normal_y`, `normal_z`.
pub struct DeriveNormalOp {
    /// Normal map strength multiplier.
    pub strength: f32,
}

impl TextureOp for DeriveNormalOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "derive_normal",
            label: "Derive Normal",
            reads: &["height"],
            writes: &["normal_x", "normal_y", "normal_z"],
            modifies: &[],
        }
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

// ─── CellRoughnessOp ─────────────────────────────────────────────────────

/// Assign per-cell roughness with mortar roughness near edges.
///
/// Reads `cell_id`, `edge_dist`; writes `roughness`.
pub struct CellRoughnessOp {
    /// Base surface roughness (0–1).
    pub base: f32,
    /// Per-cell roughness variation magnitude.
    pub variation: f32,
    /// Roughness in mortar regions.
    pub mortar: f32,
    /// Edge distance threshold for mortar roughness blending.
    pub mortar_threshold: f32,
}

impl TextureOp for CellRoughnessOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "cell_roughness",
            label: "Cell Roughness",
            reads: &["cell_id", "edge_dist"],
            writes: &["roughness"],
            modifies: &[],
        }
    }

    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let seed = rng.seed();
        field.ensure_channel("roughness");

        for y in 0..h {
            for x in 0..w {
                let cell_id = field.get("cell_id", x, y) as u32;
                let e = field.get("edge_dist", x, y);

                // Per-cell roughness variation
                let cell_shift = cell_roughness_shift(cell_id, self.variation, seed);
                let surface_rough = (self.base + cell_shift).clamp(0.0, 1.0);

                // Mortar blend
                let roughness =
                    if e < self.mortar_threshold && self.mortar_threshold > 0.0 {
                        let t = e / self.mortar_threshold;
                        self.mortar * (1.0 - t) + surface_rough * t
                    } else {
                        surface_rough
                    };

                field.set("roughness", x, y, roughness);
            }
        }
    }
}

/// Deterministic per-cell roughness shift.
fn cell_roughness_shift(cell_id: u32, variation: f32, rng_seed: u64) -> f32 {
    if variation <= 0.0 {
        return 0.0;
    }
    let cell_seed = rng_seed
        .wrapping_mul(0x517C_C1B7_2722_0A95)
        .wrapping_add(cell_id as u64);
    let mut cell_rng = SeededRng::new(cell_seed);
    cell_rng.next_range(-variation, variation)
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 1: Core Building Blocks
// ═══════════════════════════════════════════════════════════════════════════

// ─── MathOp ──────────────────────────────────────────────────────────────

/// Scalar math operations (Blender Math node equivalent).
///
/// Reads `<input_a>` (and optional `<input_b>`), writes `<output>`.
pub struct MathOp {
    pub operation: MathOperation,
    pub input_a: String,
    pub input_b: Option<String>,
    pub value_b: f32,
    pub output: String,
    pub clamp_output: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MathOperation {
    Add,
    Subtract,
    Multiply,
    Divide,
    Power,
    Sqrt,
    Abs,
    Min,
    Max,
    Fract,
    Modulo,
    Snap,
    Sin,
    Cos,
    LessThan,
    GreaterThan,
    Sign,
    Floor,
    Ceil,
    Round,
    PingPong,
    Wrap,
    Negate,
}

impl MathOperation {
    pub fn from_str(s: &str) -> Self {
        match s {
            "add" => MathOperation::Add,
            "subtract" => MathOperation::Subtract,
            "multiply" => MathOperation::Multiply,
            "divide" => MathOperation::Divide,
            "power" => MathOperation::Power,
            "sqrt" => MathOperation::Sqrt,
            "abs" => MathOperation::Abs,
            "min" => MathOperation::Min,
            "max" => MathOperation::Max,
            "fract" => MathOperation::Fract,
            "modulo" => MathOperation::Modulo,
            "snap" => MathOperation::Snap,
            "sin" => MathOperation::Sin,
            "cos" => MathOperation::Cos,
            "less_than" => MathOperation::LessThan,
            "greater_than" => MathOperation::GreaterThan,
            "sign" => MathOperation::Sign,
            "floor" => MathOperation::Floor,
            "ceil" => MathOperation::Ceil,
            "round" => MathOperation::Round,
            "pingpong" => MathOperation::PingPong,
            "wrap" => MathOperation::Wrap,
            "negate" => MathOperation::Negate,
            _ => MathOperation::Add,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            MathOperation::Add => "add",
            MathOperation::Subtract => "subtract",
            MathOperation::Multiply => "multiply",
            MathOperation::Divide => "divide",
            MathOperation::Power => "power",
            MathOperation::Sqrt => "sqrt",
            MathOperation::Abs => "abs",
            MathOperation::Min => "min",
            MathOperation::Max => "max",
            MathOperation::Fract => "fract",
            MathOperation::Modulo => "modulo",
            MathOperation::Snap => "snap",
            MathOperation::Sin => "sin",
            MathOperation::Cos => "cos",
            MathOperation::LessThan => "less_than",
            MathOperation::GreaterThan => "greater_than",
            MathOperation::Sign => "sign",
            MathOperation::Floor => "floor",
            MathOperation::Ceil => "ceil",
            MathOperation::Round => "round",
            MathOperation::PingPong => "pingpong",
            MathOperation::Wrap => "wrap",
            MathOperation::Negate => "negate",
        }
    }
}

impl TextureOp for MathOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "math",
            label: "Math",
            reads: &["<input_a>"],
            writes: &["<output>"],
            modifies: &[],
        }
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        for y in 0..h {
            for x in 0..w {
                let a = field.get(&self.input_a, x, y);
                let b = self
                    .input_b
                    .as_ref()
                    .map(|ch| field.get(ch, x, y))
                    .unwrap_or(self.value_b);

                let result = match self.operation {
                    MathOperation::Add => a + b,
                    MathOperation::Subtract => a - b,
                    MathOperation::Multiply => a * b,
                    MathOperation::Divide => {
                        if b.abs() < 1e-10 { 0.0 } else { a / b }
                    }
                    MathOperation::Power => a.powf(b),
                    MathOperation::Sqrt => a.max(0.0).sqrt(),
                    MathOperation::Abs => a.abs(),
                    MathOperation::Min => a.min(b),
                    MathOperation::Max => a.max(b),
                    MathOperation::Fract => a.fract(),
                    MathOperation::Modulo => {
                        if b.abs() < 1e-10 { 0.0 } else { a % b }
                    }
                    MathOperation::Snap => {
                        if b.abs() < 1e-10 { a } else { (a / b).floor() * b }
                    }
                    MathOperation::Sin => (a * std::f32::consts::TAU).sin(),
                    MathOperation::Cos => (a * std::f32::consts::TAU).cos(),
                    MathOperation::LessThan => if a < b { 1.0 } else { 0.0 },
                    MathOperation::GreaterThan => if a > b { 1.0 } else { 0.0 },
                    MathOperation::Sign => {
                        if a > 0.0 { 1.0 } else if a < 0.0 { -1.0 } else { 0.0 }
                    }
                    MathOperation::Floor => a.floor(),
                    MathOperation::Ceil => a.ceil(),
                    MathOperation::Round => a.round(),
                    MathOperation::PingPong => {
                        if b.abs() < 1e-10 {
                            0.0
                        } else {
                            let t = (a / b).rem_euclid(2.0);
                            if t <= 1.0 { t * b } else { (2.0 - t) * b }
                        }
                    }
                    MathOperation::Wrap => {
                        // Wrap a into [0, b)
                        if b.abs() < 1e-10 { 0.0 } else { a.rem_euclid(b) }
                    }
                    MathOperation::Negate => -a,
                };

                let result = if self.clamp_output {
                    result.clamp(0.0, 1.0)
                } else {
                    result
                };
                field.set(&self.output, x, y, result);
            }
        }
    }
}

// ─── MapRangeOp ──────────────────────────────────────────────────────────

/// Remap a value from one range to another.
///
/// Reads `<input>`, writes `<output>`.
pub struct MapRangeOp {
    pub input: String,
    pub output: String,
    pub from_min: f32,
    pub from_max: f32,
    pub to_min: f32,
    pub to_max: f32,
    pub interpolation: MapRangeInterp,
    pub clamp_output: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MapRangeInterp {
    Linear,
    Smoothstep,
    Smootherstep,
}

impl MapRangeInterp {
    pub fn from_str(s: &str) -> Self {
        match s {
            "smoothstep" => MapRangeInterp::Smoothstep,
            "smootherstep" => MapRangeInterp::Smootherstep,
            _ => MapRangeInterp::Linear,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            MapRangeInterp::Linear => "linear",
            MapRangeInterp::Smoothstep => "smoothstep",
            MapRangeInterp::Smootherstep => "smootherstep",
        }
    }
}

impl TextureOp for MapRangeOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "map_range",
            label: "Map Range",
            reads: &["<input>"],
            writes: &["<output>"],
            modifies: &[],
        }
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let from_range = self.from_max - self.from_min;
        field.ensure_channel(&self.output);

        for y in 0..h {
            for x in 0..w {
                let v = field.get(&self.input, x, y);
                let t = if from_range.abs() < 1e-10 {
                    0.0
                } else {
                    (v - self.from_min) / from_range
                };

                let t = match self.interpolation {
                    MapRangeInterp::Linear => t,
                    MapRangeInterp::Smoothstep => t.clamp(0.0, 1.0).powi(2) * (3.0 - 2.0 * t.clamp(0.0, 1.0)),
                    MapRangeInterp::Smootherstep => {
                        let tc = t.clamp(0.0, 1.0);
                        tc * tc * tc * (tc * (tc * 6.0 - 15.0) + 10.0)
                    }
                };

                let mut result = self.to_min + t * (self.to_max - self.to_min);
                if self.clamp_output {
                    let lo = self.to_min.min(self.to_max);
                    let hi = self.to_min.max(self.to_max);
                    result = result.clamp(lo, hi);
                }
                field.set(&self.output, x, y, result);
            }
        }
    }
}

// ─── ColorRampOp ─────────────────────────────────────────────────────────

/// Map a scalar channel to a multi-stop color gradient.
///
/// Reads `<input>`, writes `r`, `g`, `b`.
pub struct ColorRampOp {
    pub input: String,
    pub interpolation: ColorRampInterp,
    pub stops: Vec<ColorStop>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorRampInterp {
    Linear,
    Constant,
    Ease,
}

impl ColorRampInterp {
    pub fn from_str(s: &str) -> Self {
        match s {
            "constant" => ColorRampInterp::Constant,
            "ease" => ColorRampInterp::Ease,
            _ => ColorRampInterp::Linear,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ColorStop {
    pub position: f32,
    pub color: [f32; 3],
}

impl TextureOp for ColorRampOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "color_ramp",
            label: "Color Ramp",
            reads: &["<input>"],
            writes: &["r", "g", "b"],
            modifies: &[],
        }
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel("r");
        field.ensure_channel("g");
        field.ensure_channel("b");

        if self.stops.is_empty() {
            return;
        }

        for y in 0..h {
            for x in 0..w {
                let v = field.get(&self.input, x, y).clamp(0.0, 1.0);
                let (r, g, b) = self.evaluate(v);
                field.set("r", x, y, r);
                field.set("g", x, y, g);
                field.set("b", x, y, b);
            }
        }
    }
}

impl ColorRampOp {
    fn evaluate(&self, t: f32) -> (f32, f32, f32) {
        if self.stops.len() == 1 {
            let s = &self.stops[0];
            return (s.color[0], s.color[1], s.color[2]);
        }

        // Find the surrounding stops
        if t <= self.stops[0].position {
            let s = &self.stops[0];
            return (s.color[0], s.color[1], s.color[2]);
        }
        if t >= self.stops[self.stops.len() - 1].position {
            let s = &self.stops[self.stops.len() - 1];
            return (s.color[0], s.color[1], s.color[2]);
        }

        for i in 0..self.stops.len() - 1 {
            let a = &self.stops[i];
            let b = &self.stops[i + 1];
            if t >= a.position && t <= b.position {
                let range = b.position - a.position;
                if range < 1e-10 {
                    return (a.color[0], a.color[1], a.color[2]);
                }
                let frac = (t - a.position) / range;
                let frac = match self.interpolation {
                    ColorRampInterp::Linear => frac,
                    ColorRampInterp::Constant => 0.0,
                    ColorRampInterp::Ease => frac * frac * (3.0 - 2.0 * frac),
                };
                return (
                    lerp(a.color[0], b.color[0], frac),
                    lerp(a.color[1], b.color[1], frac),
                    lerp(a.color[2], b.color[2], frac),
                );
            }
        }

        let s = &self.stops[self.stops.len() - 1];
        (s.color[0], s.color[1], s.color[2])
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

impl TextureOp for CheckerTextureOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "checker_texture",
            label: "Checker Texture",
            reads: &[],
            writes: &["<output>"],
            modifies: &[],
        }
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        for y in 0..h {
            for x in 0..w {
                let u = x as f32 / w as f32;
                let v = y as f32 / h as f32;
                let check = ((u * self.scale_x).floor() as i32 + (v * self.scale_y).floor() as i32) % 2;
                field.set(&self.output, x, y, check.abs() as f32);
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 2: Pattern Generators
// ═══════════════════════════════════════════════════════════════════════════

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

impl TextureOp for GradientTextureOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "gradient_texture",
            label: "Gradient Texture",
            reads: &[],
            writes: &["<output>"],
            modifies: &[],
        }
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

impl TextureOp for WaveTextureOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "wave_texture",
            label: "Wave Texture",
            reads: &[],
            writes: &["<output>"],
            modifies: &[],
        }
    }

    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        // Build optional distortion noise
        let distortion_noise: Option<Box<dyn NoiseSource>> = if self.distortion > 0.0 && self.detail > 0 {
            let noise_rng = rng.fork("wave_distortion");
            Some(Box::new(
                Fbm::new(PerlinNoise::new(noise_rng.seed()))
                    .with_octaves(self.detail)
                    .with_frequency(self.scale as f64),
            ))
        } else {
            None
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
                    coord + noise.sample_2d(u * self.scale as f64, v * self.scale as f64) * self.distortion as f64
                } else {
                    coord
                };

                let phase = distorted * self.scale as f64;
                let val = match self.wave_type {
                    WaveType::Sine => (phase * std::f64::consts::TAU).sin() * 0.5 + 0.5,
                    WaveType::Saw => phase.fract(),
                    WaveType::Triangle => {
                        let t = phase.fract();
                        if t < 0.5 { t * 2.0 } else { 2.0 - t * 2.0 }
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

impl TextureOp for WhiteNoiseOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "white_noise",
            label: "White Noise",
            reads: &[],
            writes: &["<output>"],
            modifies: &[],
        }
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

impl TextureOp for VoronoiTextureOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "voronoi_texture",
            label: "Voronoi Texture",
            reads: &[],
            writes: &["<output>"],
            modifies: &[],
        }
    }

    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let seed = rng.fork("voronoi").seed();
        field.ensure_channel(&self.output);
        if let Some(ref cell_ch) = self.cell_output {
            field.ensure_channel(cell_ch);
        }

        for y in 0..h {
            for x in 0..w {
                let u = x as f64 / w as f64;
                let v = y as f64 / h as f64;
                let result = crate::algorithms::noise::voronoi_sample(
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

impl TextureOp for MusgraveTextureOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "musgrave_texture",
            label: "Musgrave Texture",
            reads: &[],
            writes: &["<output>"],
            modifies: &[],
        }
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

        for y in 0..h {
            for x in 0..w {
                let mut nx = x as f64 / w as f64 * self.scale_x as f64;
                let mut ny = y as f64 / h as f64 * self.scale_y as f64;
                if use_cell_offset {
                    let cid = field.get("cell_id", x, y) as u32;
                    let (ox, oy) = cell_noise_offset(cid, seed);
                    nx += ox;
                    ny += oy;
                }
                let val = crate::algorithms::noise::musgrave_sample(
                    source.as_ref(),
                    nx,
                    ny,
                    self.musgrave_type,
                    self.frequency as f64,
                    self.octaves,
                    self.lacunarity as f64,
                    self.dimension as f64,
                    self.offset as f64,
                    self.gain as f64,
                );
                // Remap roughly to 0-1
                let val = (val as f32 * 0.5 + 0.5).clamp(0.0, 1.0);
                field.set(&self.output, x, y, val);
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 3: Color & Value Manipulation
// ═══════════════════════════════════════════════════════════════════════════

// ─── InvertOp ────────────────────────────────────────────────────────────

/// Value inversion.
///
/// Reads `<input>`, writes `<output>`.
pub struct InvertOp {
    pub input: String,
    pub output: String,
    pub factor: f32,
}

impl TextureOp for InvertOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "invert",
            label: "Invert",
            reads: &["<input>"],
            writes: &["<output>"],
            modifies: &[],
        }
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        for y in 0..h {
            for x in 0..w {
                let v = field.get(&self.input, x, y);
                field.set(&self.output, x, y, lerp(v, 1.0 - v, self.factor));
            }
        }
    }
}

// ─── BrightnessContrastOp ────────────────────────────────────────────────

/// Brightness and contrast adjustment.
///
/// Reads `<input>`, writes `<output>`.
pub struct BrightnessContrastOp {
    pub input: String,
    pub output: String,
    pub brightness: f32,
    pub contrast: f32,
}

impl TextureOp for BrightnessContrastOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "brightness_contrast",
            label: "Brightness/Contrast",
            reads: &["<input>"],
            writes: &["<output>"],
            modifies: &[],
        }
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        for y in 0..h {
            for x in 0..w {
                let v = field.get(&self.input, x, y);
                let result = (v - 0.5) * (1.0 + self.contrast) + 0.5 + self.brightness;
                field.set(&self.output, x, y, result.clamp(0.0, 1.0));
            }
        }
    }
}

// ─── HsvAdjustOp ─────────────────────────────────────────────────────────

/// Hue/saturation/value adjustment on `r`, `g`, `b` channels.
///
/// Modifies `r`, `g`, `b` in-place.
pub struct HsvAdjustOp {
    pub hue_offset: f32,
    pub saturation_factor: f32,
    pub value_factor: f32,
}

impl TextureOp for HsvAdjustOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "hsv_adjust",
            label: "HSV Adjust",
            reads: &[],
            writes: &[],
            modifies: &["r", "g", "b"],
        }
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel("r");
        field.ensure_channel("g");
        field.ensure_channel("b");

        for y in 0..h {
            for x in 0..w {
                let r = field.get("r", x, y);
                let g = field.get("g", x, y);
                let b = field.get("b", x, y);

                let (mut hue, mut sat, mut val) = rgb_to_hsv(r, g, b);
                hue = (hue + self.hue_offset).fract();
                if hue < 0.0 {
                    hue += 1.0;
                }
                sat = (sat * self.saturation_factor).clamp(0.0, 1.0);
                val = (val * self.value_factor).clamp(0.0, 1.0);

                let (nr, ng, nb) = hsv_to_rgb(hue, sat, val);
                field.set("r", x, y, nr.clamp(0.0, 1.0));
                field.set("g", x, y, ng.clamp(0.0, 1.0));
                field.set("b", x, y, nb.clamp(0.0, 1.0));
            }
        }
    }
}

// ─── GammaOp ─────────────────────────────────────────────────────────────

/// Gamma correction.
///
/// Reads `<input>`, writes `<output>`.
pub struct GammaOp {
    pub input: String,
    pub output: String,
    pub gamma: f32,
}

impl TextureOp for GammaOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "gamma",
            label: "Gamma",
            reads: &["<input>"],
            writes: &["<output>"],
            modifies: &[],
        }
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        for y in 0..h {
            for x in 0..w {
                let v = field.get(&self.input, x, y);
                field.set(&self.output, x, y, v.max(0.0).powf(self.gamma));
            }
        }
    }
}

// ─── ClampOp ─────────────────────────────────────────────────────────────

/// Constrain values to a min/max range.
///
/// Reads `<input>`, writes `<output>`.
pub struct ClampOp {
    pub input: String,
    pub output: String,
    pub min: f32,
    pub max: f32,
}

impl TextureOp for ClampOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "clamp",
            label: "Clamp",
            reads: &["<input>"],
            writes: &["<output>"],
            modifies: &[],
        }
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel(&self.output);

        for y in 0..h {
            for x in 0..w {
                let v = field.get(&self.input, x, y);
                field.set(&self.output, x, y, v.clamp(self.min, self.max));
            }
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Phase 4: Spatial Filters
// ═══════════════════════════════════════════════════════════════════════════

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

impl TextureOp for BlurOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "blur",
            label: "Blur",
            reads: &["<input>"],
            writes: &["<output>"],
            modifies: &[],
        }
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

impl TextureOp for SharpenOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "sharpen",
            label: "Sharpen",
            reads: &["<input>"],
            writes: &["<output>"],
            modifies: &[],
        }
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
            field.set(&self.output, i as u32 % w, i as u32 / w, result.clamp(0.0, 1.0));
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

impl TextureOp for EdgeDetectOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "edge_detect",
            label: "Edge Detect",
            reads: &["<input>"],
            writes: &["<output>"],
            modifies: &[],
        }
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
                let gx = -sample(ix - 1, iy - 1) - 2.0 * sample(ix - 1, iy) - sample(ix - 1, iy + 1)
                    + sample(ix + 1, iy - 1) + 2.0 * sample(ix + 1, iy) + sample(ix + 1, iy + 1);
                let gy = -sample(ix - 1, iy - 1) - 2.0 * sample(ix, iy - 1) - sample(ix + 1, iy - 1)
                    + sample(ix - 1, iy + 1) + 2.0 * sample(ix, iy + 1) + sample(ix + 1, iy + 1);

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

impl TextureOp for EdgeErodeOp {
    fn port_info(&self) -> OpPortInfo {
        OpPortInfo {
            op_type: "edge_erode",
            label: "Edge Erode",
            reads: &[],
            writes: &[],
            modifies: &["cell_id", "edge_dist", "mask"],
        }
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
                let mask_val = noise_mask.sample_2d(u as f64, v as f64) as f32 * 0.5 + 0.5;
                if mask_val < self.threshold {
                    continue;
                }

                // Fade strength: full at edge (ed=0), zero at edge_width
                let factor = 1.0 - (ed / self.edge_width);

                // Noise returns ~[-1, 1]; scale by strength and fade factor
                let dx = noise_x.sample_2d(u as f64, v as f64) as f32 * self.strength * factor;
                let dy = noise_y.sample_2d(u as f64, v as f64) as f32 * self.strength * factor;

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

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brick_grid_creates_cells() {
        let mut field = TextureField::new(64, 64);
        let mut rng = SeededRng::new(42);
        let op = BrickGridOp {
            columns: 4,
            rows: 8,
            stagger: 0.5,
            gap_width: 0.04,
            width_variation: 0.0,
            row_height_variation: 0.0,
            warp_x: None,
            warp_y: None,
            warp_strength: 0.0,
        };
        op.apply(&mut field, &mut rng);

        // Should have cell_id and edge_dist channels
        assert!(field.has_channel("cell_id"));
        assert!(field.has_channel("edge_dist"));

        // Expected unique cell count: 4 * 8 = 32
        let mut ids = std::collections::HashSet::new();
        for y in 0..64 {
            for x in 0..64 {
                ids.insert(field.get("cell_id", x, y) as u32);
            }
        }
        assert_eq!(ids.len(), 32, "expected 32 unique cells, got {}", ids.len());
    }

    #[test]
    fn brick_grid_edge_dist_range() {
        let mut field = TextureField::new(128, 128);
        let mut rng = SeededRng::new(42);
        let op = BrickGridOp {
            columns: 8,
            rows: 16,
            stagger: 0.5,
            gap_width: 0.04,
            width_variation: 0.0,
            row_height_variation: 0.0,
            warp_x: None,
            warp_y: None,
            warp_strength: 0.0,
        };
        op.apply(&mut field, &mut rng);

        for y in 0..128 {
            for x in 0..128 {
                let e = field.get("edge_dist", x, y);
                assert!(e >= 0.0, "edge_dist negative at ({x},{y}): {e}");
                assert!(e <= 0.5, "edge_dist > 0.5 at ({x},{y}): {e}");
            }
        }
    }

    #[test]
    fn brick_grid_stagger_offset() {
        let mut field = TextureField::new(128, 128);
        let mut rng = SeededRng::new(42);
        let op = BrickGridOp {
            columns: 4,
            rows: 4,
            stagger: 0.5,
            gap_width: 0.04,
            width_variation: 0.0,
            row_height_variation: 0.0,
            warp_x: None,
            warp_y: None,
            warp_strength: 0.0,
        };
        op.apply(&mut field, &mut rng);

        // Row 0 and row 1 should have different column offsets (additive stagger)
        let row0_mid = field.get("cell_id", 16, 8) as u32; // middle of first cell in row 0
        let row1_mid = field.get("cell_id", 16, 40) as u32; // same x, row 1
        // They should be in different cells due to stagger
        assert_ne!(
            row0_mid % 4,
            row1_mid % 4,
            "stagger should offset rows progressively"
        );
    }

    #[test]
    fn cell_height_writes_height() {
        let mut field = TextureField::new(32, 32);
        // Pre-populate cell_id
        for y in 0..32 {
            for x in 0..32 {
                field.set("cell_id", x, y, ((y / 16) * 2 + x / 16) as f32);
            }
        }
        let mut rng = SeededRng::new(42);
        let op = CellHeightOp { variation: 0.5 };
        op.apply(&mut field, &mut rng);

        assert!(field.has_channel("height"));
        for y in 0..32 {
            for x in 0..32 {
                let h = field.get("height", x, y);
                assert!(h >= 0.0 && h <= 1.0, "height out of range at ({x},{y}): {h}");
            }
        }
    }

    #[test]
    fn mortar_groove_lowers_height() {
        let mut field = TextureField::new(32, 32);
        // Set uniform height and varying edge_dist
        for y in 0..32 {
            for x in 0..32 {
                field.set("height", x, y, 0.5);
                let e = x as f32 / 32.0 * 0.5; // 0 at left, 0.5 at right
                field.set("edge_dist", x, y, e);
            }
        }
        let mut rng = SeededRng::new(42);
        let op = MortarGrooveOp {
            depth: 0.2,
            width: 0.1,
        };
        op.apply(&mut field, &mut rng);

        // Left edge (edge_dist ≈ 0) should have lowered height
        let h_edge = field.get("height", 0, 0);
        assert!(h_edge < 0.5, "mortar groove should lower height at edge: {h_edge}");

        // Right side (edge_dist > width) should be unchanged
        let h_interior = field.get("height", 31, 0);
        assert!(
            (h_interior - 0.5).abs() < 1e-5,
            "interior height should be unchanged: {h_interior}"
        );
    }

    #[test]
    fn cell_color_preserves_hue() {
        let mut field = TextureField::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                field.set("cell_id", x, y, (y * 32 + x) as f32);
            }
        }
        let mut rng = SeededRng::new(42);
        let op = CellColorOp {
            base_color: [0.47, 0.28, 0.14, 1.0], // brown
            variation: 0.12,
        };
        op.apply(&mut field, &mut rng);

        // All colors should stay in valid range
        for y in 0..32 {
            for x in 0..32 {
                let r = field.get("r", x, y);
                let g = field.get("g", x, y);
                let b = field.get("b", x, y);
                assert!(r >= 0.0 && r <= 1.0);
                assert!(g >= 0.0 && g <= 1.0);
                assert!(b >= 0.0 && b <= 1.0);
            }
        }
    }

    #[test]
    fn mortar_color_blends_near_edge() {
        let mut field = TextureField::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                field.set("r", x, y, 0.5);
                field.set("g", x, y, 0.3);
                field.set("b", x, y, 0.1);
                field.set("edge_dist", x, y, x as f32 / 32.0 * 0.5);
            }
        }
        let mut rng = SeededRng::new(42);
        let op = MortarColorOp {
            color: [0.2, 0.2, 0.2, 1.0],
            threshold: 0.1,
        };
        op.apply(&mut field, &mut rng);

        // Left edge (edge_dist ≈ 0) should be close to mortar color
        let r = field.get("r", 0, 0);
        assert!(
            (r - 0.2).abs() < 0.1,
            "edge should be near mortar color, got r={r}"
        );

        // Right side (edge_dist > threshold) should be unchanged
        let r = field.get("r", 31, 0);
        assert!(
            (r - 0.5).abs() < 1e-5,
            "interior should be unchanged, got r={r}"
        );
    }

    #[test]
    fn derive_normal_z_positive() {
        let mut field = TextureField::new(32, 32);
        // Flat surface
        for y in 0..32 {
            for x in 0..32 {
                field.set("height", x, y, 0.5);
            }
        }
        let mut rng = SeededRng::new(42);
        let op = DeriveNormalOp { strength: 1.5 };
        op.apply(&mut field, &mut rng);

        for y in 0..32 {
            for x in 0..32 {
                let nz = field.get("normal_z", x, y);
                assert!(nz > 0.0, "normal Z should be positive at ({x},{y}): {nz}");
            }
        }
    }

    #[test]
    fn cell_roughness_range() {
        let mut field = TextureField::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                field.set("cell_id", x, y, (x / 8) as f32);
                field.set("edge_dist", x, y, (y as f32 / 32.0) * 0.5);
            }
        }
        let mut rng = SeededRng::new(42);
        let op = CellRoughnessOp {
            base: 0.75,
            variation: 0.05,
            mortar: 0.55,
            mortar_threshold: 0.06,
        };
        op.apply(&mut field, &mut rng);

        for y in 0..32 {
            for x in 0..32 {
                let r = field.get("roughness", x, y);
                assert!(
                    r >= 0.0 && r <= 1.0,
                    "roughness out of range at ({x},{y}): {r}"
                );
            }
        }
    }

    #[test]
    fn noise_layer_writes_output_channel() {
        let mut field = TextureField::new(32, 32);
        let mut rng = SeededRng::new(42);
        let op = NoiseLayerOp {
            output: "my_signal".into(),
            frequency: 10.0,
            octaves: 4,
            noise_type: NoiseType::Perlin,
            scale_x: 1.0,
            scale_y: 1.0,
            cell_offset: false,
        };
        op.apply(&mut field, &mut rng);

        assert!(field.has_channel("my_signal"));
        // Values should be in 0–1 range with variation
        let mut any_varied = false;
        for y in 0..32 {
            for x in 0..32 {
                let v = field.get("my_signal", x, y);
                assert!((0.0..=1.0).contains(&v), "output out of range: {v}");
                if (v - 0.5).abs() > 0.01 {
                    any_varied = true;
                }
            }
        }
        assert!(any_varied, "output channel should have variation");
    }

    #[test]
    fn blend_add_modifies_target() {
        let mut field = TextureField::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                field.set("height", x, y, 0.5);
                field.set("noise", x, y, 0.7); // above 0.5 center
            }
        }
        let mut rng = SeededRng::new(42);
        let op = BlendOp {
            source: "noise".into(),
            target: "height".into(),
            mode: BlendMode::Add,
            strength: 0.2,
            mask: None,
        };
        op.apply(&mut field, &mut rng);

        // height should have increased: 0.5 + (0.7 - 0.5) * 0.2 = 0.54
        let h = field.get("height", 0, 0);
        assert!((h - 0.54).abs() < 1e-4, "blend add result: {h}");
    }

    #[test]
    fn full_pipeline_integration() {
        let mut field = TextureField::new(64, 64);
        let mut rng = SeededRng::new(42);

        // Run all ops in sequence
        let ops: Vec<Box<dyn TextureOp>> = vec![
            Box::new(BrickGridOp {
                columns: 4,
                rows: 8,
                stagger: 0.5,
                gap_width: 0.04,
                width_variation: 0.0,
                row_height_variation: 0.0,
                warp_x: None,
                warp_y: None,
                warp_strength: 0.0,
            }),
            Box::new(CellHeightOp { variation: 0.3 }),
            Box::new(MortarGrooveOp {
                depth: 0.15,
                width: 0.04,
            }),
            Box::new(CellColorOp {
                base_color: [0.47, 0.28, 0.14, 1.0],
                variation: 0.12,
            }),
            Box::new(MortarColorOp {
                color: [0.2, 0.18, 0.15, 1.0],
                threshold: 0.06,
            }),
            Box::new(DeriveNormalOp { strength: 1.5 }),
            Box::new(CellRoughnessOp {
                base: 0.75,
                variation: 0.05,
                mortar: 0.55,
                mortar_threshold: 0.06,
            }),
        ];

        for (i, op) in ops.iter().enumerate() {
            let mut op_rng = rng.fork(&format!("op_{i}"));
            op.apply(&mut field, &mut op_rng);
        }

        // Verify all output channels exist
        assert!(field.has_channel("r"));
        assert!(field.has_channel("g"));
        assert!(field.has_channel("b"));
        assert!(field.has_channel("normal_x"));
        assert!(field.has_channel("normal_y"));
        assert!(field.has_channel("normal_z"));
        assert!(field.has_channel("roughness"));

        // Verify output images are valid
        let albedo = field.to_albedo_image();
        assert!(albedo.validate().is_ok());
        let normal = field.to_normal_image();
        assert!(normal.validate().is_ok());
        let roughness = field.to_roughness_image();
        assert!(roughness.validate().is_ok());
    }

    // ── Phase 1 tests ────────────────────────────────────────────────────

    #[test]
    fn blend_screen_mode() {
        let mut field = TextureField::new(4, 4);
        for y in 0..4 { for x in 0..4 {
            field.set("a", x, y, 0.5);
            field.set("b", x, y, 0.5);
        }}
        let mut rng = SeededRng::new(42);
        let op = BlendOp { source: "b".into(), target: "a".into(), mode: BlendMode::Screen, strength: 1.0, mask: None };
        op.apply(&mut field, &mut rng);
        let v = field.get("a", 0, 0);
        assert!((v - 0.75).abs() < 0.01, "screen(0.5,0.5) should be ~0.75, got {v}");
    }

    #[test]
    fn math_op_power() {
        let mut field = TextureField::new(4, 4);
        for y in 0..4 { for x in 0..4 { field.set("input", x, y, 0.5); }}
        let mut rng = SeededRng::new(42);
        let op = MathOp {
            operation: MathOperation::Power, input_a: "input".into(),
            input_b: None, value_b: 2.0, output: "out".into(), clamp_output: false,
        };
        op.apply(&mut field, &mut rng);
        let v = field.get("out", 0, 0);
        assert!((v - 0.25).abs() < 0.01, "0.5^2 should be 0.25, got {v}");
    }

    #[test]
    fn math_op_sin() {
        let mut field = TextureField::new(4, 4);
        field.set("input", 0, 0, 0.25); // sin(0.25 * TAU) = sin(PI/2) = 1.0
        let mut rng = SeededRng::new(42);
        let op = MathOp {
            operation: MathOperation::Sin, input_a: "input".into(),
            input_b: None, value_b: 0.0, output: "out".into(), clamp_output: false,
        };
        op.apply(&mut field, &mut rng);
        let v = field.get("out", 0, 0);
        assert!((v - 1.0).abs() < 0.01, "sin(0.25*TAU) should be ~1.0, got {v}");
    }

    #[test]
    fn map_range_linear() {
        let mut field = TextureField::new(4, 4);
        field.set("input", 0, 0, 0.5);
        let mut rng = SeededRng::new(42);
        let op = MapRangeOp {
            input: "input".into(), output: "out".into(),
            from_min: 0.3, from_max: 0.7, to_min: 0.0, to_max: 1.0,
            interpolation: MapRangeInterp::Linear, clamp_output: true,
        };
        op.apply(&mut field, &mut rng);
        let v = field.get("out", 0, 0);
        assert!((v - 0.5).abs() < 0.01, "map_range(0.5, 0.3..0.7 -> 0..1) = 0.5, got {v}");
    }

    #[test]
    fn color_ramp_two_stops() {
        let mut field = TextureField::new(4, 4);
        field.set("input", 0, 0, 0.5);
        let mut rng = SeededRng::new(42);
        let op = ColorRampOp {
            input: "input".into(),
            interpolation: ColorRampInterp::Linear,
            stops: vec![
                ColorStop { position: 0.0, color: [0.0, 0.0, 0.0] },
                ColorStop { position: 1.0, color: [1.0, 1.0, 1.0] },
            ],
        };
        op.apply(&mut field, &mut rng);
        let r = field.get("r", 0, 0);
        assert!((r - 0.5).abs() < 0.01, "color_ramp midpoint should be 0.5, got {r}");
    }

    #[test]
    fn checker_texture_pattern() {
        let mut field = TextureField::new(8, 8);
        let mut rng = SeededRng::new(42);
        let op = CheckerTextureOp { output: "check".into(), scale_x: 2.0, scale_y: 2.0 };
        op.apply(&mut field, &mut rng);
        let v00 = field.get("check", 0, 0); // top-left quadrant
        let v40 = field.get("check", 4, 0); // top-right quadrant
        assert_ne!(v00, v40, "checker should alternate");
    }

    // ── Phase 2 tests ────────────────────────────────────────────────────

    #[test]
    fn gradient_texture_linear() {
        let mut field = TextureField::new(32, 32);
        let mut rng = SeededRng::new(42);
        let op = GradientTextureOp { output: "grad".into(), gradient_type: GradientType::Linear };
        op.apply(&mut field, &mut rng);
        let left = field.get("grad", 0, 16);
        let right = field.get("grad", 31, 16);
        assert!(right > left, "linear gradient should increase left to right");
    }

    #[test]
    fn wave_texture_variation() {
        let mut field = TextureField::new(32, 32);
        let mut rng = SeededRng::new(42);
        let op = WaveTextureOp {
            output: "wave".into(), wave_type: WaveType::Sine,
            direction: WaveDirection::X, scale: 4.0, distortion: 0.0, detail: 0,
        };
        op.apply(&mut field, &mut rng);
        let mut has_variation = false;
        let first = field.get("wave", 0, 0);
        for x in 1..32 {
            if (field.get("wave", x, 0) - first).abs() > 0.01 { has_variation = true; break; }
        }
        assert!(has_variation, "wave should vary across x");
    }

    #[test]
    fn white_noise_variation() {
        let mut field = TextureField::new(32, 32);
        let mut rng = SeededRng::new(42);
        let op = WhiteNoiseOp { output: "noise".into() };
        op.apply(&mut field, &mut rng);
        let first = field.get("noise", 0, 0);
        let differs = (1..32).any(|x| (field.get("noise", x, 0) - first).abs() > 0.01);
        assert!(differs, "white noise should have variation");
    }

    #[test]
    fn voronoi_texture_output() {
        let mut field = TextureField::new(32, 32);
        let mut rng = SeededRng::new(42);
        let op = VoronoiTextureOp {
            output: "vor".into(), cell_output: Some("cell".into()),
            scale: 4.0, randomness: 1.0,
            feature: VoronoiFeature::F1, metric: VoronoiMetric::Euclidean,
        };
        op.apply(&mut field, &mut rng);
        assert!(field.has_channel("vor"));
        assert!(field.has_channel("cell"));
        let differs = (1..32).any(|x| field.get("vor", x, 0) != field.get("vor", 0, 0));
        assert!(differs, "voronoi should have variation");
    }

    #[test]
    fn musgrave_texture_output() {
        let mut field = TextureField::new(32, 32);
        let mut rng = SeededRng::new(42);
        let op = MusgraveTextureOp {
            output: "mus".into(), noise_type: NoiseType::Perlin,
            musgrave_type: MusgraveType::RidgedMultifractal,
            frequency: 4.0, octaves: 4, lacunarity: 2.0,
            dimension: 1.0, offset: 1.0, gain: 2.0,
            scale_x: 1.0, scale_y: 1.0, cell_offset: false,
        };
        op.apply(&mut field, &mut rng);
        assert!(field.has_channel("mus"));
        let differs = (1..32).any(|x| field.get("mus", x, 0) != field.get("mus", 0, 0));
        assert!(differs, "musgrave should have variation");
    }

    // ── Phase 3 tests ────────────────────────────────────────────────────

    #[test]
    fn invert_op() {
        let mut field = TextureField::new(4, 4);
        field.set("input", 0, 0, 0.3);
        let mut rng = SeededRng::new(42);
        let op = InvertOp { input: "input".into(), output: "out".into(), factor: 1.0 };
        op.apply(&mut field, &mut rng);
        let v = field.get("out", 0, 0);
        assert!((v - 0.7).abs() < 0.01, "invert(0.3) should be 0.7, got {v}");
    }

    #[test]
    fn brightness_contrast_op() {
        let mut field = TextureField::new(4, 4);
        field.set("input", 0, 0, 0.5);
        let mut rng = SeededRng::new(42);
        let op = BrightnessContrastOp {
            input: "input".into(), output: "out".into(), brightness: 0.1, contrast: 0.0,
        };
        op.apply(&mut field, &mut rng);
        let v = field.get("out", 0, 0);
        assert!((v - 0.6).abs() < 0.01, "brightness +0.1 on 0.5 should be 0.6, got {v}");
    }

    #[test]
    fn gamma_op() {
        let mut field = TextureField::new(4, 4);
        field.set("input", 0, 0, 0.25);
        let mut rng = SeededRng::new(42);
        let op = GammaOp { input: "input".into(), output: "out".into(), gamma: 0.5 };
        op.apply(&mut field, &mut rng);
        let v = field.get("out", 0, 0);
        assert!((v - 0.5).abs() < 0.01, "gamma(0.25, 0.5) should be 0.5, got {v}");
    }

    #[test]
    fn clamp_op() {
        let mut field = TextureField::new(4, 4);
        field.set("input", 0, 0, 0.8);
        let mut rng = SeededRng::new(42);
        let op = ClampOp { input: "input".into(), output: "out".into(), min: 0.2, max: 0.6 };
        op.apply(&mut field, &mut rng);
        let v = field.get("out", 0, 0);
        assert!((v - 0.6).abs() < 0.01, "clamp(0.8, 0.2, 0.6) should be 0.6, got {v}");
    }

    #[test]
    fn hsv_adjust_op() {
        let mut field = TextureField::new(4, 4);
        for y in 0..4 { for x in 0..4 {
            field.set("r", x, y, 0.5);
            field.set("g", x, y, 0.3);
            field.set("b", x, y, 0.1);
        }}
        let mut rng = SeededRng::new(42);
        let op = HsvAdjustOp { hue_offset: 0.0, saturation_factor: 1.0, value_factor: 1.0 };
        op.apply(&mut field, &mut rng);
        // Identity transform should preserve values
        let r = field.get("r", 0, 0);
        let g = field.get("g", 0, 0);
        let b = field.get("b", 0, 0);
        assert!((r - 0.5).abs() < 0.02, "r should be ~0.5, got {r}");
        assert!((g - 0.3).abs() < 0.02, "g should be ~0.3, got {g}");
        assert!((b - 0.1).abs() < 0.02, "b should be ~0.1, got {b}");
    }

    // ── Phase 4 tests ────────────────────────────────────────────────────

    #[test]
    fn blur_op_smooths() {
        let mut field = TextureField::new(16, 16);
        // Sharp edge
        for y in 0..16 { for x in 0..16 {
            field.set("input", x, y, if x < 8 { 0.0 } else { 1.0 });
        }}
        let mut rng = SeededRng::new(42);
        let op = BlurOp { input: "input".into(), output: "out".into(), radius: 2, sigma: 1.0 };
        op.apply(&mut field, &mut rng);
        // Middle should now be between 0 and 1 (blurred)
        let v = field.get("out", 8, 8);
        assert!(v > 0.1 && v < 0.9, "blur should smooth the edge, got {v}");
    }

    #[test]
    fn edge_detect_on_flat() {
        let mut field = TextureField::new(16, 16);
        for y in 0..16 { for x in 0..16 { field.set("input", x, y, 0.5); }}
        let mut rng = SeededRng::new(42);
        let op = EdgeDetectOp { input: "input".into(), output: "out".into() };
        op.apply(&mut field, &mut rng);
        // Flat surface should have ~0 edges
        for y in 0..16 { for x in 0..16 {
            let v = field.get("out", x, y);
            assert!(v < 0.01, "flat surface should have no edges at ({x},{y}): {v}");
        }}
    }

    #[test]
    fn sharpen_op_enhances() {
        let mut field = TextureField::new(16, 16);
        for y in 0..16 { for x in 0..16 {
            field.set("input", x, y, if x < 8 { 0.3 } else { 0.7 });
        }}
        let mut rng = SeededRng::new(42);
        let op = SharpenOp { input: "input".into(), output: "out".into(), strength: 1.0, radius: 1 };
        op.apply(&mut field, &mut rng);
        // Interior values should be preserved or enhanced
        let left = field.get("out", 2, 8);
        let right = field.get("out", 13, 8);
        assert!(right > left, "sharpen should maintain or enhance contrast");
    }

    // ─── CellBulgeOp tests ──────────────────────────────────────────────

    #[test]
    fn bulge_falloff_boundary_conditions() {
        for falloff in [BulgeFalloff::Linear, BulgeFalloff::Parabolic, BulgeFalloff::Cosine] {
            let at_zero = falloff.apply(0.0);
            let at_one = falloff.apply(1.0);
            assert!(
                at_zero.abs() < 1e-6,
                "{falloff:?}: expected ~0 at t=0, got {at_zero}"
            );
            assert!(
                (at_one - 1.0).abs() < 1e-6,
                "{falloff:?}: expected ~1 at t=1, got {at_one}"
            );
        }
    }

    #[test]
    fn cell_bulge_raises_center_not_boundary() {
        let mut field = TextureField::new(64, 64);
        let mut rng = SeededRng::new(42);

        // Set up two cells: left half = cell 1, right half = cell 2
        // edge_dist = distance from nearest cell boundary (higher = more interior)
        field.ensure_channel("cell_id");
        field.ensure_channel("edge_dist");
        field.ensure_channel("height");
        for y in 0..64 {
            for x in 0..64 {
                let (cell_id, dist) = if x < 32 {
                    (1.0, (x as f32).min(31.0 - x as f32) / 31.0)
                } else {
                    (2.0, ((x - 32) as f32).min(63.0 - x as f32) / 31.0)
                };
                field.set("cell_id", x, y, cell_id);
                field.set("edge_dist", x, y, dist);
                field.set("height", x, y, 0.5);
            }
        }

        let op = CellBulgeOp {
            strength: 0.3,
            falloff: BulgeFalloff::Cosine,
        };
        op.apply(&mut field, &mut rng);

        // Center of cell 1 (x=16) should be raised
        let center_h = field.get("height", 16, 32);
        assert!(center_h > 0.5, "center should be raised, got {center_h}");

        // Boundary of cell 1 (x=0) should be ~unchanged
        let edge_h = field.get("height", 0, 32);
        assert!(
            (edge_h - 0.5).abs() < 0.01,
            "boundary should be ~unchanged, got {edge_h}"
        );
    }

    #[test]
    fn cell_bulge_per_cell_normalization() {
        let mut field = TextureField::new(64, 64);
        let mut rng = SeededRng::new(42);

        field.ensure_channel("cell_id");
        field.ensure_channel("edge_dist");
        field.ensure_channel("height");

        // Cell 1 (top half): max edge_dist = 0.5
        // Cell 2 (bottom half): max edge_dist = 1.0
        for y in 0..64 {
            for x in 0..64 {
                let (cell_id, max_d) = if y < 32 { (1.0, 0.5) } else { (2.0, 1.0) };
                let fy = if y < 32 { y as f32 } else { (y - 32) as f32 };
                let dist = (fy / 31.0).min(1.0) * max_d;
                field.set("cell_id", x, y, cell_id);
                field.set("edge_dist", x, y, dist);
                field.set("height", x, y, 0.0);
            }
        }

        let op = CellBulgeOp {
            strength: 1.0,
            falloff: BulgeFalloff::Linear,
        };
        op.apply(&mut field, &mut rng);

        // Both cells should reach similar peak height at their center
        // despite different max edge_dist ranges
        let peak1 = field.get("height", 32, 31); // cell 1 center-ish
        let peak2 = field.get("height", 32, 63); // cell 2 center-ish
        let diff = (peak1 - peak2).abs();
        assert!(
            diff < 0.15,
            "peaks should be similar after normalization: cell1={peak1}, cell2={peak2}"
        );
    }

    #[test]
    fn noise_layer_cell_offset_decorrelates() {
        // Two-cell field: left half = cell 0, right half = cell 1
        let size = 64;
        let mut field = TextureField::new(size, size);
        field.ensure_channel("cell_id");
        for y in 0..size {
            for x in 0..size {
                let cid = if x < size / 2 { 0.0 } else { 1.0 };
                field.set("cell_id", x, y, cid);
            }
        }

        // With cell_offset = true, noise should differ across cells
        let mut rng_on = SeededRng::new(99);
        let op_on = NoiseLayerOp {
            output: "n_on".into(),
            frequency: 10.0, octaves: 4,
            noise_type: NoiseType::Perlin,
            scale_x: 1.0, scale_y: 1.0,
            cell_offset: true,
        };
        op_on.apply(&mut field, &mut rng_on);

        // With cell_offset = false, noise should be the same at mirrored positions
        let mut rng_off = SeededRng::new(99);
        let op_off = NoiseLayerOp {
            output: "n_off".into(),
            frequency: 10.0, octaves: 4,
            noise_type: NoiseType::Perlin,
            scale_x: 1.0, scale_y: 1.0,
            cell_offset: false,
        };
        op_off.apply(&mut field, &mut rng_off);

        // Compare a sample point in each half — with offset they should differ
        let y = size / 2;
        let x_left = size / 4;
        let x_right = size / 4 + size / 2;
        let on_left = field.get("n_on", x_left, y);
        let on_right = field.get("n_on", x_right, y);
        let off_left = field.get("n_off", x_left, y);
        let off_right = field.get("n_off", x_right, y);

        // Without offset, mirrored positions sample same UV → same value
        assert!(
            (off_left - off_right).abs() < 0.01,
            "without cell_offset, mirrored positions should match: {off_left} vs {off_right}"
        );
        // With offset, cells should be decorrelated
        assert!(
            (on_left - on_right).abs() > 0.01,
            "with cell_offset, different cells should differ: {on_left} vs {on_right}"
        );
    }

    #[test]
    fn cell_offset_no_cell_id_fallback() {
        // No cell_id channel — cell_offset=true should not panic and match cell_offset=false
        let size = 32;
        let mut field_on = TextureField::new(size, size);
        let mut rng_on = SeededRng::new(55);
        let op_on = NoiseLayerOp {
            output: "sig".into(),
            frequency: 8.0, octaves: 3,
            noise_type: NoiseType::Perlin,
            scale_x: 1.0, scale_y: 1.0,
            cell_offset: true,
        };
        op_on.apply(&mut field_on, &mut rng_on);

        let mut field_off = TextureField::new(size, size);
        let mut rng_off = SeededRng::new(55);
        let op_off = NoiseLayerOp {
            output: "sig".into(),
            frequency: 8.0, octaves: 3,
            noise_type: NoiseType::Perlin,
            scale_x: 1.0, scale_y: 1.0,
            cell_offset: false,
        };
        op_off.apply(&mut field_off, &mut rng_off);

        for y in 0..size {
            for x in 0..size {
                let a = field_on.get("sig", x, y);
                let b = field_off.get("sig", x, y);
                assert!(
                    (a - b).abs() < 1e-6,
                    "without cell_id channel, results should match: ({x},{y}) {a} vs {b}"
                );
            }
        }
    }

    #[test]
    fn musgrave_cell_offset_decorrelates() {
        let size = 64;
        let mut field = TextureField::new(size, size);
        field.ensure_channel("cell_id");
        for y in 0..size {
            for x in 0..size {
                let cid = if x < size / 2 { 0.0 } else { 1.0 };
                field.set("cell_id", x, y, cid);
            }
        }

        let mut rng_on = SeededRng::new(77);
        let op_on = MusgraveTextureOp {
            output: "m_on".into(),
            noise_type: NoiseType::Perlin,
            musgrave_type: MusgraveType::HybridMultifractal,
            frequency: 8.0, octaves: 4, lacunarity: 2.0,
            dimension: 1.0, offset: 1.0, gain: 2.0,
            scale_x: 1.0, scale_y: 1.0,
            cell_offset: true,
        };
        op_on.apply(&mut field, &mut rng_on);

        let mut rng_off = SeededRng::new(77);
        let op_off = MusgraveTextureOp {
            output: "m_off".into(),
            noise_type: NoiseType::Perlin,
            musgrave_type: MusgraveType::HybridMultifractal,
            frequency: 8.0, octaves: 4, lacunarity: 2.0,
            dimension: 1.0, offset: 1.0, gain: 2.0,
            scale_x: 1.0, scale_y: 1.0,
            cell_offset: false,
        };
        op_off.apply(&mut field, &mut rng_off);

        let y = size / 2;
        let x_left = size / 4;
        let x_right = size / 4 + size / 2;
        let off_left = field.get("m_off", x_left, y);
        let off_right = field.get("m_off", x_right, y);
        let on_left = field.get("m_on", x_left, y);
        let on_right = field.get("m_on", x_right, y);

        assert!(
            (off_left - off_right).abs() < 0.01,
            "without cell_offset, mirrored musgrave should match: {off_left} vs {off_right}"
        );
        assert!(
            (on_left - on_right).abs() > 0.01,
            "with cell_offset, different cells should differ: {on_left} vs {on_right}"
        );
    }

    #[test]
    fn cell_noise_offset_determinism() {
        // Same inputs → same outputs
        let (a1, b1) = cell_noise_offset(5, 12345);
        let (a2, b2) = cell_noise_offset(5, 12345);
        assert_eq!(a1, a2);
        assert_eq!(b1, b2);

        // Different cell_ids → different offsets
        let (a3, b3) = cell_noise_offset(6, 12345);
        assert!(a1 != a3 || b1 != b3, "different cell_ids should give different offsets");

        // Different seeds → different offsets
        let (a4, b4) = cell_noise_offset(5, 99999);
        assert!(a1 != a4 || b1 != b4, "different seeds should give different offsets");
    }

    #[test]
    fn edge_erode_modifies_boundaries_not_interiors() {
        let mut field = TextureField::new(64, 64);
        let mut rng = SeededRng::new(42);

        // Create a simple brick grid
        let grid = BrickGridOp {
            columns: 4,
            rows: 4,
            stagger: 0.5,
            gap_width: 0.04,
            width_variation: 0.0,
            row_height_variation: 0.0,
            warp_x: None,
            warp_y: None,
            warp_strength: 0.0,
        };
        grid.apply(&mut field, &mut rng);

        // Snapshot cell_id before erosion
        let pre_cell_id: Vec<f32> = (0..64 * 64)
            .map(|i| field.get("cell_id", i % 64, i / 64))
            .collect();

        let erode = EdgeErodeOp {
            noise_frequency: 40.0,
            strength: 0.02,
            edge_width: 0.06,
            noise_type: NoiseType::Perlin,
            octaves: 1,
            threshold: 0.0, // erode everywhere for test coverage
        };
        erode.apply(&mut field, &mut rng);

        // Interior pixels (edge_dist >= 0.06) should be unchanged
        let mut interior_unchanged = 0u32;
        let mut interior_total = 0u32;
        // Boundary pixels (edge_dist < 0.06) should have at least some changes
        let mut boundary_changed = 0u32;
        let mut boundary_total = 0u32;

        for y in 0..64u32 {
            for x in 0..64u32 {
                let idx = (y * 64 + x) as usize;
                let pre_ed = {
                    // Use the pre-erosion edge_dist from the snapshot
                    // Re-run grid to get a clean snapshot of edge_dist
                    // (we already have pre_cell_id, but need edge_dist too)
                    // Instead, just check post-erosion state vs pre cell_id
                    let post_cid = field.get("cell_id", x, y);
                    let pre_cid = pre_cell_id[idx];
                    // Pixels far from edges have large edge_dist and were skipped
                    if (post_cid - pre_cid).abs() < 0.001 {
                        interior_unchanged += 1;
                    }
                    if field.get("edge_dist", x, y) < 0.06 {
                        boundary_total += 1;
                        if (post_cid - pre_cid).abs() > 0.001 {
                            boundary_changed += 1;
                        }
                    } else {
                        interior_total += 1;
                    }
                };
                let _ = pre_ed;
            }
        }

        // All interior pixels should be unchanged
        assert_eq!(
            interior_unchanged,
            interior_total + boundary_total - boundary_changed,
            "some interior pixels were modified unexpectedly"
        );
        // At least some boundary pixels should have changed cell_id
        assert!(
            boundary_changed > 0,
            "edge_erode should modify at least some boundary pixels, but changed 0 out of {boundary_total}"
        );
    }
}
