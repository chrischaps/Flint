//! Animation system for Flint engine
//!
//! Provides four tiers of animation:
//! - **Tier 1**: Property tweens — animate any TOML component field via keyframes
//! - **Tier 2**: Skeletal animation — glTF skin/joint hierarchies with GPU skinning
//! - **Tier 3**: Node animation — per-node transform animation for scene graphs
//! - **Tier 4**: Sprite sheet animation — frame-based sprite animation from `.sprite.toml` clips

pub mod blend;
pub mod clip;
pub mod loader;
pub mod node_clip;
pub mod node_sync;
pub mod playback_state;
pub mod player;
pub mod sampler;
pub mod skeletal_clip;
pub mod skeletal_sampler;
pub mod skeletal_sync;
pub mod skeleton;
pub mod sprite_clip;
pub mod sprite_sync;
pub mod sync;

pub use playback_state::ClipPlaybackState;

use flint_core::Result;
use flint_ecs::FlintWorld;
use flint_runtime::RuntimeSystem;

use node_sync::NodeSync;
use player::AnimationPlayer;
use skeletal_sync::SkeletalSync;
use sprite_sync::SpriteAnimSync;
use sync::AnimationSync;

/// Top-level animation system integrating clip playback with the ECS world.
///
/// Supports property tweens (Tier 1), skeletal animation (Tier 2),
/// node animation (Tier 3), and sprite sheet animation (Tier 4).
/// Implements `RuntimeSystem`, bridges TOML components via the various Sync types.
#[derive(Default)]
pub struct AnimationSystem {
    pub player: AnimationPlayer,
    pub sync: AnimationSync,
    pub skeletal_sync: SkeletalSync,
    pub node_sync: NodeSync,
    pub sprite_sync: SpriteAnimSync,
}

impl AnimationSystem {
    pub fn new() -> Self {
        Self {
            player: AnimationPlayer::new(),
            sync: AnimationSync::new(),
            skeletal_sync: SkeletalSync::new(),
            node_sync: NodeSync::new(),
            sprite_sync: SpriteAnimSync::new(),
        }
    }

    /// Clear all animation state for a scene transition.
    /// Preserves clip registries (clips are reloadable).
    pub fn clear(&mut self) {
        self.sync.clear();
        self.skeletal_sync.clear();
        self.node_sync.clear();
        self.sprite_sync.clear();
    }
}

impl RuntimeSystem for AnimationSystem {
    fn initialize(&mut self, world: &mut FlintWorld) -> Result<()> {
        self.sync.sync_from_world(world, &self.player);
        self.skeletal_sync.sync_from_world(world);
        self.node_sync.sync_from_world(world);
        self.sprite_sync.sync_from_world(world);
        println!(
            "Animation system initialized ({} property clips, {} skeletal clips, {} node clips, {} sprite clips, {} property entities, {} skeletal entities, {} node entities, {} sprite entities)",
            self.player.clip_count(),
            self.skeletal_sync.clip_count(),
            self.node_sync.clip_count(),
            self.sprite_sync.clip_count(),
            self.sync.active_count(),
            self.skeletal_sync.active_count(),
            self.node_sync.active_count(),
            self.sprite_sync.active_count()
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

        // Tier 3: Node transform animation
        self.node_sync.sync_from_world(world);
        self.node_sync.advance_and_apply(world, dt);

        // Tier 4: Sprite sheet animation
        self.sprite_sync.sync_from_world(world);
        self.sprite_sync.advance_and_write(world, dt);

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
