//! In-memory LRU cache for procedural generation outputs.
//!
//! Keyed by `(spec_content_hash, seed)` so repeated requests for the same
//! spec+seed skip regeneration. Uses simple linear-scan LRU eviction, which
//! is fine for the expected cache sizes (dozens of entries).
//!
//! Outputs are held behind an [`Arc`]: a hit hands out a second reference to
//! the same mesh or image instead of copying megabytes of vertices.
//!
//! Recency is a per-cache access counter, not a clock. Two accesses never
//! tie (a browser's coarsened `performance.now()` can return the same
//! instant for a whole frame's worth of lookups), and the eviction order is
//! the same on every run.
//!
//! Under the memory there can be a disk tier ([`DiskCache`], #181), set by
//! [`ProcGenCacheConfig::disk`]: a memory miss reads it before the caller
//! generates, and every insert writes through to it, so the next run of the
//! player starts with what this one made.

use std::collections::HashMap;
use std::sync::Arc;

use flint_core::ContentHash;

use crate::disk_cache::{DiskCache, DiskCacheConfig, DiskStats};
use crate::output::GeneratorOutput;
use crate::Seed;

/// Configuration for the procgen cache.
#[derive(Debug, Clone)]
pub struct ProcGenCacheConfig {
    /// Maximum total memory budget in bytes (default: 256 MB).
    pub max_memory_bytes: usize,
    /// Whether caching is enabled.
    pub enabled: bool,
    /// Maximum milliseconds per frame to spend on queued procgen generation (default: 4.0).
    pub runtime_max_generation_ms_per_frame: f64,
    /// The disk tier under the memory, if any (default: none). Ignored on
    /// the web, and while `enabled` is false.
    pub disk: Option<DiskCacheConfig>,
}

impl Default for ProcGenCacheConfig {
    fn default() -> Self {
        Self {
            max_memory_bytes: 256 * 1024 * 1024,
            enabled: true,
            runtime_max_generation_ms_per_frame: 4.0,
            disk: None,
        }
    }
}

/// Cache performance statistics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Number of entries currently in the cache.
    pub entry_count: usize,
    /// Total estimated bytes of all cached entries.
    pub total_bytes: usize,
    /// Maximum memory budget.
    pub max_memory_bytes: usize,
    /// Number of cache hits since creation/last clear.
    pub hits: u64,
    /// Number of lookups that missed the memory and were read from the disk
    /// tier instead (counted in neither `hits` nor `misses`).
    pub disk_hits: u64,
    /// Number of lookups found in neither tier: the caller generates.
    pub misses: u64,
    /// Number of entries evicted to stay under budget since creation/last clear.
    pub evictions: u64,
}

/// A cached generator output with metadata for LRU tracking.
struct CachedAsset {
    output: Arc<GeneratorOutput>,
    byte_size: usize,
    /// Value of the cache's access counter when this entry was last touched.
    last_access: u64,
}

/// In-memory LRU cache for procedural generation outputs.
///
/// Keyed by `(ContentHash, Seed)` — the content hash of the spec file and
/// the generation seed. Evicts least-recently-used entries when the memory
/// budget is exceeded.
///
/// A spec with `mode = "random"` keys as `Seed::Random`, so it is generated
/// once and then reused for the life of the cache: a random tree keeps its
/// shape when a scene is revisited, and changes on the next run.
pub struct ProcGenCache {
    entries: HashMap<(ContentHash, Seed), CachedAsset>,
    total_bytes: usize,
    config: ProcGenCacheConfig,
    /// Monotonic access counter; each get/insert takes the next value.
    tick: u64,
    hits: u64,
    disk_hits: u64,
    misses: u64,
    evictions: u64,
    disk: Option<DiskCache>,
}

impl ProcGenCache {
    /// Create a new cache with the given configuration.
    pub fn new(config: ProcGenCacheConfig) -> Self {
        let disk = config.disk.clone().and_then(DiskCache::open);
        Self {
            entries: HashMap::new(),
            total_bytes: 0,
            config,
            tick: 0,
            hits: 0,
            disk_hits: 0,
            misses: 0,
            evictions: 0,
            disk,
        }
    }

    /// What the disk tier has done, if there is one.
    pub fn disk_stats(&self) -> Option<DiskStats> {
        self.disk.as_ref().map(DiskCache::stats)
    }

    /// The configuration this cache was built with.
    pub fn config(&self) -> &ProcGenCacheConfig {
        &self.config
    }

    fn next_tick(&mut self) -> u64 {
        self.tick += 1;
        self.tick
    }

    /// Look up a cached output by spec hash and seed.
    ///
    /// Counts a hit or a miss and, on a hit, marks the entry most recently
    /// used. A memory miss reads the disk tier, if there is one, and a file
    /// found there joins the memory (counted as a disk hit). Returns `None`
    /// on miss or if the cache is disabled.
    pub fn get(&mut self, spec_hash: ContentHash, seed: &Seed) -> Option<Arc<GeneratorOutput>> {
        if !self.config.enabled {
            self.misses += 1;
            return None;
        }
        let tick = self.next_tick();
        if let Some(entry) = self.entries.get_mut(&(spec_hash, seed.clone())) {
            entry.last_access = tick;
            self.hits += 1;
            return Some(Arc::clone(&entry.output));
        }
        if let Some(output) = self.disk.as_mut().and_then(|d| d.get(spec_hash, seed)) {
            self.disk_hits += 1;
            return Some(self.insert_memory(spec_hash, seed, Arc::new(output)));
        }
        self.misses += 1;
        None
    }

    /// Whether an output is cached for this key, without counting a hit or a
    /// miss and without changing its LRU position. For "should I queue
    /// this?" probes, which are not uses of the asset.
    pub fn contains(&self, spec_hash: ContentHash, seed: &Seed) -> bool {
        self.config.enabled && self.entries.contains_key(&(spec_hash, seed.clone()))
    }

    /// Insert a generator output into the cache and return the shared handle
    /// to it (the same `Arc` a later [`get`](Self::get) returns).
    ///
    /// Evicts LRU entries if the memory budget would be exceeded. Entries
    /// individually larger than the whole budget, and everything while the
    /// cache is disabled, are passed back without being stored. Writes
    /// through to the disk tier, if there is one (which keeps its own
    /// budget, and never a `Seed::Random` output).
    pub fn insert(
        &mut self,
        spec_hash: ContentHash,
        seed: &Seed,
        output: impl Into<Arc<GeneratorOutput>>,
    ) -> Arc<GeneratorOutput> {
        let output = output.into();
        if !self.config.enabled {
            return output;
        }
        if let Some(disk) = self.disk.as_mut() {
            disk.put(spec_hash, seed, &output);
        }
        self.insert_memory(spec_hash, seed, output)
    }

    /// The memory half of [`insert`](Self::insert).
    fn insert_memory(
        &mut self,
        spec_hash: ContentHash,
        seed: &Seed,
        output: Arc<GeneratorOutput>,
    ) -> Arc<GeneratorOutput> {
        let byte_size = output.estimated_bytes();

        // Don't cache entries larger than the entire budget.
        if byte_size > self.config.max_memory_bytes {
            return output;
        }

        let key = (spec_hash, seed.clone());

        // If replacing an existing entry, subtract its old size first.
        if let Some(old) = self.entries.remove(&key) {
            self.total_bytes -= old.byte_size;
        }

        // Evict LRU entries until we have room.
        while self.total_bytes + byte_size > self.config.max_memory_bytes {
            let Some(lru_key) = self.find_lru_key() else {
                break;
            };
            let evicted = self.entries.remove(&lru_key).unwrap();
            self.total_bytes -= evicted.byte_size;
            self.evictions += 1;
        }

        self.total_bytes += byte_size;
        let last_access = self.next_tick();
        self.entries.insert(
            key,
            CachedAsset {
                output: Arc::clone(&output),
                byte_size,
                last_access,
            },
        );
        output
    }

    /// Remove all entries from memory and reset counters. The disk tier
    /// keeps its files.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
        self.hits = 0;
        self.disk_hits = 0;
        self.misses = 0;
        self.evictions = 0;
    }

    /// Current cache statistics.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entry_count: self.entries.len(),
            total_bytes: self.total_bytes,
            max_memory_bytes: self.config.max_memory_bytes,
            hits: self.hits,
            disk_hits: self.disk_hits,
            misses: self.misses,
            evictions: self.evictions,
        }
    }

    /// Find the key of the least-recently-used entry.
    fn find_lru_key(&self) -> Option<(ContentHash, Seed)> {
        self.entries
            .iter()
            .min_by_key(|(_, v)| v.last_access)
            .map(|(k, _)| k.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ChannelSemantics, ImageData};

    fn small_image(size: u32) -> GeneratorOutput {
        GeneratorOutput::Image(ImageData::new(size, size, ChannelSemantics::Color))
    }

    fn hash(n: u8) -> ContentHash {
        ContentHash::from_bytes(&[n])
    }

    fn seed(n: u64) -> Seed {
        Seed::Fixed(n)
    }

    #[test]
    fn hit_and_miss() {
        let mut cache = ProcGenCache::new(ProcGenCacheConfig::default());
        let s = seed(42);
        let h = hash(1);

        // Miss on empty cache
        assert!(cache.get(h, &s).is_none());
        assert_eq!(cache.stats().misses, 1);

        // Insert and hit
        cache.insert(h, &s, small_image(2));
        assert!(cache.get(h, &s).is_some());
        assert_eq!(cache.stats().hits, 1);
        assert_eq!(cache.stats().misses, 1);

        // Miss on different key
        assert!(cache.get(hash(2), &s).is_none());
        assert_eq!(cache.stats().misses, 2);
    }

    #[test]
    fn lru_eviction_order() {
        // Budget fits 2 small images (2x2 = 16 bytes each, budget = 32)
        let config = ProcGenCacheConfig {
            max_memory_bytes: 32,
            ..Default::default()
        };
        let mut cache = ProcGenCache::new(config);
        let s = seed(1);

        cache.insert(hash(1), &s, small_image(2)); // 16 bytes
        cache.insert(hash(2), &s, small_image(2)); // 16 bytes

        // Access hash(1) to make it more recent
        assert!(cache.get(hash(1), &s).is_some());

        // Insert a third — should evict hash(2) (least recently used)
        cache.insert(hash(3), &s, small_image(2));

        assert!(cache.get(hash(1), &s).is_some()); // still there
        assert!(cache.get(hash(2), &s).is_none()); // evicted
        assert!(cache.get(hash(3), &s).is_some()); // just inserted
    }

    #[test]
    fn memory_budget_enforcement() {
        // Budget fits exactly 1 image of 4x4 = 64 bytes
        let config = ProcGenCacheConfig {
            max_memory_bytes: 64,
            ..Default::default()
        };
        let mut cache = ProcGenCache::new(config);
        let s = seed(1);

        cache.insert(hash(1), &s, small_image(4)); // 64 bytes
        assert_eq!(cache.stats().entry_count, 1);
        assert_eq!(cache.stats().total_bytes, 64);

        // Insert second — evicts first
        cache.insert(hash(2), &s, small_image(4));
        assert_eq!(cache.stats().entry_count, 1);
        assert_eq!(cache.stats().total_bytes, 64);
        assert!(cache.get(hash(1), &s).is_none());
        assert!(cache.get(hash(2), &s).is_some());
    }

    #[test]
    fn oversized_entry_not_cached() {
        let config = ProcGenCacheConfig {
            max_memory_bytes: 100,
            ..Default::default()
        };
        let mut cache = ProcGenCache::new(config);
        let s = seed(1);

        // 16x16 image = 1024 bytes, way over budget
        cache.insert(hash(1), &s, small_image(16));
        assert_eq!(cache.stats().entry_count, 0);
        assert_eq!(cache.stats().total_bytes, 0);
    }

    #[test]
    fn stats_accuracy() {
        let mut cache = ProcGenCache::new(ProcGenCacheConfig::default());
        let s = seed(1);

        cache.insert(hash(1), &s, small_image(2)); // 16 bytes
        cache.insert(hash(2), &s, small_image(4)); // 64 bytes

        let stats = cache.stats();
        assert_eq!(stats.entry_count, 2);
        assert_eq!(stats.total_bytes, 16 + 64);
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);

        cache.get(hash(1), &s); // hit
        cache.get(hash(99), &s); // miss

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
    }

    #[test]
    fn clear_resets_everything() {
        let mut cache = ProcGenCache::new(ProcGenCacheConfig::default());
        let s = seed(1);

        cache.insert(hash(1), &s, small_image(2));
        cache.get(hash(1), &s);
        cache.get(hash(2), &s);
        cache.clear();

        let stats = cache.stats();
        assert_eq!(stats.entry_count, 0);
        assert_eq!(stats.total_bytes, 0);
        assert_eq!(stats.hits, 0);
        assert_eq!(stats.misses, 0);
        assert!(cache.get(hash(1), &s).is_none());
    }

    #[test]
    fn disabled_cache() {
        let config = ProcGenCacheConfig {
            enabled: false,
            ..Default::default()
        };
        let mut cache = ProcGenCache::new(config);
        let s = seed(1);

        cache.insert(hash(1), &s, small_image(2));
        assert_eq!(cache.stats().entry_count, 0);
        assert!(cache.get(hash(1), &s).is_none());
        assert_eq!(cache.stats().misses, 1);
    }

    #[test]
    fn replace_existing_key() {
        let mut cache = ProcGenCache::new(ProcGenCacheConfig::default());
        let s = seed(1);
        let h = hash(1);

        cache.insert(h, &s, small_image(2)); // 16 bytes
        assert_eq!(cache.stats().entry_count, 1);
        assert_eq!(cache.stats().total_bytes, 16);

        cache.insert(h, &s, small_image(4)); // 64 bytes, same key
        assert_eq!(cache.stats().entry_count, 1);
        assert_eq!(cache.stats().total_bytes, 64);
    }

    #[test]
    fn ticket_lru_scenario_evicts_second() {
        // Insert 3, touch the first, insert a 4th that needs one slot:
        // the second (least recently used) goes, not the first (oldest).
        let mut cache = ProcGenCache::new(ProcGenCacheConfig {
            max_memory_bytes: 48,
            ..Default::default()
        });
        let s = seed(1);
        for n in 1..=3 {
            cache.insert(hash(n), &s, small_image(2)); // 16 bytes each
        }
        assert!(cache.get(hash(1), &s).is_some());
        cache.insert(hash(4), &s, small_image(2));

        assert!(cache.contains(hash(1), &s));
        assert!(!cache.contains(hash(2), &s));
        assert!(cache.contains(hash(3), &s));
        assert!(cache.contains(hash(4), &s));
        assert_eq!(cache.stats().evictions, 1);
    }

    #[test]
    fn budget_holds_across_many_inserts() {
        let mut cache = ProcGenCache::new(ProcGenCacheConfig {
            max_memory_bytes: 1000,
            ..Default::default()
        });
        let s = seed(1);
        // Mixed sizes: 16, 64, 144, 256 bytes.
        for n in 0..64u8 {
            cache.insert(hash(n), &s, small_image(2 + 2 * (n as u32 % 4)));
            let stats = cache.stats();
            assert!(stats.total_bytes <= 1000, "over budget after insert {n}");
        }
        let stats = cache.stats();
        assert!(stats.evictions > 0);
        assert!(stats.entry_count > 1);
    }

    #[test]
    fn lru_order_never_ties() {
        // Thousands of accesses inside one clock tick must still order
        // strictly: recency is a counter, not a timestamp.
        let mut cache = ProcGenCache::new(ProcGenCacheConfig {
            max_memory_bytes: 16 * 8,
            ..Default::default()
        });
        let s = seed(1);
        for n in 0..8 {
            cache.insert(hash(n), &s, small_image(2));
        }
        // Touch everything but #5, in reverse.
        for n in (0..8).rev().filter(|n| *n != 5) {
            cache.get(hash(n), &s);
        }
        cache.insert(hash(100), &s, small_image(2));
        assert!(!cache.contains(hash(5), &s));
        assert_eq!(cache.stats().entry_count, 8);
    }

    #[test]
    fn contains_is_not_a_use() {
        let mut cache = ProcGenCache::new(ProcGenCacheConfig {
            max_memory_bytes: 32,
            ..Default::default()
        });
        let s = seed(1);
        cache.insert(hash(1), &s, small_image(2));
        cache.insert(hash(2), &s, small_image(2));

        // Probing #1 leaves it least recently used, and counts nothing.
        assert!(cache.contains(hash(1), &s));
        assert!(!cache.contains(hash(9), &s));
        assert_eq!(cache.stats().hits, 0);
        assert_eq!(cache.stats().misses, 0);

        cache.insert(hash(3), &s, small_image(2));
        assert!(!cache.contains(hash(1), &s));
        assert!(cache.contains(hash(2), &s));
    }

    #[test]
    fn hits_share_one_allocation() {
        let mut cache = ProcGenCache::new(ProcGenCacheConfig::default());
        let s = seed(1);
        let inserted = cache.insert(hash(1), &s, small_image(64));
        let a = cache.get(hash(1), &s).unwrap();
        let b = cache.get(hash(1), &s).unwrap();
        assert!(Arc::ptr_eq(&inserted, &a));
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(*a, small_image(64));
    }

    #[test]
    fn disabled_insert_hands_output_back() {
        let mut cache = ProcGenCache::new(ProcGenCacheConfig {
            enabled: false,
            ..Default::default()
        });
        let out = cache.insert(hash(1), &seed(1), small_image(2));
        assert_eq!(*out, small_image(2));
        assert!(!cache.contains(hash(1), &seed(1)));
    }
}
