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
//! PatternField ──► derive_albedo()     ──► ImageData (Color, sRGB RGBA8)
//!              ├─► derive_normal()     ──► ImageData (Normal, linear RGBA8)
//!              └─► derive_roughness()  ──► ImageData (Roughness, linear RGBA8)
//! ```

use crate::algorithms::noise::{perlin_fbm, NoiseSource};
use crate::generators::texture::pattern::PatternField;
use crate::generators::util::{parse_hex_color, toml_f32, toml_string, toml_string_array, toml_u32};
use crate::types::{ChannelSemantics, ImageData};
use crate::{ProcGenError, Result, SeededRng};

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
    /// Per-cell color shift magnitude.
    pub color_variation: f32,
    /// Color for mortar/gap regions (linear RGBA).
    pub mortar_color: [f32; 4],
    /// Mortar region threshold — pixels with `distance_to_edge` below this
    /// value blend toward mortar color.
    pub mortar_threshold: f32,
    /// Base surface roughness (0–1).
    pub roughness_base: f32,
    /// Per-cell roughness shift magnitude.
    pub roughness_variation: f32,
    /// Roughness in mortar regions (defaults smoother than surface).
    pub roughness_mortar: f32,
    /// Normal map strength multiplier.
    pub normal_strength: f32,
    /// FBM frequency for micro-detail overlay.
    pub detail_scale: f32,
    /// FBM amplitude (0 = no detail noise).
    pub detail_strength: f32,
    /// Whether the texture should tile seamlessly.
    pub seamless: bool,
}

impl Default for TextureMapParams {
    fn default() -> Self {
        Self {
            width: 512,
            height: 512,
            output_maps: vec![
                "albedo".into(),
                "normal".into(),
                "roughness".into(),
            ],
            pattern: "voronoi_brick".into(),
            base_color: [0.45, 0.35, 0.28, 1.0],
            color_variation: 0.06,
            mortar_color: [0.25, 0.22, 0.18, 1.0],
            mortar_threshold: 0.05,
            roughness_base: 0.75,
            roughness_variation: 0.05,
            roughness_mortar: 0.5,
            normal_strength: 1.0,
            detail_scale: 8.0,
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
        if let Some(v) = table.get("base_color") {
            let hex = v.as_str().ok_or_else(|| ProcGenError::InvalidParameter {
                name: "base_color".into(),
                reason: "expected a hex color string".into(),
            })?;
            p.base_color = parse_hex_color(hex)?;
        }
        if let Some(v) = table.get("color_variation") {
            p.color_variation = toml_f32(v, "color_variation")?;
        }
        if let Some(v) = table.get("mortar_color") {
            let hex = v.as_str().ok_or_else(|| ProcGenError::InvalidParameter {
                name: "mortar_color".into(),
                reason: "expected a hex color string".into(),
            })?;
            p.mortar_color = parse_hex_color(hex)?;
        }
        if let Some(v) = table.get("mortar_threshold") {
            p.mortar_threshold = toml_f32(v, "mortar_threshold")?;
        }
        if let Some(v) = table.get("roughness_base") {
            p.roughness_base = toml_f32(v, "roughness_base")?;
        }
        if let Some(v) = table.get("roughness_variation") {
            p.roughness_variation = toml_f32(v, "roughness_variation")?;
        }
        if let Some(v) = table.get("roughness_mortar") {
            p.roughness_mortar = toml_f32(v, "roughness_mortar")?;
        }
        if let Some(v) = table.get("normal_strength") {
            p.normal_strength = toml_f32(v, "normal_strength")?;
        }
        if let Some(v) = table.get("detail_scale") {
            p.detail_scale = toml_f32(v, "detail_scale")?;
        }
        if let Some(v) = table.get("detail_strength") {
            p.detail_strength = toml_f32(v, "detail_strength")?;
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

// ─── Per-Cell Color ────────────────────────────────────────────────────────

/// Deterministic per-cell color from cell ID, using a hash-based approach
/// to avoid dependence on pixel traversal order.
fn cell_color_for_id(cell_id: u32, base_color: [f32; 4], variation: f32, rng_seed: u64) -> [f32; 4] {
    if variation <= 0.0 {
        return base_color;
    }
    // Combine the master seed with the cell ID for a deterministic per-cell seed.
    let cell_seed = rng_seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(cell_id as u64);
    let mut cell_rng = SeededRng::new(cell_seed);
    cell_rng.next_color_variation(base_color, variation)
}

/// Deterministic per-cell roughness shift from cell ID.
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

// ─── sRGB Conversion ──────────────────────────────────────────────────────

/// Gamma-correct a linear float to an sRGB byte.
fn linear_to_srgb(v: f32) -> u8 {
    let s = if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (s.clamp(0.0, 1.0) * 255.0) as u8
}

// ─── Heightfield Sampling ─────────────────────────────────────────────────

/// Sample the combined height at (x, y) with toroidal wrapping for seamless
/// tiling. Combines the pattern field height with an optional FBM detail layer.
fn combined_height(
    field: &PatternField,
    x: i32,
    y: i32,
    detail_noise: Option<&dyn NoiseSource>,
    detail_strength: f32,
    detail_scale: f32,
) -> f32 {
    let w = field.width as i32;
    let h = field.height as i32;
    let wx = ((x % w) + w) % w;
    let wy = ((y % h) + h) % h;

    let base = field.get(wx as u32, wy as u32).height;

    if let Some(noise) = detail_noise {
        let nx = wx as f64 / w as f64 * detail_scale as f64;
        let ny = wy as f64 / h as f64 * detail_scale as f64;
        base + detail_strength * noise.sample_2d(nx, ny) as f32
    } else {
        base
    }
}

// ─── Albedo Derivation ────────────────────────────────────────────────────

/// Derive an albedo (base color) map from a pattern field.
///
/// For each pixel:
/// - Look up cell_id and distance_to_edge
/// - Compute per-cell color variation deterministically via hash
/// - Blend toward mortar_color near edges
/// - Overlay subtle FBM noise for micro-detail
/// - Output sRGB RGBA8
pub fn derive_albedo(field: &PatternField, params: &TextureMapParams, rng: &mut SeededRng) -> ImageData {
    let w = field.width;
    let h = field.height;
    let mut img = ImageData::new(w, h, ChannelSemantics::Color);
    let seed = rng.seed();

    // FBM detail noise (optional)
    let detail_noise = if params.detail_strength > 0.0 {
        let noise_rng = rng.fork("albedo_detail");
        Some(perlin_fbm(noise_rng.seed(), 4, params.detail_scale as f64))
    } else {
        None
    };

    for y in 0..h {
        for x in 0..w {
            let cell = field.get(x, y);

            // Per-cell color variation
            let mut color = cell_color_for_id(cell.cell_id, params.base_color, params.color_variation, seed);

            // Mortar blend: smooth transition near edges
            if cell.distance_to_edge < params.mortar_threshold && params.mortar_threshold > 0.0 {
                let t = cell.distance_to_edge / params.mortar_threshold;
                // t=0 at edge (full mortar), t=1 at threshold (full surface)
                for (c, &mc) in color.iter_mut().zip(params.mortar_color.iter()).take(3) {
                    *c = mc * (1.0 - t) + *c * t;
                }
            }

            // Detail overlay: subtle brightness modulation
            if let Some(ref noise) = detail_noise {
                let nx = x as f64 / w as f64 * params.detail_scale as f64;
                let ny = y as f64 / h as f64 * params.detail_scale as f64;
                let detail = noise.sample_2d(nx, ny) as f32 * params.detail_strength;
                for c in color.iter_mut().take(3) {
                    *c = (*c + detail).clamp(0.0, 1.0);
                }
            }

            // Encode as sRGB RGBA8
            let offset = ((y * w + x) * 4) as usize;
            img.pixels[offset] = linear_to_srgb(color[0]);
            img.pixels[offset + 1] = linear_to_srgb(color[1]);
            img.pixels[offset + 2] = linear_to_srgb(color[2]);
            img.pixels[offset + 3] = (color[3].clamp(0.0, 1.0) * 255.0) as u8;
        }
    }

    img
}

// ─── Normal Derivation ────────────────────────────────────────────────────

/// Derive a tangent-space normal map from a pattern field.
///
/// Builds a combined heightfield (pattern height + optional FBM detail),
/// then applies a finite-difference gradient to compute tangent-space normals.
/// The Sobel-like kernel wraps toroidally for seamless tiling.
///
/// Output is linear RGBA8 with the standard encoding:
/// `((nx * 0.5 + 0.5) * 255, (ny * 0.5 + 0.5) * 255, (nz * 0.5 + 0.5) * 255, 255)`
pub fn derive_normal(field: &PatternField, params: &TextureMapParams, rng: &mut SeededRng) -> ImageData {
    let w = field.width;
    let h = field.height;
    let mut img = ImageData::new(w, h, ChannelSemantics::Normal);

    // FBM detail noise (optional)
    let detail_noise: Option<Box<dyn NoiseSource>> = if params.detail_strength > 0.0 {
        let noise_rng = rng.fork("normal_detail");
        Some(Box::new(perlin_fbm(noise_rng.seed(), 4, params.detail_scale as f64)))
    } else {
        None
    };
    let noise_ref = detail_noise.as_deref();

    for y in 0..h {
        for x in 0..w {
            let ix = x as i32;
            let iy = y as i32;

            // Finite-difference gradient (2-sample Sobel-like)
            let h_left = combined_height(field, ix - 1, iy, noise_ref, params.detail_strength, params.detail_scale);
            let h_right = combined_height(field, ix + 1, iy, noise_ref, params.detail_strength, params.detail_scale);
            let h_up = combined_height(field, ix, iy - 1, noise_ref, params.detail_strength, params.detail_scale);
            let h_down = combined_height(field, ix, iy + 1, noise_ref, params.detail_strength, params.detail_scale);

            let dx = (h_right - h_left) * params.normal_strength;
            let dy = (h_down - h_up) * params.normal_strength;

            // Tangent-space normal: Z starts at 1.0 before normalization,
            // guaranteeing nz > 0 in the output.
            let nx = -dx;
            let ny = -dy;
            let nz = 1.0_f32;
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            let nx = nx / len;
            let ny = ny / len;
            let nz = nz / len;

            // Encode to RGBA8 (linear — no gamma)
            let offset = ((y * w + x) * 4) as usize;
            img.pixels[offset] = ((nx * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            img.pixels[offset + 1] = ((ny * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            img.pixels[offset + 2] = ((nz * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0) as u8;
            img.pixels[offset + 3] = 255;
        }
    }

    img
}

// ─── Roughness Derivation ─────────────────────────────────────────────────

/// Derive a roughness map from a pattern field.
///
/// - Surface regions use `roughness_base` with per-cell variation
/// - Mortar/gap regions use `roughness_mortar`
/// - Smooth blend between mortar and surface via `distance_to_edge`
/// - Optional FBM detail overlay (less aggressive than on albedo)
/// - Output is grayscale RGBA8 (R=G=B=roughness, A=255), linear
pub fn derive_roughness(field: &PatternField, params: &TextureMapParams, rng: &mut SeededRng) -> ImageData {
    let w = field.width;
    let h = field.height;
    let mut img = ImageData::new(w, h, ChannelSemantics::Roughness);
    let seed = rng.seed();

    // FBM detail noise (optional)
    let detail_noise = if params.detail_strength > 0.0 {
        let noise_rng = rng.fork("roughness_detail");
        Some(perlin_fbm(noise_rng.seed(), 4, params.detail_scale as f64))
    } else {
        None
    };

    for y in 0..h {
        for x in 0..w {
            let cell = field.get(x, y);

            // Per-cell roughness variation
            let cell_shift = cell_roughness_shift(cell.cell_id, params.roughness_variation, seed);
            let surface_rough = (params.roughness_base + cell_shift).clamp(0.0, 1.0);

            // Mortar blend
            let roughness = if cell.distance_to_edge < params.mortar_threshold && params.mortar_threshold > 0.0 {
                let t = cell.distance_to_edge / params.mortar_threshold;
                params.roughness_mortar * (1.0 - t) + surface_rough * t
            } else {
                surface_rough
            };

            // Detail overlay (half strength compared to albedo)
            let roughness = if let Some(ref noise) = detail_noise {
                let nx = x as f64 / w as f64 * params.detail_scale as f64;
                let ny = y as f64 / h as f64 * params.detail_scale as f64;
                let detail = noise.sample_2d(nx, ny) as f32 * params.detail_strength * 0.5;
                (roughness + detail).clamp(0.0, 1.0)
            } else {
                roughness
            };

            // Encode as grayscale RGBA8 (linear — no gamma)
            let byte = (roughness.clamp(0.0, 1.0) * 255.0) as u8;
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
/// `output_maps`. Each map is generated with a forked RNG for determinism.
pub fn derive_maps(field: &PatternField, params: &TextureMapParams, rng: &mut SeededRng) -> Vec<ImageData> {
    let mut results = Vec::with_capacity(params.output_maps.len());

    for map_name in &params.output_maps {
        match map_name.as_str() {
            "albedo" => {
                let mut map_rng = rng.fork("derive_albedo");
                results.push(derive_albedo(field, params, &mut map_rng));
            }
            "normal" => {
                let mut map_rng = rng.fork("derive_normal");
                results.push(derive_normal(field, params, &mut map_rng));
            }
            "roughness" => {
                let mut map_rng = rng.fork("derive_roughness");
                results.push(derive_roughness(field, params, &mut map_rng));
            }
            _ => {} // Invalid names filtered by from_toml validation
        }
    }

    results
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generators::texture::pattern::{PatternCell, PatternField};

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
                    (x as f32 / size as f32).min(y as f32 / size as f32)
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

    #[test]
    fn normal_z_always_positive() {
        let field = test_field(32);
        let params = TextureMapParams::default();
        let mut rng = SeededRng::new(42);
        let normal_map = derive_normal(&field, &params, &mut rng);

        assert!(normal_map.validate().is_ok());

        // Z channel is at offset +2 per pixel; verify > 128 (i.e., decoded Z > 0)
        for y in 0..field.height {
            for x in 0..field.width {
                let offset = ((y * field.width + x) * 4 + 2) as usize;
                let z_byte = normal_map.pixels[offset];
                assert!(
                    z_byte >= 128,
                    "normal Z < 128 at ({x}, {y}): {z_byte}"
                );
            }
        }
    }

    #[test]
    fn roughness_in_range() {
        let field = test_field(32);
        let params = TextureMapParams::default();
        let mut rng = SeededRng::new(42);
        let rough_map = derive_roughness(&field, &params, &mut rng);

        assert!(rough_map.validate().is_ok());

        for i in (0..rough_map.pixels.len()).step_by(4) {
            let r = rough_map.pixels[i] as f32 / 255.0;
            assert!(
                (0.0..=1.0).contains(&r),
                "roughness out of range: {r}"
            );
        }
    }

    #[test]
    fn albedo_mortar_differs_from_surface() {
        let field = test_field(64);
        let params = TextureMapParams {
            mortar_threshold: 0.1,
            color_variation: 0.0, // disable variation for cleaner test
            ..TextureMapParams::default()
        };
        let mut rng = SeededRng::new(42);
        let albedo = derive_albedo(&field, &params, &mut rng);

        // Find a pixel near the edge (distance_to_edge ≈ 0) and one in the interior
        let mut edge_pixel = None;
        let mut interior_pixel = None;

        for y in 0..field.height {
            for x in 0..field.width {
                let cell = field.get(x, y);
                let offset = ((y * field.width + x) * 4) as usize;
                let rgb = [albedo.pixels[offset], albedo.pixels[offset + 1], albedo.pixels[offset + 2]];

                if cell.distance_to_edge < 0.01 && edge_pixel.is_none() {
                    edge_pixel = Some(rgb);
                }
                if cell.distance_to_edge > 0.3 && interior_pixel.is_none() {
                    interior_pixel = Some(rgb);
                }
            }
        }

        if let (Some(edge), Some(interior)) = (edge_pixel, interior_pixel) {
            // At least one channel should differ
            let differs = edge[0] != interior[0] || edge[1] != interior[1] || edge[2] != interior[2];
            assert!(differs, "edge {edge:?} should differ from interior {interior:?}");
        }
    }

    #[test]
    fn determinism() {
        let field = test_field(32);
        let params = TextureMapParams::default();

        let mut rng_a = SeededRng::new(42);
        let maps_a = derive_maps(&field, &params, &mut rng_a);

        let mut rng_b = SeededRng::new(42);
        let maps_b = derive_maps(&field, &params, &mut rng_b);

        assert_eq!(maps_a.len(), maps_b.len());
        for (a, b) in maps_a.iter().zip(maps_b.iter()) {
            assert_eq!(a.pixels, b.pixels, "maps should be identical for same seed");
            assert_eq!(a.channel_semantics, b.channel_semantics);
        }
    }

    #[test]
    fn output_maps_filtering() {
        let field = test_field(16);
        let params = TextureMapParams {
            output_maps: vec!["normal".into()],
            ..TextureMapParams::default()
        };
        let mut rng = SeededRng::new(42);
        let maps = derive_maps(&field, &params, &mut rng);

        assert_eq!(maps.len(), 1);
        assert_eq!(maps[0].channel_semantics, ChannelSemantics::Normal);
    }

    #[test]
    fn detail_strength_zero_has_no_fbm() {
        let field = test_field(32);

        let params_no_detail = TextureMapParams {
            detail_strength: 0.0,
            color_variation: 0.0,
            output_maps: vec!["albedo".into()],
            ..TextureMapParams::default()
        };
        let params_with_detail = TextureMapParams {
            detail_strength: 0.0,
            detail_scale: 999.0, // would produce very different noise if applied
            color_variation: 0.0,
            output_maps: vec!["albedo".into()],
            ..TextureMapParams::default()
        };

        let mut rng_a = SeededRng::new(42);
        let maps_a = derive_maps(&field, &params_no_detail, &mut rng_a);

        let mut rng_b = SeededRng::new(42);
        let maps_b = derive_maps(&field, &params_with_detail, &mut rng_b);

        // With detail_strength=0, the scale shouldn't matter — output is identical
        assert_eq!(maps_a[0].pixels, maps_b[0].pixels);
    }

    #[test]
    fn image_dimensions_match_params() {
        let field = test_field(64);
        let params = TextureMapParams::default();
        let mut rng = SeededRng::new(42);
        let maps = derive_maps(&field, &params, &mut rng);

        for map in &maps {
            assert_eq!(map.width, field.width);
            assert_eq!(map.height, field.height);
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
            seamless = false
        }
        .into();
        let p = TextureMapParams::from_toml(&toml_val).unwrap();
        assert_eq!(p.width, 256);
        assert_eq!(p.output_maps.len(), 2);
        assert!(!p.seamless);
        assert!((p.normal_strength - 2.0).abs() < 1e-5);
    }

    #[test]
    fn params_from_toml_invalid_map_name() {
        let toml_val: toml::Value = toml::toml! {
            output_maps = ["albedo", "specular"]
        }
        .into();
        assert!(TextureMapParams::from_toml(&toml_val).is_err());
    }

    #[test]
    #[ignore]
    fn visual_debug_pngs() {
        use crate::generators::texture::voronoi::{VoronoiBrickParams, VoronoiBrickPattern};
        use crate::generators::texture::pattern::Pattern;
        use std::path::PathBuf;

        let dir = PathBuf::from(std::env::temp_dir()).join("flint_texture_maps_debug");
        std::fs::create_dir_all(&dir).unwrap();

        let mut rng = SeededRng::new(42);
        let pattern = VoronoiBrickPattern {
            params: VoronoiBrickParams {
                cell_count: 24,
                mortar_width: 0.03,
                height_variation: 0.4,
                regularity: 0.3,
            },
        };

        let field = pattern.generate(512, 512, true, &mut rng);
        let params = TextureMapParams::default();
        let mut derive_rng = SeededRng::new(42);
        let maps = derive_maps(&field, &params, &mut derive_rng);

        let names = ["albedo", "normal", "roughness"];
        for (map, name) in maps.iter().zip(names.iter()) {
            let path = dir.join(format!("{name}.png"));
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

        // Also save the pattern debug images
        let height_img = field.to_height_image();
        let path = dir.join("pattern_height.png");
        image::save_buffer(&path, &height_img.pixels, height_img.width, height_img.height, image::ColorType::Rgba8).unwrap();
        println!("wrote {}", path.display());

        let edge_img = field.to_edge_distance_image();
        let path = dir.join("pattern_edge.png");
        image::save_buffer(&path, &edge_img.pixels, edge_img.width, edge_img.height, image::ColorType::Rgba8).unwrap();
        println!("wrote {}", path.display());

        println!("\nAll debug PNGs written to: {}", dir.display());
    }
}
