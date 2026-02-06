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

impl ImportResult {
    /// Compute the combined bounding box across all meshes
    pub fn bounds(&self) -> Option<MeshBounds> {
        self.meshes
            .iter()
            .filter_map(|m| m.bounds())
            .reduce(|a, b| a.union(&b))
    }
}

/// Axis-aligned bounding box computed from vertex positions
#[derive(Debug, Clone, Copy)]
pub struct MeshBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl MeshBounds {
    /// Compute bounds from a set of vertex positions
    pub fn from_positions(positions: &[[f32; 3]]) -> Option<Self> {
        if positions.is_empty() {
            return None;
        }
        let mut min = positions[0];
        let mut max = positions[0];
        for p in positions.iter().skip(1) {
            for i in 0..3 {
                if p[i] < min[i] { min[i] = p[i]; }
                if p[i] > max[i] { max[i] = p[i]; }
            }
        }
        Some(Self { min, max })
    }

    /// Size along each axis
    pub fn size(&self) -> [f32; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }

    /// Merge with another bounds to get the union
    pub fn union(&self, other: &MeshBounds) -> MeshBounds {
        MeshBounds {
            min: [
                self.min[0].min(other.min[0]),
                self.min[1].min(other.min[1]),
                self.min[2].min(other.min[2]),
            ],
            max: [
                self.max[0].max(other.max[0]),
                self.max[1].max(other.max[1]),
                self.max[2].max(other.max[2]),
            ],
        }
    }
}

impl std::fmt::Display for MeshBounds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self.size();
        write!(
            f,
            "{:.2} x {:.2} x {:.2} (min [{:.2}, {:.2}, {:.2}], max [{:.2}, {:.2}, {:.2}])",
            s[0], s[1], s[2],
            self.min[0], self.min[1], self.min[2],
            self.max[0], self.max[1], self.max[2],
        )
    }
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

impl ImportedMesh {
    /// Compute the axis-aligned bounding box of this mesh's vertices
    pub fn bounds(&self) -> Option<MeshBounds> {
        MeshBounds::from_positions(&self.positions)
    }
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
