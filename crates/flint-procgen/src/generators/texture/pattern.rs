//! Pattern field types and trait for texture pattern generators.
//!
//! A [`PatternField`] is the intermediate representation that all pattern
//! generators produce — one [`PatternCell`] per pixel. Downstream stages
//! (Task 4.2) consume these fields to derive albedo, normal, and roughness
//! textures.

use crate::{ChannelSemantics, ImageData, SeededRng};

// ---------------------------------------------------------------------------
// Pattern trait
// ---------------------------------------------------------------------------

/// A pattern generator that fills a 2D field with cell IDs, heights, and
/// edge distances. Implementations must be deterministic for a given seed.
pub trait Pattern: Send + Sync {
    /// Generate a pattern field of the given dimensions.
    ///
    /// When `seamless` is true, the pattern tiles without visible seams.
    fn generate(
        &self,
        width: u32,
        height: u32,
        seamless: bool,
        rng: &mut SeededRng,
    ) -> PatternField;
}

// ---------------------------------------------------------------------------
// PatternField
// ---------------------------------------------------------------------------

/// A 2D grid of [`PatternCell`] values produced by a pattern generator.
///
/// Width × height cells, stored in row-major order.
#[derive(Debug, Clone)]
pub struct PatternField {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Cell data, length = `width * height`, stored row-major.
    pub cells: Vec<PatternCell>,
}

impl PatternField {
    /// Create a new field with all cells zeroed.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            cells: vec![PatternCell::default(); (width * height) as usize],
        }
    }

    /// Get a reference to the cell at `(x, y)`.
    ///
    /// # Panics
    /// Panics if `x >= width` or `y >= height`.
    pub fn get(&self, x: u32, y: u32) -> &PatternCell {
        debug_assert!(x < self.width && y < self.height);
        &self.cells[(y * self.width + x) as usize]
    }

    /// Get a mutable reference to the cell at `(x, y)`.
    ///
    /// # Panics
    /// Panics if `x >= width` or `y >= height`.
    pub fn get_mut(&mut self, x: u32, y: u32) -> &mut PatternCell {
        debug_assert!(x < self.width && y < self.height);
        let idx = (y * self.width + x) as usize;
        &mut self.cells[idx]
    }

    /// Sample the field at normalised coordinates, treating it as a tile.
    ///
    /// `(u, v)` wrap into `[0, 1)`, and pixel centres sit at `(i + 0.5) / n`,
    /// so `get_normalized(0.0, v)` is halfway between the last column and the
    /// first: the same value whichever way round the tile you approach it.
    /// `height` and `distance_to_edge` are bilinear; `cell_id` (a label, not a
    /// quantity) comes from the nearest pixel.
    pub fn get_normalized(&self, u: f32, v: f32) -> PatternCell {
        let (w, h) = (self.width as i64, self.height as i64);
        let fx = u.rem_euclid(1.0) * w as f32 - 0.5;
        let fy = v.rem_euclid(1.0) * h as f32 - 0.5;
        let (x0, y0) = (fx.floor() as i64, fy.floor() as i64);
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: i64, y: i64| self.get(x.rem_euclid(w) as u32, y.rem_euclid(h) as u32);
        let (c00, c10, c01, c11) = (
            at(x0, y0),
            at(x0 + 1, y0),
            at(x0, y0 + 1),
            at(x0 + 1, y0 + 1),
        );
        let bi = |f: fn(&PatternCell) -> f32| {
            let top = f(c00) + (f(c10) - f(c00)) * tx;
            let bottom = f(c01) + (f(c11) - f(c01)) * tx;
            top + (bottom - top) * ty
        };
        let nearest = at((fx + 0.5).floor() as i64, (fy + 0.5).floor() as i64);
        PatternCell {
            cell_id: nearest.cell_id,
            height: bi(|c| c.height),
            distance_to_edge: bi(|c| c.distance_to_edge),
        }
    }

    /// How visible the tile's seams are, measured on `height`.
    ///
    /// The last column and the first are neighbours once the tile repeats, so
    /// in a seamless field the step across the wrap looks like a step between
    /// any other pair of neighbouring columns; they are not equal values.
    /// For every pair of adjacent columns (and rows) this takes the mean
    /// absolute height step down the pair. The seam's pair is compared with
    /// the roughest interior pair, not the average one, because a pattern
    /// may put an edge on the seam on purpose (a grid's grooves sit on its
    /// tile boundaries, and so on the wrap).
    pub fn seam_steps(&self) -> SeamSteps {
        let (w, h) = (self.width, self.height);
        let hgt = |x: u32, y: u32| self.get(x, y).height;
        let col_pair =
            |a: u32, b: u32| (0..h).map(|y| (hgt(a, y) - hgt(b, y)).abs()).sum::<f32>() / h as f32;
        let row_pair =
            |a: u32, b: u32| (0..w).map(|x| (hgt(x, a) - hgt(x, b)).abs()).sum::<f32>() / w as f32;
        let cols: Vec<f32> = (1..w).map(|x| col_pair(x - 1, x)).collect();
        let rows: Vec<f32> = (1..h).map(|y| row_pair(y - 1, y)).collect();
        let max = |v: &[f32]| v.iter().copied().fold(0.0f32, f32::max);
        SeamSteps {
            across_x: col_pair(w - 1, 0),
            across_y: row_pair(h - 1, 0),
            interior_x: max(&cols),
            interior_y: max(&rows),
            interior: cols.iter().sum::<f32>() / cols.len().max(1) as f32,
        }
    }

    /// Convert the height channel to a grayscale RGBA8 [`ImageData`] for
    /// debugging and visual inspection.
    pub fn to_height_image(&self) -> ImageData {
        let mut img = ImageData::new(self.width, self.height, ChannelSemantics::Height);
        for (i, cell) in self.cells.iter().enumerate() {
            let byte = (cell.height.clamp(0.0, 1.0) * 255.0) as u8;
            let offset = i * 4;
            img.pixels[offset] = byte;
            img.pixels[offset + 1] = byte;
            img.pixels[offset + 2] = byte;
            img.pixels[offset + 3] = 255;
        }
        img
    }

    /// Convert the cell_id channel to a color-coded RGBA8 [`ImageData`] for
    /// debugging. Each cell gets a deterministic pseudo-random color.
    pub fn to_cell_id_image(&self) -> ImageData {
        let mut img = ImageData::new(self.width, self.height, ChannelSemantics::Mask);
        for (i, cell) in self.cells.iter().enumerate() {
            let (r, g, b) = id_to_color(cell.cell_id);
            let offset = i * 4;
            img.pixels[offset] = r;
            img.pixels[offset + 1] = g;
            img.pixels[offset + 2] = b;
            img.pixels[offset + 3] = 255;
        }
        img
    }

    /// Convert the distance_to_edge channel to a grayscale RGBA8 [`ImageData`].
    pub fn to_edge_distance_image(&self) -> ImageData {
        let mut img = ImageData::new(self.width, self.height, ChannelSemantics::Mask);
        for (i, cell) in self.cells.iter().enumerate() {
            let byte = (cell.distance_to_edge.clamp(0.0, 1.0) * 255.0) as u8;
            let offset = i * 4;
            img.pixels[offset] = byte;
            img.pixels[offset + 1] = byte;
            img.pixels[offset + 2] = byte;
            img.pixels[offset + 3] = 255;
        }
        img
    }
}

/// Mean absolute height steps across a field's seams and inside it; see
/// [`PatternField::seam_steps`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeamSteps {
    /// Across the left/right seam (last column to first).
    pub across_x: f32,
    /// Across the top/bottom seam (last row to first).
    pub across_y: f32,
    /// The roughest pair of adjacent interior columns.
    pub interior_x: f32,
    /// The roughest pair of adjacent interior rows.
    pub interior_y: f32,
    /// Mean over all pairs of adjacent interior columns.
    pub interior: f32,
}

impl SeamSteps {
    /// The worse seam as a multiple of the roughest interior pair on its
    /// axis: at most about 1 for a seamless field, well above it for a seam.
    pub fn ratio(&self) -> f32 {
        let x = self.across_x / self.interior_x.max(1e-6);
        let y = self.across_y / self.interior_y.max(1e-6);
        x.max(y)
    }
}

// ---------------------------------------------------------------------------
// PatternCell
// ---------------------------------------------------------------------------

/// Per-pixel data produced by a pattern generator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PatternCell {
    /// Which cell/region this pixel belongs to.
    pub cell_id: u32,
    /// Height value in \[0, 1\] for normal map derivation.
    pub height: f32,
    /// Distance to nearest cell edge in \[0, 1\], where 0 = on the edge.
    pub distance_to_edge: f32,
}

impl Default for PatternCell {
    fn default() -> Self {
        Self {
            cell_id: 0,
            height: 0.0,
            distance_to_edge: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Hash a cell ID into a deterministic RGB color for debug visualization.
fn id_to_color(id: u32) -> (u8, u8, u8) {
    // Simple hash based on mixing bits — visually distinct for low cell counts.
    let mut h = id.wrapping_mul(2654435761); // Knuth multiplicative hash
    let r = (h & 0xFF) as u8;
    h = h.wrapping_mul(2246822519);
    let g = ((h >> 8) & 0xFF) as u8;
    h = h.wrapping_mul(3266489917);
    let b = ((h >> 16) & 0xFF) as u8;
    // Ensure minimum brightness so cells are visible.
    (r.max(40), g.max(40), b.max(40))
}

// ===========================================================================
// Tests
// ===========================================================================

/// Where pattern tests write their debug PNGs: `<target>/test-output/`,
/// honouring `CARGO_TARGET_DIR`.
#[cfg(test)]
pub(crate) fn debug_output_dir() -> std::path::PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let dir = target.join("test-output");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write an [`ImageData`] as a PNG into [`debug_output_dir`].
#[cfg(test)]
pub(crate) fn write_debug_png(name: &str, img: &ImageData) {
    let path = debug_output_dir().join(format!("{name}.png"));
    image::save_buffer(
        &path,
        &img.pixels,
        img.width,
        img.height,
        image::ColorType::Rgba8,
    )
    .unwrap();
    println!("wrote {}", path.display());
}

/// Shared acceptance checks for any pattern's output: dimensions, no NaN,
/// height and edge distance in \[0, 1\].
#[cfg(test)]
pub(crate) fn assert_valid(field: &PatternField, width: u32, height: u32) {
    assert_eq!((field.width, field.height), (width, height));
    assert_eq!(field.cells.len(), (width * height) as usize);
    for c in &field.cells {
        assert!(
            c.height.is_finite() && (0.0..=1.0).contains(&c.height),
            "height {}",
            c.height
        );
        assert!(
            c.distance_to_edge.is_finite() && (0.0..=1.0).contains(&c.distance_to_edge),
            "distance_to_edge {}",
            c.distance_to_edge
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp_field() -> PatternField {
        let mut f = PatternField::new(4, 2);
        for y in 0..2 {
            for x in 0..4 {
                let c = f.get_mut(x, y);
                c.height = x as f32 / 4.0;
                c.cell_id = x;
            }
        }
        f
    }

    #[test]
    fn get_normalized_hits_pixel_centres() {
        let f = ramp_field();
        for x in 0..4u32 {
            let u = (x as f32 + 0.5) / 4.0;
            let c = f.get_normalized(u, 0.25);
            assert!((c.height - x as f32 / 4.0).abs() < 1e-6);
            assert_eq!(c.cell_id, x);
        }
    }

    #[test]
    fn get_normalized_interpolates_and_wraps() {
        let f = ramp_field();
        // Halfway between columns 1 (0.25) and 2 (0.5).
        assert!((f.get_normalized(0.5, 0.25).height - 0.375).abs() < 1e-6);
        // u = 0 sits between the last column (0.75) and the first (0.0).
        assert!((f.get_normalized(0.0, 0.25).height - 0.375).abs() < 1e-6);
        // Whole tiles away is the same place (to float rounding of u).
        let (a, b) = (f.get_normalized(0.3, 0.6), f.get_normalized(2.3, -0.4));
        assert_eq!(a.cell_id, b.cell_id);
        assert!((a.height - b.height).abs() < 1e-5);
    }

    #[test]
    fn seam_steps_see_a_seam() {
        // A ramp has a step of 0.25 between neighbours and 0.75 across the wrap.
        let s = ramp_field().seam_steps();
        assert!((s.interior - 0.25).abs() < 1e-6);
        assert!((s.interior_x - 0.25).abs() < 1e-6);
        assert!((s.across_x - 0.75).abs() < 1e-6);
        assert!((s.ratio() - 3.0).abs() < 1e-5);
    }

    #[test]
    fn pattern_field_new() {
        let field = PatternField::new(16, 8);
        assert_eq!(field.width, 16);
        assert_eq!(field.height, 8);
        assert_eq!(field.cells.len(), 128);
    }

    #[test]
    fn pattern_field_get_set() {
        let mut field = PatternField::new(4, 4);
        field.get_mut(2, 3).cell_id = 42;
        field.get_mut(2, 3).height = 0.75;
        field.get_mut(2, 3).distance_to_edge = 0.5;

        let cell = field.get(2, 3);
        assert_eq!(cell.cell_id, 42);
        assert_eq!(cell.height, 0.75);
        assert_eq!(cell.distance_to_edge, 0.5);
    }

    #[test]
    fn to_height_image_dimensions() {
        let mut field = PatternField::new(8, 8);
        for cell in &mut field.cells {
            cell.height = 0.5;
        }
        let img = field.to_height_image();
        assert_eq!(img.width, 8);
        assert_eq!(img.height, 8);
        assert!(img.validate().is_ok());
        // 0.5 * 255 = 127
        assert_eq!(img.pixels[0], 127);
    }

    #[test]
    fn to_cell_id_image_dimensions() {
        let field = PatternField::new(4, 4);
        let img = field.to_cell_id_image();
        assert_eq!(img.width, 4);
        assert_eq!(img.height, 4);
        assert!(img.validate().is_ok());
    }

    #[test]
    fn id_to_color_deterministic() {
        let (r1, g1, b1) = id_to_color(7);
        let (r2, g2, b2) = id_to_color(7);
        assert_eq!((r1, g1, b1), (r2, g2, b2));
    }

    #[test]
    fn id_to_color_different_ids_differ() {
        let a = id_to_color(0);
        let b = id_to_color(1);
        assert_ne!(a, b);
    }

    #[test]
    fn default_cell() {
        let cell = PatternCell::default();
        assert_eq!(cell.cell_id, 0);
        assert_eq!(cell.height, 0.0);
        assert_eq!(cell.distance_to_edge, 0.0);
    }
}
