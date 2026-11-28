//! A grid of heights and its bilinear sampling: the plain half of the
//! terrain package's heightmap (#1623, ADR 0138 decision 5).
//!
//! It was flint-terrain's, where reading and writing its PNG still are
//! (`flint_terrain::HeightmapFiles`). Here it is what the kernel's ground
//! ([`GroundField`](crate::ground::GroundField)) samples, so flint-script and
//! a game's own ground need no terrain crate.

/// A grayscale heightmap with bilinear sampling
#[derive(Debug, Clone, PartialEq)]
pub struct Heightmap {
    /// Row-major height values normalized to [0..1]
    heights: Vec<f32>,
    /// Width in pixels
    pub width: u32,
    /// Depth (height) in pixels
    pub depth: u32,
}

impl Heightmap {
    /// A heightmap of `width` x `depth` samples, row by row.
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

    /// Get a reference to the raw height data.
    pub fn heights(&self) -> &[f32] {
        &self.heights
    }

    /// Get a mutable reference to the raw height data.
    pub fn heights_mut(&mut self) -> &mut [f32] {
        &mut self.heights
    }

    /// Replace the height data entirely.
    pub fn set_heights(&mut self, heights: Vec<f32>) {
        assert_eq!(heights.len(), (self.width * self.depth) as usize);
        self.heights = heights;
    }

    fn get(&self, x: u32, z: u32) -> f32 {
        self.heights[(z * self.width + x) as usize]
    }
}

/// A drawn ground's heights on its vertex grid, in the ground's own frame:
/// column 0 at x = 0, the last at x = `width`; row 0 at z = 0, the last at
/// z = `depth`. A terrain's is `flint_terrain::Terrain::height_grid`, the
/// grid physics' heightfield follows (#1219).
#[derive(Debug, Clone, PartialEq)]
pub struct HeightGrid {
    /// Samples along X.
    pub columns: usize,
    /// Samples along Z.
    pub rows: usize,
    /// World heights, row by row: `heights[row * columns + column]`.
    pub heights: Vec<f32>,
    /// The X extent the columns span.
    pub width: f32,
    /// The Z extent the rows span.
    pub depth: f32,
}
