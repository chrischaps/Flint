//! Heightmap loading and sampling

use std::path::Path;

use crate::terrain::TerrainConfig;

/// A grayscale heightmap with bilinear sampling
pub struct Heightmap {
    /// Row-major height values normalized to [0..1]
    heights: Vec<f32>,
    /// Width in pixels
    pub width: u32,
    /// Depth (height) in pixels
    pub depth: u32,
}

impl Heightmap {
    /// Load a heightmap from a grayscale PNG file.
    /// Values are normalized to [0..1] regardless of bit depth.
    pub fn from_png(path: &Path) -> Result<Self, String> {
        let img = image::open(path)
            .map_err(|e| format!("Failed to load heightmap '{}': {}", path.display(), e))?;

        let gray = img.into_luma16();
        let width = gray.width();
        let depth = gray.height();

        let heights: Vec<f32> = gray
            .pixels()
            .map(|p| p.0[0] as f32 / 65535.0)
            .collect();

        Ok(Self {
            heights,
            width,
            depth,
        })
    }

    /// Create a heightmap from raw float data (for testing)
    pub fn from_raw(heights: Vec<f32>, width: u32, depth: u32) -> Self {
        assert_eq!(heights.len(), (width * depth) as usize);
        Self {
            heights,
            width,
            depth,
        }
    }

    /// Bilinear sample at normalized UV coordinates (0..1, 0..1).
    /// Returns interpolated height in [0..1].
    pub fn sample(&self, u: f32, v: f32) -> f32 {
        let u = u.clamp(0.0, 1.0);
        let v = v.clamp(0.0, 1.0);

        let fx = u * (self.width - 1) as f32;
        let fz = v * (self.depth - 1) as f32;

        let x0 = (fx as u32).min(self.width - 2);
        let z0 = (fz as u32).min(self.depth - 2);
        let x1 = x0 + 1;
        let z1 = z0 + 1;

        let tx = fx - x0 as f32;
        let tz = fz - z0 as f32;

        let h00 = self.get(x0, z0);
        let h10 = self.get(x1, z0);
        let h01 = self.get(x0, z1);
        let h11 = self.get(x1, z1);

        let h0 = h00 * (1.0 - tx) + h10 * tx;
        let h1 = h01 * (1.0 - tx) + h11 * tx;

        h0 * (1.0 - tz) + h1 * tz
    }

    /// Sample height at world coordinates, returning world-space Y.
    /// Terrain origin is at (0, 0) in XZ; extends to (width, depth).
    pub fn sample_world(&self, x: f32, z: f32, config: &TerrainConfig) -> f32 {
        let u = x / config.width;
        let v = z / config.depth;
        self.sample(u, v) * config.height_scale
    }

    /// Compute the surface normal at a UV position using finite differences.
    pub fn compute_normal(
        &self,
        u: f32,
        v: f32,
        world_width: f32,
        world_depth: f32,
        height_scale: f32,
    ) -> [f32; 3] {
        let eps_u = 1.0 / (self.width as f32);
        let eps_v = 1.0 / (self.depth as f32);

        let h_left = self.sample((u - eps_u).max(0.0), v) * height_scale;
        let h_right = self.sample((u + eps_u).min(1.0), v) * height_scale;
        let h_down = self.sample(u, (v - eps_v).max(0.0)) * height_scale;
        let h_up = self.sample(u, (v + eps_v).min(1.0)) * height_scale;

        let dx = (h_right - h_left) / (2.0 * eps_u * world_width);
        let dz = (h_up - h_down) / (2.0 * eps_v * world_depth);

        // Normal = normalize(-dh/dx, 1, -dh/dz)
        let nx = -dx;
        let ny = 1.0;
        let nz = -dz;
        let len = (nx * nx + ny * ny + nz * nz).sqrt();

        [nx / len, ny / len, nz / len]
    }

    /// Clone the raw height data (used by Terrain to take ownership)
    pub fn clone_heights(&self) -> Vec<f32> {
        self.heights.clone()
    }

    fn get(&self, x: u32, z: u32) -> f32 {
        self.heights[(z * self.width + x) as usize]
    }
}
