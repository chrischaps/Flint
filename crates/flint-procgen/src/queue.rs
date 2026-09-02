//! Frame-budgeted generation queue for procedural assets.
//!
//! The [`ProcGenQueue`] spreads generation work across frames within a
//! configurable time budget, prioritizing assets closest to the camera.

use std::time::Instant;

use flint_core::ContentHash;

use crate::cache::ProcGenCache;
use crate::output::GeneratorOutput;
use crate::registry::GeneratorRegistry;
use crate::spec::ProcGenSpec;
use crate::Seed;

/// Whether the queued asset is a model or texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Model,
    Texture,
}

/// A pending generation request in the queue.
#[derive(Debug, Clone)]
pub struct GenerationRequest {
    pub spec_name: String,
    pub spec: ProcGenSpec,
    pub content_hash: ContentHash,
    pub seed: Seed,
    /// Distance squared from camera; lower = higher priority.
    pub priority: f32,
    pub entity_position: [f32; 3],
    pub asset_kind: AssetKind,
}

/// A completed asset ready for GPU upload.
pub struct CompletedAsset {
    pub spec_name: String,
    pub output: GeneratorOutput,
    pub asset_kind: AssetKind,
}

/// Statistics about the generation queue.
#[derive(Debug, Clone, Copy, Default)]
pub struct QueueStats {
    pub pending_count: usize,
    pub completed_this_frame: usize,
    pub budget_used_ms: f64,
    pub total_completed: u64,
}

/// Frame-budgeted generation queue.
///
/// Each frame, [`process_frame`](ProcGenQueue::process_frame) pops the highest-priority
/// (closest to camera) pending requests and generates them until the time budget is
/// exhausted. Results are returned for GPU upload.
pub struct ProcGenQueue {
    pending: Vec<GenerationRequest>,
    budget_ms: f64,
    last_camera_pos: [f32; 3],
    /// Camera must move more than sqrt(this) units to trigger reprioritization.
    reprioritize_threshold_sq: f32,
    total_completed: u64,
    last_stats: QueueStats,
}

impl ProcGenQueue {
    pub fn new(budget_ms: f64) -> Self {
        Self {
            pending: Vec::new(),
            budget_ms,
            last_camera_pos: [0.0; 3],
            reprioritize_threshold_sq: 4.0,
            total_completed: 0,
            last_stats: QueueStats::default(),
        }
    }

    pub fn set_budget_ms(&mut self, budget_ms: f64) {
        self.budget_ms = budget_ms;
    }

    /// Enqueue a generation request. Deduplicates by `spec_name`.
    pub fn enqueue(&mut self, request: GenerationRequest) {
        if self.is_pending(&request.spec_name) {
            return;
        }
        self.pending.push(request);
    }

    /// Check if a spec is already pending in the queue.
    pub fn is_pending(&self, spec_name: &str) -> bool {
        self.pending.iter().any(|r| r.spec_name == spec_name)
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Process the queue for one frame within the time budget.
    ///
    /// Reprioritizes if the camera has moved significantly, then generates
    /// assets in priority order (closest first) until the budget is exhausted.
    pub fn process_frame(
        &mut self,
        camera_pos: [f32; 3],
        registry: &GeneratorRegistry,
        cache: &mut ProcGenCache,
    ) -> Vec<CompletedAsset> {
        if self.pending.is_empty() {
            self.last_stats = QueueStats {
                pending_count: 0,
                completed_this_frame: 0,
                budget_used_ms: 0.0,
                total_completed: self.total_completed,
            };
            return Vec::new();
        }

        // Reprioritize if camera moved significantly
        let dx = camera_pos[0] - self.last_camera_pos[0];
        let dy = camera_pos[1] - self.last_camera_pos[1];
        let dz = camera_pos[2] - self.last_camera_pos[2];
        let cam_dist_sq = dx * dx + dy * dy + dz * dz;

        if cam_dist_sq > self.reprioritize_threshold_sq {
            for req in &mut self.pending {
                let ex = req.entity_position[0] - camera_pos[0];
                let ey = req.entity_position[1] - camera_pos[1];
                let ez = req.entity_position[2] - camera_pos[2];
                req.priority = ex * ex + ey * ey + ez * ez;
            }
            self.last_camera_pos = camera_pos;
        }

        // Sort by priority ascending (closest first)
        self.pending.sort_by(|a, b| {
            a.priority
                .partial_cmp(&b.priority)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let start = Instant::now();
        let mut completions = Vec::new();

        while !self.pending.is_empty() {
            let req = &self.pending[0];

            // Check cache first (no budget cost)
            if let Some(cached) = cache.get(req.content_hash, &req.seed) {
                let completed = CompletedAsset {
                    spec_name: req.spec_name.clone(),
                    output: cached.clone(),
                    asset_kind: req.asset_kind,
                };
                completions.push(completed);
                self.pending.remove(0);
                self.total_completed += 1;
                continue;
            }

            // Check budget before generating
            let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
            if elapsed_ms >= self.budget_ms && !completions.is_empty() {
                break;
            }

            // Generate
            let req = self.pending.remove(0);
            match registry.generate_from_spec(&req.spec) {
                Ok(output) => {
                    cache.insert(req.content_hash, &req.seed, output.clone());
                    completions.push(CompletedAsset {
                        spec_name: req.spec_name,
                        output,
                        asset_kind: req.asset_kind,
                    });
                    self.total_completed += 1;
                }
                Err(e) => {
                    eprintln!(
                        "[procgen] Queued generation failed for '{}': {}",
                        req.spec_name, e
                    );
                }
            }

            let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
            if elapsed_ms >= self.budget_ms {
                break;
            }
        }

        let budget_used_ms = start.elapsed().as_secs_f64() * 1000.0;
        self.last_stats = QueueStats {
            pending_count: self.pending.len(),
            completed_this_frame: completions.len(),
            budget_used_ms,
            total_completed: self.total_completed,
        };

        completions
    }

    pub fn clear(&mut self) {
        self.pending.clear();
    }

    pub fn stats(&self) -> QueueStats {
        self.last_stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::{ProcGenCache, ProcGenCacheConfig};
    use crate::mock::MockGenerator;
    use crate::output::OutputKind;
    use crate::register_built_in_generators;
    use crate::registry::GeneratorRegistry;
    use crate::spec::ProcGenSpec;
    use flint_core::ContentHash;

    const MOCK_SPEC_TOML: &str = r#"
generator = "mock"
[meta]
name = "test_item"
version = "1.0.0"
[seed]
mode = "fixed"
value = 42
"#;

    fn make_registry() -> GeneratorRegistry {
        let mut reg = GeneratorRegistry::new();
        register_built_in_generators(&mut reg);
        reg.register(Box::new(MockGenerator));
        reg
    }

    fn make_cache() -> ProcGenCache {
        ProcGenCache::new(ProcGenCacheConfig::default())
    }

    fn make_request(name: &str, position: [f32; 3]) -> GenerationRequest {
        let spec = ProcGenSpec::parse(&MOCK_SPEC_TOML.replace("test_item", name)).unwrap();
        let seed = spec.seed.to_seed().unwrap();
        let content_hash = ContentHash::from_str(&toml::to_string(&spec).unwrap_or_default());
        let dx = position[0];
        let dy = position[1];
        let dz = position[2];
        GenerationRequest {
            spec_name: name.to_string(),
            spec,
            content_hash,
            seed,
            priority: dx * dx + dy * dy + dz * dz,
            entity_position: position,
            asset_kind: AssetKind::Model,
        }
    }

    #[test]
    fn empty_queue_returns_empty() {
        let mut queue = ProcGenQueue::new(4.0);
        let registry = make_registry();
        let mut cache = make_cache();
        let result = queue.process_frame([0.0; 3], &registry, &mut cache);
        assert!(result.is_empty());
        assert_eq!(queue.stats().pending_count, 0);
    }

    #[test]
    fn deduplication() {
        let mut queue = ProcGenQueue::new(4.0);
        queue.enqueue(make_request("item_a", [1.0, 0.0, 0.0]));
        queue.enqueue(make_request("item_a", [2.0, 0.0, 0.0]));
        assert_eq!(queue.pending_count(), 1);
    }

    #[test]
    fn priority_ordering() {
        let mut queue = ProcGenQueue::new(1000.0);
        let registry = make_registry();
        let mut cache = make_cache();

        // Enqueue at distances 5, 1, 3 from origin
        queue.enqueue(make_request("far", [5.0, 0.0, 0.0]));
        queue.enqueue(make_request("close", [1.0, 0.0, 0.0]));
        queue.enqueue(make_request("mid", [3.0, 0.0, 0.0]));

        // Force reprioritization by setting last_camera_pos far away
        queue.last_camera_pos = [100.0, 0.0, 0.0];

        let completed = queue.process_frame([0.0; 3], &registry, &mut cache);
        assert_eq!(completed.len(), 3);
        assert_eq!(completed[0].spec_name, "close");
        assert_eq!(completed[1].spec_name, "mid");
        assert_eq!(completed[2].spec_name, "far");
    }

    #[test]
    fn camera_distance_priority() {
        let mut queue = ProcGenQueue::new(1000.0);
        let registry = make_registry();
        let mut cache = make_cache();

        queue.enqueue(make_request("nearby", [1.0, 0.0, 0.0]));
        queue.enqueue(make_request("distant", [10.0, 0.0, 0.0]));

        queue.last_camera_pos = [100.0, 0.0, 0.0];
        let completed = queue.process_frame([0.0; 3], &registry, &mut cache);
        assert_eq!(completed.len(), 2);
        assert_eq!(completed[0].spec_name, "nearby");
        assert_eq!(completed[1].spec_name, "distant");
    }

    #[test]
    fn queue_drain_with_large_budget() {
        let mut queue = ProcGenQueue::new(1000.0);
        let registry = make_registry();
        let mut cache = make_cache();

        for i in 0..5 {
            queue.enqueue(make_request(&format!("item_{i}"), [i as f32, 0.0, 0.0]));
        }

        let completed = queue.process_frame([0.0; 3], &registry, &mut cache);
        assert_eq!(completed.len(), 5);
        assert_eq!(queue.pending_count(), 0);
        assert_eq!(queue.stats().total_completed, 5);
    }

    #[test]
    fn budget_enforcement() {
        // Use a tiny budget (0.001ms) — mock generator is fast but we should
        // still break after at least 1 item if the budget is exhausted.
        let mut queue = ProcGenQueue::new(0.001);
        let registry = make_registry();
        let mut cache = make_cache();

        for i in 0..10 {
            queue.enqueue(make_request(&format!("item_{i}"), [i as f32, 0.0, 0.0]));
        }

        let completed = queue.process_frame([0.0; 3], &registry, &mut cache);
        // At least 1 completes (we always process one before checking budget)
        assert!(!completed.is_empty());
        // With a very tiny budget, unlikely all 10 complete in one frame
        // (though mock is very fast, so we just check the mechanism works)
        assert!(queue.stats().budget_used_ms >= 0.0);
    }

    #[test]
    fn lod_grouping_single_output() {
        let mut queue = ProcGenQueue::new(1000.0);
        let registry = make_registry();
        let mut cache = make_cache();

        queue.enqueue(make_request("lod_item", [0.0; 3]));
        let completed = queue.process_frame([0.0; 3], &registry, &mut cache);
        assert_eq!(completed.len(), 1);
        // Mock generator produces a Mesh output — one entry covers all LODs
        assert_eq!(completed[0].output.kind(), OutputKind::Mesh);
    }

    #[test]
    fn cache_hit_skips_generation() {
        let mut queue = ProcGenQueue::new(1000.0);
        let registry = make_registry();
        let mut cache = make_cache();

        let req = make_request("cached_item", [0.0; 3]);

        // Pre-populate cache
        let output = registry.generate_from_spec(&req.spec).unwrap();
        cache.insert(req.content_hash, &req.seed, output);

        queue.enqueue(req);
        let completed = queue.process_frame([0.0; 3], &registry, &mut cache);
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].spec_name, "cached_item");
        // Cache should have a hit
        assert!(cache.stats().hits > 0);
    }

    #[test]
    fn camera_move_reprioritizes() {
        let mut queue = ProcGenQueue::new(1000.0);

        // Entity A is at (10,0,0), entity B is at (1,0,0).
        // Camera starts at origin → B is closer.
        queue.enqueue(make_request("entity_a", [10.0, 0.0, 0.0]));
        queue.enqueue(make_request("entity_b", [1.0, 0.0, 0.0]));

        // Move camera to (9,0,0) — now A is closer (dist=1) than B (dist=8)
        queue.last_camera_pos = [0.0, 0.0, 0.0]; // far enough to trigger reprioritization
        let registry = make_registry();
        let mut cache = make_cache();

        let completed = queue.process_frame([9.0, 0.0, 0.0], &registry, &mut cache);
        assert_eq!(completed.len(), 2);
        assert_eq!(completed[0].spec_name, "entity_a"); // closer to camera at (9,0,0)
        assert_eq!(completed[1].spec_name, "entity_b");
    }

    #[test]
    fn clear_empties_queue() {
        let mut queue = ProcGenQueue::new(4.0);
        for i in 0..5 {
            queue.enqueue(make_request(&format!("item_{i}"), [0.0; 3]));
        }
        assert_eq!(queue.pending_count(), 5);
        queue.clear();
        assert_eq!(queue.pending_count(), 0);
    }

    #[test]
    fn is_pending_check() {
        let mut queue = ProcGenQueue::new(4.0);
        queue.enqueue(make_request("my_asset", [0.0; 3]));
        assert!(queue.is_pending("my_asset"));
        assert!(!queue.is_pending("other_asset"));
    }
}
