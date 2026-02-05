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
