//! Asset type definitions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Types of assets the engine can manage
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetType {
    Mesh,
    Texture,
    Material,
    Audio,
    Script,
}

/// Metadata for a stored asset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetMeta {
    pub name: String,
    #[serde(rename = "type")]
    pub asset_type: AssetType,
    pub hash: String,
    #[serde(default)]
    pub source_path: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub properties: HashMap<String, toml::Value>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A reference to an asset, supporting multiple resolution methods
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AssetRef {
    /// Reference by name string
    ByName(String),
    /// Reference by content hash
    ByHash { hash: String },
    /// Reference by file path
    ByPath { path: String },
}

/// TOML sidecar file format for asset metadata
#[derive(Debug, Deserialize)]
pub struct AssetFile {
    pub asset: AssetMeta,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asset_meta_serde() {
        let toml_str = r#"
[asset]
name = "tavern_chair"
type = "mesh"
hash = "sha256:a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"
source_path = "meshes/chair.glb"
format = "glb"
tags = ["furniture", "medieval"]

[asset.properties]
vertex_count = 1234
"#;

        let file: AssetFile = toml::from_str(toml_str).unwrap();
        assert_eq!(file.asset.name, "tavern_chair");
        assert_eq!(file.asset.asset_type, AssetType::Mesh);
        assert_eq!(file.asset.tags, vec!["furniture", "medieval"]);
    }

    #[test]
    fn test_asset_ref_by_name() {
        // AssetRef::ByName works via untagged enum when value is a plain string in TOML context
        let val: toml::Value = toml::Value::String("tavern_chair".to_string());
        let asset_ref: AssetRef = val.try_into().unwrap();
        assert!(matches!(asset_ref, AssetRef::ByName(name) if name == "tavern_chair"));
    }

    #[test]
    fn test_asset_ref_by_hash() {
        let val: AssetRef = toml::from_str(r#"hash = "sha256:abc123""#).unwrap();
        assert!(matches!(val, AssetRef::ByHash { hash } if hash == "sha256:abc123"));
    }

    #[test]
    fn test_asset_ref_by_path() {
        let val: AssetRef = toml::from_str(r#"path = "meshes/chair.glb""#).unwrap();
        assert!(matches!(val, AssetRef::ByPath { path } if path == "meshes/chair.glb"));
    }

    #[test]
    fn test_asset_type_variants() {
        let types = vec![
            (r#""mesh""#, AssetType::Mesh),
            (r#""texture""#, AssetType::Texture),
            (r#""material""#, AssetType::Material),
            (r#""audio""#, AssetType::Audio),
            (r#""script""#, AssetType::Script),
        ];

        for (s, expected) in types {
            let parsed: AssetType = toml::from_str(&format!("type = {}", s))
                .map(|v: toml::Value| {
                    serde::Deserialize::deserialize(v.get("type").unwrap().clone()).unwrap()
                })
                .unwrap();
            assert_eq!(parsed, expected);
        }
    }
}
