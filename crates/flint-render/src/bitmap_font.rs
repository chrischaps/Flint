//! Text for `ui_text` sprites, laid out over a [`GlyphAtlas`] (ADR 0113).
//!
//! World and sprite text uses the same glyphs as the ASCII styles: a
//! `ui_text.font` names any glyph source the engine knows, and every one of
//! them becomes a [`GlyphAtlas`]:
//!
//! - `builtin`, the hand-drawn 8x8 set;
//! - a glyph set, by name (`fonts/<name>.glyphs.toml` beside the scene or in
//!   its parent) or by a path ending `.glyphs.toml`, so a TTF, a PNG grid or
//!   hand-drawn pixel text;
//! - a `.font.toml`, the older format (#283 kept it): a `[font]` table naming a
//!   PNG grid (`texture`, `glyph_width`, `glyph_height`, `columns`,
//!   `first_char`, `char_count`), cut into an atlas with
//!   [`GlyphAtlas::from_grid_image`], or naming a set (`glyphs = "<name>"`)
//!   to give it a `spacing`.
//!
//! The atlas's coverage goes up once as white RGBA whose alpha is the ink
//! ([`BitmapFont::rgba`]), under [`BitmapFont::texture`] in the
//! `TextureCache`, and each character becomes one [`Sprite2dInstanceGpu`], so
//! whole strings batch into the sprite2d pipeline with no shader of their own.

use crate::glyph_atlas::{GlyphAtlas, GlyphError, GlyphSetCache, BUILTIN_SET};
use crate::sprite2d_pipeline::Sprite2dInstanceGpu;
use flint_core::project::{shown, AssetKind, AssetScope};
use flint_core::toml_util::toml_f32;
use flint_core::vfs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The suffix of the older bitmap-font description.
pub const FONT_TOML_SUFFIX: &str = ".font.toml";

/// Prefix of the `TextureCache` names fonts upload under, so a font never
/// shares a name with an image file.
pub const FONT_TEXTURE_PREFIX: &str = "font:";

/// A font for sprite text: a glyph atlas and how far apart to set its cells.
#[derive(Clone, Debug)]
pub struct BitmapFont {
    /// `TextureCache` name of the uploaded atlas (`font:<what ui_text named>`).
    pub texture: String,
    /// Horizontal spacing multiplier (1.0 = cells edge to edge).
    pub spacing: f32,
    atlas: Arc<GlyphAtlas>,
}

impl BitmapFont {
    /// A font over `atlas`, uploaded under `font:<name>`.
    pub fn new(name: &str, atlas: Arc<GlyphAtlas>, spacing: f32) -> Self {
        Self {
            texture: format!("{FONT_TEXTURE_PREFIX}{name}"),
            spacing,
            atlas,
        }
    }

    /// Resolve what a `ui_text.font` names, by the project's one asset
    /// search from `scope` (ADR 0114): `builtin`, a glyph set name or
    /// `.glyphs.toml` path ([`AssetKind::GlyphSets`]), or a `.font.toml`
    /// ([`AssetKind::Fonts`]: the path from the scene's directory, from
    /// each directory above it up to the project root, then in the mounted
    /// engine). Everything is read through the vfs.
    pub fn resolve(spec: &str, scope: &AssetScope) -> Result<Self, GlyphError> {
        if spec.ends_with(FONT_TOML_SUFFIX) {
            let path = scope
                .resolve(AssetKind::Fonts, spec)
                .map_err(|missing| GlyphError::Read(missing.to_string()))?;
            return Self::load_font_toml(spec, &path, scope);
        }
        let atlas = glyph_set_atlas(spec, scope)?;
        Ok(Self::new(spec, atlas, 1.0))
    }

    /// Load a `.font.toml` (`name` is what `ui_text.font` said). Its PNG
    /// is found as an image texture is ([`AssetKind::Textures`], from the
    /// scene), and failing that beside the font file.
    pub fn load_font_toml(name: &str, path: &Path, scope: &AssetScope) -> Result<Self, GlyphError> {
        let text = vfs::read_to_string(path)
            .map_err(|e| GlyphError::Read(format!("{}: {e}", path.display())))?;
        let table: toml::Value = text
            .parse()
            .map_err(|e: toml::de::Error| parse_error(path, e.to_string().trim_end()))?;
        let font = table
            .get("font")
            .ok_or_else(|| parse_error(path, "no [font] table"))?;
        let spacing = font.get("spacing").and_then(toml_f32).unwrap_or(1.0);

        if let Some(set) = font.get("glyphs").and_then(|v| v.as_str()) {
            let atlas = glyph_set_atlas(set, scope)?;
            return Ok(Self::new(name, atlas, spacing));
        }

        let int = |key: &str| font.get(key).and_then(|v| v.as_integer());
        let need = |key: &str| {
            int(key)
                .filter(|v| *v > 0)
                .map(|v| v as u32)
                .ok_or_else(|| parse_error(path, &format!("[font] needs `{key}` (or `glyphs`)")))
        };
        let texture = font
            .get("texture")
            .and_then(|v| v.as_str())
            .ok_or_else(|| parse_error(path, "[font] needs `texture` (or `glyphs`)"))?;
        let cell = [need("glyph_width")?, need("glyph_height")?];
        let columns = need("columns")?;
        let first_char = int("first_char").unwrap_or(32).max(0) as u32;
        let char_count = int("char_count").unwrap_or(95).max(0) as u32;

        let image_path = font_image_path(texture, path, scope).map_err(|tried| {
            let mut text = format!("{}: no image `{texture}`. Tried:", shown(path));
            for place in tried {
                text.push_str("\n  ");
                text.push_str(&shown(&place));
            }
            GlyphError::Read(text)
        })?;
        let image = crate::open_image(&image_path)
            .map_err(|e| GlyphError::Read(format!("{}: {e}", image_path.display())))?;
        // The grid is `columns` cells wide whatever the image's width, as the
        // old UV maths had it: crop to that before cutting.
        let image = image.crop_imm(0, 0, (columns * cell[0]).min(image.width()), image.height());
        let chars = (first_char..first_char + char_count).filter_map(char::from_u32);
        let atlas = GlyphAtlas::from_grid_image(&image, cell, chars, false)?;
        Ok(Self::new(name, Arc::new(atlas), spacing))
    }

    /// The glyph atlas the font draws from.
    pub fn atlas(&self) -> &Arc<GlyphAtlas> {
        &self.atlas
    }

    /// The atlas as the texture the sprite2d pipeline samples: white, with
    /// the coverage as alpha. `width * height * 4` bytes, `[w, h]`.
    pub fn rgba(&self) -> (Vec<u8>, [u32; 2]) {
        let pixels = self
            .atlas
            .pixels()
            .iter()
            .flat_map(|&ink| [255, 255, 255, ink])
            .collect();
        (pixels, self.atlas.size())
    }

    /// UV rect `[u_min, v_min, u_max, v_max]` of a character's cell, 0..1,
    /// or `None` when the font has no glyph for it.
    pub fn glyph_uv(&self, ch: char) -> Option<[f32; 4]> {
        self.atlas.uv_rect(ch)
    }

    /// Width over height of one glyph cell.
    fn aspect(&self) -> f32 {
        let [w, h] = self.atlas.cell_size();
        w as f32 / h as f32
    }

    /// World-unit width of a single glyph at the given scale.
    fn glyph_advance(&self, scale: f32) -> f32 {
        scale * self.aspect() * self.spacing
    }

    /// Expand a text string into per-glyph sprite instances.
    ///
    /// * `x, y` — baseline-left origin in world coordinates
    /// * `scale` — text height in world units
    /// * `tint` — RGBA colour
    /// * `layer` — sprite layer for z-ordering
    /// * `align` — "left", "center", or "right"
    ///
    /// A character the font lacks draws nothing but still takes its cell.
    /// Returns `Vec<(texture_name, layer, instance)>` ready to push into
    /// `sprite2d_collected`.
    #[allow(clippy::too_many_arguments)]
    pub fn layout_text(
        &self,
        text: &str,
        x: f32,
        y: f32,
        scale: f32,
        tint: [f32; 4],
        layer: i32,
        align: &str,
    ) -> Vec<(String, i32, Sprite2dInstanceGpu)> {
        let advance = self.glyph_advance(scale);
        let total_width = text.chars().count() as f32 * advance;

        let start_x = match align {
            "center" => x - total_width * 0.5,
            "right" => x - total_width,
            _ => x, // "left" or default
        };

        let glyph_w = scale * self.aspect();
        let glyph_h = scale;

        let mut result = Vec::with_capacity(text.len());
        let mut cursor_x = start_x;

        for ch in text.chars() {
            if let Some(uv_rect) = self.glyph_uv(ch) {
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

/// The atlas of the glyph set `spec` names (`builtin`, a set name or a
/// `.glyphs.toml` path), found by the project's search from `scope`.
fn glyph_set_atlas(spec: &str, scope: &AssetScope) -> Result<Arc<GlyphAtlas>, GlyphError> {
    if spec.is_empty() || spec == BUILTIN_SET {
        return Ok(crate::glyph_atlas::GlyphSet::builtin().atlas().clone());
    }
    let set = GlyphSetCache::new(scope.clone()).get(spec)?;
    Ok(set.atlas().clone())
}

/// Where a `.font.toml`'s `texture` is: where a texture of that name is
/// (the project's search from the scene), then beside the font file, which
/// is a file the description names relative to itself and no search. `Err`
/// is every path tried.
fn font_image_path(
    texture: &str,
    font_path: &Path,
    scope: &AssetScope,
) -> Result<PathBuf, Vec<PathBuf>> {
    let mut tried = match scope.resolve(AssetKind::Textures, texture) {
        Ok(path) => return Ok(path),
        Err(missing) => missing.tried,
    };
    let beside = font_path
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .join(texture);
    if vfs::is_file(&beside) {
        return Ok(beside);
    }
    if !tried.contains(&beside) {
        tried.push(beside);
    }
    Err(tried)
}

fn parse_error(path: &Path, message: &str) -> GlyphError {
    GlyphError::Parse(format!("{}: {message}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::vfs::MemorySource;

    /// A 16-column, 6-row grid of 8x16 cells, like `demo/ui_test`'s font:
    /// each glyph's cell has a solid column at `x = index % 8`, so which cell
    /// a character came from can be read back off the atlas.
    fn grid_png() -> Vec<u8> {
        let mut img = image::RgbaImage::new(128, 96);
        for i in 0..95u32 {
            let (gx, gy) = ((i % 16) * 8, (i / 16) * 16);
            for y in 0..16 {
                img.put_pixel(gx + i % 8, gy + y, image::Rgba([255, 255, 255, 255]));
            }
        }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png
    }

    const LEGACY: &str = "[font]\ntexture = \"fonts/default_font.png\"\nglyph_width = 8\n\
        glyph_height = 16\ncolumns = 16\nfirst_char = 32\nchar_count = 95\nspacing = 1.25\n";

    fn scene_files() -> Arc<MemorySource> {
        let mut src = MemorySource::new();
        src.insert("game/levels/fonts/default.font.toml", LEGACY);
        src.insert("game/levels/fonts/default_font.png", grid_png());
        src.insert(
            "game/levels/fonts/wide.font.toml",
            "[font]\nglyphs = \"builtin\"\nspacing = 2.0\n",
        );
        src.insert(
            "game/fonts/dots.glyphs.toml",
            "[ramps.dots]\nglyphs = \" .:\"\n",
        );
        Arc::new(src)
    }

    /// The scope of a scene in `dir` of the file source in use.
    fn scope(dir: &str) -> AssetScope {
        AssetScope::for_dir(Path::new(dir))
    }

    fn test_font() -> BitmapFont {
        vfs::scoped(scene_files(), || {
            BitmapFont::resolve("fonts/default.font.toml", &scope("game/levels")).unwrap()
        })
    }

    /// ADR 0114: from a scene two folders down, the font description, the
    /// image it names and a glyph set are all found in the project.
    #[test]
    fn a_scene_two_folders_down_finds_the_projects_fonts() {
        let mut src = MemorySource::new();
        src.insert("game/flint.toml", "");
        src.insert("game/fonts/default.font.toml", LEGACY);
        src.insert("game/fonts/default_font.png", grid_png());
        src.insert(
            "game/fonts/dots.glyphs.toml",
            "[ramps.dots]\nglyphs = \" .:\"\n",
        );
        src.insert("game/levels/act1/cave/a.scene.toml", "");
        vfs::scoped(Arc::new(src), || {
            let deep = scope("game/levels/act1/cave");
            let font = BitmapFont::resolve("fonts/default.font.toml", &deep).unwrap();
            assert_eq!(font.atlas().cell_size(), [8, 16]);
            assert!(BitmapFont::resolve("dots", &deep).is_ok());
            // A missing one lists every place tried, nearest first
            let gone = BitmapFont::resolve("fonts/gone.font.toml", &deep).unwrap_err();
            let text = gone.to_string();
            let tried: Vec<&str> = text.lines().skip(1).map(str::trim).collect();
            assert_eq!(
                tried[0], "game/levels/act1/cave/fonts/gone.font.toml",
                "{text}"
            );
            assert!(tried.contains(&"game/fonts/gone.font.toml"), "{text}");
        });
    }

    /// A font's image that is nowhere says where it was looked for: the
    /// texture search from the scene, then beside the font file.
    #[test]
    fn a_missing_font_image_lists_the_paths_tried() {
        let mut src = MemorySource::new();
        src.insert("game/flint.toml", "");
        src.insert(
            "game/fonts/ui.font.toml",
            "[font]\ntexture = \"ui.png\"\nglyph_width = 8\nglyph_height = 8\ncolumns = 16\n",
        );
        src.insert("game/levels/a.scene.toml", "");
        vfs::scoped(Arc::new(src.clone()), || {
            let err = BitmapFont::resolve("fonts/ui.font.toml", &scope("game/levels")).unwrap_err();
            let text = err.to_string();
            let tried: Vec<&str> = text.lines().skip(1).map(str::trim).collect();
            assert_eq!(tried[0], "game/levels/ui.png", "{text}");
            assert!(tried.contains(&"game/textures/ui.png"), "{text}");
            assert_eq!(tried.last(), Some(&"game/fonts/ui.png"), "{text}");
        });
        // Beside the font file it is found, as before
        src.insert("game/fonts/ui.png", grid_png());
        vfs::scoped(Arc::new(src), || {
            assert!(BitmapFont::resolve("fonts/ui.font.toml", &scope("game/levels")).is_ok());
        });
    }

    #[test]
    fn a_font_toml_cuts_its_grid_into_an_atlas() {
        let font = test_font();
        assert_eq!(font.texture, "font:fonts/default.font.toml");
        assert_eq!(font.spacing, 1.25);
        let atlas = font.atlas();
        assert_eq!(atlas.cell_size(), [8, 16]);
        assert_eq!(atlas.glyphs().len(), 95);
        // 'A' = 65 was grid cell 33: its ink column is at x = 33 % 8 = 1.
        assert_eq!(atlas.sample('A', 1, 0), Some(255));
        assert_eq!(atlas.sample('A', 0, 0), Some(0));
        assert_eq!(atlas.sample('~', ('~' as u32 - 32) % 8, 15), Some(255));
    }

    #[test]
    fn glyph_uv_is_the_atlas_cell() {
        let font = test_font();
        assert_eq!(font.glyph_uv('A'), font.atlas().uv_rect('A'));
        assert!(font.glyph_uv('\x00').is_none());
        assert!(font.glyph_uv('\u{FF}').is_none());
    }

    #[test]
    fn rgba_is_white_with_coverage_alpha() {
        let font = test_font();
        let (rgba, [w, h]) = font.rgba();
        assert_eq!(rgba.len(), (w * h * 4) as usize);
        let cov = font.atlas().pixels();
        for (px, ink) in rgba.as_chunks::<4>().0.iter().zip(cov) {
            assert_eq!(px, &[255, 255, 255, *ink]);
        }
    }

    #[test]
    fn builtin_and_glyph_sets_resolve_like_the_ascii_styles() {
        vfs::scoped(scene_files(), || {
            let dir = &scope("game/levels");
            let builtin = BitmapFont::resolve("builtin", dir).unwrap();
            assert_eq!(builtin.atlas().cell_size(), [8, 8]);
            assert_eq!(builtin.spacing, 1.0);
            // A set name looks in `fonts/` beside the scene, then its parent.
            let dots = BitmapFont::resolve("dots", dir).unwrap();
            assert!(dots.atlas().contains('A'));
            let by_path = BitmapFont::resolve("fonts/dots.glyphs.toml", dir).unwrap();
            assert_eq!(by_path.atlas().size(), dots.atlas().size());
            // A `.font.toml` can name a set to give it spacing.
            let wide = BitmapFont::resolve("fonts/wide.font.toml", dir).unwrap();
            assert_eq!(wide.spacing, 2.0);
            assert_eq!(wide.atlas().cell_size(), [8, 8]);
        });
    }

    #[test]
    fn bad_fonts_say_why() {
        vfs::scoped(scene_files(), || {
            let dir = &scope("game/levels");
            assert!(matches!(
                BitmapFont::resolve("fonts/gone.font.toml", dir),
                Err(GlyphError::Read(_))
            ));
            assert!(BitmapFont::resolve("nope", dir).is_err());
        });
        let mut src = MemorySource::new();
        src.insert("s/a.font.toml", "[font]\ntexture = \"x.png\"\n");
        src.insert("s/b.font.toml", "[font]\nglyph_width = 8\n");
        vfs::scoped(Arc::new(src), || {
            let a = BitmapFont::resolve("a.font.toml", &scope("s")).unwrap_err();
            assert!(a.to_string().contains("glyph_width"), "{a}");
            let b = BitmapFont::resolve("b.font.toml", &scope("s")).unwrap_err();
            assert!(b.to_string().contains("texture"), "{b}");
        });
    }

    #[test]
    fn layout_text_count() {
        let font = test_font();
        let glyphs = font.layout_text("Hi", 0.0, 0.0, 1.0, [1.0; 4], 0, "left");
        assert_eq!(glyphs.len(), 2);
        assert!(glyphs.iter().all(|g| g.0 == font.texture));
    }

    #[test]
    fn a_missing_glyph_keeps_its_cell() {
        let font = test_font();
        let glyphs = font.layout_text("A\u{FF}B", 0.0, 0.0, 1.0, [1.0; 4], 0, "left");
        assert_eq!(glyphs.len(), 2);
        let advance = font.glyph_advance(1.0);
        assert!((glyphs[1].2.pos_layer[0] - glyphs[0].2.pos_layer[0] - 2.0 * advance).abs() < 1e-5);
    }

    #[test]
    fn layout_text_center_alignment() {
        let font = test_font();
        let advance = font.glyph_advance(1.0);
        let total_w = 3.0 * advance;

        let glyphs = font.layout_text("ABC", 10.0, 5.0, 1.0, [1.0; 4], 0, "center");
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

        let glyphs = font.layout_text("AB", 10.0, 5.0, 1.0, [1.0; 4], 0, "right");
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
            (
                [uv_rect[0], uv_rect[1], new_u_max, uv_rect[3]],
                new_w,
                height,
                x_off,
                0.0,
            )
        }
        "right_to_left" => {
            let new_w = width * value;
            let u_range = uv_rect[2] - uv_rect[0];
            let new_u_min = uv_rect[2] - u_range * value;
            let x_off = (width - new_w) * 0.5;
            (
                [new_u_min, uv_rect[1], uv_rect[2], uv_rect[3]],
                new_w,
                height,
                x_off,
                0.0,
            )
        }
        "bottom_to_top" => {
            let new_h = height * value;
            let v_range = uv_rect[3] - uv_rect[1];
            let new_v_min = uv_rect[3] - v_range * value;
            let y_off = -(height - new_h) * 0.5;
            (
                [uv_rect[0], new_v_min, uv_rect[2], uv_rect[3]],
                width,
                new_h,
                0.0,
                y_off,
            )
        }
        "top_to_bottom" => {
            let new_h = height * value;
            let v_range = uv_rect[3] - uv_rect[1];
            let new_v_max = uv_rect[1] + v_range * value;
            let y_off = (height - new_h) * 0.5;
            (
                [uv_rect[0], uv_rect[1], uv_rect[2], new_v_max],
                width,
                new_h,
                0.0,
                y_off,
            )
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
        let (uv, w, _h, xo, _yo) =
            apply_fill([0.0, 0.0, 1.0, 1.0], 10.0, 5.0, 0.5, "right_to_left");
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
        let (uv, _w, h, _xo, yo) =
            apply_fill([0.0, 0.0, 1.0, 1.0], 10.0, 8.0, 0.25, "top_to_bottom");
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
