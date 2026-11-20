//! Cell layouts: the ops that divide the texture into cells and write
//! `cell_id` and `edge_dist` (`brick_grid`, `voronoi_grid`), and
//! `domain_warp`, which bends the coordinates a later op reads.

use crate::SeededRng;

use super::{lerp, TextureOp};
use crate::generators::texture::descriptor::{OpDescriptor, ParamDesc as P};
use crate::generators::texture::field::TextureField;
use crate::generators::texture::voronoi_util::{self, SpatialGrid};

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

/// The [`BrickGridOp`] op, as the pipeline knows it.
pub const BRICK_GRID: OpDescriptor = OpDescriptor {
    type_name: "brick_grid",
    label: "Brick Grid",
    summary: "Lay rectangular cells in staggered rows (running bond): bricks, planks, tiles.",
    reads: &[],
    writes: &["cell_id", "edge_dist", "mask"],
    modifies: &[],
    params: &[
        P::int("columns", 8).min(1.0).help("Bricks across each row"),
        P::int("rows", 16).min(1.0).help("Rows of bricks down the texture"),
        P::float("stagger", 0.5).range(0.0, 1.0).help("How far each row shifts from the one above, as a fraction of a brick: 0.5 is running bond, 0 stacks them"),
        P::float("gap_width", 0.04).range(0.0, 1.0).help("The mortar gap as a fraction of a brick's size; sets how fast edge_dist rises from the edge"),
        P::float("width_variation", 0.0).range(0.0, 1.0).help("How much brick widths vary along a row: 0 all the same, 1 the most"),
        P::float("row_height_variation", 0.0).range(0.0, 1.0).help("How much row heights vary: 0 all the same, 1 the most"),
        P::opt_channel("warp_x").help("Channel (0.5 is no shift) that moves the grid sideways before each pixel finds its brick; needs warp_y and warp_strength"),
        P::opt_channel("warp_y").help("Channel (0.5 is no shift) that moves the grid up and down before each pixel finds its brick; needs warp_x and warp_strength"),
        P::float("warp_strength", 0.0).min(0.0).help("How far warp_x and warp_y move the grid, in texture widths; 0 keeps the courses straight"),
    ],
    build: |p| {
        Ok(Box::new(BrickGridOp {
            columns: p.u32("columns")?,
            rows: p.u32("rows")?,
            stagger: p.f32("stagger")?,
            gap_width: p.f32("gap_width")?,
            width_variation: p.f32("width_variation")?,
            row_height_variation: p.f32("row_height_variation")?,
            warp_x: p.opt_text("warp_x")?,
            warp_y: p.opt_text("warp_y")?,
            warp_strength: p.f32("warp_strength")?,
        }))
    },
};

impl TextureOp for BrickGridOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &BRICK_GRID
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
            let row_h_seed = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0);
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

/// The [`VoronoiGridOp`] op, as the pipeline knows it.
pub const VORONOI_GRID: OpDescriptor = OpDescriptor {
    type_name: "voronoi_grid",
    label: "Voronoi Grid",
    summary: "Lay irregular Voronoi cells, for fieldstone and cobbles; every cell op reads them.",
    reads: &[],
    writes: &["cell_id", "edge_dist", "mask"],
    modifies: &[],
    params: &[
        P::int("cell_count", 20).min(1.0).help("Cells (stones) across the whole texture"),
        P::float("regularity", 0.0).range(0.0, 1.0).help("0 scatters the cells at random; toward 1 relaxes them (Lloyd) into even sizes"),
        P::float("mortar_width", 0.03).range(0.0, 1.0).help("Width of the mortar between cells, as a fraction of the texture"),
        P::opt_channel("warp_x").help("Channel (0.5 is no shift) that moves the cells sideways before each pixel finds its cell; needs warp_y and warp_strength"),
        P::opt_channel("warp_y").help("Channel (0.5 is no shift) that moves the cells up and down before each pixel finds its cell; needs warp_x and warp_strength"),
        P::float("warp_strength", 0.0).min(0.0).help("How far warp_x and warp_y move the cells, in texture widths; 0 leaves them unbent"),
    ],
    build: |p| {
        Ok(Box::new(VoronoiGridOp {
            cell_count: p.u32("cell_count")?,
            regularity: p.f32("regularity")?,
            mortar_width: p.f32("mortar_width")?,
            warp_x: p.opt_text("warp_x")?,
            warp_y: p.opt_text("warp_y")?,
            warp_strength: p.f32("warp_strength")?,
        }))
    },
};

impl TextureOp for VoronoiGridOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &VORONOI_GRID
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
            voronoi_util::blend_points(&mut points, &relaxed, self.regularity, true);
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

                let near = grid.nearest(u, v);
                let closest_id = near.id;

                // Edge distance: normalized gap between nearest and second-nearest
                // (kept as d2 - d1 so existing pipeline specs keep their groove
                // widths; the search itself is now exact).
                let raw_edge_dist = near.d2 - near.d1;
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

/// The [`DomainWarpOp`] op, as the pipeline knows it.
pub const DOMAIN_WARP: OpDescriptor = OpDescriptor {
    type_name: "domain_warp",
    label: "Domain Warp",
    summary: "Displace a channel by two others (bilinear lookup): swirl, bend or smear it.",
    reads: &["<input>", "<warp_x>", "<warp_y>"],
    writes: &["<output>"],
    modifies: &[],
    params: &[
        P::channel("input", "input").help("Channel to warp"),
        P::channel("output", "output").help("Channel the warped copy goes to; may be input itself"),
        P::channel("warp_x", "warp_x").help("Channel giving the horizontal displacement"),
        P::channel("warp_y", "warp_y").help("Channel giving the vertical displacement"),
        P::float("strength", 0.1)
            .min(0.0)
            .help("How far the displacement reaches, in texture widths"),
    ],
    build: |p| {
        Ok(Box::new(DomainWarpOp {
            input: p.text("input")?,
            output: p.text("output")?,
            warp_x: p.text("warp_x")?,
            warp_y: p.text("warp_y")?,
            strength: p.f32("strength")?,
        }))
    },
};

impl TextureOp for DomainWarpOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &DOMAIN_WARP
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

                let result = lerp(lerp(v00, v10, fx), lerp(v01, v11, fx), fy);

                field.set(&self.output, x, y, result);
            }
        }
    }
}
