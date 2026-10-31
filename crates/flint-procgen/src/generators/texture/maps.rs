//! PBR texture map derivation from pattern fields.
//!
//! Bridges the gap between the abstract [`PatternField`] (per-pixel cell ID,
//! height, edge distance) and usable PBR textures — **albedo**, **normal**, and
//! **roughness** maps. This is the core "material from math" step that turns
//! procedural patterns into something beautiful.
//!
//! # Pipeline
//!
//! ```text
//!                 ┌─ per-cell tint (hash of cell_id, hue kept) ─┐
//! PatternField ───┤                                             ├─► derive_albedo()    ─► Color, sRGB RGBA8
//!                 ├─ mortar mask (distance_to_edge)  ───────────┤
//!                 └─ one detail field (tiling FBM, unit RMS) ───┼─► derive_normal()    ─► Normal, linear RGBA8
//!                                                               └─► derive_roughness() ─► Roughness, linear RGBA8
//! ```
//!
//! The three maps read **the same** detail field, so the material agrees with
//! itself: a dent in the normal map is a little darker in the albedo and a
//! little rougher in the roughness map. The field tiles whenever the texture
//! is seamless, and octaves finer than [`MIN_DETAIL_PERIOD_PX`] are dropped so
//! it never aliases into per-pixel static.

use crate::algorithms::noise::PerlinNoise;
use crate::generators::texture::pattern::PatternField;
use crate::generators::util::{
    parse_hex_color, srgb_to_linear, toml_f32, toml_string, toml_string_array, toml_u32,
};
use crate::types::{ChannelSemantics, ImageData};
use crate::{ProcGenError, Result, SeededRng};

/// The texture size at which `normal_strength` keeps the meaning it had when
/// it was a central difference in pixels. Normals are computed per UV unit
/// and scaled to this, so a 1024² map shows the same relief as a 512² one.
pub const NORMAL_REFERENCE_SIZE: f32 = 512.0;

/// The shortest period, in pixels, a detail octave may have. Finer octaves
/// alias: at the old `detail_scale` semantics a 45-cell base with 4 octaves
/// put a lattice cell under every 0.4 px, and the "detail" was white noise.
pub const MIN_DETAIL_PERIOD_PX: f64 = 4.0;

/// How much height one unit of `detail_strength` adds to the heightfield the
/// normal map is built from, per unit of (unit-RMS) detail.
const DETAIL_RELIEF: f32 = 0.25;

/// Roughness change per unit of `detail_strength` and of detail: low spots
/// (negative detail) hold grime and read rougher.
const DETAIL_ROUGHNESS: f32 = 0.5;

/// Default `mortar_threshold` for patterns whose `distance_to_edge` spans the
/// whole groove (Voronoi, grid): the groove's deepest half is mortar and the
/// bevel blends to stone.
const GROOVE_MORTAR_THRESHOLD: f32 = 0.5;

/// Default `mortar_threshold` for banded patterns (organic), where
/// `distance_to_edge` spans half a band: thin strata lines.
const BAND_MORTAR_THRESHOLD: f32 = 0.05;

// ─── TextureMapParams ──────────────────────────────────────────────────────

/// Parameters controlling PBR map derivation from a pattern field.
#[derive(Debug, Clone)]
pub struct TextureMapParams {
    /// Texture width in pixels.
    pub width: u32,
    /// Texture height in pixels.
    pub height: u32,
    /// Which maps to generate: `"albedo"`, `"normal"`, `"roughness"`.
    pub output_maps: Vec<String>,
    /// Pattern type: `"voronoi_brick"`, `"perlin_organic"`, `"tiling_grid"`.
    pub pattern: String,
    /// Base albedo color as linear RGBA.
    pub base_color: [f32; 4],
    /// Per-cell shift of lightness (±`color_variation`) and saturation
    /// (±half that), in sRGB HSL. Hue is kept, so a brown wall stays brown.
    pub color_variation: f32,
    /// Color for mortar/gap regions (linear RGBA).
    pub mortar_color: [f32; 4],
    /// Pixels with `distance_to_edge` below this blend toward the mortar
    /// colour and roughness (smoothstep). Defaults by pattern: 0.5 for
    /// `voronoi_brick` / `tiling_grid` (half the groove), 0.05 for organic.
    pub mortar_threshold: f32,
    /// Base surface roughness (0–1).
    pub roughness_base: f32,
    /// Per-cell roughness shift magnitude.
    pub roughness_variation: f32,
    /// Roughness in mortar regions (defaults smoother than surface).
    pub roughness_mortar: f32,
    /// Normal map strength multiplier (relief at [`NORMAL_REFERENCE_SIZE`]).
    pub normal_strength: f32,
    /// Detail lattice cells across the texture width for the first octave.
    pub detail_scale: f32,
    /// Detail octaves (each doubles the frequency, halves the amplitude);
    /// ones finer than [`MIN_DETAIL_PERIOD_PX`] are dropped.
    pub detail_octaves: u32,
    /// Detail amplitude: the RMS brightness change it makes in the albedo
    /// (0.1 = ±10%), and proportionally the relief and roughness it adds.
    /// 0 = no detail.
    pub detail_strength: f32,
    /// Whether the texture should tile seamlessly.
    pub seamless: bool,
}

impl Default for TextureMapParams {
    fn default() -> Self {
        Self {
            width: 512,
            height: 512,
            output_maps: vec!["albedo".into(), "normal".into(), "roughness".into()],
            pattern: "voronoi_brick".into(),
            base_color: [0.45, 0.35, 0.28, 1.0],
            color_variation: 0.06,
            mortar_color: [0.25, 0.22, 0.18, 1.0],
            mortar_threshold: GROOVE_MORTAR_THRESHOLD,
            roughness_base: 0.75,
            roughness_variation: 0.05,
            roughness_mortar: 0.5,
            normal_strength: 1.0,
            detail_scale: 8.0,
            detail_octaves: 4,
            detail_strength: 0.1,
            seamless: true,
        }
    }
}

impl TextureMapParams {
    /// Parse map derivation parameters from a TOML `params` value.
    ///
    /// Missing fields fall back to sensible defaults. Invalid values produce
    /// descriptive errors.
    pub fn from_toml(params: &toml::Value) -> Result<Self> {
        let table = params
            .as_table()
            .ok_or_else(|| ProcGenError::InvalidParameter {
                name: "params".into(),
                reason: "expected a TOML table".into(),
            })?;

        let mut p = Self::default();

        if let Some(v) = table.get("width") {
            p.width = toml_u32(v, "width")?;
            if p.width == 0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "width".into(),
                    reason: "must be positive".into(),
                });
            }
        }
        if let Some(v) = table.get("height") {
            p.height = toml_u32(v, "height")?;
            if p.height == 0 {
                return Err(ProcGenError::InvalidParameter {
                    name: "height".into(),
                    reason: "must be positive".into(),
                });
            }
        }
        if let Some(v) = table.get("output_maps") {
            p.output_maps = toml_string_array(v, "output_maps")?;
            for map_name in &p.output_maps {
                if !["albedo", "normal", "roughness"].contains(&map_name.as_str()) {
                    return Err(ProcGenError::InvalidParameter {
                        name: "output_maps".into(),
                        reason: format!(
                            "unknown map type \"{map_name}\"; expected \"albedo\", \"normal\", or \"roughness\""
                        ),
                    });
                }
            }
        }
        if let Some(v) = table.get("pattern") {
            p.pattern = toml_string(v, "pattern")?;
        }
        if p.pattern == "perlin_organic" {
            p.mortar_threshold = BAND_MORTAR_THRESHOLD;
        }
        if let Some(v) = table.get("base_color") {
            let hex = v.as_str().ok_or_else(|| ProcGenError::InvalidParameter {
                name: "base_color".into(),
                reason: "expected a hex color string".into(),
            })?;
            p.base_color = parse_hex_color(hex)?;
        }
        if let Some(v) = table.get("color_variation") {
            p.color_variation = non_negative(toml_f32(v, "color_variation")?, "color_variation")?;
        }
        if let Some(v) = table.get("mortar_color") {
            let hex = v.as_str().ok_or_else(|| ProcGenError::InvalidParameter {
                name: "mortar_color".into(),
                reason: "expected a hex color string".into(),
            })?;
            p.mortar_color = parse_hex_color(hex)?;
        }
        if let Some(v) = table.get("mortar_threshold") {
            p.mortar_threshold =
                non_negative(toml_f32(v, "mortar_threshold")?, "mortar_threshold")?;
        }
        if let Some(v) = table.get("roughness_base") {
            p.roughness_base = toml_f32(v, "roughness_base")?;
        }
        if let Some(v) = table.get("roughness_variation") {
            p.roughness_variation =
                non_negative(toml_f32(v, "roughness_variation")?, "roughness_variation")?;
        }
        if let Some(v) = table.get("roughness_mortar") {
            p.roughness_mortar = toml_f32(v, "roughness_mortar")?;
        }
        if let Some(v) = table.get("normal_strength") {
            p.normal_strength = toml_f32(v, "normal_strength")?;
        }
        if let Some(v) = table.get("detail_scale") {
            p.detail_scale = non_negative(toml_f32(v, "detail_scale")?, "detail_scale")?;
        }
        if let Some(v) = table.get("detail_octaves") {
            p.detail_octaves = toml_u32(v, "detail_octaves")?.max(1);
        }
        if let Some(v) = table.get("detail_strength") {
            p.detail_strength = non_negative(toml_f32(v, "detail_strength")?, "detail_strength")?;
        }
        if let Some(v) = table.get("seamless") {
            p.seamless = v.as_bool().ok_or_else(|| ProcGenError::InvalidParameter {
                name: "seamless".into(),
                reason: "expected a boolean".into(),
            })?;
        }

        Ok(p)
    }
}

fn non_negative(v: f32, name: &str) -> Result<f32> {
    if v < 0.0 || !v.is_finite() {
        return Err(ProcGenError::InvalidParameter {
            name: name.into(),
            reason: format!("must be a non-negative number (got {v})"),
        });
    }
    Ok(v)
}

// ─── Shared inputs ─────────────────────────────────────────────────────────

/// What all three maps share: the per-cell seed and the detail field.
struct MapContext {
    cell_seed: u64,
    detail: Option<DetailField>,
}

impl MapContext {
    fn new(field: &PatternField, params: &TextureMapParams, rng: &mut SeededRng) -> Self {
        let cell_seed = rng.fork("cells").seed();
        let detail_seed = rng.fork("detail").seed();
        Self {
            cell_seed,
            detail: DetailField::build(field.width, field.height, params, detail_seed),
        }
    }

    fn detail(&self, x: u32, y: u32) -> f32 {
        self.detail.as_ref().map_or(0.0, |d| d.at(x, y))
    }
}

/// Micro-surface variation: periodic Perlin FBM sampled at pixel centres,
/// shifted to zero mean and scaled to unit RMS so `detail_strength` means the
/// same thing at any scale or octave count.
struct DetailField {
    width: u32,
    values: Vec<f32>,
}

impl DetailField {
    fn build(w: u32, h: u32, params: &TextureMapParams, seed: u64) -> Option<Self> {
        if params.detail_strength <= 0.0 || params.detail_scale <= 0.0 {
            return None;
        }
        let noise = PerlinNoise::new(seed);
        let octaves = detail_octaves(w, h, params);

        let mut values: Vec<f32> = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            let v = (y as f64 + 0.5) / h as f64;
            for x in 0..w {
                let u = (x as f64 + 0.5) / w as f64;
                let s: f64 = octaves
                    .iter()
                    .map(|&(px, py, amp)| {
                        amp * noise.sample_2d_periodic(u * px as f64, v * py as f64, px, py)
                    })
                    .sum();
                values.push(s as f32);
            }
        }

        let n = values.len() as f64;
        let mean = values.iter().map(|&v| v as f64).sum::<f64>() / n;
        let var = values
            .iter()
            .map(|&v| (v as f64 - mean).powi(2))
            .sum::<f64>()
            / n;
        let rms = var.sqrt();
        if rms < 1e-9 {
            return None;
        }
        for v in &mut values {
            *v = ((*v as f64 - mean) / rms) as f32;
        }
        Some(Self { width: w, values })
    }

    fn at(&self, x: u32, y: u32) -> f32 {
        self.values[(y * self.width + x) as usize]
    }
}

/// The detail octaves as (cells across, cells down, amplitude). Each period is
/// a whole number of lattice cells so the field tiles; cells stay square on a
/// non-square texture. Octaves finer than [`MIN_DETAIL_PERIOD_PX`] are
/// dropped, but the first is always kept.
fn detail_octaves(w: u32, h: u32, params: &TextureMapParams) -> Vec<(i64, i64, f64)> {
    let mut out = Vec::new();
    let (mut freq, mut amp) = (params.detail_scale as f64, 1.0);
    for _ in 0..params.detail_octaves.max(1) {
        let px = (freq.round() as i64).max(1);
        let py = ((freq * h as f64 / w as f64).round() as i64).max(1);
        if !out.is_empty() && (w as f64 / px as f64) < MIN_DETAIL_PERIOD_PX {
            break;
        }
        out.push((px, py, amp));
        freq *= 2.0;
        amp *= 0.5;
    }
    out
}

// ─── Per-Cell Variation ────────────────────────────────────────────────────

fn cell_rng(cell_id: u32, seed: u64, salt: u64) -> SeededRng {
    SeededRng::new(seed.wrapping_mul(salt).wrapping_add(cell_id as u64))
}

/// Deterministic per-cell albedo (linear) from the cell ID.
///
/// The shift is made in sRGB HSL — lightness ±`variation`, saturation
/// ±`variation`/2, hue untouched — so every cell is a plausible member of the
/// same material. Jittering linear channels independently (the old way)
/// turned a dark grey-brown into magenta, lime and teal.
fn cell_albedo(cell_id: u32, base: [f32; 4], variation: f32, seed: u64) -> [f32; 4] {
    if variation <= 0.0 {
        return base;
    }
    let srgb = [
        linear_to_srgb_f(base[0]),
        linear_to_srgb_f(base[1]),
        linear_to_srgb_f(base[2]),
        base[3],
    ];
    let varied =
        cell_rng(cell_id, seed, 0x9E37_79B9_7F4A_7C15).next_color_variation_hsl(srgb, variation);
    [
        srgb_to_linear(varied[0]),
        srgb_to_linear(varied[1]),
        srgb_to_linear(varied[2]),
        base[3],
    ]
}

/// Deterministic per-cell roughness shift from the cell ID.
fn cell_roughness_shift(cell_id: u32, variation: f32, seed: u64) -> f32 {
    if variation <= 0.0 {
        return 0.0;
    }
    cell_rng(cell_id, seed, 0x517C_C1B7_2722_0A95).next_range(-variation, variation)
}

/// How much of a pixel is mortar: 1 on the edge, 0 from `mortar_threshold`
/// inward, smoothstep between.
fn mortar_mix(distance_to_edge: f32, threshold: f32) -> f32 {
    if threshold <= 0.0 {
        return 0.0;
    }
    let t = (distance_to_edge / threshold).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

// ─── Encoding ──────────────────────────────────────────────────────────────

fn linear_to_srgb_f(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Quantise a [0, 1] float to a byte, rounding to nearest.
fn to_byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

// ─── Albedo Derivation ────────────────────────────────────────────────────

/// Derive an albedo (base color) map from a pattern field.
///
/// Per pixel: the cell's tint (hue-preserving, see [`cell_albedo`]), blended
/// to `mortar_color` near edges, then multiplied by `1 + detail_strength ·
/// detail` so dents darken and crests lighten in proportion to the colour
/// rather than by a fixed amount (which drowned dark materials). sRGB RGBA8.
pub fn derive_albedo(
    field: &PatternField,
    params: &TextureMapParams,
    rng: &mut SeededRng,
) -> ImageData {
    albedo(field, params, &MapContext::new(field, params, rng))
}

fn albedo(field: &PatternField, params: &TextureMapParams, ctx: &MapContext) -> ImageData {
    let (w, h) = (field.width, field.height);
    let mut img = ImageData::new(w, h, ChannelSemantics::Color);

    for y in 0..h {
        for x in 0..w {
            let cell = field.get(x, y);
            let tint = cell_albedo(
                cell.cell_id,
                params.base_color,
                params.color_variation,
                ctx.cell_seed,
            );
            let m = mortar_mix(cell.distance_to_edge, params.mortar_threshold);
            let gain = (1.0 + params.detail_strength * ctx.detail(x, y)).max(0.0);

            let offset = ((y * w + x) * 4) as usize;
            let mortar = &params.mortar_color;
            for (c, (&t, &mc)) in tint.iter().zip(mortar).take(3).enumerate() {
                let lin = (t * (1.0 - m) + mc * m) * gain;
                img.pixels[offset + c] = to_byte(linear_to_srgb_f(lin));
            }
            img.pixels[offset + 3] = to_byte(tint[3]);
        }
    }
    img
}

// ─── Normal Derivation ────────────────────────────────────────────────────

/// Derive a tangent-space normal map from a pattern field.
///
/// The heightfield is the pattern height plus the shared detail field. A 3×3
/// Sobel kernel takes its gradient per UV unit, so the relief doesn't change
/// with resolution; `normal_strength` is calibrated at
/// [`NORMAL_REFERENCE_SIZE`]. The kernel wraps when the texture is seamless
/// and clamps at the border otherwise.
///
/// `n = normalize(-dx, -dy, 1)`, so Z is always positive; encoded linear as
/// `(n · 0.5 + 0.5) · 255`, A = 255. Y follows image rows (the same
/// convention as the pipeline's `derive_normal` op).
pub fn derive_normal(
    field: &PatternField,
    params: &TextureMapParams,
    rng: &mut SeededRng,
) -> ImageData {
    normal(field, params, &MapContext::new(field, params, rng))
}

fn normal(field: &PatternField, params: &TextureMapParams, ctx: &MapContext) -> ImageData {
    let (w, h) = (field.width, field.height);
    let mut img = ImageData::new(w, h, ChannelSemantics::Normal);

    let relief = params.detail_strength * DETAIL_RELIEF;
    let heights: Vec<f32> = (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| field.get(x, y).height + relief * ctx.detail(x, y))
        .collect();

    let (wi, hi) = (w as i32, h as i32);
    let seamless = params.seamless;
    let at = |x: i32, y: i32| -> f32 {
        let (x, y) = if seamless {
            (x.rem_euclid(wi), y.rem_euclid(hi))
        } else {
            (x.clamp(0, wi - 1), y.clamp(0, hi - 1))
        };
        heights[(y * wi + x) as usize]
    };

    // Sobel's gradient is 8× the per-pixel slope; per-pixel slope × size is
    // slope per UV; × 2/reference matches the old central difference at 512.
    let sx = params.normal_strength * w as f32 * 2.0 / (8.0 * NORMAL_REFERENCE_SIZE);
    let sy = params.normal_strength * h as f32 * 2.0 / (8.0 * NORMAL_REFERENCE_SIZE);

    for y in 0..hi {
        for x in 0..wi {
            let gx = (at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1))
                - (at(x - 1, y - 1) + 2.0 * at(x - 1, y) + at(x - 1, y + 1));
            let gy = (at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1))
                - (at(x - 1, y - 1) + 2.0 * at(x, y - 1) + at(x + 1, y - 1));

            let (nx, ny, nz) = (-gx * sx, -gy * sy, 1.0_f32);
            let len = (nx * nx + ny * ny + nz * nz).sqrt();

            let offset = ((y * wi + x) * 4) as usize;
            img.pixels[offset] = to_byte(nx / len * 0.5 + 0.5);
            img.pixels[offset + 1] = to_byte(ny / len * 0.5 + 0.5);
            img.pixels[offset + 2] = to_byte(nz / len * 0.5 + 0.5);
            img.pixels[offset + 3] = 255;
        }
    }
    img
}

// ─── Roughness Derivation ─────────────────────────────────────────────────

/// Derive a roughness map from a pattern field.
///
/// - Surface: `roughness_base` ± a per-cell shift of `roughness_variation`
/// - Mortar: `roughness_mortar`, smoothstep-blended by `distance_to_edge`
/// - Detail: low spots of the shared detail field read rougher
/// - Output is grayscale RGBA8 (R=G=B=roughness, A=255), linear
pub fn derive_roughness(
    field: &PatternField,
    params: &TextureMapParams,
    rng: &mut SeededRng,
) -> ImageData {
    roughness(field, params, &MapContext::new(field, params, rng))
}

fn roughness(field: &PatternField, params: &TextureMapParams, ctx: &MapContext) -> ImageData {
    let (w, h) = (field.width, field.height);
    let mut img = ImageData::new(w, h, ChannelSemantics::Roughness);

    for y in 0..h {
        for x in 0..w {
            let cell = field.get(x, y);
            let shift =
                cell_roughness_shift(cell.cell_id, params.roughness_variation, ctx.cell_seed);
            let surface = (params.roughness_base + shift).clamp(0.0, 1.0);
            let m = mortar_mix(cell.distance_to_edge, params.mortar_threshold);
            let r = surface * (1.0 - m) + params.roughness_mortar * m
                - DETAIL_ROUGHNESS * params.detail_strength * ctx.detail(x, y);

            let byte = to_byte(r);
            let offset = ((y * w + x) * 4) as usize;
            img.pixels[offset] = byte;
            img.pixels[offset + 1] = byte;
            img.pixels[offset + 2] = byte;
            img.pixels[offset + 3] = 255;
        }
    }
    img
}

// ─── Orchestrator ──────────────────────────────────────────────────────────

/// Derive PBR texture maps from a pattern field according to `params.output_maps`.
///
/// Returns one [`ImageData`] per requested map, in the order they appear in
/// `output_maps`. The maps share one per-cell seed and one detail field, so
/// they agree with each other; the result is deterministic for a given seed.
pub fn derive_maps(
    field: &PatternField,
    params: &TextureMapParams,
    rng: &mut SeededRng,
) -> Vec<ImageData> {
    let ctx = MapContext::new(field, params, rng);
    params
        .output_maps
        .iter()
        .filter_map(|name| match name.as_str() {
            "albedo" => Some(albedo(field, params, &ctx)),
            "normal" => Some(normal(field, params, &ctx)),
            "roughness" => Some(roughness(field, params, &ctx)),
            _ => None, // Invalid names filtered by from_toml validation
        })
        .collect()
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators::texture::grid::{TilingGridParams, TilingGridPattern};
    use crate::generators::texture::organic::{PerlinOrganicParams, PerlinOrganicPattern};
    use crate::generators::texture::pattern::{Pattern, PatternCell, PatternField};
    use crate::generators::texture::voronoi::{VoronoiBrickParams, VoronoiBrickPattern};

    /// Create a synthetic pattern field for testing:
    /// 4 quadrant cells with varying heights and edge distances.
    fn test_field(size: u32) -> PatternField {
        let mut field = PatternField::new(size, size);
        let half = size / 2;

        for y in 0..size {
            for x in 0..size {
                let cell_id = match (x < half, y < half) {
                    (true, true) => 0,
                    (false, true) => 1,
                    (true, false) => 2,
                    (false, false) => 3,
                };

                // Distance to the nearest quadrant boundary
                let dx = (x as f32 - half as f32).abs() / half as f32;
                let dy = (y as f32 - half as f32).abs() / half as f32;
                let dist_to_edge = dx.min(dy).min(
                    (x as f32 / size as f32)
                        .min(y as f32 / size as f32)
                        .min((1.0 - x as f32 / size as f32).min(1.0 - y as f32 / size as f32)),
                );

                // Height varies across the field
                let height = (x as f32 / size as f32 + y as f32 / size as f32) * 0.5;

                *field.get_mut(x, y) = PatternCell {
                    cell_id,
                    height,
                    distance_to_edge: dist_to_edge.clamp(0.0, 1.0),
                };
            }
        }

        field
    }

    /// The three shipped pattern families, as the showcase specs set them up.
    fn real_fields(size: u32) -> Vec<(&'static str, PatternField, TextureMapParams)> {
        let mut out = Vec::new();
        let base = TextureMapParams {
            width: size,
            height: size,
            ..TextureMapParams::default()
        };

        let mut rng = SeededRng::new(7);
        let voronoi = VoronoiBrickPattern::new(VoronoiBrickParams {
            cell_count: 18,
            regularity: 0.2,
            mortar_width: 0.04,
            height_variation: 0.4,
            ..Default::default()
        });
        out.push((
            "voronoi",
            voronoi.generate(size, size, true, &mut rng),
            TextureMapParams {
                color_variation: 0.12,
                detail_scale: 45.0,
                detail_strength: 0.08,
                normal_strength: 1.3,
                ..base.clone()
            },
        ));

        let mut rng = SeededRng::new(8);
        let organic = PerlinOrganicPattern::new(PerlinOrganicParams::default());
        out.push((
            "organic",
            organic.generate(size, size, true, &mut rng),
            TextureMapParams {
                pattern: "perlin_organic".into(),
                mortar_threshold: BAND_MORTAR_THRESHOLD,
                color_variation: 0.2,
                detail_scale: 40.0,
                ..base.clone()
            },
        ));

        let mut rng = SeededRng::new(9);
        let grid = TilingGridPattern::new(TilingGridParams {
            rows: 4,
            columns: 4,
            gap_width: 0.02,
            ..Default::default()
        });
        out.push((
            "grid",
            grid.generate(size, size, true, &mut rng),
            TextureMapParams {
                pattern: "tiling_grid".into(),
                color_variation: 0.08,
                detail_scale: 35.0,
                detail_strength: 0.06,
                ..base
            },
        ));
        out
    }

    fn rgb(img: &ImageData, x: u32, y: u32) -> [f32; 3] {
        let o = ((y * img.width + x) * 4) as usize;
        [
            img.pixels[o] as f32 / 255.0,
            img.pixels[o + 1] as f32 / 255.0,
            img.pixels[o + 2] as f32 / 255.0,
        ]
    }

    fn decode_normal(img: &ImageData, x: u32, y: u32) -> [f32; 3] {
        rgb(img, x, y).map(|c| c * 2.0 - 1.0)
    }

    fn luma(c: [f32; 3]) -> f32 {
        0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
    }

    #[test]
    fn normal_z_always_positive() {
        for (name, field, params) in real_fields(128) {
            let normal_map = derive_normal(&field, &params, &mut SeededRng::new(42));
            assert!(normal_map.validate().is_ok());
            for y in 0..field.height {
                for x in 0..field.width {
                    let z = decode_normal(&normal_map, x, y)[2];
                    assert!(z > 0.0, "{name}: normal Z {z} at ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn normals_are_unit_length() {
        for (name, field, params) in real_fields(128) {
            let map = derive_normal(&field, &params, &mut SeededRng::new(42));
            let mut rng = SeededRng::new(5);
            for _ in 0..100 {
                let x = rng.next_range_i32(0, 127) as u32;
                let y = rng.next_range_i32(0, 127) as u32;
                let n = decode_normal(&map, x, y);
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                assert!(
                    (len - 1.0).abs() < 0.01,
                    "{name}: |n| = {len} at ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn roughness_in_range_and_grey() {
        for (name, field, params) in real_fields(64) {
            let map = derive_roughness(&field, &params, &mut SeededRng::new(42));
            assert!(map.validate().is_ok());
            for px in map.pixels.chunks(4) {
                assert!(
                    px[0] == px[1] && px[1] == px[2],
                    "{name}: roughness not grey"
                );
                assert_eq!(px[3], 255);
            }
        }
    }

    #[test]
    fn albedo_mortar_differs_from_surface() {
        let field = test_field(64);
        let params = TextureMapParams {
            mortar_threshold: 0.1,
            color_variation: 0.0,
            detail_strength: 0.0,
            ..TextureMapParams::default()
        };
        let albedo = derive_albedo(&field, &params, &mut SeededRng::new(42));

        let mut edge = None;
        let mut interior = None;
        for y in 0..field.height {
            for x in 0..field.width {
                let d = field.get(x, y).distance_to_edge;
                if d < 0.01 && edge.is_none() {
                    edge = Some(rgb(&albedo, x, y));
                }
                if d > 0.3 && interior.is_none() {
                    interior = Some(rgb(&albedo, x, y));
                }
            }
        }
        let (edge, interior) = (edge.unwrap(), interior.unwrap());
        assert!(luma(edge) < luma(interior), "{edge:?} vs {interior:?}");
    }

    /// The groove the normal map shows is the mortar the albedo shows: on a
    /// Voronoi wall, nearly every pixel at the bottom of the groove is mortar.
    #[test]
    fn voronoi_groove_is_mortar_coloured() {
        let (_, field, params) = real_fields(128).remove(0);
        let params = TextureMapParams {
            detail_strength: 0.0,
            ..params
        };
        let albedo = derive_albedo(&field, &params, &mut SeededRng::new(1));
        let mortar: [f32; 3] = [0, 1, 2].map(|c| linear_to_srgb_f(params.mortar_color[c]));
        let mut deep = 0;
        let mut mortar_like = 0;
        for y in 0..field.height {
            for x in 0..field.width {
                if field.get(x, y).distance_to_edge < 0.1 {
                    deep += 1;
                    let c = rgb(&albedo, x, y);
                    if (0..3).all(|i| (c[i] - mortar[i]).abs() < 0.04) {
                        mortar_like += 1;
                    }
                }
            }
        }
        assert!(deep > 0);
        assert!(
            mortar_like * 10 >= deep * 9,
            "{mortar_like} of {deep} groove pixels are mortar"
        );
    }

    /// Per-cell variation keeps the material's hue: the old per-channel jitter
    /// turned a grey-brown stone into magenta, lime and teal cells.
    #[test]
    fn cell_colours_keep_the_base_hue() {
        fn hue(c: [f32; 3]) -> f32 {
            let (max, min) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
            let d = max - min;
            let h = if max == c[0] {
                ((c[1] - c[2]) / d).rem_euclid(6.0)
            } else if max == c[1] {
                (c[2] - c[0]) / d + 2.0
            } else {
                (c[0] - c[1]) / d + 4.0
            };
            h * 60.0
        }
        let base_srgb = [0x4A, 0x45, 0x40].map(|b| b as f32 / 255.0);
        let base = parse_hex_color("#4A4540").unwrap();
        let base_hue = hue(base_srgb);
        for id in 0..200 {
            let lin = cell_albedo(id, base, 0.12, 99);
            let srgb = [0, 1, 2].map(|c| linear_to_srgb_f(lin[c]));
            let max = srgb[0].max(srgb[1]).max(srgb[2]);
            let min = srgb[0].min(srgb[1]).min(srgb[2]);
            if max - min < 0.02 {
                continue; // near-grey: hue is undefined
            }
            let dh = (hue(srgb) - base_hue + 540.0).rem_euclid(360.0) - 180.0;
            assert!(
                dh.abs() < 12.0,
                "cell {id}: hue {} vs base {base_hue}",
                hue(srgb)
            );
        }
    }

    #[test]
    fn determinism() {
        for (_, field, params) in real_fields(64) {
            let a = derive_maps(&field, &params, &mut SeededRng::new(42));
            let b = derive_maps(&field, &params, &mut SeededRng::new(42));
            assert_eq!(a.len(), b.len());
            for (a, b) in a.iter().zip(b.iter()) {
                assert_eq!(a.pixels, b.pixels, "maps should be identical for same seed");
                assert_eq!(a.channel_semantics, b.channel_semantics);
            }
            let c = derive_maps(&field, &params, &mut SeededRng::new(43));
            assert_ne!(a[0].pixels, c[0].pixels, "another seed, another wall");
        }
    }

    #[test]
    fn output_maps_follow_the_spec() {
        let field = test_field(16);
        let three = derive_maps(
            &field,
            &TextureMapParams::default(),
            &mut SeededRng::new(42),
        );
        assert_eq!(
            three
                .iter()
                .map(|m| m.channel_semantics)
                .collect::<Vec<_>>(),
            [
                ChannelSemantics::Color,
                ChannelSemantics::Normal,
                ChannelSemantics::Roughness
            ]
        );

        let params = TextureMapParams {
            output_maps: vec!["roughness".into(), "normal".into()],
            ..TextureMapParams::default()
        };
        let two = derive_maps(&field, &params, &mut SeededRng::new(42));
        assert_eq!(two.len(), 2);
        assert_eq!(two[0].channel_semantics, ChannelSemantics::Roughness);
        assert_eq!(two[1].channel_semantics, ChannelSemantics::Normal);
        // A subset is the same pixels as in the full set.
        assert_eq!(two[1].pixels, three[1].pixels);
    }

    #[test]
    fn detail_strength_zero_has_no_fbm() {
        let field = test_field(32);
        let a = TextureMapParams {
            detail_strength: 0.0,
            ..TextureMapParams::default()
        };
        let b = TextureMapParams {
            detail_scale: 999.0,
            detail_octaves: 9,
            ..a.clone()
        };
        let ma = derive_maps(&field, &a, &mut SeededRng::new(42));
        let mb = derive_maps(&field, &b, &mut SeededRng::new(42));
        for (x, y) in ma.iter().zip(mb.iter()) {
            assert_eq!(x.pixels, y.pixels);
        }
    }

    /// Detail is surface texture, not static: neighbouring pixels of the
    /// field correlate strongly even at the showcase's `detail_scale = 45`,
    /// where the old double-applied frequency gave ~0 correlation.
    #[test]
    fn detail_is_not_aliased() {
        let params = TextureMapParams {
            detail_scale: 45.0,
            detail_octaves: 4,
            ..TextureMapParams::default()
        };
        let d = DetailField::build(512, 512, &params, 3).unwrap();
        let (mut num, mut den) = (0.0f64, 0.0f64);
        for y in 0..512 {
            for x in 0..511 {
                num += (d.at(x, y) * d.at(x + 1, y)) as f64;
                den += (d.at(x, y) * d.at(x, y)) as f64;
            }
        }
        let corr = num / den;
        assert!(corr > 0.6, "neighbour correlation {corr}");
        // Unit RMS, zero mean.
        let n = d.values.len() as f64;
        let mean: f64 = d.values.iter().map(|&v| v as f64).sum::<f64>() / n;
        let rms = (d.values.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / n).sqrt();
        assert!(
            mean.abs() < 1e-3 && (rms - 1.0).abs() < 1e-3,
            "{mean} {rms}"
        );
    }

    /// The detail shows but doesn't dominate: in the albedo it moves
    /// brightness inside a stone by a few percent, less than the stones
    /// differ from each other.
    #[test]
    fn detail_is_visible_but_not_dominant() {
        let (_, field, params) = real_fields(256).remove(0);
        let with = derive_albedo(&field, &params, &mut SeededRng::new(4));
        let without = derive_albedo(
            &field,
            &TextureMapParams {
                detail_strength: 0.0,
                ..params.clone()
            },
            &mut SeededRng::new(4),
        );

        let mut diff2 = 0.0f64;
        let mut n = 0.0f64;
        let mut cell_lumas = std::collections::BTreeMap::<u32, f32>::new();
        for y in 0..field.height {
            for x in 0..field.width {
                let cell = field.get(x, y);
                if cell.distance_to_edge < 1.0 {
                    continue;
                }
                let (a, b) = (luma(rgb(&with, x, y)), luma(rgb(&without, x, y)));
                diff2 += ((a - b) as f64).powi(2);
                n += 1.0;
                cell_lumas.insert(cell.cell_id, b);
            }
        }
        let detail_rms = (diff2 / n).sqrt() as f32;
        let lumas: Vec<f32> = cell_lumas.into_values().collect();
        let mean = lumas.iter().sum::<f32>() / lumas.len() as f32;
        let cell_sd =
            (lumas.iter().map(|l| (l - mean).powi(2)).sum::<f32>() / lumas.len() as f32).sqrt();
        assert!(detail_rms > 0.004, "detail invisible: {detail_rms}");
        assert!(
            detail_rms < cell_sd,
            "detail {detail_rms} drowns the stones (sd {cell_sd})"
        );
    }

    /// Every map of a seamless texture tiles: the step across the wrap is no
    /// bigger than the roughest step inside.
    #[test]
    fn seamless_maps_tile() {
        fn worst_interior_and_seam(img: &ImageData) -> (f32, f32) {
            let (w, h) = (img.width, img.height);
            let step = |x0: u32, x1: u32| -> f32 {
                (0..h)
                    .map(|y| {
                        let (a, b) = (rgb(img, x0, y), rgb(img, x1, y));
                        (0..3).map(|c| (a[c] - b[c]).abs()).sum::<f32>()
                    })
                    .sum::<f32>()
                    / h as f32
            };
            let interior = (0..w - 1).map(|x| step(x, x + 1)).fold(0.0, f32::max);
            (interior, step(w - 1, 0))
        }
        for (name, field, params) in real_fields(128) {
            let maps = derive_maps(&field, &params, &mut SeededRng::new(11));
            for map in &maps {
                let (interior, seam) = worst_interior_and_seam(map);
                assert!(
                    seam <= interior * 1.05 + 1e-4,
                    "{name} {:?}: seam step {seam} > roughest interior {interior}",
                    map.channel_semantics
                );
            }
        }
    }

    /// Relief is per UV, not per pixel: the same slope gives the same normal
    /// at 64² and 256².
    #[test]
    fn normal_relief_is_resolution_independent() {
        let params = TextureMapParams {
            detail_strength: 0.0,
            normal_strength: 50.0,
            seamless: false,
            ..TextureMapParams::default()
        };
        let small = derive_normal(&test_field(64), &params, &mut SeededRng::new(1));
        let large = derive_normal(&test_field(256), &params, &mut SeededRng::new(1));
        let a = decode_normal(&small, 20, 20);
        let b = decode_normal(&large, 80, 80);
        for c in 0..3 {
            assert!((a[c] - b[c]).abs() < 0.01, "{a:?} vs {b:?}");
        }
        // And the ramp tilts the normal against the slope.
        assert!(a[0] < -0.05 && a[1] < -0.05);
    }

    #[test]
    fn image_dimensions_match_params() {
        let field = test_field(64);
        let maps = derive_maps(
            &field,
            &TextureMapParams::default(),
            &mut SeededRng::new(42),
        );
        for map in &maps {
            assert_eq!((map.width, map.height), (field.width, field.height));
            assert!(map.validate().is_ok());
        }
    }

    #[test]
    fn params_from_toml_defaults() {
        let toml_val: toml::Value = toml::toml! {
            pattern = "voronoi_brick"
        }
        .into();
        let p = TextureMapParams::from_toml(&toml_val).unwrap();
        assert_eq!(p.width, 512);
        assert_eq!(p.height, 512);
        assert_eq!(p.output_maps.len(), 3);
        assert_eq!(p.mortar_threshold, GROOVE_MORTAR_THRESHOLD);

        let organic: toml::Value = toml::toml! { pattern = "perlin_organic" }.into();
        let p = TextureMapParams::from_toml(&organic).unwrap();
        assert_eq!(p.mortar_threshold, BAND_MORTAR_THRESHOLD);
    }

    #[test]
    fn params_from_toml_custom() {
        let toml_val: toml::Value = toml::toml! {
            width = 256
            height = 256
            output_maps = ["albedo", "normal"]
            pattern = "tiling_grid"
            base_color = "#8B4513"
            mortar_color = "#444444"
            roughness_base = 0.8
            normal_strength = 2.0
            detail_strength = 0.2
            detail_octaves = 6
            seamless = false
        }
        .into();
        let p = TextureMapParams::from_toml(&toml_val).unwrap();
        assert_eq!(p.width, 256);
        assert_eq!(p.output_maps.len(), 2);
        assert!(!p.seamless);
        assert!((p.normal_strength - 2.0).abs() < 1e-5);
        assert_eq!(p.detail_octaves, 6);
    }

    #[test]
    fn params_from_toml_rejects_bad_values() {
        let bad_map: toml::Value = toml::toml! { output_maps = ["albedo", "specular"] }.into();
        assert!(TextureMapParams::from_toml(&bad_map).is_err());
        let negative: toml::Value = toml::toml! { color_variation = -0.1 }.into();
        assert!(TextureMapParams::from_toml(&negative).is_err());
    }

    /// `cargo test -p flint-procgen -- --ignored map_debug_pngs` writes each
    /// pattern's maps to `target/test-output/`.
    #[test]
    #[ignore]
    fn map_debug_pngs() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-output");
        std::fs::create_dir_all(&dir).unwrap();
        for (name, field, params) in real_fields(512) {
            let maps = derive_maps(&field, &params, &mut SeededRng::new(42));
            for (map, kind) in maps.iter().zip(["albedo", "normal", "roughness"]) {
                let path = dir.join(format!("maps_{name}_{kind}.png"));
                image::save_buffer(
                    &path,
                    &map.pixels,
                    map.width,
                    map.height,
                    image::ColorType::Rgba8,
                )
                .unwrap();
                println!("wrote {}", path.display());
            }
        }
    }
}
