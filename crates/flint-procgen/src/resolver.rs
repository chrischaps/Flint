//! Runtime resolver — discovers procgen specs and generates assets on demand.
//!
//! The [`ProcGenResolver`] indexes `*.procgen.toml` files by their `meta.name`
//! and provides cached generation through the [`ProcGenCache`].

use std::collections::HashMap;
use std::path::Path;

use crate::cache::{ProcGenCache, ProcGenCacheConfig};
use crate::output::GeneratorOutput;
use crate::queue::{AssetKind, CompletedAsset, GenerationRequest, ProcGenQueue, QueueStats};
use crate::registry::GeneratorRegistry;
use crate::spec::ProcGenSpec;
use crate::{register_built_in_generators, CacheStats, ProcGenError, Result};
use flint_core::ContentHash;

/// Discovers procgen specs by name and generates assets with caching.
pub struct ProcGenResolver {
    /// Indexed specs: `meta.name` → spec
    specs: HashMap<String, ProcGenSpec>,
    /// Generator registry with all built-in generators
    registry: GeneratorRegistry,
    /// In-memory LRU cache for generated outputs
    cache: ProcGenCache,
    /// Frame-budgeted generation queue
    queue: ProcGenQueue,
}

impl ProcGenResolver {
    /// Create a new resolver with default cache config and built-in generators.
    pub fn new() -> Self {
        let config = ProcGenCacheConfig::default();
        let budget_ms = config.runtime_max_generation_ms_per_frame;
        let mut registry = GeneratorRegistry::new();
        register_built_in_generators(&mut registry);
        // Also register mock generator for testing/validation
        registry.register(Box::new(crate::mock::MockGenerator));
        Self {
            specs: HashMap::new(),
            registry,
            cache: ProcGenCache::new(config),
            queue: ProcGenQueue::new(budget_ms),
        }
    }

    /// Scan directories for `*.procgen.toml` files and index by `meta.name`.
    ///
    /// Later directories override earlier ones when specs share the same name.
    pub fn discover_and_index(&mut self, dirs: &[&Path]) {
        for dir in dirs {
            if let Ok(found) = crate::spec::discover_specs(dir) {
                for spec in found {
                    self.specs.insert(spec.meta.name.clone(), spec);
                }
            }
        }
    }

    /// Check whether a spec with the given name is indexed.
    pub fn has_spec(&self, name: &str) -> bool {
        self.specs.contains_key(name)
    }

    /// Number of indexed specs.
    pub fn spec_count(&self) -> usize {
        self.specs.len()
    }

    /// Generate (or retrieve from cache) the output for the named spec.
    pub fn generate(&mut self, name: &str) -> Result<GeneratorOutput> {
        let spec = self
            .specs
            .get(name)
            .ok_or_else(|| ProcGenError::UnknownGenerator(format!("no spec named '{name}'")))?
            .clone();

        let seed = spec.seed.to_seed()?;
        let content_hash = ContentHash::from_str(&toml::to_string(&spec).unwrap_or_default());

        // Check cache first
        if let Some(cached) = self.cache.get(content_hash, &seed) {
            return Ok(cached.clone());
        }

        // Generate
        let output = self.registry.generate_from_spec(&spec)?;

        // Cache the result
        self.cache.insert(content_hash, &seed, output.clone());

        Ok(output)
    }

    /// Current cache statistics.
    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// Enqueue a named spec for frame-budgeted generation.
    ///
    /// Returns `true` if the spec was enqueued (not already cached or pending).
    /// Returns `false` if the spec doesn't exist, is already cached, or is
    /// already pending.
    pub fn enqueue(
        &mut self,
        name: &str,
        entity_position: [f32; 3],
        asset_kind: AssetKind,
    ) -> bool {
        let spec = match self.specs.get(name) {
            Some(s) => s.clone(),
            None => return false,
        };

        let seed = match spec.seed.to_seed() {
            Ok(s) => s,
            Err(_) => return false,
        };
        let content_hash = ContentHash::from_str(&toml::to_string(&spec).unwrap_or_default());

        // Already cached — no need to queue
        if self.cache.get(content_hash, &seed).is_some() {
            return false;
        }

        // Already pending
        if self.queue.is_pending(name) {
            return false;
        }

        let dx = entity_position[0];
        let dy = entity_position[1];
        let dz = entity_position[2];
        let priority = dx * dx + dy * dy + dz * dz;

        self.queue.enqueue(GenerationRequest {
            spec_name: name.to_string(),
            spec,
            content_hash,
            seed,
            priority,
            entity_position,
            asset_kind,
        });
        true
    }

    /// Process the generation queue for one frame within the time budget.
    pub fn process_frame(&mut self, camera_pos: [f32; 3]) -> Vec<CompletedAsset> {
        self.queue
            .process_frame(camera_pos, &self.registry, &mut self.cache)
    }

    /// Check if a spec name is pending in the queue.
    pub fn is_pending(&self, name: &str) -> bool {
        self.queue.is_pending(name)
    }

    /// Check if a spec name has a cached output.
    pub fn is_cached(&mut self, name: &str) -> bool {
        let spec = match self.specs.get(name) {
            Some(s) => s,
            None => return false,
        };
        let seed = match spec.seed.to_seed() {
            Ok(s) => s,
            Err(_) => return false,
        };
        let content_hash = ContentHash::from_str(&toml::to_string(spec).unwrap_or_default());
        self.cache.get(content_hash, &seed).is_some()
    }

    /// Current queue statistics.
    pub fn queue_stats(&self) -> QueueStats {
        self.queue.stats()
    }

    /// Clear all pending items from the queue.
    pub fn clear_queue(&mut self) {
        self.queue.clear();
    }
}

impl Default for ProcGenResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const MOCK_SPEC: &str = r#"
generator = "mock"

[meta]
name = "test_cube"
version = "1.0.0"

[seed]
mode = "fixed"
value = 42
"#;

    fn temp_dir(suffix: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("flint_resolver_test_{suffix}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn discover_specs_by_name() {
        let dir = temp_dir("discover");
        fs::write(dir.join("cube.procgen.toml"), MOCK_SPEC).unwrap();

        let spec2 = MOCK_SPEC.replace("test_cube", "test_sphere");
        fs::write(dir.join("sphere.procgen.toml"), &spec2).unwrap();

        // Non-spec file should be ignored
        fs::write(dir.join("readme.txt"), "hello").unwrap();

        let mut resolver = ProcGenResolver::new();
        resolver.discover_and_index(&[dir.as_path()]);

        assert_eq!(resolver.spec_count(), 2);
        assert!(resolver.has_spec("test_cube"));
        assert!(resolver.has_spec("test_sphere"));
        assert!(!resolver.has_spec("nonexistent"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn generate_and_cache() {
        let dir = temp_dir("gen_cache");
        fs::write(dir.join("cube.procgen.toml"), MOCK_SPEC).unwrap();

        let mut resolver = ProcGenResolver::new();
        resolver.discover_and_index(&[dir.as_path()]);

        // First generation — cache miss
        let output1 = resolver.generate("test_cube").unwrap();
        assert_eq!(resolver.cache_stats().misses, 1);
        assert_eq!(resolver.cache_stats().hits, 0);

        // Second generation — cache hit
        let output2 = resolver.generate("test_cube").unwrap();
        assert_eq!(resolver.cache_stats().hits, 1);
        assert_eq!(output1, output2);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_spec_returns_err() {
        let mut resolver = ProcGenResolver::new();
        assert!(resolver.generate("nonexistent").is_err());
    }

    #[test]
    fn later_dir_overrides_earlier() {
        let dir_a = temp_dir("override_a");
        let dir_b = temp_dir("override_b");

        // Both dirs have a spec named "shared_name" but with different seeds
        let spec_a = r#"
generator = "mock"
[meta]
name = "shared_name"
version = "1.0.0"
[seed]
mode = "fixed"
value = 1
"#;
        let spec_b = r#"
generator = "mock"
[meta]
name = "shared_name"
version = "2.0.0"
[seed]
mode = "fixed"
value = 999
"#;
        fs::write(dir_a.join("a.procgen.toml"), spec_a).unwrap();
        fs::write(dir_b.join("b.procgen.toml"), spec_b).unwrap();

        let mut resolver = ProcGenResolver::new();
        resolver.discover_and_index(&[dir_a.as_path(), dir_b.as_path()]);

        assert_eq!(resolver.spec_count(), 1);
        assert!(resolver.has_spec("shared_name"));

        // The later dir (dir_b) should win — version "2.0.0"
        let spec = resolver.specs.get("shared_name").unwrap();
        assert_eq!(spec.meta.version, "2.0.0");

        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }
}
