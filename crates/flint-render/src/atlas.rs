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

use flint_core::toml_util::{check_keys, Shape};
use flint_core::vfs;
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

/// The keys a `.atlas.toml` may hold (#282); region names are free.
const ATLAS_FILE: Shape = Shape::Table(&[(
    "atlas",
    Shape::Table(&[
        ("texture", Shape::Value),
        ("regions", Shape::Map(&Shape::Value)),
    ]),
)]);

impl TextureAtlas {
    /// Parse an atlas from a TOML string, ignoring (silently) what it
    /// can't read; [`TextureAtlas::parse`] says what that was.
    pub fn from_toml(toml_str: &str) -> Result<Self, String> {
        Self::parse(toml_str).map(|(atlas, _)| atlas)
    }

    /// Parse an atlas: the atlas, and one warning for each key the format
    /// doesn't know and each region that isn't `[x, y, w, h]` (skipped).
    pub fn parse(toml_str: &str) -> Result<(Self, Vec<String>), String> {
        let value: toml::Value =
            toml::from_str(toml_str).map_err(|e| format!("Atlas TOML parse error: {e}"))?;
        let mut warnings: Vec<String> = check_keys(&value, &ATLAS_FILE)
            .iter()
            .map(ToString::to_string)
            .collect();

        let atlas = value.get("atlas").ok_or("Missing [atlas] table")?;

        let texture = atlas
            .get("texture")
            .and_then(|v| v.as_str())
            .ok_or("Missing atlas.texture string")?
            .to_string();

        let mut regions = HashMap::new();
        if let Some(regions_table) = atlas.get("regions").and_then(|v| v.as_table()) {
            for (name, val) in regions_table {
                let rect: Option<Vec<u32>> =
                    val.as_array().filter(|a| a.len() == 4).and_then(|a| {
                        a.iter()
                            .map(|v| v.as_integer().and_then(|i| u32::try_from(i).ok()))
                            .collect()
                    });
                match rect {
                    Some(n) => {
                        regions.insert(name.clone(), [n[0], n[1], n[2], n[3]]);
                    }
                    None => warnings.push(format!(
                        "region `{name}` in [atlas.regions] should be [x, y, w, h], four \
                         whole pixels, not {val}; skipped"
                    )),
                }
            }
        }

        Ok((Self { texture, regions }, warnings))
    }

    /// Load an atlas from a `.atlas.toml` file, warning about keys and
    /// regions it ignores.
    pub fn load(path: &Path) -> Result<Self, String> {
        let content =
            vfs::read_to_string(path).map_err(|e| format!("Failed to read atlas: {e}"))?;
        let (atlas, warnings) = Self::parse(&content)?;
        for warning in warnings {
            tracing::warn!("{}: {warning}", path.display());
        }
        Ok(atlas)
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
        if !vfs::exists(dir) {
            return Ok(0);
        }
        let entries = vfs::read_dir(dir).map_err(|e| format!("Failed to read atlas dir: {e}"))?;
        for path in entries {
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
    fn misplaced_and_malformed_atlas_keys_are_named() {
        // #282: `texure` and `[atlas.region]` were ignored without a word
        let toml = r#"
[atlas]
texture = "sheet.png"
texure = "other.png"

[atlas.region]
idle_0 = [0, 0, 64, 64]

[atlas.regions]
run_0 = [0, 64, 64]
run_1 = [0, 64, 64, 64]
"#;
        let (atlas, warnings) = TextureAtlas::parse(toml).unwrap();
        assert_eq!(atlas.regions.len(), 1);
        assert_eq!(warnings.len(), 3, "{warnings:#?}");
        let all = warnings.join("\n");
        assert!(
            all.contains("`region` in [atlas]") && all.contains("`regions`"),
            "{all}"
        );
        assert!(
            all.contains("`texure` in [atlas]") && all.contains("`texture`"),
            "{all}"
        );
        assert!(all.contains("region `run_0`"), "{all}");
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
