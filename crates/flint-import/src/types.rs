//! Import result types

use flint_asset::AssetMeta;

/// Result of importing a file
#[derive(Debug, Clone)]
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
                if p[i] < min[i] {
                    min[i] = p[i];
                }
                if p[i] > max[i] {
                    max[i] = p[i];
                }
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
            s[0],
            s[1],
            s[2],
            self.min[0],
            self.min[1],
            self.min[2],
            self.max[0],
            self.max[1],
            self.max[2],
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
    /// The distances this mesh draws at, from its glTF mesh's
    /// `extras.flint_lod` (see [`LodBand`]). `None` draws at every distance.
    pub lod: Option<LodBand>,
}

/// One level of detail of a model: the band of camera distances a mesh draws
/// in, authored per glTF primitive (or per mesh, for all its primitives) as
/// `extras = { flint_lod = { near, far, draw_distance, ref_min, ref_max } }`
/// (every key optional). Keep a model's levels as primitives of one mesh on
/// one node: several mesh nodes are expanded into a child entity each.
/// (Root nodes named `LOD_0`, `LOD_1`, ... without bands get them from
/// [`band_lod_chain`](crate::band_lod_chain), #193.)
///
/// The distance is measured from the camera to the closest point of the
/// reference box `ref_min..ref_max` (model space; the mesh's own bounds when
/// absent). Every level of one group names the same box, so exactly one of
/// them draws at any distance: a merged chunk of props and its simplified
/// copy switch together instead of overlapping. `near..far` is a switch
/// between levels and scales with the renderer's LOD scale; `draw_distance`
/// is where the mesh stops drawing at all and scales with the draw-distance
/// scale. A missing `far` or `draw_distance` is unbounded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LodBand {
    pub near: f32,
    pub far: f32,
    pub draw_distance: f32,
    pub ref_box: Option<([f32; 3], [f32; 3])>,
}

impl Default for LodBand {
    fn default() -> Self {
        Self {
            near: 0.0,
            far: f32::INFINITY,
            draw_distance: f32::INFINITY,
            ref_box: None,
        }
    }
}

impl LodBand {
    /// The band in a glTF `extras` object, if it has a `flint_lod` table.
    /// Malformed numbers fall back to the defaults rather than failing the
    /// import: a model with a bad band still draws.
    pub fn from_extras(extras: &serde_json::Value) -> Option<Self> {
        let lod = extras.get("flint_lod")?.as_object()?;
        let num = |k: &str| lod.get(k).and_then(|v| v.as_f64()).map(|v| v as f32);
        let vec3 = |k: &str| -> Option<[f32; 3]> {
            let a = lod.get(k)?.as_array()?;
            if a.len() != 3 {
                return None;
            }
            Some([
                a[0].as_f64()? as f32,
                a[1].as_f64()? as f32,
                a[2].as_f64()? as f32,
            ])
        };
        let positive = |v: Option<f32>| v.filter(|d| *d > 0.0).unwrap_or(f32::INFINITY);
        Some(Self {
            near: num("near").unwrap_or(0.0).max(0.0),
            far: positive(num("far")),
            draw_distance: positive(num("draw_distance")),
            ref_box: vec3("ref_min").zip(vec3("ref_max")),
        })
    }

    /// Whether a camera `distance` metres from the reference box draws this
    /// band, with switch distances times `lod_scale` and the draw distance
    /// times `draw_scale`. `near` is inclusive and `far` exclusive, so two
    /// levels that meet at one distance never both draw.
    pub fn visible_at(&self, distance: f32, lod_scale: f32, draw_scale: f32) -> bool {
        distance >= self.near * lod_scale
            && distance < self.far * lod_scale
            && distance < self.draw_distance * draw_scale
    }
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
    /// Rest (bind) LOCAL transform from the glTF joint node's TRS. Runtime
    /// pose buffers must start from this, not identity: a clip that only
    /// animates some joints/properties leaves the rest untouched, and an
    /// identity local translation collapses a joint onto its parent.
    pub rest_translation: [f32; 3],
    /// Quaternion xyzw
    pub rest_rotation: [f32; 4],
    pub rest_scale: [f32; 3],
    /// The transform of the non-joint glTF nodes between this joint and
    /// `parent` (column-major), or between it and the scene root when
    /// `parent` is `None`. `None` when there are none, or they're identity.
    ///
    /// A glTF joint's world matrix includes every ancestor, and the inverse
    /// bind matrices are written against it. A Mixamo export puts its root
    /// joint under an `Armature` node that holds the centimetre-to-metre
    /// scale and the Z-up turn, so a pose that skips that node comes out
    /// 100 times too large and lying down (#294).
    pub parent_offset: Option<[[f32; 4]; 4]>,
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
    /// Incoming tangent (CUBICSPLINE only; empty otherwise). Same arity as `value`.
    pub in_tangent: Vec<f32>,
    /// Outgoing tangent (CUBICSPLINE only; empty otherwise). Same arity as `value`.
    pub out_tangent: Vec<f32>,
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
    pub children: Vec<usize>,               // indices into ImportResult.nodes
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
    /// Light the surface gives off, as linear RGB (glTF `emissiveFactor`).
    /// Added after lighting and before tonemapping, so it reads the same
    /// from every side and under any light, and bloom picks it up. Zero
    /// (the default) gives off nothing.
    pub emissive: [f32; 3],
    /// Multiplies `emissive`, and the per-vertex emission of
    /// `vertex_emission`, so radiance can go past 1 (glTF
    /// `KHR_materials_emissive_strength`). Default 1.
    pub emissive_strength: f32,
    /// The emissive map (glTF `emissiveTexture`), sRGB. Its rgb multiplies
    /// `emissive` × `emissive_strength`, so painted-on lights (window grids,
    /// signage) glow only where painted. A texture with `emissive` left at
    /// zero gives off nothing, as in glTF. Per-vertex emission ignores it.
    pub emissive_texture: Option<String>,
    /// Per-vertex emission, for vertex-coloured meshes that mix lit and
    /// glowing faces. The vertex alpha says how much of the face is
    /// surface: 1 (every ordinary vertex colour) is lit as usual, 0 reflects
    /// nothing and gives off its RGB times `emissive_strength` as light, and
    /// values between blend the two. RGB may go past 1 for bright lights.
    /// Vertex alpha is then no longer opacity; the entity material's
    /// `opacity` still applies. Needs `use_vertex_color`.
    pub vertex_emission: bool,
    /// Whether both faces of the surface are drawn (glTF `doubleSided`).
    /// False culls back faces, as glTF's default does; true draws thin cards
    /// (leaves, fronds, flat figures) from either side, lights a back face
    /// with its normal turned toward the viewer, and casts shadows from both.
    pub double_sided: bool,
}

impl Default for ImportedMaterial {
    /// Opaque white, non-metallic, half rough, no textures and no emission.
    fn default() -> Self {
        Self {
            name: String::new(),
            base_color: [1.0; 4],
            metallic: 0.0,
            roughness: 0.5,
            base_color_texture: None,
            normal_texture: None,
            metallic_roughness_texture: None,
            use_vertex_color: false,
            alpha_mode: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            emissive: [0.0; 3],
            emissive_strength: 1.0,
            emissive_texture: None,
            vertex_emission: false,
            double_sided: false,
        }
    }
}

impl ImportedMaterial {
    /// An opaque, untextured, non-metallic material that takes its colour
    /// from the vertices, for procedural meshes coloured per vertex.
    /// Override the rest with struct update syntax:
    /// `ImportedMaterial { roughness: 0.9, ..ImportedMaterial::vertex_colored("ground") }`.
    pub fn vertex_colored(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            use_vertex_color: true,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod lod_tests {
    use super::LodBand;

    #[test]
    fn reads_a_band_from_extras() {
        let extras = serde_json::json!({
            "flint_lod": { "near": 80, "far": 200.5, "draw_distance": 400,
                           "ref_min": [-1, 0, -1], "ref_max": [1, 2, 1] }
        });
        let band = LodBand::from_extras(&extras).unwrap();
        assert_eq!(band.near, 80.0);
        assert_eq!(band.far, 200.5);
        assert_eq!(band.draw_distance, 400.0);
        assert_eq!(band.ref_box, Some(([-1.0, 0.0, -1.0], [1.0, 2.0, 1.0])));
    }

    #[test]
    fn missing_keys_are_unbounded_and_other_extras_are_ignored() {
        let band = LodBand::from_extras(&serde_json::json!({ "flint_lod": {} })).unwrap();
        assert_eq!(band, LodBand::default());
        assert!(LodBand::from_extras(&serde_json::json!({ "units": "m" })).is_none());
        // a zero far means "no far", not "never draws"
        let band = LodBand::from_extras(&serde_json::json!({ "flint_lod": { "far": 0 } })).unwrap();
        assert!(band.far.is_infinite());
    }

    #[test]
    fn adjacent_levels_hand_over_with_no_gap_and_no_overlap() {
        let lod0 = LodBand {
            far: 90.0,
            ..LodBand::default()
        };
        let lod1 = LodBand {
            near: 90.0,
            far: 220.0,
            ..LodBand::default()
        };
        let lod2 = LodBand {
            near: 220.0,
            ..LodBand::default()
        };
        for scale in [0.5_f32, 1.0, 1.7] {
            for d in (0..600).map(|i| i as f32 * 0.5) {
                let n = [lod0, lod1, lod2]
                    .iter()
                    .filter(|b| b.visible_at(d, scale, 1.0))
                    .count();
                assert_eq!(n, 1, "distance {d} at scale {scale}");
            }
        }
    }

    #[test]
    fn draw_distance_scales_on_its_own() {
        let band = LodBand {
            draw_distance: 150.0,
            ..LodBand::default()
        };
        assert!(band.visible_at(140.0, 0.5, 1.0));
        assert!(!band.visible_at(160.0, 1.0, 1.0));
        assert!(band.visible_at(160.0, 1.0, 1.2));
        assert!(!band.visible_at(100.0, 1.0, 0.6));
    }
}
