//! Generator output types.
//!
//! A generator produces a [`GeneratorOutput`] which wraps the concrete data
//! (mesh, image, or raw audio bytes) along with accessors for type-safe
//! extraction.

use serde::{Deserialize, Serialize};

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
    /// Generated audio (raw bytes, format TBD).
    Sound(Vec<u8>),
}

impl GeneratorOutput {
    /// Which kind of output this is.
    pub fn kind(&self) -> OutputKind {
        match self {
            Self::Mesh(_) | Self::MeshWithLods(_) => OutputKind::Mesh,
            Self::Image(_) => OutputKind::Image,
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
    fn into_mesh_lods_returns_some() {
        let out = GeneratorOutput::MeshWithLods(vec![sample_mesh(), sample_mesh()]);
        let lods = out.into_mesh_lods().unwrap();
        assert_eq!(lods.len(), 2);
    }
}
