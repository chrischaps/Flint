//! Semantic asset definitions
//!
//! Intent-based asset descriptions that map to generation requests.
//! Instead of specifying exact parameters, describe the asset's
//! purpose and let the system choose the best provider and params.

use crate::provider::{AssetKind, AudioParams, GenerateRequest, ModelParams, TextureParams};
use serde::{Deserialize, Serialize};

/// A semantic asset definition — describes intent, not implementation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticAssetDef {
    /// Asset name
    pub name: String,
    /// What this asset is for (e.g., "tavern wall texture", "wooden chair")
    pub description: String,
    /// Asset type
    #[serde(rename = "type")]
    pub asset_type: String,
    /// Material intent (e.g., "aged wood", "rough stone")
    #[serde(default)]
    pub material_intent: Option<String>,
    /// How worn/damaged (0.0 = pristine, 1.0 = heavily worn)
    #[serde(default)]
    pub wear_level: Option<f64>,
    /// Size class (small, medium, large, huge)
    #[serde(default)]
    pub size_class: Option<String>,
    /// Tags for categorization
    #[serde(default)]
    pub tags: Vec<String>,
}

/// TOML wrapper for entity-level asset_def component
#[derive(Debug, Deserialize)]
pub struct AssetDefComponent {
    #[serde(flatten)]
    pub def: SemanticAssetDef,
}

impl SemanticAssetDef {
    /// Convert semantic definition to a concrete generation request
    pub fn to_request(&self) -> GenerateRequest {
        let kind = match self.asset_type.as_str() {
            "texture" => AssetKind::Texture,
            "model" | "mesh" => AssetKind::Model,
            "audio" | "sound" => AssetKind::Audio,
            _ => AssetKind::Texture,
        };

        // Build enriched description from semantic fields
        let mut desc_parts = vec![self.description.clone()];

        if let Some(ref material) = self.material_intent {
            desc_parts.push(format!("Material: {}", material));
        }

        if let Some(wear) = self.wear_level {
            let wear_desc = if wear < 0.2 {
                "nearly new"
            } else if wear < 0.5 {
                "lightly worn"
            } else if wear < 0.8 {
                "well-worn and weathered"
            } else {
                "heavily damaged and aged"
            };
            desc_parts.push(wear_desc.to_string());
        }

        if let Some(ref size) = self.size_class {
            desc_parts.push(format!("Size: {}", size));
        }

        let enriched_description = desc_parts.join(". ");

        // Choose texture size based on size_class
        let texture_params = if kind == AssetKind::Texture {
            let (w, h) = match self.size_class.as_deref() {
                Some("small") => (512, 512),
                Some("large") | Some("huge") => (2048, 2048),
                _ => (1024, 1024),
            };
            Some(TextureParams {
                width: w,
                height: h,
                ..Default::default()
            })
        } else {
            None
        };

        GenerateRequest {
            name: self.name.clone(),
            description: enriched_description,
            kind,
            texture_params,
            model_params: if kind == AssetKind::Model {
                Some(ModelParams::default())
            } else {
                None
            },
            audio_params: if kind == AssetKind::Audio {
                Some(AudioParams::default())
            } else {
                None
            },
            tags: self.tags.clone(),
        }
    }
}

/// Extract semantic asset definitions from a scene TOML
pub fn extract_semantic_defs(scene_path: &str) -> Vec<SemanticAssetDef> {
    let content = match std::fs::read_to_string(scene_path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let scene: toml::Value = match toml::from_str(&content) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let mut defs = Vec::new();

    if let Some(entities) = scene.get("entities").and_then(|e| e.as_table()) {
        for (_entity_name, entity_data) in entities {
            if let Some(asset_def) = entity_data.get("asset_def").and_then(|v| v.as_table()) {
                if let Ok(def) = toml::Value::Table(asset_def.clone()).try_into::<SemanticAssetDef>()
                {
                    defs.push(def);
                }
            }
        }
    }

    defs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_semantic_to_request_texture() {
        let def = SemanticAssetDef {
            name: "brick_wall".to_string(),
            description: "Weathered red brick wall".to_string(),
            asset_type: "texture".to_string(),
            material_intent: Some("rough clay brick".to_string()),
            wear_level: Some(0.7),
            size_class: Some("large".to_string()),
            tags: vec!["wall".to_string()],
        };

        let request = def.to_request();
        assert_eq!(request.name, "brick_wall");
        assert_eq!(request.kind, AssetKind::Texture);
        assert!(request.description.contains("Weathered red brick"));
        assert!(request.description.contains("rough clay brick"));
        assert!(request.description.contains("well-worn and weathered"));
        assert_eq!(request.texture_params.unwrap().width, 2048); // large = 2048
    }

    #[test]
    fn test_semantic_to_request_model() {
        let def = SemanticAssetDef {
            name: "tavern_chair".to_string(),
            description: "Sturdy wooden tavern chair".to_string(),
            asset_type: "model".to_string(),
            material_intent: Some("aged oak".to_string()),
            wear_level: Some(0.3),
            size_class: Some("medium".to_string()),
            tags: vec!["furniture".to_string()],
        };

        let request = def.to_request();
        assert_eq!(request.kind, AssetKind::Model);
        assert!(request.model_params.is_some());
        assert!(request.description.contains("lightly worn"));
    }

    #[test]
    fn test_semantic_to_request_minimal() {
        let def = SemanticAssetDef {
            name: "test".to_string(),
            description: "A simple test".to_string(),
            asset_type: "audio".to_string(),
            material_intent: None,
            wear_level: None,
            size_class: None,
            tags: vec![],
        };

        let request = def.to_request();
        assert_eq!(request.kind, AssetKind::Audio);
        assert_eq!(request.description, "A simple test");
    }

    #[test]
    fn test_semantic_serde_roundtrip() {
        let def = SemanticAssetDef {
            name: "test_asset".to_string(),
            description: "A test asset".to_string(),
            asset_type: "texture".to_string(),
            material_intent: Some("stone".to_string()),
            wear_level: Some(0.5),
            size_class: Some("medium".to_string()),
            tags: vec!["test".to_string()],
        };

        let toml_str = toml::to_string_pretty(&def).unwrap();
        let parsed: SemanticAssetDef = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.name, "test_asset");
        assert_eq!(parsed.material_intent, Some("stone".to_string()));
    }
}
