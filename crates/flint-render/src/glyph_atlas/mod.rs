//! Glyph atlas: monospaced glyphs rasterized into one coverage texture, for
//! the ASCII styles and any later text-based look (#264, epic #260).
//!
//! A [`GlyphAtlas`] is a grid of equal cells, one glyph each, in an 8-bit
//! coverage image (0 paper, 255 ink) with a one-pixel gutter round every cell
//! so a filtered sample never bleeds into a neighbour. It comes from one of
//! three sources:
//!
//! - the built-in 8x8 set ([`GlyphAtlas::builtin`]), drawn by hand for the
//!   engine in `builtin_8x8.txt` and free of any font licence;
//! - a TTF/OTF rasterized at a pixel height ([`GlyphAtlas::from_ttf`]);
//! - a bitmap grid image, one glyph per cell ([`GlyphAtlas::from_grid_image`]).
//!
//! A [`GlyphSet`] adds what a style needs on top: named luminance ramps and
//! the edge glyphs, read from a `*.glyphs.toml` (see [`set`]). Everything is
//! read through [`flint_core::vfs`], so a game bundle on the web serves it the
//! same as the disk. [`GpuGlyphAtlas`] uploads an atlas as an `R8Unorm`
//! texture.

mod gpu;
pub mod set;

pub use gpu::{stack_pixels, GpuGlyphAtlas, GpuGlyphStack};
pub use set::{
    calibrated_bounds, EdgeGlyphs, GlyphRamp, GlyphSet, GlyphSetCache, RampMap, RampOrder,
    BUILTIN_SET, GLYPHS_SUFFIX,
};

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use std::collections::{BTreeSet, HashMap};
use std::fmt;

/// Cells per atlas row.
pub const ATLAS_COLUMNS: u32 = 16;
/// Paper round every cell, in pixels.
pub const CELL_PADDING: u32 = 1;

/// Why a glyph atlas or set could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlyphError {
    /// A file could not be read through the vfs.
    Read(String),
    /// A `*.glyphs.toml` (or the built-in font text) did not parse.
    Parse(String),
    /// The font bytes are not a font ab_glyph can read.
    Font(String),
    /// Glyphs a ramp or edge set names that the font does not have.
    Missing(Vec<char>),
}

impl fmt::Display for GlyphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GlyphError::Read(m) => write!(f, "glyphs: {m}"),
            GlyphError::Parse(m) => write!(f, "glyphs: {m}"),
            GlyphError::Font(m) => write!(f, "glyphs: font: {m}"),
            GlyphError::Missing(chars) => {
                let list: Vec<String> = chars.iter().map(|c| format!("{c:?}")).collect();
                write!(f, "glyphs: the font has no glyph for {}", list.join(", "))
            }
        }
    }
}

impl std::error::Error for GlyphError {}

/// Every printable ASCII character, `' '` to `'~'`.
pub fn printable_ascii() -> impl Iterator<Item = char> {
    (0x20u8..=0x7e).map(char::from)
}

/// The quadrant blocks, each with its quadrants as bits: 1 upper left,
/// 2 upper right, 4 lower left, 8 lower right. The halves are two
/// quadrants; ` ` (none) and `█` (all four) are drawn by hand already.
pub const QUADRANT_BLOCKS: [(char, u8); 14] = [
    ('▘', 0b0001),
    ('▝', 0b0010),
    ('▖', 0b0100),
    ('▗', 0b1000),
    ('▀', 0b0011),
    ('▄', 0b1100),
    ('▌', 0b0101),
    ('▐', 0b1010),
    ('▚', 0b1001),
    ('▞', 0b0110),
    ('▙', 0b1101),
    ('▛', 0b0111),
    ('▜', 0b1011),
    ('▟', 0b1110),
];

/// The braille pattern for a 2x4 dot grid given as Unicode's dot bits
/// (bit 0 dot 1 at the top left, down the left column to dot 3, then dots
/// 4-6 down the right, then dots 7 and 8 along the bottom row).
pub fn braille_char(dots: u8) -> char {
    char::from_u32(0x2800 + dots as u32).expect("U+2800..=U+28FF are chars")
}

/// Unicode's braille dot bit for the dot at `column` (0..2), `row` (0..4).
pub fn braille_dot(column: u32, row: u32) -> u8 {
    match (column, row) {
        (0, 3) => 0x40,
        (1, 3) => 0x80,
        (c, r) => 1 << (c * 3 + r),
    }
}

/// The generated groups of the built-in set, in `builtin_8x8.txt`'s format:
/// their glyphs are their bit patterns, so they are written by rule rather
/// than by hand. A quadrant fills a 4x4 corner of the cell. A braille dot is
/// 2 px wide and 1 tall, at x 1-2 or 5-6 and y 0, 2, 4 or 6: dots on a
/// regular 4 x 2 px pitch that carries on into the next cell, with a dark
/// row and column between them so a dense pattern still reads as dots.
fn generated_8x8_text() -> String {
    let mut text = String::new();
    let mut glyph = |ch: char, ink: &dyn Fn(u32, u32) -> bool| {
        text.push_str(&format!("== {ch}\n"));
        for y in 0..8 {
            for x in 0..8 {
                text.push(if ink(x, y) { '#' } else { '.' });
            }
            text.push('\n');
        }
    };
    for (ch, quadrants) in QUADRANT_BLOCKS {
        glyph(ch, &|x, y| quadrants & (1 << ((y / 4) * 2 + x / 4)) != 0);
    }
    for dots in 0..=255u8 {
        glyph(braille_char(dots), &|x, y| {
            let column = x / 4;
            let on_dot = x % 4 == 1 || x % 4 == 2;
            on_dot && y % 2 == 0 && dots & braille_dot(column, y / 2) != 0
        });
    }
    text
}

/// Monospaced glyphs packed into one coverage image.
#[derive(Clone)]
pub struct GlyphAtlas {
    cell: [u32; 2],
    columns: u32,
    width: u32,
    height: u32,
    /// Coverage, row-major, `width * height` bytes.
    pixels: Vec<u8>,
    glyphs: Vec<char>,
    index: HashMap<char, u32>,
    /// Mean ink per glyph, 0..1, in `glyphs` order.
    coverage: Vec<f32>,
}

impl fmt::Debug for GlyphAtlas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlyphAtlas")
            .field("cell", &self.cell)
            .field("size", &[self.width, self.height])
            .field("glyphs", &self.glyphs.len())
            .finish()
    }
}

impl GlyphAtlas {
    /// An empty atlas of `glyphs` (deduplicated, in the order given) with
    /// cells of `cell` pixels; [`Self::paint`] fills them.
    fn blank(cell: [u32; 2], glyphs: Vec<char>) -> Self {
        let mut seen = BTreeSet::new();
        let glyphs: Vec<char> = glyphs.into_iter().filter(|c| seen.insert(*c)).collect();
        let columns = ATLAS_COLUMNS.min(glyphs.len().max(1) as u32);
        let rows = (glyphs.len() as u32).div_ceil(columns).max(1);
        let width = CELL_PADDING + columns * (cell[0] + CELL_PADDING);
        let height = CELL_PADDING + rows * (cell[1] + CELL_PADDING);
        let index = glyphs
            .iter()
            .enumerate()
            .map(|(i, c)| (*c, i as u32))
            .collect();
        Self {
            cell,
            columns,
            width,
            height,
            pixels: vec![0; (width * height) as usize],
            coverage: vec![0.0; glyphs.len()],
            glyphs,
            index,
        }
    }

    /// Write one pixel of glyph `i`'s cell, clipped to the cell.
    fn paint(&mut self, i: u32, x: i32, y: i32, value: u8) {
        if x < 0 || y < 0 || x as u32 >= self.cell[0] || y as u32 >= self.cell[1] {
            return;
        }
        let [ox, oy] = self.cell_origin(i);
        let at = ((oy + y as u32) * self.width + ox + x as u32) as usize;
        self.pixels[at] = self.pixels[at].max(value);
    }

    /// Measure every glyph's ink once the cells are painted.
    fn measure(mut self) -> Self {
        let area = (self.cell[0] * self.cell[1]) as f32;
        for i in 0..self.glyphs.len() as u32 {
            let [ox, oy] = self.cell_origin(i);
            let mut sum = 0u32;
            for y in oy..oy + self.cell[1] {
                let row = (y * self.width) as usize;
                for x in ox..ox + self.cell[0] {
                    sum += self.pixels[row + x as usize] as u32;
                }
            }
            self.coverage[i as usize] = sum as f32 / (255.0 * area);
        }
        self
    }

    /// The built-in 8x8 set: printable ASCII, the box-drawing edge glyphs
    /// `─│╱╲` with their corners and tees `┌┐└┘├┤┬┴┼`, the shade blocks
    /// `░▒▓█`, and two generated groups (#285): the half and quadrant
    /// blocks `▀▄▌▐▖▗▘▝▚▞▙▛▜▟` and all 256 braille patterns `⠀`..`⣿`.
    /// Built once per process.
    pub fn builtin() -> std::sync::Arc<GlyphAtlas> {
        static BUILTIN: std::sync::OnceLock<std::sync::Arc<GlyphAtlas>> =
            std::sync::OnceLock::new();
        BUILTIN
            .get_or_init(|| {
                let mut text = include_str!("builtin_8x8.txt").to_string();
                text.push_str(&generated_8x8_text());
                std::sync::Arc::new(
                    Self::from_pixel_text(&text, [8, 8]).expect("the built-in glyph text parses"),
                )
            })
            .clone()
    }

    /// Parse the hand-drawn format of `builtin_8x8.txt`: `== c` starts a
    /// glyph (`== space` for `' '`), then one line per pixel row of `#` and
    /// `.`. Rows narrower than the cell are centred in it.
    pub fn from_pixel_text(text: &str, cell: [u32; 2]) -> Result<Self, GlyphError> {
        let mut drawn: Vec<(char, Vec<&str>)> = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with(";;") {
                continue;
            }
            if let Some(name) = line.strip_prefix("== ") {
                let ch = match name {
                    "space" => ' ',
                    _ => {
                        let mut chars = name.chars();
                        match (chars.next(), chars.next()) {
                            (Some(c), None) => c,
                            _ => {
                                return Err(GlyphError::Parse(format!(
                                    "line {}: `== {name}` names more than one character",
                                    n + 1
                                )))
                            }
                        }
                    }
                };
                drawn.push((ch, Vec::new()));
            } else if let Some((ch, rows)) = drawn.last_mut() {
                if line.chars().any(|c| c != '#' && c != '.') {
                    return Err(GlyphError::Parse(format!(
                        "line {}: glyph {ch:?} row {line:?} is not `#` and `.`",
                        n + 1
                    )));
                }
                rows.push(line);
            } else {
                return Err(GlyphError::Parse(format!(
                    "line {}: pixels before any `== c` header",
                    n + 1
                )));
            }
        }

        let mut atlas = Self::blank(cell, drawn.iter().map(|(c, _)| *c).collect());
        for (ch, rows) in &drawn {
            if rows.len() as u32 != cell[1] {
                return Err(GlyphError::Parse(format!(
                    "glyph {ch:?} has {} rows, the cell {}",
                    rows.len(),
                    cell[1]
                )));
            }
            let i = atlas.index[ch];
            for (y, row) in rows.iter().enumerate() {
                let w = row.len() as u32;
                if w > cell[0] {
                    return Err(GlyphError::Parse(format!(
                        "glyph {ch:?} row {row:?} is wider than the cell ({})",
                        cell[0]
                    )));
                }
                let offset = ((cell[0] - w) / 2) as i32;
                for (x, px) in row.bytes().enumerate() {
                    if px == b'#' {
                        atlas.paint(i, offset + x as i32, y as i32, 255);
                    }
                }
            }
        }
        Ok(atlas.measure())
    }

    /// Rasterize `chars` (plus `' '`) from a TTF/OTF whose line is `px_height`
    /// pixels tall. The cell is the line height by the widest advance; each
    /// glyph sits on the font's baseline, centred in its cell, so a
    /// proportional font still packs (a monospaced one fills its cells).
    /// Characters the font lacks are an error listing them.
    pub fn from_ttf(
        bytes: &[u8],
        px_height: f32,
        chars: impl IntoIterator<Item = char>,
    ) -> Result<Self, GlyphError> {
        if !(px_height.is_finite() && px_height >= 4.0) {
            return Err(GlyphError::Parse(format!(
                "font size {px_height} is under the 4 px minimum"
            )));
        }
        let font = FontRef::try_from_slice(bytes).map_err(|e| GlyphError::Font(e.to_string()))?;
        let scaled = font.as_scaled(PxScale::from(px_height));

        let mut wanted: Vec<char> = vec![' '];
        wanted.extend(chars);
        let missing: BTreeSet<char> = wanted
            .iter()
            .copied()
            .filter(|c| *c != ' ' && font.glyph_id(*c).0 == 0)
            .collect();
        if !missing.is_empty() {
            return Err(GlyphError::Missing(missing.into_iter().collect()));
        }

        let ascent = scaled.ascent();
        let line = ascent - scaled.descent();
        let advance = wanted
            .iter()
            .map(|c| scaled.h_advance(font.glyph_id(*c)))
            .fold(0.0f32, f32::max);
        let cell = [advance.ceil().max(1.0) as u32, line.ceil().max(1.0) as u32];

        let mut atlas = Self::blank(cell, wanted);
        for i in 0..atlas.glyphs.len() as u32 {
            let ch = atlas.glyphs[i as usize];
            let id = font.glyph_id(ch);
            let x = (cell[0] as f32 - scaled.h_advance(id)) * 0.5;
            let glyph = id.with_scale_and_position(
                PxScale::from(px_height),
                ab_glyph::point(x.round(), ascent.round()),
            );
            if let Some(outline) = font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                let (bx, by) = (bounds.min.x as i32, bounds.min.y as i32);
                let mut ink = Vec::new();
                outline.draw(|gx, gy, c| {
                    ink.push((bx + gx as i32, by + gy as i32, (c * 255.0).round() as u8));
                });
                for (px, py, v) in ink {
                    atlas.paint(i, px, py, v);
                }
            }
        }
        Ok(atlas.measure())
    }

    /// Cut glyphs from a grid image whose cells are `cell` pixels, read left to
    /// right, top to bottom: cell `n` holds `chars[n]`. Ink is luminance times
    /// alpha (white on transparent or white on black), or `invert` for dark
    /// ink on light paper.
    pub fn from_grid_image(
        image: &image::DynamicImage,
        cell: [u32; 2],
        chars: impl IntoIterator<Item = char>,
        invert: bool,
    ) -> Result<Self, GlyphError> {
        if cell[0] == 0 || cell[1] == 0 {
            return Err(GlyphError::Parse("a grid cell needs a size".into()));
        }
        let rgba = image.to_rgba8();
        let grid_columns = rgba.width() / cell[0];
        let grid_rows = rgba.height() / cell[1];
        let chars: Vec<char> = chars.into_iter().collect();
        let capacity = (grid_columns * grid_rows) as usize;
        if chars.len() > capacity {
            return Err(GlyphError::Missing(chars[capacity..].to_vec()));
        }
        let mut atlas = Self::blank(cell, chars.clone());
        for (n, ch) in chars.iter().enumerate() {
            let i = atlas.index[ch];
            let (gx, gy) = (
                (n as u32 % grid_columns) * cell[0],
                (n as u32 / grid_columns) * cell[1],
            );
            for y in 0..cell[1] {
                for x in 0..cell[0] {
                    let p = rgba.get_pixel(gx + x, gy + y).0;
                    let luma = (p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19) / 256;
                    let luma = if invert { 255 - luma } else { luma };
                    let v = (luma * p[3] as u32 / 255) as u8;
                    atlas.paint(i, x as i32, y as i32, v);
                }
            }
        }
        Ok(atlas.measure())
    }

    /// Cell size in pixels, `[w, h]`.
    pub fn cell_size(&self) -> [u32; 2] {
        self.cell
    }

    /// Atlas size in pixels, `[w, h]`.
    pub fn size(&self) -> [u32; 2] {
        [self.width, self.height]
    }

    /// Cells per atlas row.
    pub fn columns(&self) -> u32 {
        self.columns
    }

    /// The coverage bytes, row-major, `width * height` of them.
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// The glyphs, in atlas order.
    pub fn glyphs(&self) -> &[char] {
        &self.glyphs
    }

    /// Whether the atlas has a glyph for `ch`.
    pub fn contains(&self, ch: char) -> bool {
        self.index.contains_key(&ch)
    }

    /// The atlas index of `ch`.
    pub fn index_of(&self, ch: char) -> Option<u32> {
        self.index.get(&ch).copied()
    }

    /// Top-left pixel of glyph `i`'s cell (inside its gutter).
    pub fn cell_origin(&self, i: u32) -> [u32; 2] {
        let (col, row) = (i % self.columns, i / self.columns);
        [
            CELL_PADDING + col * (self.cell[0] + CELL_PADDING),
            CELL_PADDING + row * (self.cell[1] + CELL_PADDING),
        ]
    }

    /// `[u_min, v_min, u_max, v_max]` of `ch`'s cell, 0..1.
    pub fn uv_rect(&self, ch: char) -> Option<[f32; 4]> {
        let [x, y] = self.cell_origin(self.index_of(ch)?);
        let (w, h) = (self.width as f32, self.height as f32);
        Some([
            x as f32 / w,
            y as f32 / h,
            (x + self.cell[0]) as f32 / w,
            (y + self.cell[1]) as f32 / h,
        ])
    }

    /// Mean ink of `ch`'s cell, 0 (none) to 1 (solid).
    pub fn coverage(&self, ch: char) -> Option<f32> {
        Some(self.coverage[self.index_of(ch)? as usize])
    }

    /// Coverage of one pixel of `ch`'s cell, 0..255; `None` off the cell.
    pub fn sample(&self, ch: char, x: u32, y: u32) -> Option<u8> {
        if x >= self.cell[0] || y >= self.cell[1] {
            return None;
        }
        let [ox, oy] = self.cell_origin(self.index_of(ch)?);
        Some(self.pixels[((oy + y) * self.width + ox + x) as usize])
    }

    /// The atlas as a grayscale image, for inspection.
    pub fn to_image(&self) -> image::GrayImage {
        image::GrayImage::from_raw(self.width, self.height, self.pixels.clone())
            .expect("pixels are width * height")
    }
}

#[cfg(test)]
mod tests;
