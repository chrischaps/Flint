//! In-memory LRU cache for procedural generation outputs.
//!
//! Keyed by `(spec_content_hash, seed)` so repeated requests for the same
//! spec+seed skip regeneration. Uses simple linear-scan LRU eviction, which
//! is fine for the expected cache sizes (dozens of entries).

use std::collections::HashMap;
use std::time::Instant;

use flint_core::ContentHash;

use crate::output::GeneratorOutput;
use crate::Seed;

/// Configuration for the procgen cache.
#[derive(Debug, Clone)]
pub struct ProcGenCacheConfig {
    /// Maximum total memory budget in bytes (default: 256 MB).
    pub max_memory_bytes: usize,
    /// Whether caching is enabled.
    pub enabled: bool,
}

impl Default for ProcGenCacheConfig {
    fn default() -> Self {
        Self {
            max_memory_bytes: 256 * 1024 * 1024,
            enabled: true,
        }
    }
}

/// Cache performance statistics.
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheStats {
    /// Number of entries currently in the cache.
    pub entry_count: usize,
    /// Total estimated bytes of all cached entries.
    pub total_bytes: usize,
    /// Maximum memory budget.
    pub max_memory_bytes: usize,
    /// Number of cache hits since creation/last clear.
    pub hits: u64,
    /// Number of cache misses since creation/last clear.
    pub misses: u64,
}

/// A cached generator output with metadata for LRU tracking.
struct CachedAsset {
    output: GeneratorOutput,
    byte_size: usize,
    last_access: Instant,
}

/// In-memory LRU cache for procedural generation outputs.
///
/// Keyed by `(ContentHash, Seed)` — the content hash of the spec file and
/// the generation seed. Evicts least-recently-used entries when the memory
/// budget is exceeded.
pub struct ProcGenCache {
    entries: HashMap<(ContentHash, Seed), CachedAsset>,
    total_bytes: usize,
    config: ProcGenCacheConfig,
    hits: u64,
    misses: u64,
}

impl ProcGenCache {
    /// Create a new cache with the given configuration.
    pub fn new(config: ProcGenCacheConfig) -> Self {
        Self {
            entries: HashMap::new(),
            total_bytes: 0,
            config,
            hits: 0,
            misses: 0,
        }
    }

    /// Look up a cached output by spec hash and seed.
    ///
    /// Updates the LRU timestamp on hit. Returns `None` on miss or if the
    /// cache is disabled.
    pub fn get(&mut self, spec_hash: ContentHash, seed: &Seed) -> Option<&GeneratorOutput> {
        if !self.config.enabled {
            self.misses += 1;
            return None;
        }
        let key = (spec_hash, seed.clone());
        if self.entries.contains_key(&key) {
            self.entries.get_mut(&key).unwrap().last_access = Instant::now();
            self.hits += 1;
            Some(&self.entries[&key].output)
        } else {
            self.misses += 1;
            None
        }
    }

    /// Insert a generator output into the cache.
    ///
    /// Evicts LRU entries if the memory budget would be exceeded. Silently
    /// skips entries that are individually larger than the budget.
    pub fn insert(&mut self, spec_hash: ContentHash, seed: &Seed, output: GeneratorOutput) {
        if !self.config.enabled {
            return;
        }

        let byte_size = output.estimated_bytes();

        // Don't cache entries larger than the entire budget.
        if byte_size > self.config.max_memory_bytes {
            return;
        }

        let key = (spec_hash, seed.clone());

        // If replacing an existing entry, subtract its old size first.
        if let Some(old) = self.entries.remove(&key) {
            self.total_bytes -= old.byte_size;
        }

        // Evict LRU entries until we have room.
        while self.total_bytes + byte_size > self.config.max_memory_bytes {
            if let Some(lru_key) = self.find_lru_key() {
                let evicted = self.entries.remove(&lru_key).unwrap();
                self.total_bytes -= evicted.byte_size;
            } else {
                break;
            }
        }

        self.total_bytes += byte_size;
        self.entries.insert(
            key,
            CachedAsset {
                output,
                byte_size,
                last_access: Instant::now(),
            },
        );
    }

    /// Remove all entries and reset counters.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
        self.hits = 0;
        self.misses = 0;
    }

    /// Current cache statistics.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entry_count: self.entries.len(),
            total_bytes: self.total_bytes,
            max_memory_bytes: self.config.max_memory_bytes,
            hits: self.hits,
            misses: self.misses,
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
            enabled: true,
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
            enabled: true,
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
            enabled: true,
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
            max_memory_bytes: 256 * 1024 * 1024,
            enabled: false,
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
}
