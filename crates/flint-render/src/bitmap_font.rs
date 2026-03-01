//! Bitmap font loader and text layout for GPU sprite rendering.
//!
//! Grid-based ASCII font atlas: a `.font.toml` file defines the texture,
//! glyph grid dimensions, and character range. Each character maps to one
//! [`Sprite2dInstanceGpu`] so entire strings batch into the existing sprite2d
//! pipeline with zero new shaders.

use crate::sprite2d_pipeline::Sprite2dInstanceGpu;
use std::collections::HashMap;
use std::path::Path;

/// Parsed bitmap font definition.
#[derive(Clone, Debug)]
pub struct BitmapFont {
    /// Texture name (used as key in TextureCache)
    pub texture: String,
    /// Glyph cell width in pixels
    pub glyph_width: u32,
    /// Glyph cell height in pixels
    pub glyph_height: u32,
    /// Number of columns in the atlas grid
    pub columns: u32,
    /// ASCII code of the first glyph in the atlas
    pub first_char: u32,
    /// Number of glyphs in the atlas
    pub char_count: u32,
    /// Horizontal spacing multiplier (1.0 = no extra space)
    pub spacing: f32,
    /// Per-character width overrides (glyph_char -> width in pixels), for variable-width
    pub char_widths: HashMap<char, u32>,
}

impl BitmapFont {
    /// Parse a `.font.toml` file. Returns `None` on parse failure.
    pub fn load(path: &Path) -> Option<Self> {
        let content = std::fs::read_to_string(path).ok()?;
        let table: toml::Value = content.parse().ok()?;
        let font = table.get("font")?;

        let texture = font.get("texture")?.as_str()?.to_string();
        let glyph_width = font.get("glyph_width")?.as_integer()? as u32;
        let glyph_height = font.get("glyph_height")?.as_integer()? as u32;
        let columns = font.get("columns")?.as_integer()? as u32;
        let first_char = font
            .get("first_char")
            .and_then(|v| v.as_integer())
            .unwrap_or(32) as u32;
        let char_count = font
            .get("char_count")
            .and_then(|v| v.as_integer())
            .unwrap_or(95) as u32;
        let spacing = font
            .get("spacing")
            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
            .unwrap_or(1.0) as f32;

        Some(Self {
            texture,
            glyph_width,
            glyph_height,
            columns,
            first_char,
            char_count,
            spacing,
            char_widths: HashMap::new(),
        })
    }

    /// UV rect `[u_min, v_min, u_max, v_max]` for a character, normalized 0–1.
    /// `tex_w`/`tex_h` are the texture dimensions in pixels.
    pub fn glyph_uv(&self, ch: char, tex_w: u32, tex_h: u32) -> Option<[f32; 4]> {
        let code = ch as u32;
        if code < self.first_char || code >= self.first_char + self.char_count {
            return None;
        }
        let index = code - self.first_char;
        let col = index % self.columns;
        let row = index / self.columns;

        let u_min = (col * self.glyph_width) as f32 / tex_w as f32;
        let v_min = (row * self.glyph_height) as f32 / tex_h as f32;
        let u_max = ((col + 1) * self.glyph_width) as f32 / tex_w as f32;
        let v_max = ((row + 1) * self.glyph_height) as f32 / tex_h as f32;

        Some([u_min, v_min, u_max, v_max])
    }

    /// World-unit width of a single glyph at the given scale.
    fn glyph_advance(&self, scale: f32) -> f32 {
        let aspect = self.glyph_width as f32 / self.glyph_height as f32;
        scale * aspect * self.spacing
    }

    /// Expand a text string into per-glyph sprite instances.
    ///
    /// * `x, y` — baseline-left origin in world coordinates
    /// * `scale` — text height in world units
    /// * `tint` — RGBA colour
    /// * `layer` — sprite layer for z-ordering
    /// * `align` — "left", "center", or "right"
    /// * `tex_w, tex_h` — font texture dimensions in pixels
    ///
    /// Returns `Vec<(texture_name, layer, instance)>` ready to push into
    /// `sprite2d_collected`.
    pub fn layout_text(
        &self,
        text: &str,
        x: f32,
        y: f32,
        scale: f32,
        tint: [f32; 4],
        layer: i32,
        align: &str,
        tex_w: u32,
        tex_h: u32,
    ) -> Vec<(String, i32, Sprite2dInstanceGpu)> {
        let advance = self.glyph_advance(scale);
        let total_width = text.len() as f32 * advance;

        let start_x = match align {
            "center" => x - total_width * 0.5,
            "right" => x - total_width,
            _ => x, // "left" or default
        };

        let glyph_w = scale * (self.glyph_width as f32 / self.glyph_height as f32);
        let glyph_h = scale;

        let mut result = Vec::with_capacity(text.len());
        let mut cursor_x = start_x;

        for ch in text.chars() {
            if let Some(uv_rect) = self.glyph_uv(ch, tex_w, tex_h) {
                let instance = Sprite2dInstanceGpu {
                    pos_layer: [cursor_x + glyph_w * 0.5, y, 0.5, layer as f32 * 0.01],
                    size: [glyph_w, glyph_h, 0.0, 0.0],
                    uv_rect,
                    tint,
                    flags: [0.0, 0.0, 0.0, 0.0],
                };
                result.push((self.texture.clone(), layer, instance));
            }
            cursor_x += advance;
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_font() -> BitmapFont {
        BitmapFont {
            texture: "test_font.png".to_string(),
            glyph_width: 8,
            glyph_height: 16,
            columns: 16,
            first_char: 32,
            char_count: 95,
            spacing: 1.0,
            char_widths: HashMap::new(),
        }
    }

    #[test]
    fn glyph_uv_space() {
        let font = test_font();
        // Space is char 32, first_char is 32, so index 0 → col 0, row 0
        let uv = font.glyph_uv(' ', 128, 96).unwrap();
        assert_eq!(uv[0], 0.0); // u_min
        assert_eq!(uv[1], 0.0); // v_min
        assert!((uv[2] - 8.0 / 128.0).abs() < 1e-6); // u_max
        assert!((uv[3] - 16.0 / 96.0).abs() < 1e-6); // v_max
    }

    #[test]
    fn glyph_uv_a() {
        let font = test_font();
        // 'A' = 65, index = 65 - 32 = 33, col = 33 % 16 = 1, row = 33 / 16 = 2
        let uv = font.glyph_uv('A', 128, 96).unwrap();
        assert!((uv[0] - 1.0 * 8.0 / 128.0).abs() < 1e-6);
        assert!((uv[1] - 2.0 * 16.0 / 96.0).abs() < 1e-6);
    }

    #[test]
    fn glyph_uv_out_of_range() {
        let font = test_font();
        assert!(font.glyph_uv('\x00', 128, 96).is_none());
        assert!(font.glyph_uv('\u{FF}', 128, 96).is_none());
    }

    #[test]
    fn layout_text_count() {
        let font = test_font();
        let glyphs = font.layout_text("Hi", 0.0, 0.0, 1.0, [1.0; 4], 0, "left", 128, 96);
        assert_eq!(glyphs.len(), 2);
    }

    #[test]
    fn layout_text_center_alignment() {
        let font = test_font();
        let advance = font.glyph_advance(1.0);
        let total_w = 3.0 * advance;

        let glyphs = font.layout_text("ABC", 10.0, 5.0, 1.0, [1.0; 4], 0, "center", 128, 96);
        assert_eq!(glyphs.len(), 3);

        let glyph_w = 1.0 * (8.0 / 16.0); // scale * aspect
        let expected_first_x = 10.0 - total_w * 0.5 + glyph_w * 0.5;
        assert!((glyphs[0].2.pos_layer[0] - expected_first_x).abs() < 1e-4);
    }

    #[test]
    fn layout_text_right_alignment() {
        let font = test_font();
        let advance = font.glyph_advance(1.0);
        let total_w = 2.0 * advance;

        let glyphs = font.layout_text("AB", 10.0, 5.0, 1.0, [1.0; 4], 0, "right", 128, 96);
        let glyph_w = 1.0 * (8.0 / 16.0);
        let expected_first_x = 10.0 - total_w + glyph_w * 0.5;
        assert!((glyphs[0].2.pos_layer[0] - expected_first_x).abs() < 1e-4);
    }

    #[test]
    fn anchor_origin_nine_point() {
        // Verify all 9 anchor positions
        let half_w = 8.0;
        let half_h = 5.0;

        let cases = [
            ("top-left", (-half_w, half_h)),
            ("top-center", (0.0, half_h)),
            ("top-right", (half_w, half_h)),
            ("center-left", (-half_w, 0.0)),
            ("center", (0.0, 0.0)),
            ("center-right", (half_w, 0.0)),
            ("bottom-left", (-half_w, -half_h)),
            ("bottom-center", (0.0, -half_h)),
            ("bottom-right", (half_w, -half_h)),
        ];

        for (name, expected) in cases {
            let (ax, ay) = anchor_origin(name, half_w, half_h);
            assert!(
                (ax - expected.0).abs() < 1e-6 && (ay - expected.1).abs() < 1e-6,
                "anchor '{}': got ({}, {}), expected ({}, {})",
                name,
                ax,
                ay,
                expected.0,
                expected.1
            );
        }
    }
}

/// Compute the screen-space origin for a named anchor point.
///
/// * `half_w` = `ortho_height * aspect_ratio / 2.0`
/// * `half_h` = `ortho_height / 2.0`
///
/// Returns `(ax, ay)` relative to camera center.
pub fn anchor_origin(anchor: &str, half_w: f32, half_h: f32) -> (f32, f32) {
    match anchor {
        "top-left" => (-half_w, half_h),
        "top-center" => (0.0, half_h),
        "top-right" => (half_w, half_h),
        "center-left" => (-half_w, 0.0),
        "center" => (0.0, 0.0),
        "center-right" => (half_w, 0.0),
        "bottom-left" => (-half_w, -half_h),
        "bottom-center" => (0.0, -half_h),
        "bottom-right" => (half_w, -half_h),
        _ => (0.0, 0.0), // default to center
    }
}

/// Apply ui_fill clipping to a sprite's UV rect and dimensions.
///
/// Returns `(new_uv_rect, new_width, new_height, x_offset, y_offset)`.
/// The offsets compensate position so the visible portion stays aligned.
pub fn apply_fill(
    uv_rect: [f32; 4],
    width: f32,
    height: f32,
    value: f32,
    direction: &str,
) -> ([f32; 4], f32, f32, f32, f32) {
    let value = value.clamp(0.0, 1.0);

    match direction {
        "left_to_right" => {
            let new_w = width * value;
            let u_range = uv_rect[2] - uv_rect[0];
            let new_u_max = uv_rect[0] + u_range * value;
            let x_off = -(width - new_w) * 0.5;
            ([uv_rect[0], uv_rect[1], new_u_max, uv_rect[3]], new_w, height, x_off, 0.0)
        }
        "right_to_left" => {
            let new_w = width * value;
            let u_range = uv_rect[2] - uv_rect[0];
            let new_u_min = uv_rect[2] - u_range * value;
            let x_off = (width - new_w) * 0.5;
            ([new_u_min, uv_rect[1], uv_rect[2], uv_rect[3]], new_w, height, x_off, 0.0)
        }
        "bottom_to_top" => {
            let new_h = height * value;
            let v_range = uv_rect[3] - uv_rect[1];
            let new_v_min = uv_rect[3] - v_range * value;
            let y_off = -(height - new_h) * 0.5;
            ([uv_rect[0], new_v_min, uv_rect[2], uv_rect[3]], width, new_h, 0.0, y_off)
        }
        "top_to_bottom" => {
            let new_h = height * value;
            let v_range = uv_rect[3] - uv_rect[1];
            let new_v_max = uv_rect[1] + v_range * value;
            let y_off = (height - new_h) * 0.5;
            ([uv_rect[0], uv_rect[1], uv_rect[2], new_v_max], width, new_h, 0.0, y_off)
        }
        _ => (uv_rect, width, height, 0.0, 0.0),
    }
}

#[cfg(test)]
mod fill_tests {
    use super::*;

    #[test]
    fn fill_left_to_right_half() {
        let (uv, w, h, xo, yo) = apply_fill([0.0, 0.0, 1.0, 1.0], 10.0, 5.0, 0.5, "left_to_right");
        assert!((w - 5.0).abs() < 1e-6);
        assert!((h - 5.0).abs() < 1e-6);
        assert!((uv[2] - 0.5).abs() < 1e-6);
        assert!((xo - -2.5).abs() < 1e-6);
        assert!((yo).abs() < 1e-6);
    }

    #[test]
    fn fill_right_to_left_half() {
        let (uv, w, _h, xo, _yo) = apply_fill([0.0, 0.0, 1.0, 1.0], 10.0, 5.0, 0.5, "right_to_left");
        assert!((w - 5.0).abs() < 1e-6);
        assert!((uv[0] - 0.5).abs() < 1e-6);
        assert!((xo - 2.5).abs() < 1e-6);
    }

    #[test]
    fn fill_bottom_to_top_full() {
        let (uv, w, h, xo, yo) = apply_fill([0.0, 0.0, 1.0, 1.0], 10.0, 5.0, 1.0, "bottom_to_top");
        assert!((w - 10.0).abs() < 1e-6);
        assert!((h - 5.0).abs() < 1e-6);
        assert!((uv[1] - 0.0).abs() < 1e-6);
        assert!((xo).abs() < 1e-6);
        assert!((yo).abs() < 1e-6);
    }

    #[test]
    fn fill_top_to_bottom_quarter() {
        let (uv, _w, h, _xo, yo) = apply_fill([0.0, 0.0, 1.0, 1.0], 10.0, 8.0, 0.25, "top_to_bottom");
        assert!((h - 2.0).abs() < 1e-6);
        assert!((uv[3] - 0.25).abs() < 1e-6);
        assert!((yo - 3.0).abs() < 1e-6);
    }

    #[test]
    fn fill_clamps_value() {
        let (_, w, _, _, _) = apply_fill([0.0, 0.0, 1.0, 1.0], 10.0, 5.0, 1.5, "left_to_right");
        assert!((w - 10.0).abs() < 1e-6);
        let (_, w2, _, _, _) = apply_fill([0.0, 0.0, 1.0, 1.0], 10.0, 5.0, -0.5, "left_to_right");
        assert!((w2).abs() < 1e-6);
    }
}
