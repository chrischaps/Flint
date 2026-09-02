//! Generator output types.
//!
//! A generator produces a [`GeneratorOutput`] which wraps the concrete data
//! (mesh, image, or raw audio bytes) along with accessors for type-safe
//! extraction.

use serde::{Deserialize, Serialize};

use crate::skinning::SkinnedMeshData;
use crate::types::{ImageData, MeshData};

/// The kind of asset a generator produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OutputKind {
    /// 3D mesh geometry with materials.
    Mesh,
    /// 2D image / texture.
    Image,
    /// Raw audio data.
    Sound,
}

/// The concrete output of a procedural generator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GeneratorOutput {
    /// Generated 3D mesh.
    Mesh(MeshData),
    /// Generated 3D mesh with multiple LOD levels (index 0 = highest detail).
    MeshWithLods(Vec<MeshData>),
    /// Generated image / texture.
    Image(ImageData),
    /// Multiple related images (e.g. albedo + normal + roughness maps).
    ImageSet(Vec<ImageData>),
    /// Generated skinned mesh with skeleton (single LOD).
    SkinnedMesh(SkinnedMeshData),
    /// Generated audio (raw bytes, format TBD).
    Sound(Vec<u8>),
}

impl GeneratorOutput {
    /// Estimated heap memory usage in bytes.
    pub fn estimated_bytes(&self) -> usize {
        match self {
            Self::Mesh(m) => m.estimated_bytes(),
            Self::MeshWithLods(lods) => lods.iter().map(|m| m.estimated_bytes()).sum(),
            Self::Image(i) => i.estimated_bytes(),
            Self::ImageSet(images) => images.iter().map(|i| i.estimated_bytes()).sum(),
            Self::SkinnedMesh(m) => m.estimated_bytes(),
            Self::Sound(bytes) => bytes.len(),
        }
    }

    /// Which kind of output this is.
    pub fn kind(&self) -> OutputKind {
        match self {
            Self::Mesh(_) | Self::MeshWithLods(_) | Self::SkinnedMesh(_) => OutputKind::Mesh,
            Self::Image(_) | Self::ImageSet(_) => OutputKind::Image,
            Self::Sound(_) => OutputKind::Sound,
        }
    }

    /// Borrow the mesh data, if this is a `Mesh` variant.
    pub fn as_mesh(&self) -> Option<&MeshData> {
        match self {
            Self::Mesh(m) => Some(m),
            _ => None,
        }
    }

    /// Borrow the LOD mesh slices, if this is a `MeshWithLods` variant.
    pub fn as_mesh_lods(&self) -> Option<&[MeshData]> {
        match self {
            Self::MeshWithLods(lods) => Some(lods),
            _ => None,
        }
    }

    /// Borrow the image data, if this is an `Image` variant.
    pub fn as_image(&self) -> Option<&ImageData> {
        match self {
            Self::Image(i) => Some(i),
            _ => None,
        }
    }

    /// Consume and return the mesh data, if this is a `Mesh` variant.
    pub fn into_mesh(self) -> Option<MeshData> {
        match self {
            Self::Mesh(m) => Some(m),
            _ => None,
        }
    }

    /// Consume and return the LOD meshes, if this is a `MeshWithLods` variant.
    pub fn into_mesh_lods(self) -> Option<Vec<MeshData>> {
        match self {
            Self::MeshWithLods(lods) => Some(lods),
            _ => None,
        }
    }

    /// Consume and return the image data, if this is an `Image` variant.
    pub fn into_image(self) -> Option<ImageData> {
        match self {
            Self::Image(i) => Some(i),
            _ => None,
        }
    }

    /// Borrow the image set, if this is an `ImageSet` variant.
    pub fn as_image_set(&self) -> Option<&[ImageData]> {
        match self {
            Self::ImageSet(images) => Some(images),
            _ => None,
        }
    }

    /// Consume and return the image set, if this is an `ImageSet` variant.
    pub fn into_image_set(self) -> Option<Vec<ImageData>> {
        match self {
            Self::ImageSet(images) => Some(images),
            _ => None,
        }
    }

    /// Borrow the skinned mesh data, if this is a `SkinnedMesh` variant.
    pub fn as_skinned_mesh(&self) -> Option<&SkinnedMeshData> {
        match self {
            Self::SkinnedMesh(m) => Some(m),
            _ => None,
        }
    }

    /// Consume and return the skinned mesh data, if this is a `SkinnedMesh` variant.
    pub fn into_skinned_mesh(self) -> Option<SkinnedMeshData> {
        match self {
            Self::SkinnedMesh(m) => Some(m),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use flint_core::Vec3;

    fn sample_mesh() -> MeshData {
        MeshData {
            vertices: vec![Vertex {
                position: [0.0, 0.0, 0.0],
                normal: [0.0, 1.0, 0.0],
                tangent: [1.0, 0.0, 0.0, 1.0],
                uv: [0.0, 0.0],
            }],
            indices: vec![],
            materials: vec![],
            submeshes: vec![],
            bounding_box: BoundingBox {
                min: Vec3::ZERO,
                max: Vec3::ZERO,
            },
        }
    }

    #[test]
    fn output_kind_mesh() {
        let out = GeneratorOutput::Mesh(sample_mesh());
        assert_eq!(out.kind(), OutputKind::Mesh);
    }

    #[test]
    fn output_kind_image() {
        let img = ImageData::new(2, 2, ChannelSemantics::Color);
        let out = GeneratorOutput::Image(img);
        assert_eq!(out.kind(), OutputKind::Image);
    }

    #[test]
    fn output_kind_sound() {
        let out = GeneratorOutput::Sound(vec![0u8; 16]);
        assert_eq!(out.kind(), OutputKind::Sound);
    }

    #[test]
    fn as_mesh_returns_some_for_mesh() {
        let out = GeneratorOutput::Mesh(sample_mesh());
        assert!(out.as_mesh().is_some());
        assert!(out.as_image().is_none());
    }

    #[test]
    fn into_image_returns_some_for_image() {
        let img = ImageData::new(4, 4, ChannelSemantics::Normal);
        let out = GeneratorOutput::Image(img);
        assert!(out.into_image().is_some());
    }

    #[test]
    fn output_kind_mesh_with_lods() {
        let out = GeneratorOutput::MeshWithLods(vec![sample_mesh(), sample_mesh()]);
        assert_eq!(out.kind(), OutputKind::Mesh);
    }

    #[test]
    fn as_mesh_lods_returns_some() {
        let out = GeneratorOutput::MeshWithLods(vec![sample_mesh()]);
        assert!(out.as_mesh_lods().is_some());
        assert_eq!(out.as_mesh_lods().unwrap().len(), 1);
        assert!(out.as_mesh().is_none());
    }

    #[test]
    fn estimated_bytes_mesh() {
        let out = GeneratorOutput::Mesh(sample_mesh());
        assert!(out.estimated_bytes() > 0);
    }

    #[test]
    fn estimated_bytes_image() {
        let img = ImageData::new(4, 4, ChannelSemantics::Color);
        let out = GeneratorOutput::Image(img);
        assert_eq!(out.estimated_bytes(), 4 * 4 * 4);
    }

    #[test]
    fn estimated_bytes_sound() {
        let out = GeneratorOutput::Sound(vec![0u8; 100]);
        assert_eq!(out.estimated_bytes(), 100);
    }

    #[test]
    fn estimated_bytes_mesh_with_lods() {
        let out = GeneratorOutput::MeshWithLods(vec![sample_mesh(), sample_mesh()]);
        let single = GeneratorOutput::Mesh(sample_mesh()).estimated_bytes();
        assert_eq!(out.estimated_bytes(), single * 2);
    }

    #[test]
    fn estimated_bytes_image_set() {
        let img = ImageData::new(2, 2, ChannelSemantics::Color);
        let single = img.estimated_bytes();
        let out = GeneratorOutput::ImageSet(vec![
            ImageData::new(2, 2, ChannelSemantics::Color),
            ImageData::new(2, 2, ChannelSemantics::Normal),
        ]);
        assert_eq!(out.estimated_bytes(), single * 2);
    }

    #[test]
    fn into_mesh_lods_returns_some() {
        let out = GeneratorOutput::MeshWithLods(vec![sample_mesh(), sample_mesh()]);
        let lods = out.into_mesh_lods().unwrap();
        assert_eq!(lods.len(), 2);
    }
}
