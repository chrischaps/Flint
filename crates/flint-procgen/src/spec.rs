//! Procedural generation specification — the TOML-driven description of what
//! to generate and how.
//!
//! A [`ProcGenSpec`] is loaded from a `*.procgen.toml` file and carries all
//! the information a [`Generator`](crate::Generator) needs: which generator to
//! invoke, parameters, seed configuration, and optional LOD levels.

use std::path::Path;

use flint_core::vfs;
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

// ── Living props ───────────────────────────────────────────────────────────

/// The params `[living] drift` may name.
pub const DRIFT_PARAMS: &[&str] = &["branch_angle", "lsystem_tropism", "trunk_curve"];

/// What `[living] jitter` may target.
pub const JITTER_TARGETS: &[&str] = &["leaf_cards"];

/// The `[living]` table: this spec is a prop that keeps drifting between
/// versions of itself. `flint gen`'s bake doesn't read it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LivingConfig {
    /// The seed that fixes the structure: the prop's identity. Required,
    /// since a prop's identity shouldn't be left to a default.
    pub lineage_seed: u64,
    /// How many poses to keep ready.
    #[serde(default = "default_pose_ring")]
    pub pose_ring: u32,
    /// The slow, large layer: changes the silhouette and always glides.
    #[serde(default)]
    pub drift: DriftConfig,
    /// The fast, small layer: makes it feel drawn and may snap.
    #[serde(default)]
    pub jitter: JitterConfig,
    /// Seconds between pose targets: one beat of the hold-and-glide clock.
    #[serde(default = "default_step")]
    pub step: f32,
    /// How much of each step is spent easing to the next pose, 0 to 1: 0
    /// holds and then snaps, 1 never holds.
    #[serde(default = "default_glide")]
    pub glide: f32,
    /// How the branches fall out of phase with each other.
    #[serde(default)]
    pub stagger: Stagger,
    /// How far poses wander from the lineage's, and where on the tree.
    #[serde(default)]
    pub variance: VarianceConfig,
}

fn default_pose_ring() -> u32 {
    8
}

fn default_step() -> f32 {
    0.5
}

fn default_glide() -> f32 {
    0.3
}

impl LivingConfig {
    /// Check what the types can't, naming the key at fault.
    pub fn validate(&self) -> Result<()> {
        let bad = |key: &str, reason: String| {
            Err(ProcGenError::InvalidParameter {
                name: format!("living.{key}"),
                reason,
            })
        };
        let at_least_zero = |x: f32| x.is_finite() && x >= 0.0;
        let unit = |x: f32| (0.0..=1.0).contains(&x);

        if self.pose_ring < 2 {
            return bad(
                "pose_ring",
                format!("{}: a ring needs at least 2 poses to blend", self.pose_ring),
            );
        }
        if !self.step.is_finite() || self.step <= 0.0 {
            return bad(
                "step",
                format!("{}: must be more than 0 seconds", self.step),
            );
        }
        if !unit(self.glide) {
            return bad("glide", format!("{}: must be in [0.0, 1.0]", self.glide));
        }
        if !at_least_zero(self.drift.amplitude) {
            return bad(
                "drift.amplitude",
                format!("{}: must be 0 or more", self.drift.amplitude),
            );
        }
        if let Some(p) = self
            .drift
            .params
            .iter()
            .find(|p| !DRIFT_PARAMS.contains(&p.as_str()))
        {
            return bad(
                "drift.params",
                format!("unknown param \"{p}\"; expected one of {DRIFT_PARAMS:?}"),
            );
        }
        if !at_least_zero(self.jitter.amplitude) {
            return bad(
                "jitter.amplitude",
                format!("{}: must be 0 or more", self.jitter.amplitude),
            );
        }
        if let Some(t) = self
            .jitter
            .targets
            .iter()
            .find(|t| !JITTER_TARGETS.contains(&t.as_str()))
        {
            return bad(
                "jitter.targets",
                format!("unknown target \"{t}\"; expected one of {JITTER_TARGETS:?}"),
            );
        }
        if !unit(self.variance.amount) {
            return bad(
                "variance.amount",
                format!("{}: must be in [0.0, 1.0]", self.variance.amount),
            );
        }
        if !unit(self.variance.steady) {
            return bad(
                "variance.steady",
                format!("{}: must be in [0.0, 1.0]", self.variance.steady),
            );
        }
        Ok(())
    }
}

/// `[living] drift`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DriftConfig {
    /// How large the drift is.
    pub amplitude: f32,
    /// Which spec params it moves, from [`DRIFT_PARAMS`].
    pub params: Vec<String>,
}

impl Default for DriftConfig {
    fn default() -> Self {
        Self {
            amplitude: 0.35,
            params: DRIFT_PARAMS.iter().map(|p| p.to_string()).collect(),
        }
    }
}

/// `[living] jitter`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct JitterConfig {
    /// How large the jitter is.
    pub amplitude: f32,
    /// What it moves, from [`JITTER_TARGETS`].
    pub targets: Vec<String>,
}

impl Default for JitterConfig {
    fn default() -> Self {
        Self {
            amplitude: 0.05,
            targets: JITTER_TARGETS.iter().map(|t| t.to_string()).collect(),
        }
    }
}

/// `[living] stagger`: each part's phase offset into the step cycle.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stagger {
    /// The whole prop moves at once.
    None,
    /// Offset by branching depth.
    #[default]
    PerLevel,
    /// Offset by segment.
    PerBranch,
}

/// `[living] variance` (#229): how much of the difference between poses is
/// left, and where on the tree.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct VarianceConfig {
    /// How much of each pose's difference from the base is left, 0 to 1.
    pub amount: f32,
    /// How much of that is held back from the trunk side: 0 even, 1 all at
    /// the tips.
    pub steady: f32,
    /// Scale each pose so its bounding box matches the base's, as near as one
    /// scale can: the side furthest over is as far over as the side furthest
    /// under is under.
    pub keep_size: bool,
}

impl VarianceConfig {
    /// Nothing held back: a pose is exactly what its shape seed grew.
    pub const FULL: Self = Self {
        amount: 1.0,
        steady: 0.0,
        keep_size: false,
    };

    /// How much of its difference from the base a segment at `depth` keeps:
    /// `amount × ((1 − steady) + steady × (depth / max_depth)^1.3)`.
    pub fn weight(&self, depth: u32, max_depth: u32) -> f32 {
        let level = depth as f32 / max_depth.max(1) as f32;
        self.amount * ((1.0 - self.steady) + self.steady * level.powf(1.3))
    }
}

impl Default for VarianceConfig {
    fn default() -> Self {
        Self {
            amount: 0.35,
            steady: 0.8,
            keep_size: true,
        }
    }
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
    /// Optional `[living]` table. Left out when absent, so a spec without one
    /// keeps its content hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub living: Option<LivingConfig>,
}

fn default_params() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

impl ProcGenSpec {
    /// Parse a spec from a TOML string.
    pub fn parse(toml_str: &str) -> Result<Self> {
        let spec: Self =
            toml::from_str(toml_str).map_err(|e| ProcGenError::SpecParseError(e.to_string()))?;
        if let Some(living) = &spec.living {
            living.validate()?;
        }
        Ok(spec)
    }

    /// Load a spec from a TOML file.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = vfs::read_to_string(path)?;
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
    if !vfs::is_dir(dir) {
        return Ok(specs);
    }
    visit_dir(dir, &mut specs)?;
    Ok(specs)
}

fn visit_dir(dir: &Path, specs: &mut Vec<ProcGenSpec>) -> Result<()> {
    for path in vfs::read_dir(dir)? {
        if vfs::is_dir(&path) {
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

    fn with_living(table: &str) -> String {
        format!("{MINIMAL_SPEC}\n[living]\n{table}\n")
    }

    #[test]
    fn a_spec_without_living_keeps_its_hash_text() {
        let spec = ProcGenSpec::parse(MINIMAL_SPEC).unwrap();
        assert!(spec.living.is_none());
        assert!(!toml::to_string(&spec).unwrap().contains("living"));
    }

    #[test]
    fn living_parses_the_designs_table() {
        let spec = ProcGenSpec::parse(&with_living(
            r#"
lineage_seed = 7
pose_ring = 12
drift = { amplitude = 0.2, params = ["branch_angle", "lsystem_tropism"] }
jitter = { amplitude = 0.01, targets = ["leaf_cards"] }
stagger = "per_branch"
variance = { amount = 1, steady = 0, keep_size = false }
"#,
        ))
        .unwrap();
        let living = spec.living.as_ref().unwrap();
        assert_eq!(living.lineage_seed, 7);
        assert_eq!(living.pose_ring, 12);
        assert_eq!(living.drift.amplitude, 0.2);
        assert_eq!(living.drift.params.len(), 2);
        assert_eq!(living.jitter.amplitude, 0.01);
        assert_eq!(living.stagger, Stagger::PerBranch);
        assert_eq!(living.variance.amount, 1.0);
        assert_eq!(living.variance.steady, 0.0);
        assert!(!living.variance.keep_size);

        // It survives the round trip every save and content hash takes.
        let again = ProcGenSpec::parse(&toml::to_string(&spec).unwrap()).unwrap();
        assert_eq!(spec, again);
    }

    #[test]
    fn living_reads_and_checks_step_and_glide() {
        let table = "lineage_seed = 3\nstep = 0.25\nglide = 0.8";
        let spec = ProcGenSpec::parse(&with_living(table)).unwrap();
        let living = spec.living.as_ref().unwrap();
        assert_eq!((living.step, living.glide), (0.25, 0.8));

        for (table, says) in [
            ("lineage_seed = 3\nstep = 0", "living.step"),
            ("lineage_seed = 3\nstep = -1", "living.step"),
            ("lineage_seed = 3\nglide = 1.5", "living.glide"),
            ("lineage_seed = 3\nglide = -0.1", "living.glide"),
        ] {
            let error = ProcGenSpec::parse(&with_living(table))
                .unwrap_err()
                .to_string();
            assert!(error.contains(says), "{table}: {error}");
        }
    }

    #[test]
    fn living_fills_in_defaults_from_the_lineage_alone() {
        let spec = ProcGenSpec::parse(&with_living("lineage_seed = 3")).unwrap();
        let living = spec.living.unwrap();
        assert_eq!(living.pose_ring, 8);
        assert_eq!(living.step, 0.5);
        assert_eq!(living.glide, 0.3);
        assert_eq!(living.stagger, Stagger::PerLevel);
        assert_eq!(living.drift, DriftConfig::default());
        assert_eq!(living.jitter, JitterConfig::default());
        assert_eq!(
            living.variance,
            VarianceConfig {
                amount: 0.35,
                steady: 0.8,
                keep_size: true
            }
        );
    }

    #[test]
    fn a_partial_nested_table_keeps_its_other_defaults() {
        let spec = ProcGenSpec::parse(&with_living(
            "lineage_seed = 3\nvariance = { amount = 0.5 }",
        ))
        .unwrap();
        let variance = spec.living.unwrap().variance;
        assert_eq!(variance.amount, 0.5);
        assert_eq!(variance.steady, 0.8);
    }

    #[test]
    fn malformed_living_names_the_bad_key() {
        for (table, key) in [
            ("pose_ring = 8", "lineage_seed"),
            ("lineage_seed = -1", "lineage_seed"),
            ("lineage_seed = 7\npose_rng = 8", "pose_rng"),
            ("lineage_seed = 7\npose_ring = \"eight\"", "pose_ring"),
            ("lineage_seed = 7\npose_ring = 1", "living.pose_ring"),
            (
                "lineage_seed = 7\ndrift = { amplitude = 0.1, parms = [] }",
                "parms",
            ),
            (
                "lineage_seed = 7\ndrift = { amplitude = -0.1 }",
                "living.drift.amplitude",
            ),
            (
                "lineage_seed = 7\ndrift = { params = [\"branch_angel\"] }",
                "branch_angel",
            ),
            (
                "lineage_seed = 7\njitter = { amplitude = -1 }",
                "living.jitter.amplitude",
            ),
            (
                "lineage_seed = 7\njitter = { targets = [\"bark\"] }",
                "living.jitter.targets",
            ),
            ("lineage_seed = 7\nstagger = \"per_twig\"", "stagger"),
            (
                "lineage_seed = 7\nvariance = { amount = 1.5 }",
                "living.variance.amount",
            ),
            (
                "lineage_seed = 7\nvariance = { steady = -0.5 }",
                "living.variance.steady",
            ),
            (
                "lineage_seed = 7\nvariance = { keep_size = 1 }",
                "keep_size",
            ),
        ] {
            let message = ProcGenSpec::parse(&with_living(table))
                .expect_err(table)
                .to_string();
            assert!(message.contains(key), "{table:?} gave: {message}");
        }
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
