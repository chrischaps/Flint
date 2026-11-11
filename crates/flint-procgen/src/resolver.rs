//! Runtime resolver — discovers procgen specs and generates assets on demand.
//!
//! The [`ProcGenResolver`] indexes `*.procgen.toml` files by their `meta.name`
//! and provides cached generation through the [`ProcGenCache`].

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use crate::cache::{ProcGenCache, ProcGenCacheConfig};
use crate::disk_cache::DiskStats;
use crate::living_props::{
    LineageDone, LivingDefaults, LivingPropId, LivingPropRequest, LivingProps, POSE_CACHE_BYTES,
};
use crate::output::GeneratorOutput;
use crate::queue::{AssetKind, CompletedAsset, GenerationRequest, ProcGenQueue, QueueStats};
use crate::registry::GeneratorRegistry;
use crate::spec::ProcGenSpec;
use crate::{register_built_in_generators, CacheStats, ProcGenError, Result, Seed};
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
    /// The scene's living props, which share the queue's frame budget
    living: LivingProps,
}

impl ProcGenResolver {
    /// Create a new resolver with default cache config and built-in generators.
    pub fn new() -> Self {
        Self::with_config(ProcGenCacheConfig::default())
    }

    /// Create a resolver whose cache and queue use `config`, with the
    /// built-in generators (and `mock`, for tests and validation).
    pub fn with_config(config: ProcGenCacheConfig) -> Self {
        let mut registry = GeneratorRegistry::new();
        register_built_in_generators(&mut registry);
        registry.register(Box::new(crate::mock::MockGenerator));
        Self::with_registry(config, registry)
    }

    /// Create a resolver over a registry built elsewhere, used as it is: no
    /// built-in is added, so call [`register_built_in_generators`] on it
    /// first to keep them. This is how a resolver is rebuilt with another
    /// cache without losing the generators a game registered (#195):
    ///
    /// ```
    /// # use flint_procgen::{ProcGenCacheConfig, ProcGenResolver};
    /// let mut old = ProcGenResolver::new();
    /// let registry = std::mem::take(old.registry_mut());
    /// let resolver = ProcGenResolver::with_registry(ProcGenCacheConfig::default(), registry);
    /// assert!(resolver.registry().get("tree_v1").is_some());
    /// ```
    pub fn with_registry(config: ProcGenCacheConfig, registry: GeneratorRegistry) -> Self {
        let budget_ms = config.runtime_max_generation_ms_per_frame;
        Self {
            specs: HashMap::new(),
            registry,
            cache: ProcGenCache::new(config),
            queue: ProcGenQueue::new(budget_ms),
            living: LivingProps::new(POSE_CACHE_BYTES),
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

    /// Index one spec, as discovery would have.
    pub fn index_spec(&mut self, spec: ProcGenSpec) {
        self.specs.insert(spec.meta.name.clone(), spec);
    }

    /// Check whether a spec with the given name is indexed.
    pub fn has_spec(&self, name: &str) -> bool {
        self.specs.contains_key(name)
    }

    /// Number of indexed specs.
    pub fn spec_count(&self) -> usize {
        self.specs.len()
    }

    /// Whether the named spec could be generated at all: it is indexed and
    /// its generator is registered. `Err` says why not, without generating.
    pub fn check_generator(&self, name: &str) -> std::result::Result<(), String> {
        let spec = self
            .specs
            .get(name)
            .ok_or_else(|| format!("no spec named '{name}'"))?;
        if self.registry.get(&spec.generator).is_none() {
            let mut known = self.registry.generator_names();
            known.sort_unstable();
            return Err(format!(
                "its generator '{}' is not registered (known: {})",
                spec.generator,
                known.join(", ")
            ));
        }
        Ok(())
    }

    /// Register another generator alongside the built-ins (a game's own, or
    /// a test's). One with a built-in's type name replaces it.
    pub fn register_generator(&mut self, generator: Box<dyn crate::Generator>) {
        self.registry.register(generator);
    }

    /// The generators specs are resolved with.
    pub fn registry(&self) -> &GeneratorRegistry {
        &self.registry
    }

    /// The generators specs are resolved with, to register more: a game's
    /// own (#195), or a test's. A spec names its generator by type name, so
    /// one added here serves every spec naming it from the next request on.
    /// Replacing a generator under the same name keeps what was cached
    /// unless the replacement's [revision](crate::Generator::revision)
    /// differs, since the cache key holds the name and the revision.
    pub fn registry_mut(&mut self) -> &mut GeneratorRegistry {
        &mut self.registry
    }

    /// Queued requests that failed to generate since the last call. Each is
    /// reported once; the caller decides what stands in for it.
    pub fn take_failures(&mut self) -> Vec<crate::FailedAsset> {
        self.queue.take_failures()
    }

    /// The cache key for a spec: a hash of its (normalised) TOML and of the
    /// generator that builds it, name and [revision](crate::Generator::revision),
    /// with its seed. The revision is what lets the disk tier survive a
    /// generator change: bump it and every spec that generator builds misses.
    ///
    /// A spec whose generator isn't registered still gets a key (its
    /// generator's name, no revision) so it can be queued and its failure
    /// reported through [`take_failures`](Self::take_failures); nothing is
    /// ever stored under it, since generating it fails.
    fn cache_key(registry: &GeneratorRegistry, spec: &ProcGenSpec) -> Result<(ContentHash, Seed)> {
        let seed = spec.seed.to_seed()?;
        let generator = match registry.get(&spec.generator) {
            Some(g) => format!("{}@{}", g.type_name(), g.revision()),
            None => format!("{}@unregistered", spec.generator),
        };
        let keyed = format!("{generator}\n{}", toml::to_string(spec).unwrap_or_default());
        Ok((ContentHash::from_str(&keyed), seed))
    }

    /// Generate (or retrieve from cache) the output for the named spec.
    ///
    /// A hit returns the cached output itself, shared: no copy is made.
    pub fn generate(&mut self, name: &str) -> Result<Arc<GeneratorOutput>> {
        let spec = self
            .specs
            .get(name)
            .ok_or_else(|| ProcGenError::UnknownGenerator(format!("no spec named '{name}'")))?;
        let (content_hash, seed) = Self::cache_key(&self.registry, spec)?;

        if let Some(cached) = self.cache.get(content_hash, &seed) {
            return Ok(cached);
        }

        let output = self.registry.generate_from_spec(spec)?;
        Ok(self.cache.insert(content_hash, &seed, output))
    }

    /// Current cache statistics.
    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// What the cache's disk tier has done, if it has one.
    pub fn disk_stats(&self) -> Option<DiskStats> {
        self.cache.disk_stats()
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

        let Ok((content_hash, seed)) = Self::cache_key(&self.registry, &spec) else {
            return false;
        };

        // Already cached — no need to queue. A probe, not a use: the queue's
        // own lookup counts the hit when it hands the asset out.
        if self.cache.contains(content_hash, &seed) {
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

    /// Check if a spec name has a cached output. Does not count as a hit or
    /// a miss, or change what the cache evicts next.
    pub fn is_cached(&self, name: &str) -> bool {
        self.specs
            .get(name)
            .and_then(|spec| Self::cache_key(&self.registry, spec).ok())
            .is_some_and(|(hash, seed)| self.cache.contains(hash, &seed))
    }

    /// Add a living prop. Returns its id and the name of the mesh it draws, one
    /// per spec and lineage; `Err` says why its spec can't live, in which case
    /// nothing was added. Nothing is generated yet.
    pub fn add_living_prop(
        &mut self,
        request: LivingPropRequest,
    ) -> std::result::Result<(LivingPropId, String), String> {
        let name = &request.spec_name;
        let spec = self
            .specs
            .get(name)
            .ok_or_else(|| format!("no spec named '{name}'"))?;
        if spec.living.is_none() {
            return Err(format!("its spec '{name}' has no [living] table"));
        }
        let generator = self
            .registry
            .get(&spec.generator)
            .ok_or_else(|| format!("its generator '{}' is not registered", spec.generator))?;
        generator
            .as_living()
            .ok_or_else(|| format!("its generator '{}' can't live", spec.generator))?
            .check_living(spec)
            .map_err(|e| e.to_string())?;
        // What a pose depends on: the generator's revision and the whole spec.
        let identity = format!(
            "{}@{}\n{}",
            generator.type_name(),
            generator.revision(),
            toml::to_string(spec).unwrap_or_default()
        );
        Ok(self.living.add(request, spec, &identity))
    }

    /// Build every living lineage and fill every ring now, unbudgeted, for a
    /// scene load whose first frame should find them full.
    pub fn make_living_now(&mut self) -> Vec<LineageDone> {
        self.living.make_all_now(&self.registry)
    }

    /// One frame of living props: move their clock on by `dt` (0 while paused),
    /// then generate what the rings are missing, in whatever the asset queue
    /// left of the frame budget. Call after [`process_frame`](Self::process_frame).
    /// Lineages that finished come back to be uploaded. Does nothing at all in
    /// a scene with no living props.
    pub fn process_living(&mut self, dt: f64) -> Vec<LineageDone> {
        self.process_living_in(dt, None)
    }

    /// [`process_living`](Self::process_living) in a scene with a wooze field,
    /// given with the moment on its own clock to sample it at. `None` for a scene
    /// with none, in which props keep to their specs and nothing is sampled.
    pub fn process_living_in(
        &mut self,
        dt: f64,
        wooze: Option<(&flint_core::wooze::WoozeField, f64)>,
    ) -> Vec<LineageDone> {
        if self.living.is_empty() {
            return Vec::new();
        }
        let left = self.queue.budget_ms() - self.queue.stats().budget_used_ms;
        self.living.tick(dt, left, &self.registry, wooze)
    }

    /// The milliseconds a frame may spend generating, assets and poses together.
    pub fn frame_budget_ms(&self) -> f64 {
        self.queue.budget_ms()
    }

    /// One step of living props with no frame budget: every pose the rings want
    /// is there when this returns. For a capture, where nobody is waiting on the
    /// frame and a pose arriving late would change the picture (#216).
    pub fn step_living_now(
        &mut self,
        dt: f64,
        wooze: Option<(&flint_core::wooze::WoozeField, f64)>,
    ) -> Vec<LineageDone> {
        if self.living.is_empty() {
            return Vec::new();
        }
        self.living.tick(dt, f64::INFINITY, &self.registry, wooze)
    }

    /// [`process_living_in`](Self::process_living_in) for a second look at the
    /// scene's props (#238), made with [`LivingProps::fork`]: its own knobs, the
    /// same clock and frame budget, and the scene's pose cache, so a pose either
    /// side has made the other finds. Its lineages are built, so nothing comes back.
    pub fn process_living_beside(
        &mut self,
        side: &mut LivingProps,
        dt: f64,
        wooze: Option<(&flint_core::wooze::WoozeField, f64)>,
    ) {
        if side.is_empty() {
            return;
        }
        let left = self.queue.budget_ms() - self.queue.stats().budget_used_ms;
        self.living.swap_cache(side);
        side.tick(dt, left, &self.registry, wooze);
        self.living.swap_cache(side);
    }

    /// Finish what the knobs of the scene's props have started, now and with no
    /// budget: see [`LivingProps::settle_now`]. With `side`, that side's instead,
    /// on the scene's pose cache.
    pub fn settle_living_now(&mut self, side: Option<&mut LivingProps>) {
        match side {
            Some(side) => {
                self.living.swap_cache(side);
                side.settle_now(&self.registry);
                self.living.swap_cache(side);
            }
            None => self.living.settle_now(&self.registry),
        }
    }

    /// The frame's whole budget, for tests that want it tight or wide.
    #[cfg(test)]
    pub(crate) fn frame_budget_for_test(&mut self, budget_ms: f64) {
        self.queue.set_budget_ms(budget_ms);
    }

    /// The scene's living props and how their rings have fared.
    pub fn living_props(&self) -> &LivingProps {
        &self.living
    }

    /// The scene's living props, to blend them.
    pub fn living_props_mut(&mut self) -> &mut LivingProps {
        &mut self.living
    }

    /// What the scene says about living props, for the props added after this.
    pub fn set_living_defaults(&mut self, defaults: LivingDefaults) {
        self.living.set_defaults(defaults);
    }

    /// Forget the scene's living props, keeping the poses already made.
    pub fn clear_living_props(&mut self) {
        self.living.clear_props();
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
        // The hit is the cached output itself, not a copy of it.
        assert!(Arc::ptr_eq(&output1, &output2));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn probes_do_not_count_as_uses() {
        let dir = temp_dir("probes");
        fs::write(dir.join("cube.procgen.toml"), MOCK_SPEC).unwrap();

        let mut resolver = ProcGenResolver::new();
        resolver.discover_and_index(&[dir.as_path()]);

        assert!(!resolver.is_cached("test_cube"));
        resolver.generate("test_cube").unwrap(); // the one miss
        assert!(resolver.is_cached("test_cube"));
        assert!(!resolver.is_cached("nonexistent"));
        // Already cached: enqueue declines, and checking that counts nothing.
        assert!(!resolver.enqueue("test_cube", [0.0; 3], AssetKind::Model));

        let stats = resolver.cache_stats();
        assert_eq!((stats.hits, stats.misses), (0, 1));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn with_config_reaches_the_cache() {
        let resolver = ProcGenResolver::with_config(ProcGenCacheConfig {
            max_memory_bytes: 4096,
            ..Default::default()
        });
        assert_eq!(resolver.cache_stats().max_memory_bytes, 4096);
    }

    /// The mock, at whatever revision a test says.
    struct MockAt(u32);

    impl crate::Generator for MockAt {
        fn type_name(&self) -> &str {
            "mock"
        }
        fn revision(&self) -> u32 {
            self.0
        }
        fn output_kind(&self) -> crate::OutputKind {
            crate::OutputKind::Mesh
        }
        fn generate(
            &self,
            spec: &ProcGenSpec,
            rng: &mut crate::SeededRng,
        ) -> Result<GeneratorOutput> {
            crate::mock::MockGenerator.generate(spec, rng)
        }
        fn param_schema(&self) -> serde_json::Value {
            crate::mock::MockGenerator.param_schema()
        }
    }

    /// A player launch: a fresh resolver over `spec_dir`, its disk tier in
    /// `cache_dir`, the mock at `revision`.
    fn launch(spec_dir: &Path, cache_dir: &Path, revision: u32) -> ProcGenResolver {
        let mut resolver = ProcGenResolver::with_config(ProcGenCacheConfig {
            disk: Some(crate::DiskCacheConfig::new(cache_dir)),
            ..Default::default()
        });
        resolver.register_generator(Box::new(MockAt(revision)));
        resolver.discover_and_index(&[spec_dir]);
        resolver
    }

    #[test]
    fn the_second_launch_reads_the_disk_and_a_new_revision_regenerates() {
        let dir = temp_dir("disk_launches");
        fs::write(dir.join("cube.procgen.toml"), MOCK_SPEC).unwrap();
        let cache_dir = dir.join(".flint").join("procgen-cache");

        // First launch: generated, and kept.
        let mut first = launch(&dir, &cache_dir, 1);
        let made = first.generate("test_cube").unwrap();
        assert_eq!(first.cache_stats().misses, 1);
        assert_eq!(first.disk_stats().unwrap().writes, 1);

        // Second launch, same revision: read back, not generated.
        let mut second = launch(&dir, &cache_dir, 1);
        let read = second.generate("test_cube").unwrap();
        assert_eq!(*read, *made);
        let stats = second.cache_stats();
        assert_eq!((stats.misses, stats.disk_hits), (0, 1));
        // And then it's in memory: the next use is a plain hit.
        second.generate("test_cube").unwrap();
        assert_eq!(second.cache_stats().hits, 1);

        // The generator's revision moves: that launch misses and regenerates.
        let mut third = launch(&dir, &cache_dir, 2);
        third.generate("test_cube").unwrap();
        let stats = third.cache_stats();
        assert_eq!((stats.misses, stats.disk_hits), (1, 0));
        assert_eq!(third.disk_stats().unwrap().writes, 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_random_spec_is_regenerated_each_launch() {
        let dir = temp_dir("disk_random");
        let spec = MOCK_SPEC.replace(
            "mode = \"fixed\"
value = 42",
            "mode = \"random\"",
        );
        assert!(spec.contains("random"));
        fs::write(dir.join("cube.procgen.toml"), spec).unwrap();
        let cache_dir = dir.join("cache");

        for _ in 0..2 {
            let mut resolver = launch(&dir, &cache_dir, 1);
            resolver.generate("test_cube").unwrap();
            assert_eq!(resolver.cache_stats().misses, 1);
            assert_eq!(resolver.disk_stats().unwrap().writes, 0);
        }
        assert!(!cache_dir.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn check_generator_names_what_is_missing() {
        let dir = temp_dir("check_generator");
        fs::write(dir.join("cube.procgen.toml"), MOCK_SPEC).unwrap();
        let rock = MOCK_SPEC
            .replace("test_cube", "rock")
            .replace("\"mock\"", "\"boulder_v9\"");
        fs::write(dir.join("rock.procgen.toml"), rock).unwrap();

        let mut resolver = ProcGenResolver::new();
        resolver.discover_and_index(&[dir.as_path()]);

        assert_eq!(resolver.check_generator("test_cube"), Ok(()));
        let err = resolver.check_generator("rock").unwrap_err();
        assert!(err.contains("'boulder_v9' is not registered"), "{err}");
        assert!(err.contains("mock, texture_v1"), "sorted known list: {err}");
        assert!(resolver.check_generator("nonexistent").is_err());
        // Checking generates nothing.
        assert_eq!(resolver.cache_stats().misses, 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn queued_failures_are_reported_once() {
        let dir = temp_dir("queued_failures");
        let bad = MOCK_SPEC.replace("\"mock\"", "\"boulder_v9\"");
        fs::write(dir.join("cube.procgen.toml"), bad).unwrap();

        let mut resolver = ProcGenResolver::new();
        resolver.discover_and_index(&[dir.as_path()]);
        assert!(resolver.enqueue("test_cube", [0.0; 3], AssetKind::Texture));
        assert!(resolver.process_frame([0.0; 3]).is_empty());

        let failures = resolver.take_failures();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].spec_name, "test_cube");
        assert_eq!(failures[0].asset_kind, AssetKind::Texture);
        assert!(
            failures[0].error.contains("boulder_v9"),
            "{}",
            failures[0].error
        );
        assert!(resolver.take_failures().is_empty());

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
