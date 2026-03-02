//! Core data types for procedural generation output.
//!
//! These types represent generated geometry, materials, and images in a
//! renderer-agnostic format. Conversion to GPU-specific vertex layouts
//! happens at the integration boundary.

use std::path::Path;

use flint_core::Vec3;
use serde::{Deserialize, Serialize};

/// A mesh vertex in renderer-agnostic format.
///
/// Uses raw `[f32; N]` arrays to avoid pulling in `wgpu`/`bytemuck`.
/// The tangent `w` component encodes handedness per the glTF convention.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vertex {
    /// Position in local space.
    pub position: [f32; 3],
    /// Surface normal (unit length).
    pub normal: [f32; 3],
    /// Tangent vector with handedness in `w` (±1.0).
    pub tangent: [f32; 4],
    /// Texture coordinates.
    pub uv: [f32; 2],
}

/// PBR material parameters for a generated mesh.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MaterialData {
    /// Human-readable material name.
    pub name: String,
    /// Base color as linear RGBA.
    pub base_color: [f32; 4],
    /// Metallic factor (0.0 = dielectric, 1.0 = metal).
    pub metallic: f32,
    /// Roughness factor (0.0 = mirror, 1.0 = fully rough).
    pub roughness: f32,
}

impl Default for MaterialData {
    fn default() -> Self {
        Self {
            name: "default".into(),
            base_color: [0.5, 0.5, 0.5, 1.0],
            metallic: 0.0,
            roughness: 0.5,
        }
    }
}

/// Axis-aligned bounding box.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoundingBox {
    /// Minimum corner.
    pub min: Vec3,
    /// Maximum corner.
    pub max: Vec3,
}

impl BoundingBox {
    /// Compute the tightest bounding box around a set of positions.
    ///
    /// Returns a zero-volume box at the origin if the slice is empty.
    pub fn from_positions(positions: &[[f32; 3]]) -> Self {
        if positions.is_empty() {
            return Self {
                min: Vec3::ZERO,
                max: Vec3::ZERO,
            };
        }
        let mut min = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut max = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for p in positions {
            min.x = min.x.min(p[0]);
            min.y = min.y.min(p[1]);
            min.z = min.z.min(p[2]);
            max.x = max.x.max(p[0]);
            max.y = max.y.max(p[1]);
            max.z = max.z.max(p[2]);
        }
        Self { min, max }
    }

    /// Dimensions along each axis.
    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }

    /// Center point.
    pub fn center(&self) -> Vec3 {
        Vec3::new(
            (self.min.x + self.max.x) * 0.5,
            (self.min.y + self.max.y) * 0.5,
            (self.min.z + self.max.z) * 0.5,
        )
    }

    /// Smallest box enclosing both `self` and `other`.
    pub fn union(&self, other: &Self) -> Self {
        Self {
            min: Vec3::new(
                self.min.x.min(other.min.x),
                self.min.y.min(other.min.y),
                self.min.z.min(other.min.z),
            ),
            max: Vec3::new(
                self.max.x.max(other.max.x),
                self.max.y.max(other.max.y),
                self.max.z.max(other.max.z),
            ),
        }
    }

    /// Whether a point lies inside (inclusive).
    pub fn contains(&self, point: Vec3) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
            && point.z >= self.min.z
            && point.z <= self.max.z
    }
}

/// A complete generated mesh with geometry, materials, and bounds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshData {
    /// Vertex buffer.
    pub vertices: Vec<Vertex>,
    /// Index buffer (triangles).
    pub indices: Vec<u32>,
    /// Material slots referenced by the mesh.
    pub materials: Vec<MaterialData>,
    /// Axis-aligned bounding box.
    pub bounding_box: BoundingBox,
}

impl MeshData {
    /// Number of triangles (index count / 3).
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Number of vertices.
    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    /// Recompute the bounding box from current vertex positions.
    pub fn recompute_bounds(&mut self) {
        let positions: Vec<[f32; 3]> = self.vertices.iter().map(|v| v.position).collect();
        self.bounding_box = BoundingBox::from_positions(&positions);
    }

    /// Validate mesh integrity.
    ///
    /// Checks that indices are in range and the index count is a multiple of 3.
    pub fn validate(&self) -> crate::Result<()> {
        if !self.indices.len().is_multiple_of(3) {
            return Err(crate::ProcGenError::InvalidMesh(format!(
                "index count {} is not a multiple of 3",
                self.indices.len()
            )));
        }
        let vert_count = self.vertices.len() as u32;
        for (i, &idx) in self.indices.iter().enumerate() {
            if idx >= vert_count {
                return Err(crate::ProcGenError::InvalidMesh(format!(
                    "index [{}] = {} is out of range (vertex count: {})",
                    i, idx, vert_count
                )));
            }
        }
        Ok(())
    }
}

/// A single branch segment produced by branching algorithms (L-system,
/// space colonization). These are the shared output type that the tree
/// generator converts to meshes via `extrude_along_curve`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BranchSegment {
    /// World-space start position.
    pub start: Vec3,
    /// World-space end position.
    pub end: Vec3,
    /// Radius at the start of the segment.
    pub radius_start: f32,
    /// Radius at the end of the segment.
    pub radius_end: f32,
    /// Branching depth (0 = trunk, incremented at each Push).
    pub depth: u32,
}

impl BranchSegment {
    /// Euclidean length of the segment.
    pub fn length(&self) -> f32 {
        let d = self.end - self.start;
        (d.x * d.x + d.y * d.y + d.z * d.z).sqrt()
    }

    /// Unit direction from start to end, or `Vec3::ZERO` for degenerate segments.
    pub fn direction(&self) -> Vec3 {
        let d = self.end - self.start;
        let len = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
        if len < 1e-10 {
            Vec3::ZERO
        } else {
            Vec3::new(d.x / len, d.y / len, d.z / len)
        }
    }

    /// Average of start and end radii.
    pub fn mean_radius(&self) -> f32 {
        (self.radius_start + self.radius_end) * 0.5
    }
}

/// Semantic meaning of an image channel, used for correct interpretation
/// during material assembly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ChannelSemantics {
    /// Albedo / base color.
    Color,
    /// Tangent-space normal map.
    Normal,
    /// Surface roughness.
    Roughness,
    /// Metallic factor.
    Metallic,
    /// Heightmap / displacement.
    Height,
    /// General-purpose mask.
    Mask,
}

/// A generated image with pixel data and semantic metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageData {
    /// Raw pixel data in RGBA8 format (4 bytes per pixel).
    pub pixels: Vec<u8>,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// What this image represents.
    pub channel_semantics: ChannelSemantics,
}

impl ImageData {
    /// Create a new image, pre-allocating the pixel buffer.
    ///
    /// All pixels are initialized to zero (transparent black).
    pub fn new(width: u32, height: u32, channel_semantics: ChannelSemantics) -> Self {
        Self {
            pixels: vec![0u8; (width * height * 4) as usize],
            width,
            height,
            channel_semantics,
        }
    }

    /// Total number of pixels.
    pub fn pixel_count(&self) -> u32 {
        self.width * self.height
    }

    /// Validate that the pixel buffer size matches dimensions.
    pub fn validate(&self) -> crate::Result<()> {
        let expected = (self.width * self.height * 4) as usize;
        if self.pixels.len() != expected {
            return Err(crate::ProcGenError::ImageError(format!(
                "pixel buffer length {} doesn't match {}x{}x4 = {}",
                self.pixels.len(),
                self.width,
                self.height,
                expected
            )));
        }
        Ok(())
    }

    /// Encode the image as PNG bytes in memory.
    pub fn to_png_bytes(&self) -> crate::Result<Vec<u8>> {
        use image::codecs::png::PngEncoder;
        use image::ImageEncoder;

        let mut buf = Vec::new();
        let encoder = PngEncoder::new(&mut buf);
        encoder
            .write_image(
                &self.pixels,
                self.width,
                self.height,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| crate::ProcGenError::ImageError(format!("PNG encoding failed: {e}")))?;
        Ok(buf)
    }

    /// Save the image as a PNG file.
    pub fn save_png(&self, path: &Path) -> crate::Result<()> {
        let bytes = self.to_png_bytes()?;
        std::fs::write(path, bytes)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_copy() {
        let v = Vertex {
            position: [1.0, 2.0, 3.0],
            normal: [0.0, 1.0, 0.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
            uv: [0.5, 0.5],
        };
        let v2 = v;
        assert_eq!(v.position, v2.position);
    }

    #[test]
    fn material_default_is_gray_dielectric() {
        let m = MaterialData::default();
        assert_eq!(m.base_color, [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(m.metallic, 0.0);
        assert_eq!(m.roughness, 0.5);
    }

    #[test]
    fn bounding_box_from_positions() {
        let positions = vec![[0.0, 0.0, 0.0], [1.0, 2.0, 3.0], [-1.0, -2.0, -3.0]];
        let bb = BoundingBox::from_positions(&positions);
        assert_eq!(bb.min, Vec3::new(-1.0, -2.0, -3.0));
        assert_eq!(bb.max, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn bounding_box_empty_positions() {
        let bb = BoundingBox::from_positions(&[]);
        assert_eq!(bb.min, Vec3::ZERO);
        assert_eq!(bb.max, Vec3::ZERO);
    }

    #[test]
    fn bounding_box_size_and_center() {
        let bb = BoundingBox {
            min: Vec3::new(-1.0, 0.0, -2.0),
            max: Vec3::new(1.0, 4.0, 2.0),
        };
        assert_eq!(bb.size(), Vec3::new(2.0, 4.0, 4.0));
        assert_eq!(bb.center(), Vec3::new(0.0, 2.0, 0.0));
    }

    #[test]
    fn bounding_box_union() {
        let a = BoundingBox {
            min: Vec3::new(0.0, 0.0, 0.0),
            max: Vec3::new(1.0, 1.0, 1.0),
        };
        let b = BoundingBox {
            min: Vec3::new(-1.0, -1.0, -1.0),
            max: Vec3::new(0.5, 0.5, 0.5),
        };
        let u = a.union(&b);
        assert_eq!(u.min, Vec3::new(-1.0, -1.0, -1.0));
        assert_eq!(u.max, Vec3::new(1.0, 1.0, 1.0));
    }

    #[test]
    fn bounding_box_contains() {
        let bb = BoundingBox {
            min: Vec3::new(0.0, 0.0, 0.0),
            max: Vec3::new(1.0, 1.0, 1.0),
        };
        assert!(bb.contains(Vec3::new(0.5, 0.5, 0.5)));
        assert!(bb.contains(Vec3::new(0.0, 0.0, 0.0))); // inclusive
        assert!(!bb.contains(Vec3::new(1.5, 0.5, 0.5)));
    }

    fn make_triangle() -> MeshData {
        MeshData {
            vertices: vec![
                Vertex {
                    position: [0.0, 0.0, 0.0],
                    normal: [0.0, 0.0, 1.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [0.0, 0.0],
                },
                Vertex {
                    position: [1.0, 0.0, 0.0],
                    normal: [0.0, 0.0, 1.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [1.0, 0.0],
                },
                Vertex {
                    position: [0.0, 1.0, 0.0],
                    normal: [0.0, 0.0, 1.0],
                    tangent: [1.0, 0.0, 0.0, 1.0],
                    uv: [0.0, 1.0],
                },
            ],
            indices: vec![0, 1, 2],
            materials: vec![MaterialData::default()],
            bounding_box: BoundingBox::from_positions(&[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
            ]),
        }
    }

    #[test]
    fn mesh_counts() {
        let mesh = make_triangle();
        assert_eq!(mesh.vertex_count(), 3);
        assert_eq!(mesh.triangle_count(), 1);
    }

    #[test]
    fn mesh_validate_ok() {
        let mesh = make_triangle();
        assert!(mesh.validate().is_ok());
    }

    #[test]
    fn mesh_validate_bad_index_count() {
        let mut mesh = make_triangle();
        mesh.indices.push(0); // now 4 indices, not multiple of 3
        assert!(mesh.validate().is_err());
    }

    #[test]
    fn mesh_validate_out_of_range_index() {
        let mut mesh = make_triangle();
        mesh.indices[1] = 99;
        assert!(mesh.validate().is_err());
    }

    #[test]
    fn mesh_recompute_bounds() {
        let mut mesh = make_triangle();
        mesh.vertices[2].position = [0.0, 5.0, 0.0];
        mesh.recompute_bounds();
        assert_eq!(mesh.bounding_box.max, Vec3::new(1.0, 5.0, 0.0));
    }

    #[test]
    fn image_new_and_validate() {
        let img = ImageData::new(16, 16, ChannelSemantics::Color);
        assert_eq!(img.pixel_count(), 256);
        assert!(img.validate().is_ok());
    }

    #[test]
    fn image_validate_bad_size() {
        let mut img = ImageData::new(4, 4, ChannelSemantics::Normal);
        img.pixels.pop(); // corrupt length
        assert!(img.validate().is_err());
    }
}
