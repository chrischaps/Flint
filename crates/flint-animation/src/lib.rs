//! Animation system for Flint engine
//!
//! Provides two tiers of animation:
//! - **Tier 1**: Property tweens — animate any TOML component field via keyframes
//! - **Tier 2**: Skeletal animation — glTF skin/joint hierarchies with GPU skinning

pub mod blend;
pub mod clip;
pub mod loader;
pub mod player;
pub mod sampler;
pub mod skeletal_clip;
pub mod skeletal_sampler;
pub mod skeletal_sync;
pub mod skeleton;
pub mod sync;

use flint_core::Result;
use flint_ecs::FlintWorld;
use flint_runtime::RuntimeSystem;

use player::AnimationPlayer;
use skeletal_sync::SkeletalSync;
use sync::AnimationSync;

/// Top-level animation system integrating clip playback with the ECS world.
///
/// Supports both property tweens (Tier 1) and skeletal animation (Tier 2).
/// Implements `RuntimeSystem`, bridges TOML components via `AnimationSync`
/// and `SkeletalSync`.
pub struct AnimationSystem {
    pub player: AnimationPlayer,
    pub sync: AnimationSync,
    pub skeletal_sync: SkeletalSync,
}

impl AnimationSystem {
    pub fn new() -> Self {
        Self {
            player: AnimationPlayer::new(),
            sync: AnimationSync::new(),
            skeletal_sync: SkeletalSync::new(),
        }
    }

    /// Clear all animation state for a scene transition.
    /// Preserves the AnimationPlayer's clip registry (clips are reloadable).
    pub fn clear(&mut self) {
        self.sync.clear();
        self.skeletal_sync.clear();
    }
}

impl Default for AnimationSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeSystem for AnimationSystem {
    fn initialize(&mut self, world: &mut FlintWorld) -> Result<()> {
        self.sync.sync_from_world(world, &self.player);
        self.skeletal_sync.sync_from_world(world);
        println!(
            "Animation system initialized ({} property clips, {} skeletal clips, {} property entities, {} skeletal entities)",
            self.player.clip_count(),
            self.skeletal_sync.clip_count(),
            self.sync.active_count(),
            self.skeletal_sync.active_count()
        );
        Ok(())
    }

    fn fixed_update(&mut self, _world: &mut FlintWorld, _dt: f64) -> Result<()> {
        // Animation interpolates smoothly in variable update — no-op here
        Ok(())
    }

    fn update(&mut self, world: &mut FlintWorld, dt: f64) -> Result<()> {
        // Tier 1: Property animation
        self.sync.sync_from_world(world, &self.player);
        self.sync.advance_and_write(world, &self.player, dt);

        // Tier 2: Skeletal animation
        self.skeletal_sync.sync_from_world(world);
        self.skeletal_sync.advance_and_compute(dt);

        Ok(())
    }

    fn shutdown(&mut self) -> Result<()> {
        println!("Animation system shut down");
        Ok(())
    }

    fn name(&self) -> &str {
        "animation"
    }
}
