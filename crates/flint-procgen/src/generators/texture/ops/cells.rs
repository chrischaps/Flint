//! Per-cell values: a height, bulge, colour or roughness drawn once per
//! cell from its id, and the mortar between cells (`cell_height`,
//! `cell_bulge`, `mortar_groove`, `cell_color`, `mortar_color`,
//! `cell_roughness`).

use crate::SeededRng;

use super::TextureOp;
use crate::generators::texture::cell_memo::CellMemo;
use crate::generators::texture::descriptor::{OpDescriptor, ParamDesc as P};
use crate::generators::texture::field::TextureField;

// ─── CellHeightOp ─────────────────────────────────────────────────────────

/// Assign a random base height per cell.
///
/// Reads `cell_id`, writes `height`.
pub struct CellHeightOp {
    /// Height randomness magnitude in [0, 1].
    pub variation: f32,
}

/// The [`CellHeightOp`] op, as the pipeline knows it.
pub const CELL_HEIGHT: OpDescriptor = OpDescriptor {
    type_name: "cell_height",
    label: "Cell Height",
    summary: "Give each cell a random base height, so stones stand proud of one another.",
    reads: &["cell_id"],
    writes: &["height"],
    modifies: &[],
    params: &[P::float("variation", 0.3)
        .range(0.0, 1.0)
        .help("How far each cell's height strays at random: 0 all level, 1 the most")],
    build: |p| {
        Ok(Box::new(CellHeightOp {
            variation: p.f32("variation")?,
        }))
    },
};

impl TextureOp for CellHeightOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &CELL_HEIGHT
    }

    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let seed = rng.seed();
        field.ensure_channel("height");
        let mut cells = CellMemo::new(|id| cell_height_for_id(id, self.variation, seed));

        for y in 0..h {
            for x in 0..w {
                let cell_id = field.get("cell_id", x, y) as u32;
                let cell_h = cells.get(cell_id);
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
pub(super) fn cell_noise_offset(cell_id: u32, rng_seed: u64) -> (f64, f64) {
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

/// The [`CellBulgeOp`] op, as the pipeline knows it.
pub const CELL_BULGE: OpDescriptor = OpDescriptor {
    type_name: "cell_bulge",
    label: "Cell Bulge",
    summary: "Dome each cell, highest at its centre, whatever its size or shape.",
    reads: &["cell_id", "edge_dist"],
    writes: &[],
    modifies: &["height"],
    params: &[
        P::float("strength", 0.3).min(0.0).help("Height added at each cell's centre"),
        P::choice("falloff", &["cosine", "parabolic", "linear"], "cosine").help("The dome's profile from edge to centre: cosine an S-curve, flat at both ends; parabolic a flat-topped dome; linear a cone"),
    ],
    build: |p| {
        Ok(Box::new(CellBulgeOp {
            strength: p.f32("strength")?,
            falloff: BulgeFalloff::from_str(p.choice("falloff")?),
        }))
    },
};

impl TextureOp for CellBulgeOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &CELL_BULGE
    }

    fn apply(&self, field: &mut TextureField, _rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        field.ensure_channel("height");

        // Pass 1: find max edge_dist per cell
        let mut max_dist: std::collections::HashMap<u32, f32> = std::collections::HashMap::new();
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

/// The [`MortarGrooveOp`] op, as the pipeline knows it.
pub const MORTAR_GROOVE: OpDescriptor = OpDescriptor {
    type_name: "mortar_groove",
    label: "Mortar Groove",
    summary: "Sink the height along cell edges, cutting the grooves between stones.",
    reads: &["edge_dist"],
    writes: &[],
    modifies: &["height"],
    params: &[
        P::float("depth", 0.15)
            .range(0.0, 1.0)
            .help("How far the height drops at the bottom of the groove"),
        P::float("width", 0.04)
            .range(0.0, 1.0)
            .help("The edge_dist below which the groove cuts in"),
    ],
    build: |p| {
        Ok(Box::new(MortarGrooveOp {
            depth: p.f32("depth")?,
            width: p.f32("width")?,
        }))
    },
};

impl TextureOp for MortarGrooveOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &MORTAR_GROOVE
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

/// The [`CellColorOp`] op, as the pipeline knows it.
pub const CELL_COLOR: OpDescriptor = OpDescriptor {
    type_name: "cell_color",
    label: "Cell Color",
    summary: "Colour each cell a shade of one base colour, varying lightness and keeping hue.",
    reads: &["cell_id"],
    writes: &["r", "g", "b"],
    modifies: &[],
    params: &[
        P::color("base_color", [0.45, 0.35, 0.28, 1.0])
            .help("Albedo of the cells, hex; each cell gets its own shade of it"),
        P::float("variation", 0.12)
            .range(0.0, 1.0)
            .help("How far each cell's lightness strays (saturation by half that; hue stays)"),
    ],
    build: |p| {
        Ok(Box::new(CellColorOp {
            base_color: p.color("base_color")?,
            variation: p.f32("variation")?,
        }))
    },
};

impl TextureOp for CellColorOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &CELL_COLOR
    }

    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let seed = rng.seed();
        field.ensure_channel("r");
        field.ensure_channel("g");
        field.ensure_channel("b");
        let mut cells =
            CellMemo::new(|id| cell_color_hsl(id, self.base_color, self.variation, seed));

        for y in 0..h {
            for x in 0..w {
                let cell_id = field.get("cell_id", x, y) as u32;
                let color = cells.get(cell_id);
                field.set("r", x, y, color[0]);
                field.set("g", x, y, color[1]);
                field.set("b", x, y, color[2]);
            }
        }
    }
}

/// Deterministic per-cell color using HSL-space variation.
fn cell_color_hsl(cell_id: u32, base_color: [f32; 4], variation: f32, rng_seed: u64) -> [f32; 4] {
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

/// The [`MortarColorOp`] op, as the pipeline knows it.
pub const MORTAR_COLOR: OpDescriptor = OpDescriptor {
    type_name: "mortar_color",
    label: "Mortar Color",
    summary: "Blend a mortar colour into the albedo near cell edges.",
    reads: &["edge_dist"],
    writes: &[],
    modifies: &["r", "g", "b"],
    params: &[
        P::color("color", [0.25, 0.22, 0.18, 1.0]).help("Albedo of the mortar, hex"),
        P::float("threshold", 0.06)
            .range(0.0, 1.0)
            .help("The edge_dist below which the mortar colour blends in"),
    ],
    build: |p| {
        Ok(Box::new(MortarColorOp {
            color: p.color("color")?,
            threshold: p.f32("threshold")?,
        }))
    },
};

impl TextureOp for MortarColorOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &MORTAR_COLOR
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

/// The [`CellRoughnessOp`] op, as the pipeline knows it.
pub const CELL_ROUGHNESS: OpDescriptor = OpDescriptor {
    type_name: "cell_roughness",
    label: "Cell Roughness",
    summary: "Give each cell its own roughness, and the mortar near its edges another.",
    reads: &["cell_id", "edge_dist"],
    writes: &["roughness"],
    modifies: &[],
    params: &[
        P::float("base", 0.75)
            .range(0.0, 1.0)
            .help("Roughness of the cells' faces"),
        P::float("variation", 0.05)
            .range(0.0, 1.0)
            .help("Per-cell random spread of roughness"),
        P::float("mortar", 0.55)
            .range(0.0, 1.0)
            .help("Roughness of the mortar"),
        P::float("mortar_threshold", 0.06)
            .range(0.0, 1.0)
            .help("The edge_dist below which the mortar roughness blends in"),
    ],
    build: |p| {
        Ok(Box::new(CellRoughnessOp {
            base: p.f32("base")?,
            variation: p.f32("variation")?,
            mortar: p.f32("mortar")?,
            mortar_threshold: p.f32("mortar_threshold")?,
        }))
    },
};

impl TextureOp for CellRoughnessOp {
    fn descriptor(&self) -> &'static OpDescriptor {
        &CELL_ROUGHNESS
    }

    fn apply(&self, field: &mut TextureField, rng: &mut SeededRng) {
        let w = field.width;
        let h = field.height;
        let seed = rng.seed();
        field.ensure_channel("roughness");
        let mut cells = CellMemo::new(|id| cell_roughness_shift(id, self.variation, seed));

        for y in 0..h {
            for x in 0..w {
                let cell_id = field.get("cell_id", x, y) as u32;
                let e = field.get("edge_dist", x, y);

                // Per-cell roughness variation
                let cell_shift = cells.get(cell_id);
                let surface_rough = (self.base + cell_shift).clamp(0.0, 1.0);

                // Mortar blend
                let roughness = if e < self.mortar_threshold && self.mortar_threshold > 0.0 {
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
