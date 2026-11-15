//! Flint Import - Asset importers
//!
//! This crate provides importers for various asset formats,
//! starting with glTF/GLB 3D models.

mod gltf_import;
mod lod_chain;
mod types;

pub use gltf_import::{import_gltf, memoize_imports};
pub use lod_chain::{band_lod_chain, lod_chain, FIRST_SWITCH_RADII, MIN_SWITCH_STEP};
pub use types::{
    AlphaMode, ImportResult, ImportedChannel, ImportedJoint, ImportedKeyframe, ImportedMaterial,
    ImportedMesh, ImportedNode, ImportedNodeChannel, ImportedNodeClip, ImportedSkeletalClip,
    ImportedSkeleton, ImportedTexture, JointProperty, LodBand, MeshBounds,
};
