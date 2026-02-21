//! Flint Import - Asset importers
//!
//! This crate provides importers for various asset formats,
//! starting with glTF/GLB 3D models.

mod gltf_import;
mod types;

pub use gltf_import::import_gltf;
pub use types::{
    AlphaMode, ImportResult, ImportedChannel, ImportedJoint, ImportedKeyframe, ImportedMaterial,
    ImportedMesh, ImportedNode, ImportedNodeChannel, ImportedNodeClip, ImportedSkeleton,
    ImportedSkeletalClip, ImportedTexture, JointProperty, MeshBounds,
};
