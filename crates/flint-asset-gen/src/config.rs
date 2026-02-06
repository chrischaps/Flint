//! Layered configuration system
//!
//! Config is loaded with three layers of precedence (highest wins):
//! 1. Environment variables: `FLINT_{PROVIDER}_API_KEY`
//! 2. Project-local: `.flint/config.toml`
//! 3. Global: `~/.flint/config.toml`

use flint_core::{FlintError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Provider-specific configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_url: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Generation defaults
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationConfig {
    #[serde(default = "default_texture_provider")]
    pub default_texture_provider: String,
    #[serde(default = "default_model_provider")]
    pub default_model_provider: String,
    #[serde(default = "default_audio_provider")]
    pub default_audio_provider: String,
    #[serde(default)]
    pub style: Option<String>,
}

impl Default for GenerationConfig {
    fn default() -> Self {
        Self {
            default_texture_provider: default_texture_provider(),
            default_model_provider: default_model_provider(),
            default_audio_provider: default_audio_provider(),
            style: None,
        }
    }
}

fn default_texture_provider() -> String {
    "flux".to_string()
}
fn default_model_provider() -> String {
    "meshy".to_string()
}
fn default_audio_provider() -> String {
    "elevenlabs".to_string()
}

/// Top-level config file structure
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlintConfigFile {
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub generation: GenerationConfig,
}

/// Resolved configuration with environment variable overrides applied
#[derive(Debug, Clone)]
pub struct FlintConfig {
    pub providers: HashMap<String, ProviderConfig>,
    pub generation: GenerationConfig,
}

impl FlintConfig {
    /// Load config with layered precedence: global < project < env vars
    pub fn load() -> Result<Self> {
        let mut config = FlintConfigFile::default();

        // Layer 1: Global config (~/.flint/config.toml)
        if let Some(global_path) = Self::global_config_path() {
            if global_path.exists() {
                let global = Self::load_file(&global_path)?;
                Self::merge_into(&mut config, global);
            }
        }

        // Layer 2: Project-local config (.flint/config.toml)
        let local_path = PathBuf::from(".flint/config.toml");
        if local_path.exists() {
            let local = Self::load_file(&local_path)?;
            Self::merge_into(&mut config, local);
        }

        // Layer 3: Environment variable overrides
        Self::apply_env_overrides(&mut config);

        Ok(FlintConfig {
            providers: config.providers,
            generation: config.generation,
        })
    }

    /// Load config from a specific file path only (for testing)
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let file = Self::load_file(path)?;
        let mut config = file;
        Self::apply_env_overrides(&mut config);
        Ok(FlintConfig {
            providers: config.providers,
            generation: config.generation,
        })
    }

    /// Get API key for a provider
    pub fn api_key(&self, provider_name: &str) -> Option<&str> {
        self.providers
            .get(provider_name)
            .and_then(|p| p.api_key.as_deref())
    }

    /// Get API URL for a provider (or its default)
    pub fn api_url(&self, provider_name: &str) -> Option<&str> {
        self.providers
            .get(provider_name)
            .and_then(|p| p.api_url.as_deref())
    }

    /// Check if a provider is enabled
    pub fn is_enabled(&self, provider_name: &str) -> bool {
        self.providers
            .get(provider_name)
            .map(|p| p.enabled)
            .unwrap_or(true)
    }

    /// Get the default provider name for an asset kind
    pub fn default_provider(&self, kind: crate::provider::AssetKind) -> &str {
        match kind {
            crate::provider::AssetKind::Texture => &self.generation.default_texture_provider,
            crate::provider::AssetKind::Model => &self.generation.default_model_provider,
            crate::provider::AssetKind::Audio => &self.generation.default_audio_provider,
        }
    }

    /// Get the default style name
    pub fn default_style(&self) -> Option<&str> {
        self.generation.style.as_deref()
    }

    fn global_config_path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".flint").join("config.toml"))
    }

    fn load_file(path: &Path) -> Result<FlintConfigFile> {
        let content = std::fs::read_to_string(path)?;
        let config: FlintConfigFile = toml::from_str(&content).map_err(|e| {
            FlintError::GenerationError(format!("Failed to parse config {}: {}", path.display(), e))
        })?;
        Ok(config)
    }

    fn merge_into(base: &mut FlintConfigFile, overlay: FlintConfigFile) {
        for (name, provider) in overlay.providers {
            let entry = base.providers.entry(name).or_default();
            if provider.api_key.is_some() {
                entry.api_key = provider.api_key;
            }
            if provider.api_url.is_some() {
                entry.api_url = provider.api_url;
            }
            entry.enabled = provider.enabled;
        }

        if overlay.generation.default_texture_provider != default_texture_provider() {
            base.generation.default_texture_provider =
                overlay.generation.default_texture_provider;
        }
        if overlay.generation.default_model_provider != default_model_provider() {
            base.generation.default_model_provider = overlay.generation.default_model_provider;
        }
        if overlay.generation.default_audio_provider != default_audio_provider() {
            base.generation.default_audio_provider = overlay.generation.default_audio_provider;
        }
        if overlay.generation.style.is_some() {
            base.generation.style = overlay.generation.style;
        }
    }

    fn apply_env_overrides(config: &mut FlintConfigFile) {
        let provider_names = ["flux", "meshy", "elevenlabs"];
        for name in &provider_names {
            let env_key = format!("FLINT_{}_API_KEY", name.to_uppercase());
            if let Ok(key) = std::env::var(&env_key) {
                let entry = config.providers.entry(name.to_string()).or_default();
                entry.api_key = Some(key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_config(content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("flint_config_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn test_load_config_from_file() {
        // Clear any env var that might interfere
        std::env::remove_var("FLINT_FLUX_API_KEY");

        let config_str = r#"
[providers.flux]
api_key = "test-key-123"
api_url = "https://api.example.com/flux"
enabled = true

[providers.meshy]
api_key = "msy_test"
enabled = false

[generation]
default_texture_provider = "flux"
style = "medieval_tavern"
"#;
        let path = temp_config(config_str);
        let config = FlintConfig::load_from_file(&path).unwrap();

        assert!(config.is_enabled("flux"));
        assert!(!config.is_enabled("meshy"));
        assert_eq!(config.default_style(), Some("medieval_tavern"));
        assert_eq!(
            config.api_url("flux"),
            Some("https://api.example.com/flux")
        );

        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(path.parent().unwrap()).ok();
    }

    #[test]
    fn test_env_var_override() {
        let config_str = r#"
[providers.elevenlabs]
api_key = "file-key"
"#;
        let path = temp_config(config_str);

        // Set env var
        std::env::set_var("FLINT_ELEVENLABS_API_KEY", "env-key-override");

        let config = FlintConfig::load_from_file(&path).unwrap();
        assert_eq!(config.api_key("elevenlabs"), Some("env-key-override"));

        // Clean up
        std::env::remove_var("FLINT_ELEVENLABS_API_KEY");
        std::fs::remove_file(&path).ok();
        std::fs::remove_dir(path.parent().unwrap()).ok();
    }

    #[test]
    fn test_default_providers() {
        let config = FlintConfig {
            providers: HashMap::new(),
            generation: GenerationConfig::default(),
        };

        assert_eq!(
            config.default_provider(crate::provider::AssetKind::Texture),
            "flux"
        );
        assert_eq!(
            config.default_provider(crate::provider::AssetKind::Model),
            "meshy"
        );
        assert_eq!(
            config.default_provider(crate::provider::AssetKind::Audio),
            "elevenlabs"
        );
    }

    #[test]
    fn test_missing_provider_returns_none() {
        let config = FlintConfig {
            providers: HashMap::new(),
            generation: GenerationConfig::default(),
        };
        assert_eq!(config.api_key("nonexistent"), None);
        assert!(config.is_enabled("nonexistent")); // defaults to true
    }
}
