//! Style guide system for enriching generation prompts
//!
//! Style guides define a visual vocabulary (palette, materials, geometry constraints)
//! that providers use to enrich prompts for consistent asset generation.

use flint_core::{FlintError, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A style guide that enriches generation prompts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StyleGuide {
    /// Style name (e.g., "medieval_tavern")
    pub name: String,
    /// Human-readable description
    #[serde(default)]
    pub description: Option<String>,
    /// Prompt prefix prepended to all generation prompts
    #[serde(default)]
    pub prompt_prefix: Option<String>,
    /// Prompt suffix appended to all generation prompts
    #[serde(default)]
    pub prompt_suffix: Option<String>,
    /// Negative prompt (things to avoid)
    #[serde(default)]
    pub negative_prompt: Option<String>,
    /// Color palette as hex strings
    #[serde(default)]
    pub palette: Vec<String>,
    /// Material constraints
    #[serde(default)]
    pub materials: MaterialConstraints,
    /// Geometry constraints
    #[serde(default)]
    pub geometry: GeometryConstraints,
}

/// Constraints on material properties
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MaterialConstraints {
    /// Roughness range [min, max]
    #[serde(default)]
    pub roughness_range: Option<[f64; 2]>,
    /// Metallic range [min, max]
    #[serde(default)]
    pub metallic_range: Option<[f64; 2]>,
    /// Preferred material descriptors
    #[serde(default)]
    pub preferred_materials: Vec<String>,
}

/// Constraints on geometry
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GeometryConstraints {
    /// Maximum triangle count for meshes
    #[serde(default)]
    pub max_triangles: Option<u32>,
    /// Required features
    #[serde(default)]
    pub require_uvs: Option<bool>,
    #[serde(default)]
    pub require_normals: Option<bool>,
}

/// TOML file wrapper
#[derive(Debug, Deserialize)]
struct StyleFile {
    style: StyleGuide,
}

impl StyleGuide {
    /// Load a style guide from a TOML file
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let file: StyleFile = toml::from_str(&content).map_err(|e| {
            FlintError::GenerationError(format!(
                "Failed to parse style guide {}: {}",
                path.display(),
                e
            ))
        })?;
        Ok(file.style)
    }

    /// Find and load a style guide by name, searching standard locations
    pub fn find(name: &str) -> Result<Self> {
        let candidates = [
            format!("styles/{}.style.toml", name),
            format!(".flint/styles/{}.style.toml", name),
        ];

        for candidate in &candidates {
            let path = Path::new(candidate);
            if path.exists() {
                return Self::load(path);
            }
        }

        Err(FlintError::GenerationError(format!(
            "Style guide '{}' not found (searched: {})",
            name,
            candidates.join(", ")
        )))
    }

    /// Enrich a prompt with style guide context
    pub fn enrich_prompt(&self, base_prompt: &str) -> String {
        let mut parts = Vec::new();

        if let Some(ref prefix) = self.prompt_prefix {
            parts.push(prefix.clone());
        }

        parts.push(base_prompt.to_string());

        if !self.palette.is_empty() {
            parts.push(format!("Color palette: {}", self.palette.join(", ")));
        }

        if !self.materials.preferred_materials.is_empty() {
            parts.push(format!(
                "Materials: {}",
                self.materials.preferred_materials.join(", ")
            ));
        }

        if let Some(ref suffix) = self.prompt_suffix {
            parts.push(suffix.clone());
        }

        parts.join(". ")
    }

    /// Get the negative prompt (if any)
    pub fn negative(&self) -> Option<&str> {
        self.negative_prompt.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_style(content: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("flint_style_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.style.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn test_load_style_guide() {
        let style_str = r##"
[style]
name = "medieval_tavern"
description = "Weathered medieval fantasy tavern aesthetic"
prompt_prefix = "Medieval fantasy tavern style"
prompt_suffix = "Photorealistic, detailed textures, warm lighting"
negative_prompt = "modern, sci-fi, neon, plastic"
palette = ["#8B4513", "#A0522D", "#4A4A4A", "#D4A574", "#2F1B0E"]

[style.materials]
roughness_range = [0.6, 0.95]
metallic_range = [0.0, 0.15]
preferred_materials = ["aged wood", "rough stone", "hammered iron", "worn leather"]

[style.geometry]
max_triangles = 5000
require_uvs = true
require_normals = true
"##;
        let path = temp_style(style_str);
        let style = StyleGuide::load(&path).unwrap();

        assert_eq!(style.name, "medieval_tavern");
        assert_eq!(style.palette.len(), 5);
        assert_eq!(style.materials.roughness_range, Some([0.6, 0.95]));
        assert_eq!(style.geometry.max_triangles, Some(5000));

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(path.parent().unwrap()).ok();
    }

    #[test]
    fn test_enrich_prompt() {
        let style = StyleGuide {
            name: "test".to_string(),
            description: None,
            prompt_prefix: Some("Fantasy style".to_string()),
            prompt_suffix: Some("High quality".to_string()),
            negative_prompt: Some("modern".to_string()),
            palette: vec!["#8B4513".to_string(), "#A0522D".to_string()],
            materials: MaterialConstraints {
                roughness_range: None,
                metallic_range: None,
                preferred_materials: vec!["wood".to_string(), "stone".to_string()],
            },
            geometry: GeometryConstraints::default(),
        };

        let enriched = style.enrich_prompt("weathered brick wall");
        assert!(enriched.contains("Fantasy style"));
        assert!(enriched.contains("weathered brick wall"));
        assert!(enriched.contains("#8B4513"));
        assert!(enriched.contains("wood"));
        assert!(enriched.contains("High quality"));
    }

    #[test]
    fn test_enrich_prompt_minimal_style() {
        let style = StyleGuide {
            name: "minimal".to_string(),
            description: None,
            prompt_prefix: None,
            prompt_suffix: None,
            negative_prompt: None,
            palette: vec![],
            materials: MaterialConstraints::default(),
            geometry: GeometryConstraints::default(),
        };

        let enriched = style.enrich_prompt("simple cube");
        assert_eq!(enriched, "simple cube");
    }

    #[test]
    fn test_style_not_found() {
        let result = StyleGuide::find("nonexistent_style_xyz");
        assert!(result.is_err());
    }
}
