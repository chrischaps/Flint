//! Import result types

use flint_asset::AssetMeta;

/// Result of importing a file
#[derive(Debug)]
pub struct ImportResult {
    /// Metadata about the imported asset
    pub asset_meta: AssetMeta,
    /// Extracted meshes
    pub meshes: Vec<ImportedMesh>,
    /// Extracted textures
    pub textures: Vec<ImportedTexture>,
    /// Extracted materials
    pub materials: Vec<ImportedMaterial>,
    /// Extracted skeletons (skins)
    pub skeletons: Vec<ImportedSkeleton>,
    /// Extracted skeletal animation clips
    pub skeletal_clips: Vec<ImportedSkeletalClip>,
}

/// An imported mesh with vertex data
#[derive(Debug, Clone)]
pub struct ImportedMesh {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub material_index: Option<usize>,
    /// Per-vertex bone indices (4 joints per vertex)
    pub joint_indices: Option<Vec<[u16; 4]>>,
    /// Per-vertex bone weights (4 weights per vertex)
    pub joint_weights: Option<Vec<[f32; 4]>>,
    /// Index into ImportResult.skeletons
    pub skin_index: Option<usize>,
}

// --- Skeletal animation import types ---

/// Which joint property a channel animates
#[derive(Debug, Clone, PartialEq)]
pub enum JointProperty {
    Translation,
    Rotation,
    Scale,
}

/// A single joint in a skeleton hierarchy
#[derive(Debug, Clone)]
pub struct ImportedJoint {
    pub name: String,
    pub index: usize,
    pub parent: Option<usize>,
    pub inverse_bind_matrix: [[f32; 4]; 4],
}

/// A complete skeleton extracted from a glTF skin
#[derive(Debug, Clone)]
pub struct ImportedSkeleton {
    pub name: String,
    pub joints: Vec<ImportedJoint>,
}

/// A single keyframe in a skeletal animation channel
#[derive(Debug, Clone)]
pub struct ImportedKeyframe {
    pub time: f32,
    /// 3 floats for translation/scale, 4 for rotation (quaternion xyzw)
    pub value: Vec<f32>,
}

/// An animation channel targeting a specific joint property
#[derive(Debug, Clone)]
pub struct ImportedChannel {
    pub joint_index: usize,
    pub property: JointProperty,
    pub interpolation: String,
    pub keyframes: Vec<ImportedKeyframe>,
}

/// A complete skeletal animation clip
#[derive(Debug, Clone)]
pub struct ImportedSkeletalClip {
    pub name: String,
    pub duration: f32,
    pub channels: Vec<ImportedChannel>,
}

/// An imported texture
#[derive(Debug, Clone)]
pub struct ImportedTexture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub data: Vec<u8>,
}

/// An imported PBR material
#[derive(Debug, Clone)]
pub struct ImportedMaterial {
    pub name: String,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub base_color_texture: Option<String>,
    pub normal_texture: Option<String>,
    pub metallic_roughness_texture: Option<String>,
}
