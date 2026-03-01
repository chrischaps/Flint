//! Texture atlas support — parses `.atlas.toml` sidecar files and resolves
//! named regions to pixel-space `[x, y, w, h]` rectangles.
//!
//! Atlas TOML format:
//! ```toml
//! [atlas]
//! texture = "assets/textures/player_sheet.png"
//!
//! [atlas.regions]
//! idle_0 = [0, 0, 64, 64]
//! run_0  = [0, 64, 64, 64]
//! ```

use std::collections::HashMap;
use std::path::Path;

/// A parsed texture atlas — maps region names to pixel-space `[x, y, w, h]`.
#[derive(Debug, Clone)]
pub struct TextureAtlas {
    /// The texture file this atlas refers to
    pub texture: String,
    /// Named regions: name → [x, y, w, h] in pixels
    pub regions: HashMap<String, [u32; 4]>,
}

impl TextureAtlas {
    /// Parse an atlas from a TOML string.
    pub fn from_toml(toml_str: &str) -> Result<Self, String> {
        let value: toml::Value =
            toml::from_str(toml_str).map_err(|e| format!("Atlas TOML parse error: {e}"))?;

        let atlas = value.get("atlas").ok_or("Missing [atlas] table")?;

        let texture = atlas
            .get("texture")
            .and_then(|v| v.as_str())
            .ok_or("Missing atlas.texture string")?
            .to_string();

        let mut regions = HashMap::new();
        if let Some(regions_table) = atlas.get("regions").and_then(|v| v.as_table()) {
            for (name, val) in regions_table {
                if let Some(arr) = val.as_array() {
                    if arr.len() == 4 {
                        let nums: Vec<u32> = arr
                            .iter()
                            .filter_map(|v| v.as_integer().map(|i| i as u32))
                            .collect();
                        if nums.len() == 4 {
                            regions.insert(name.clone(), [nums[0], nums[1], nums[2], nums[3]]);
                        }
                    }
                }
            }
        }

        Ok(Self { texture, regions })
    }

    /// Load an atlas from a `.atlas.toml` file.
    pub fn load(path: &Path) -> Result<Self, String> {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("Failed to read atlas: {e}"))?;
        Self::from_toml(&content)
    }
}

/// Registry of loaded texture atlases, keyed by texture name.
#[derive(Default)]
pub struct AtlasRegistry {
    atlases: HashMap<String, TextureAtlas>,
}

impl AtlasRegistry {
    pub fn new() -> Self {
        Self {
            atlases: HashMap::new(),
        }
    }

    /// Register an atlas under its texture name.
    pub fn insert(&mut self, atlas: TextureAtlas) {
        self.atlases.insert(atlas.texture.clone(), atlas);
    }

    /// Resolve a region name within a texture's atlas.
    /// Returns `[x, y, w, h]` in pixels if found.
    pub fn resolve(&self, texture: &str, region: &str) -> Option<[u32; 4]> {
        self.atlases
            .get(texture)
            .and_then(|a| a.regions.get(region).copied())
    }

    /// Load all `.atlas.toml` files from a directory.
    pub fn load_from_directory(&mut self, dir: &Path) -> Result<usize, String> {
        let mut count = 0;
        if !dir.exists() {
            return Ok(0);
        }
        let entries =
            std::fs::read_dir(dir).map_err(|e| format!("Failed to read atlas dir: {e}"))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.ends_with(".atlas.toml"))
                .unwrap_or(false)
            {
                match TextureAtlas::load(&path) {
                    Ok(atlas) => {
                        self.insert(atlas);
                        count += 1;
                    }
                    Err(e) => tracing::warn!("Failed to load atlas {:?}: {e}", path),
                }
            }
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atlas_parse() {
        let toml = r#"
[atlas]
texture = "player_sheet.png"

[atlas.regions]
idle_0 = [0, 0, 64, 64]
run_0 = [64, 0, 64, 64]
run_1 = [128, 0, 64, 64]
"#;
        let atlas = TextureAtlas::from_toml(toml).unwrap();
        assert_eq!(atlas.texture, "player_sheet.png");
        assert_eq!(atlas.regions.len(), 3);
        assert_eq!(atlas.regions["idle_0"], [0, 0, 64, 64]);
        assert_eq!(atlas.regions["run_1"], [128, 0, 64, 64]);
    }

    #[test]
    fn test_atlas_registry_resolve() {
        let toml = r#"
[atlas]
texture = "sprites.png"

[atlas.regions]
hero = [0, 0, 32, 32]
"#;
        let atlas = TextureAtlas::from_toml(toml).unwrap();
        let mut registry = AtlasRegistry::new();
        registry.insert(atlas);

        assert_eq!(
            registry.resolve("sprites.png", "hero"),
            Some([0, 0, 32, 32])
        );
        assert_eq!(registry.resolve("sprites.png", "missing"), None);
        assert_eq!(registry.resolve("other.png", "hero"), None);
    }
}
