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

use std::collections::HashMap;

use flint_core::{EntityId, Result};
use flint_ecs::FlintWorld;
use flint_runtime::RuntimeSystem;

use clip::AnimationClip;
use node_clip::NodeClip;
use node_sync::{NodePlaybackState, NodeSync};
use player::AnimationPlayer;
use skeletal_clip::SkeletalClip;
use skeletal_sync::{SkeletalPlaybackState, SkeletalSync};
use skeleton::Skeleton;
use sprite_clip::SpriteAnimClip;
use sprite_sync::{SpriteAnimEndEvent, SpriteAnimSync};
use sync::AnimationSync;

/// Top-level animation system integrating clip playback with the ECS world.
///
/// Supports property tweens (Tier 1), skeletal animation (Tier 2),
/// node animation (Tier 3), and sprite sheet animation (Tier 4).
/// Implements `RuntimeSystem`, bridges TOML components via the various Sync types.
#[derive(Default)]
pub struct AnimationSystem {
    pub(crate) player: AnimationPlayer,
    pub(crate) sync: AnimationSync,
    pub(crate) skeletal_sync: SkeletalSync,
    pub(crate) node_sync: NodeSync,
    pub(crate) sprite_sync: SpriteAnimSync,
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

    // ── Clip registration (scene loading) ──

    /// Register a property tween clip
    pub fn add_property_clip(&mut self, clip: AnimationClip) {
        self.player.add_clip(clip);
    }

    /// Register a skeletal animation clip
    pub fn add_skeletal_clip(&mut self, clip: SkeletalClip) {
        self.skeletal_sync.add_clip(clip);
    }

    /// Register a skeleton for an entity
    pub fn add_skeleton(&mut self, entity_id: EntityId, skeleton: Skeleton) {
        self.skeletal_sync.add_skeleton(entity_id, skeleton);
    }

    /// Register a node transform animation clip
    pub fn add_node_clip(&mut self, clip: NodeClip) {
        self.node_sync.add_clip(clip);
    }

    /// Register an entity's node name → EntityId mapping for node animation
    pub fn register_node_entity(
        &mut self,
        entity_id: EntityId,
        node_map: HashMap<String, EntityId>,
    ) {
        self.node_sync.register_entity(entity_id, node_map);
    }

    /// Register a sprite sheet animation clip
    pub fn add_sprite_clip(&mut self, clip: SpriteAnimClip) {
        self.sprite_sync.add_clip(clip);
    }

    // ── Runtime queries ──

    /// Get computed bone matrices for an entity (skeletal animation)
    pub fn bone_matrices(&self, entity_id: &EntityId) -> Option<&[[[f32; 4]; 4]]> {
        self.skeletal_sync.bone_matrices(entity_id)
    }

    /// The skeleton driving an entity (joint hierarchy + model-space globals
    /// from the last pose computation — e.g. for a debug armature overlay)
    pub fn skeleton(&self, entity_id: &EntityId) -> Option<&Skeleton> {
        self.skeletal_sync.skeleton(entity_id)
    }

    /// Model-space position of a named joint (bone_probe: camera anchors,
    /// attachment points)
    pub fn joint_position(&self, entity_id: &EntityId, joint: &str) -> Option<[f32; 3]> {
        self.skeletal_sync.joint_position(entity_id, joint)
    }

    /// Drain sprite animation end events
    pub fn drain_sprite_events(&mut self) -> Vec<SpriteAnimEndEvent> {
        self.sprite_sync.drain_events()
    }

    // ── Sync & advance (player/preview update loops) ──

    /// Sync property animation state from ECS world
    pub fn sync_property_from_world(&mut self, world: &FlintWorld) {
        self.sync.sync_from_world(world, &self.player);
    }

    /// Sync skeletal animation state from ECS world
    pub fn sync_skeletal_from_world(&mut self, world: &FlintWorld) {
        self.skeletal_sync.sync_from_world(world);
    }

    /// Sync node animation state from ECS world
    pub fn sync_node_from_world(&mut self, world: &FlintWorld) {
        self.node_sync.sync_from_world(world);
    }

    /// Advance property animations and write results back to ECS
    pub fn advance_property_and_write(&mut self, world: &mut FlintWorld, dt: f64) {
        self.sync.advance_and_write(world, &self.player, dt);
    }

    /// Advance skeletal animations and compute bone matrices
    pub fn advance_skeletal(&mut self, dt: f64) {
        self.skeletal_sync.advance_and_compute(dt);
    }

    /// Advance node animations and apply transforms to ECS
    pub fn advance_node_and_apply(&mut self, world: &mut FlintWorld, dt: f64) {
        self.node_sync.advance_and_apply(world, dt);
    }

    // ── Preview scrubbing ──

    /// Reset skeletal playback state for an entity
    pub fn reset_skeletal_state(&mut self, entity_id: &EntityId) {
        self.skeletal_sync.reset_state(entity_id);
    }

    /// Get skeletal playback state for an entity
    pub fn skeletal_playback_state(&self, entity_id: &EntityId) -> Option<&SkeletalPlaybackState> {
        self.skeletal_sync.get_playback_state(entity_id)
    }

    /// Get node playback state for an entity
    pub fn node_playback_state(&self, entity_id: &EntityId) -> Option<&NodePlaybackState> {
        self.node_sync.get_playback_state(entity_id)
    }

    /// Get the duration of a skeletal clip by name
    pub fn skeletal_clip_duration(&self, clip_name: &str) -> Option<f64> {
        self.skeletal_sync.get_clip_duration(clip_name)
    }

    /// Get the duration of a node clip by name
    pub fn node_clip_duration(&self, clip_name: &str) -> Option<f64> {
        self.node_sync.get_clip_duration(clip_name)
    }

    /// Set skeletal playback time for an entity (scrubbing)
    pub fn set_skeletal_playback_time(&mut self, entity_id: &EntityId, time: f64) {
        self.skeletal_sync.set_playback_time(entity_id, time);
    }

    /// Set node playback time for an entity (scrubbing)
    pub fn set_node_playback_time(&mut self, entity_id: &EntityId, time: f64) {
        self.node_sync.set_playback_time(entity_id, time);
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
        // Retire finished crossfades in the ECS, or they re-arm forever.
        self.skeletal_sync.write_back(world);

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
