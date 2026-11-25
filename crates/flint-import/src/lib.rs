//! Flint Import - Asset importers
//!
//! This crate provides importers for various asset formats,
//! starting with glTF/GLB 3D models.

mod gltf_import;
mod lod_chain;
// How far one picture is from another: the refine loops' ruler, and
// `flint render --reference`'s, which every build keeps. A picture ruler,
// not procgen: it moved here from flint-procgen (#1543) so a flint without
// the procgen package still measures. flint-procgen-ai re-exports it as
// `flint_procgen_ai::measure`.
pub mod measure;
mod types;

pub use gltf_import::{import_gltf, import_gltf_geometry, memoize_imports};
pub use lod_chain::{band_lod_chain, lod_chain, FIRST_SWITCH_RADII, MIN_SWITCH_STEP};
pub use types::{
    AlphaMode, ImportResult, ImportedChannel, ImportedJoint, ImportedKeyframe, ImportedMaterial,
    ImportedMesh, ImportedNode, ImportedNodeChannel, ImportedNodeClip, ImportedSkeletalClip,
    ImportedSkeleton, ImportedTexture, JointProperty, LodBand, MeshBounds,
};
