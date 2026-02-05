//! Asset reference resolution

use crate::catalog::AssetCatalog;
use crate::types::{AssetMeta, AssetRef, AssetType};
use serde::{Deserialize, Serialize};

/// Strategy for handling unresolved asset references
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResolutionStrategy {
    /// Fail on missing assets
    Strict,
    /// Use a placeholder for missing assets
    Placeholder,
    /// Create a human task for missing assets (deferred to Phase 5)
    HumanTask,
    /// Use AI to generate missing assets (deferred to Phase 5)
    AiGenerate,
}

/// Result of attempting to resolve an asset reference
#[derive(Debug, Clone)]
pub enum ResolveResult {
    /// Asset was found
    Found(AssetMeta),
    /// Asset not found, placeholder will be used
    Placeholder { name: String, asset_type: AssetType },
    /// Asset not found, human task created
    HumanTask {
        name: String,
        asset_type: AssetType,
        description: String,
    },
    /// Asset not found with no fallback
    Missing { name: String },
}

impl ResolveResult {
    /// Check if the asset was found
    pub fn is_found(&self) -> bool {
        matches!(self, ResolveResult::Found(_))
    }
}

/// Resolver that applies a resolution strategy to asset references
pub struct AssetResolver {
    strategy: ResolutionStrategy,
}

impl AssetResolver {
    /// Create a new resolver with the given strategy
    pub fn new(strategy: ResolutionStrategy) -> Self {
        Self { strategy }
    }

    /// Resolve an asset reference against a catalog
    pub fn resolve(&self, asset_ref: &AssetRef, catalog: &AssetCatalog) -> ResolveResult {
        if let Some(meta) = catalog.resolve_ref(asset_ref) {
            return ResolveResult::Found(meta.clone());
        }

        let name = match asset_ref {
            AssetRef::ByName(n) => n.clone(),
            AssetRef::ByHash { hash } => hash.clone(),
            AssetRef::ByPath { path } => path.clone(),
        };

        match self.strategy {
            ResolutionStrategy::Strict => ResolveResult::Missing { name },
            ResolutionStrategy::Placeholder => ResolveResult::Placeholder {
                name,
                asset_type: AssetType::Mesh, // Default assumption
            },
            ResolutionStrategy::HumanTask => ResolveResult::HumanTask {
                description: format!("Create asset: {}", &name),
                name,
                asset_type: AssetType::Mesh,
            },
            ResolutionStrategy::AiGenerate => {
                // Deferred to Phase 5 — fall back to placeholder
                ResolveResult::Placeholder {
                    name,
                    asset_type: AssetType::Mesh,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn catalog_with_chair() -> AssetCatalog {
        let mut catalog = AssetCatalog::new();
        catalog.register(AssetMeta {
            name: "chair".to_string(),
            asset_type: AssetType::Mesh,
            hash: "sha256:abc".to_string(),
            source_path: None,
            format: None,
            properties: HashMap::new(),
            tags: vec![],
        });
        catalog
    }

    #[test]
    fn test_resolve_found() {
        let catalog = catalog_with_chair();
        let resolver = AssetResolver::new(ResolutionStrategy::Strict);
        let result = resolver.resolve(&AssetRef::ByName("chair".to_string()), &catalog);
        assert!(result.is_found());
    }

    #[test]
    fn test_resolve_strict_missing() {
        let catalog = catalog_with_chair();
        let resolver = AssetResolver::new(ResolutionStrategy::Strict);
        let result = resolver.resolve(&AssetRef::ByName("table".to_string()), &catalog);
        assert!(matches!(result, ResolveResult::Missing { .. }));
    }

    #[test]
    fn test_resolve_placeholder_missing() {
        let catalog = catalog_with_chair();
        let resolver = AssetResolver::new(ResolutionStrategy::Placeholder);
        let result = resolver.resolve(&AssetRef::ByName("table".to_string()), &catalog);
        assert!(matches!(result, ResolveResult::Placeholder { .. }));
    }
}
