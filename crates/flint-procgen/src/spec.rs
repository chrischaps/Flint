//! Procedural generation specification — the TOML-driven description of what
//! to generate and how.
//!
//! A [`ProcGenSpec`] is loaded from a `*.procgen.toml` file and carries all
//! the information a [`Generator`](crate::Generator) needs: which generator to
//! invoke, parameters, seed configuration, and optional LOD levels.

use std::path::Path;

use flint_core::ContentHash;
use serde::{Deserialize, Serialize};

use crate::{ProcGenError, Result, Seed};

// ── Spec metadata ──────────────────────────────────────────────────────────

/// Human-readable metadata about a procedural generation spec.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpecMeta {
    /// Unique name (e.g. `"oak_tree_01"`).
    pub name: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Semantic version of this spec (e.g. `"1.0.0"`).
    pub version: String,
    /// Freeform tags for search and filtering.
    #[serde(default)]
    pub tags: Vec<String>,
}

// ── Seed configuration ─────────────────────────────────────────────────────

/// How the generator's seed should be determined.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SeedMode {
    /// Use a specific numeric seed.
    Fixed,
    /// Generate a fresh random seed each time.
    Random,
    /// Derive a seed from a string key.
    Derived,
}

/// Seed configuration within a spec.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SeedConfig {
    /// How to determine the seed.
    pub mode: SeedMode,
    /// The numeric value (required when `mode = "fixed"`).
    #[serde(default)]
    pub value: Option<u64>,
    /// The derivation key (required when `mode = "derived"`).
    #[serde(default)]
    pub derive_from: Option<String>,
}

impl SeedConfig {
    /// Convert to the engine's [`Seed`] enum.
    pub fn to_seed(&self) -> Result<Seed> {
        match self.mode {
            SeedMode::Fixed => {
                let value = self.value.ok_or_else(|| {
                    ProcGenError::SeedError("seed mode is 'fixed' but no value provided".into())
                })?;
                Ok(Seed::Fixed(value))
            }
            SeedMode::Random => Ok(Seed::Random),
            SeedMode::Derived => {
                let key = self.derive_from.as_ref().ok_or_else(|| {
                    ProcGenError::SeedError(
                        "seed mode is 'derived' but no derive_from provided".into(),
                    )
                })?;
                Ok(Seed::Derived(key.clone()))
            }
        }
    }
}

// ── LOD ────────────────────────────────────────────────────────────────────

/// A level-of-detail target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LodLevel {
    /// LOD index (0 = highest detail).
    pub level: u32,
    /// Target triangle count for this LOD.
    pub target_triangles: u32,
}

// ── ProcGenSpec ────────────────────────────────────────────────────────────

/// A complete procedural generation specification.
///
/// Parsed from a `*.procgen.toml` file, this carries everything needed to
/// invoke a generator: which generator type, parameters, seed, and LOD.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcGenSpec {
    /// Metadata (name, version, tags, etc.).
    pub meta: SpecMeta,
    /// The generator type name (must match a registered [`Generator`](crate::Generator)).
    pub generator: String,
    /// Seed configuration.
    pub seed: SeedConfig,
    /// Generator-specific parameters (opaque TOML table).
    #[serde(default = "default_params")]
    pub params: toml::Value,
    /// Optional LOD levels.
    #[serde(default)]
    pub lod: Option<Vec<LodLevel>>,
}

fn default_params() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

impl ProcGenSpec {
    /// Parse a spec from a TOML string.
    pub fn parse(toml_str: &str) -> Result<Self> {
        toml::from_str(toml_str).map_err(|e| ProcGenError::SpecParseError(e.to_string()))
    }

    /// Load a spec from a TOML file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::parse(&content)
    }

    /// Resolve the spec's seed configuration to a concrete `u64`.
    pub fn resolve_seed(&self) -> Result<u64> {
        Ok(self.seed.to_seed()?.resolve())
    }

    /// Compute a content hash of the canonical TOML representation.
    ///
    /// Two specs with identical content produce the same hash regardless of
    /// key ordering. The hash is the first 8 bytes of SHA-256 as `u64`.
    pub fn content_hash(&self) -> u64 {
        let canonical = toml::to_string(self).expect("spec should always serialize");
        let hash = ContentHash::from_str(&canonical);
        let bytes = hash.as_bytes();
        u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ])
    }
}

/// Recursively discover all `*.procgen.toml` files in a directory.
///
/// Returns successfully parsed specs; files that fail to parse are silently
/// skipped (callers should use more targeted loading for error reporting).
pub fn discover_specs(dir: &Path) -> Result<Vec<ProcGenSpec>> {
    let mut specs = Vec::new();
    if !dir.is_dir() {
        return Ok(specs);
    }
    visit_dir(dir, &mut specs)?;
    Ok(specs)
}

fn visit_dir(dir: &Path, specs: &mut Vec<ProcGenSpec>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            visit_dir(&path, specs)?;
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".procgen.toml"))
        {
            if let Ok(spec) = ProcGenSpec::from_file(&path) {
                specs.push(spec);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_SPEC: &str = r#"
generator = "mock"

[meta]
name = "test_object"
version = "1.0.0"

[seed]
mode = "fixed"
value = 42
"#;

    const FULL_SPEC: &str = r#"
generator = "terrain_heightmap"

[meta]
name = "mountain_range"
description = "A rugged mountain range"
version = "2.1.0"
tags = ["terrain", "outdoor", "mountains"]

[seed]
mode = "derived"
derive_from = "world_seed:mountains"

[params]
width = 256
height = 256
octaves = 6
persistence = 0.5
scale = 100.0

[[lod]]
level = 0
target_triangles = 50000

[[lod]]
level = 1
target_triangles = 10000

[[lod]]
level = 2
target_triangles = 2000
"#;

    #[test]
    fn parse_minimal_spec() {
        let spec = ProcGenSpec::parse(MINIMAL_SPEC).unwrap();
        assert_eq!(spec.meta.name, "test_object");
        assert_eq!(spec.meta.version, "1.0.0");
        assert_eq!(spec.generator, "mock");
        assert!(spec.meta.description.is_none());
        assert!(spec.meta.tags.is_empty());
        assert!(spec.lod.is_none());
    }

    #[test]
    fn parse_full_spec() {
        let spec = ProcGenSpec::parse(FULL_SPEC).unwrap();
        assert_eq!(spec.meta.name, "mountain_range");
        assert_eq!(
            spec.meta.description.as_deref(),
            Some("A rugged mountain range")
        );
        assert_eq!(spec.meta.tags, vec!["terrain", "outdoor", "mountains"]);
        assert_eq!(spec.generator, "terrain_heightmap");

        let lod = spec.lod.as_ref().unwrap();
        assert_eq!(lod.len(), 3);
        assert_eq!(lod[0].level, 0);
        assert_eq!(lod[0].target_triangles, 50000);
        assert_eq!(lod[2].target_triangles, 2000);
    }

    #[test]
    fn seed_config_fixed() {
        let spec = ProcGenSpec::parse(MINIMAL_SPEC).unwrap();
        let seed = spec.seed.to_seed().unwrap();
        assert_eq!(seed, Seed::Fixed(42));
        assert_eq!(spec.resolve_seed().unwrap(), 42);
    }

    #[test]
    fn seed_config_random() {
        let toml_str = r#"
generator = "mock"
[meta]
name = "t"
version = "1.0.0"
[seed]
mode = "random"
"#;
        let spec = ProcGenSpec::parse(toml_str).unwrap();
        let seed = spec.seed.to_seed().unwrap();
        assert_eq!(seed, Seed::Random);
    }

    #[test]
    fn seed_config_derived() {
        let spec = ProcGenSpec::parse(FULL_SPEC).unwrap();
        let seed = spec.seed.to_seed().unwrap();
        assert_eq!(seed, Seed::Derived("world_seed:mountains".into()));
    }

    #[test]
    fn seed_config_fixed_missing_value() {
        let cfg = SeedConfig {
            mode: SeedMode::Fixed,
            value: None,
            derive_from: None,
        };
        assert!(cfg.to_seed().is_err());
    }

    #[test]
    fn seed_config_derived_missing_key() {
        let cfg = SeedConfig {
            mode: SeedMode::Derived,
            value: None,
            derive_from: None,
        };
        assert!(cfg.to_seed().is_err());
    }

    #[test]
    fn content_hash_stability() {
        let spec = ProcGenSpec::parse(MINIMAL_SPEC).unwrap();
        let h1 = spec.content_hash();
        let h2 = spec.content_hash();
        assert_eq!(h1, h2);
    }

    #[test]
    fn content_hash_sensitivity() {
        let spec_a = ProcGenSpec::parse(MINIMAL_SPEC).unwrap();
        let modified = MINIMAL_SPEC.replace("42", "99");
        let spec_b = ProcGenSpec::parse(&modified).unwrap();
        assert_ne!(spec_a.content_hash(), spec_b.content_hash());
    }

    #[test]
    fn parse_error_on_bad_toml() {
        let result = ProcGenSpec::parse("not valid { toml");
        assert!(result.is_err());
    }

    #[test]
    fn discover_specs_empty_dir() {
        let dir = std::env::temp_dir().join("flint_procgen_test_empty");
        let _ = std::fs::create_dir_all(&dir);
        let specs = discover_specs(&dir).unwrap();
        // May be empty or contain leftovers, but shouldn't error.
        let _ = specs;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn discover_specs_finds_files() {
        let dir = std::env::temp_dir().join("flint_procgen_test_discover");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let spec_path = dir.join("test.procgen.toml");
        std::fs::write(&spec_path, MINIMAL_SPEC).unwrap();

        // Non-matching file should be ignored.
        std::fs::write(dir.join("readme.txt"), "hello").unwrap();

        let specs = discover_specs(&dir).unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].meta.name, "test_object");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
