//! Asset catalog for managing asset metadata

use crate::types::{AssetFile, AssetMeta, AssetRef, AssetType};
use flint_core::{FlintError, Result};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Catalog of known assets and their metadata
#[derive(Debug, Default)]
pub struct AssetCatalog {
    /// Assets indexed by name
    assets: HashMap<String, AssetMeta>,
    /// Hash to name index for reverse lookup
    hash_index: HashMap<String, String>,
}

impl AssetCatalog {
    /// Create a new empty catalog
    pub fn new() -> Self {
        Self::default()
    }

    /// Load asset metadata from `.asset.toml` sidecar files in a directory tree
    pub fn load_from_directory<P: AsRef<Path>>(path: P) -> Result<Self> {
        let mut catalog = Self::new();
        Self::scan_directory(&mut catalog, path.as_ref())?;
        Ok(catalog)
    }

    fn scan_directory(catalog: &mut AssetCatalog, dir: &Path) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                Self::scan_directory(catalog, &path)?;
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".asset.toml"))
                .unwrap_or(false)
            {
                let content = fs::read_to_string(&path)?;
                let file: AssetFile = toml::from_str(&content).map_err(|e| {
                    FlintError::AssetError(format!(
                        "Failed to parse {}: {}",
                        path.display(),
                        e
                    ))
                })?;
                catalog.register(file.asset);
            }
        }

        Ok(())
    }

    /// Register an asset in the catalog
    pub fn register(&mut self, meta: AssetMeta) {
        self.hash_index
            .insert(meta.hash.clone(), meta.name.clone());
        self.assets.insert(meta.name.clone(), meta);
    }

    /// Get asset metadata by name
    pub fn get(&self, name: &str) -> Option<&AssetMeta> {
        self.assets.get(name)
    }

    /// Get asset metadata by content hash
    pub fn get_by_hash(&self, hash: &str) -> Option<&AssetMeta> {
        self.hash_index
            .get(hash)
            .and_then(|name| self.assets.get(name))
    }

    /// Get all assets of a given type
    pub fn by_type(&self, asset_type: AssetType) -> Vec<&AssetMeta> {
        self.assets
            .values()
            .filter(|a| a.asset_type == asset_type)
            .collect()
    }

    /// Get all assets with a given tag
    pub fn by_tag(&self, tag: &str) -> Vec<&AssetMeta> {
        self.assets
            .values()
            .filter(|a| a.tags.iter().any(|t| t == tag))
            .collect()
    }

    /// Resolve an asset reference
    pub fn resolve_ref(&self, asset_ref: &AssetRef) -> Option<&AssetMeta> {
        match asset_ref {
            AssetRef::ByName(name) => self.get(name),
            AssetRef::ByHash { hash } => self.get_by_hash(hash),
            AssetRef::ByPath { path } => {
                // Search by source_path
                self.assets
                    .values()
                    .find(|a| a.source_path.as_deref() == Some(path))
            }
        }
    }

    /// Get all asset names
    pub fn names(&self) -> Vec<&str> {
        self.assets.keys().map(|s| s.as_str()).collect()
    }

    /// Get the number of registered assets
    pub fn len(&self) -> usize {
        self.assets.len()
    }

    /// Check if the catalog is empty
    pub fn is_empty(&self) -> bool {
        self.assets.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_meta() -> AssetMeta {
        AssetMeta {
            name: "tavern_chair".to_string(),
            asset_type: AssetType::Mesh,
            hash: "sha256:abc123".to_string(),
            source_path: Some("meshes/chair.glb".to_string()),
            format: Some("glb".to_string()),
            properties: HashMap::new(),
            tags: vec!["furniture".to_string(), "medieval".to_string()],
        }
    }

    #[test]
    fn test_register_and_get() {
        let mut catalog = AssetCatalog::new();
        catalog.register(sample_meta());

        assert!(catalog.get("tavern_chair").is_some());
        assert_eq!(catalog.len(), 1);
    }

    #[test]
    fn test_get_by_hash() {
        let mut catalog = AssetCatalog::new();
        catalog.register(sample_meta());

        let meta = catalog.get_by_hash("sha256:abc123").unwrap();
        assert_eq!(meta.name, "tavern_chair");
    }

    #[test]
    fn test_by_type() {
        let mut catalog = AssetCatalog::new();
        catalog.register(sample_meta());
        catalog.register(AssetMeta {
            name: "stone_texture".to_string(),
            asset_type: AssetType::Texture,
            hash: "sha256:def456".to_string(),
            source_path: None,
            format: None,
            properties: HashMap::new(),
            tags: vec![],
        });

        let meshes = catalog.by_type(AssetType::Mesh);
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].name, "tavern_chair");

        let textures = catalog.by_type(AssetType::Texture);
        assert_eq!(textures.len(), 1);
    }

    #[test]
    fn test_by_tag() {
        let mut catalog = AssetCatalog::new();
        catalog.register(sample_meta());

        let furniture = catalog.by_tag("furniture");
        assert_eq!(furniture.len(), 1);

        let empty = catalog.by_tag("nonexistent");
        assert_eq!(empty.len(), 0);
    }

    #[test]
    fn test_resolve_ref_by_name() {
        let mut catalog = AssetCatalog::new();
        catalog.register(sample_meta());

        let result = catalog.resolve_ref(&AssetRef::ByName("tavern_chair".to_string()));
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_ref_by_hash() {
        let mut catalog = AssetCatalog::new();
        catalog.register(sample_meta());

        let result = catalog.resolve_ref(&AssetRef::ByHash {
            hash: "sha256:abc123".to_string(),
        });
        assert!(result.is_some());
    }

    #[test]
    fn test_resolve_ref_by_path() {
        let mut catalog = AssetCatalog::new();
        catalog.register(sample_meta());

        let result = catalog.resolve_ref(&AssetRef::ByPath {
            path: "meshes/chair.glb".to_string(),
        });
        assert!(result.is_some());
    }
}
