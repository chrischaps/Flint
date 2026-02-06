//! Build manifest for tracking generated assets
//!
//! Records all generated assets with their provenance (provider, prompt, seed, time)
//! for reproducibility and auditing.

use flint_core::{FlintError, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A record of a single generated asset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub name: String,
    pub asset_type: String,
    pub provider: String,
    pub prompt: String,
    #[serde(default)]
    pub seed: Option<u64>,
    pub content_hash: String,
    pub generated_at: String,
    pub duration_secs: f64,
    #[serde(default)]
    pub style: Option<String>,
    #[serde(default)]
    pub output_path: Option<String>,
    #[serde(default)]
    pub validation_passed: Option<bool>,
}

/// Build manifest tracking all generated assets
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildManifest {
    pub generated_at: String,
    #[serde(default)]
    pub style: Option<String>,
    pub entries: Vec<ManifestEntry>,
}

/// TOML wrapper
#[derive(Debug, Serialize, Deserialize)]
struct ManifestFile {
    manifest: BuildManifest,
}

impl BuildManifest {
    /// Create a new empty manifest
    pub fn new() -> Self {
        Self {
            generated_at: now_iso8601(),
            style: None,
            entries: Vec::new(),
        }
    }

    /// Add an entry from a generation result
    pub fn add_entry(&mut self, entry: ManifestEntry) {
        self.entries.push(entry);
    }

    /// Load manifest from file
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let file: ManifestFile = toml::from_str(&content).map_err(|e| {
            FlintError::GenerationError(format!("Failed to parse manifest: {}", e))
        })?;
        Ok(file.manifest)
    }

    /// Save manifest to file
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = ManifestFile {
            manifest: self.clone(),
        };
        let content = toml::to_string_pretty(&file).map_err(|e| {
            FlintError::GenerationError(format!("Failed to serialize manifest: {}", e))
        })?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Create manifest by scanning existing .asset.toml sidecar files for generated assets
    pub fn from_assets_directory(path: &Path) -> Result<Self> {
        let catalog = flint_asset::AssetCatalog::load_from_directory(path)?;
        let mut manifest = Self::new();

        for name in catalog.names() {
            if let Some(meta) = catalog.get(name) {
                // Only include assets that have a "provider" property (i.e., generated)
                if let Some(provider) = meta.properties.get("provider") {
                    manifest.add_entry(ManifestEntry {
                        name: meta.name.clone(),
                        asset_type: format!("{:?}", meta.asset_type).to_lowercase(),
                        provider: provider
                            .as_str()
                            .unwrap_or("unknown")
                            .to_string(),
                        prompt: meta
                            .properties
                            .get("prompt")
                            .and_then(|p| p.as_str())
                            .unwrap_or("")
                            .to_string(),
                        seed: meta
                            .properties
                            .get("seed")
                            .and_then(|s| s.as_str())
                            .and_then(|s| s.parse().ok()),
                        content_hash: meta.hash.clone(),
                        generated_at: manifest.generated_at.clone(),
                        duration_secs: 0.0,
                        style: None,
                        output_path: meta.source_path.clone(),
                        validation_passed: None,
                    });
                }
            }
        }

        Ok(manifest)
    }
}

impl Default for BuildManifest {
    fn default() -> Self {
        Self::new()
    }
}

fn now_iso8601() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();
    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let mins = (time_secs % 3600) / 60;
    let s = time_secs % 60;

    let mut y = 1970i64;
    let mut remaining_days = days as i64;
    loop {
        let days_in_year = if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            366
        } else {
            365
        };
        if remaining_days < days_in_year {
            break;
        }
        remaining_days -= days_in_year;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 0usize;
    for (i, &md) in month_days.iter().enumerate() {
        if remaining_days < md as i64 {
            m = i;
            break;
        }
        remaining_days -= md as i64;
    }

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m + 1,
        remaining_days + 1,
        hours,
        mins,
        s
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "flint_manifest_test_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_manifest_roundtrip() {
        let dir = temp_dir();
        let path = dir.join("manifest.toml");

        let mut manifest = BuildManifest::new();
        manifest.style = Some("medieval_tavern".to_string());
        manifest.add_entry(ManifestEntry {
            name: "brick_wall".to_string(),
            asset_type: "texture".to_string(),
            provider: "flux".to_string(),
            prompt: "weathered red brick".to_string(),
            seed: Some(42),
            content_hash: "sha256:abc123".to_string(),
            generated_at: manifest.generated_at.clone(),
            duration_secs: 8.3,
            style: Some("medieval_tavern".to_string()),
            output_path: Some(".flint/generated/brick_wall.png".to_string()),
            validation_passed: None,
        });

        manifest.save(&path).unwrap();
        let loaded = BuildManifest::load(&path).unwrap();

        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].name, "brick_wall");
        assert_eq!(loaded.entries[0].provider, "flux");
        assert_eq!(loaded.entries[0].seed, Some(42));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_manifest_empty() {
        let manifest = BuildManifest::new();
        assert!(manifest.entries.is_empty());
        assert!(manifest.generated_at.contains('T'));
    }
}
