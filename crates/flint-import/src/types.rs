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
    /// Extracted node-level animation clips (non-skeletal transform animations)
    pub node_clips: Vec<ImportedNodeClip>,
    /// glTF scene graph nodes with transforms
    pub nodes: Vec<ImportedNode>,
    /// Indices of top-level (root) nodes in the scene graph
    pub root_nodes: Vec<usize>,
}

impl ImportResult {
    /// Compute the combined bounding box across all meshes
    pub fn bounds(&self) -> Option<MeshBounds> {
        self.meshes
            .iter()
            .filter_map(|m| m.bounds())
            .reduce(|a, b| a.union(&b))
    }

    /// Returns true when the GLB contains node-level (non-skeletal) animation clips
    pub fn has_node_animations(&self) -> bool {
        !self.node_clips.is_empty()
    }

    /// Returns true when the GLB contains multiple mesh-bearing nodes or
    /// any mesh node with a non-identity transform, meaning child entities
    /// should be created to preserve the glTF spatial layout.
    pub fn needs_expansion(&self) -> bool {
        let mesh_nodes: Vec<&ImportedNode> = self
            .nodes
            .iter()
            .filter(|n| !n.mesh_primitive_indices.is_empty())
            .collect();

        if mesh_nodes.len() > 1 {
            return true;
        }

        // Single mesh node with a non-identity transform
        if let Some(node) = mesh_nodes.first() {
            let t = &node.translation;
            let r = &node.rotation;
            let s = &node.scale;
            let is_identity = (t[0].abs() < 1e-6 && t[1].abs() < 1e-6 && t[2].abs() < 1e-6)
                && (r[0].abs() < 1e-6
                    && r[1].abs() < 1e-6
                    && r[2].abs() < 1e-6
                    && (r[3] - 1.0).abs() < 1e-6)
                && ((s[0] - 1.0).abs() < 1e-6
                    && (s[1] - 1.0).abs() < 1e-6
                    && (s[2] - 1.0).abs() < 1e-6);
            if !is_identity {
                return true;
            }
        }

        false
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

// --- Node-level animation import types ---

/// An animation channel targeting a specific node's transform property
#[derive(Debug, Clone)]
pub struct ImportedNodeChannel {
    pub node_index: usize,       // index into ImportResult.nodes
    pub node_name: String,       // for entity name resolution at runtime
    pub property: JointProperty, // reuse Translation/Rotation/Scale
    pub interpolation: String,
    pub keyframes: Vec<ImportedKeyframe>,
}

/// A complete node-level animation clip (non-skeletal transform animation)
#[derive(Debug, Clone)]
pub struct ImportedNodeClip {
    pub name: String,
    pub duration: f32,
    pub channels: Vec<ImportedNodeChannel>,
}

/// A node from the glTF scene graph, preserving transform hierarchy
#[derive(Debug, Clone)]
pub struct ImportedNode {
    pub name: String,
    pub translation: [f32; 3],
    pub rotation: [f32; 4], // quaternion [x, y, z, w]
    pub scale: [f32; 3],
    pub mesh_primitive_indices: Vec<usize>, // indices into ImportResult.meshes
    pub children: Vec<usize>,              // indices into ImportResult.nodes
    pub skin_index: Option<usize>,
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

/// glTF alpha rendering mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaMode {
    /// Fully opaque (default)
    Opaque,
    /// Binary alpha test (discard below cutoff)
    Mask,
    /// Alpha blending
    Blend,
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
    /// When true, the shader reads per-vertex color instead of the uniform base_color.
    pub use_vertex_color: bool,
    /// Alpha rendering mode from glTF
    pub alpha_mode: AlphaMode,
    /// Alpha cutoff for Mask mode (default 0.5)
    pub alpha_cutoff: f32,
}
