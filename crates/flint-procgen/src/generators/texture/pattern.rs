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

#[cfg(test)]
mod tests {
    use super::*;

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
