//! Heightmap files: a grid of heights read from and written to a PNG.
//!
//! The grid and its sampling are flint-geometry's since #1623
//! ([`flint_geometry::heightmap::Heightmap`], re-exported here), the plain
//! data the kernel's ground samples; what needs an image codec or a
//! terrain's config is this crate's, as [`HeightmapFiles`].

use flint_core::vfs;
use std::path::Path;

pub use flint_geometry::heightmap::Heightmap;

use crate::terrain::TerrainConfig;

/// A [`Heightmap`]'s files and its terrain's frame: what the plain grid
/// leaves to the terrain package. Bring it into scope for
/// `Heightmap::from_png`, `save_png` and `sample_world`.
pub trait HeightmapFiles: Sized {
    /// Load a heightmap from a grayscale PNG file.
    /// Values are normalized to [0..1] regardless of bit depth.
    fn from_png(path: &Path) -> Result<Self, String>;

    /// Sample height at world coordinates, returning world-space Y.
    /// Terrain origin is at (0, 0) in XZ; extends to (width, depth).
    fn sample_world(&self, x: f32, z: f32, config: &TerrainConfig) -> f32;

    /// Export the heightmap as a 16-bit grayscale PNG.
    fn save_png(&self, path: &Path) -> Result<(), String>;
}

impl HeightmapFiles for Heightmap {
    fn from_png(path: &Path) -> Result<Self, String> {
        flint_core::scope!("terrain::heightmap", path);
        let fail = |e: &dyn std::fmt::Display| {
            format!("Failed to load heightmap '{}': {}", path.display(), e)
        };
        let bytes = vfs::read(path).map_err(|e| fail(&e))?;
        // Decode from memory, trusting the extension the way `image::open`
        // does and sniffing the bytes only when it names no format.
        let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes));
        match image::ImageFormat::from_path(path) {
            Ok(format) => reader.set_format(format),
            Err(_) => reader = reader.with_guessed_format().map_err(|e| fail(&e))?,
        }
        let img = reader.decode().map_err(|e| fail(&e))?;

        let gray = img.into_luma16();
        let width = gray.width();
        let depth = gray.height();

        let heights: Vec<f32> = gray.pixels().map(|p| p.0[0] as f32 / 65535.0).collect();

        Ok(Heightmap::from_raw(heights, width, depth))
    }

    fn sample_world(&self, x: f32, z: f32, config: &TerrainConfig) -> f32 {
        let u = x / config.width;
        let v = z / config.depth;
        self.sample(u, v) * config.height_scale
    }

    fn save_png(&self, path: &Path) -> Result<(), String> {
        let png = crate::bake::encode_heightmap_png(self.heights(), self.width, self.depth)?;
        std::fs::write(path, png)
            .map_err(|e| format!("Failed to write '{}': {}", path.display(), e))
    }
}
