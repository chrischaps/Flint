//! Animation system for Flint engine
//!
//! Provides four tiers of animation:
//! - **Tier 1**: Property tweens — animate any TOML component field via keyframes
//! - **Tier 2**: Skeletal animation — glTF skin/joint hierarchies with GPU skinning
//! - **Tier 3**: Node animation — per-node transform animation for scene graphs
//! - **Tier 4**: Sprite sheet animation — frame-based sprite animation from `.sprite.toml` clips

pub mod blend;
pub mod clip;
pub mod layer_edit;
pub mod loader;
pub mod node_clip;
pub mod node_sync;
pub mod playback_state;
pub mod player;
pub mod sampler;
pub mod sequence;
pub mod skeletal_clip;
pub mod skeletal_sampler;
pub mod skeletal_sync;
pub mod skeleton;
pub mod sprite_clip;
pub mod sprite_sync;
pub mod sync;

pub use playback_state::{AnimLayer, ClipPlaybackState, LayerMode};
pub use sequence::{
    load_sequence_from_file, AnimSequence, SequenceCueEvent, SequenceEvent, SequenceRuntime,
    SequenceStep,
};
pub use skeletal_sync::{LayerContribution, WRITER_BASE, WRITER_REST};

use std::collections::HashMap;

use flint_core::{EntityId, Result};
use flint_ecs::FlintWorld;
use flint_runtime::RuntimeSystem;

use clip::AnimationClip;
use node_clip::NodeClip;
use node_sync::{NodePlaybackState, NodeSync};
use player::AnimationPlayer;
use sequence::SequenceSync;
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
    pub(crate) sequence_sync: SequenceSync,
}

impl AnimationSystem {
    pub fn new() -> Self {
        Self {
            player: AnimationPlayer::new(),
            sync: AnimationSync::new(),
            skeletal_sync: SkeletalSync::new(),
            node_sync: NodeSync::new(),
            sprite_sync: SpriteAnimSync::new(),
            sequence_sync: SequenceSync::new(),
        }
    }

    /// Clear all animation state for a scene transition.
    /// Preserves clip registries (clips are reloadable).
    pub fn clear(&mut self) {
        self.sync.clear();
        self.skeletal_sync.clear();
        self.node_sync.clear();
        self.sprite_sync.clear();
        self.sequence_sync.clear();
    }

    // ── Sequences (timestamped animator events) ──

    /// Register a sequence by name
    pub fn add_sequence(&mut self, seq: AnimSequence) {
        self.sequence_sync.add_sequence(seq);
    }

    pub fn sequence(&self, name: &str) -> Option<&std::sync::Arc<AnimSequence>> {
        self.sequence_sync.get(name)
    }

    pub fn sequence_names(&self) -> Vec<String> {
        self.sequence_sync.sequence_names()
    }

    /// Start a sequence on an entity from t = 0; false if unknown
    pub fn play_sequence(&mut self, entity_id: EntityId, name: &str) -> bool {
        self.sequence_sync.play(entity_id, name)
    }

    pub fn stop_sequence(&mut self, entity_id: &EntityId) {
        self.sequence_sync.stop(entity_id);
    }

    pub fn restart_sequence(&mut self, entity_id: &EntityId) {
        self.sequence_sync.restart(entity_id);
    }

    pub fn set_sequence_playing(&mut self, entity_id: &EntityId, playing: bool) {
        self.sequence_sync.set_playing(entity_id, playing);
    }

    pub fn set_sequence_loop_override(&mut self, entity_id: &EntityId, looping: Option<bool>) {
        self.sequence_sync.set_loop_override(entity_id, looping);
    }

    pub fn sequence_state(&self, entity_id: &EntityId) -> Option<&SequenceRuntime> {
        self.sequence_sync.state(entity_id)
    }

    /// Cues passed since the last drain
    pub fn drain_sequence_cues(&mut self) -> Vec<SequenceCueEvent> {
        self.sequence_sync.drain_cues()
    }

    /// Follow `animator.sequence` edges (play/stop requests)
    pub fn sync_sequences_from_world(&mut self, world: &FlintWorld) {
        self.sequence_sync.sync_from_world(world);
    }

    /// Advance sequences, applying passed events to the ECS. Call before
    /// the skeletal sync so the writes land this frame.
    pub fn advance_sequences(&mut self, world: &mut FlintWorld, dt: f64) {
        self.sequence_sync.advance(world, dt);
    }

    /// Seek an entity's sequence to `t` by deterministic replay: reset the
    /// skeletal state, restart the sequence, and step sequence + skeletal
    /// tiers together in `step_dt` increments. The caller must first
    /// restore the animator component to its pre-sequence state (the
    /// sequence only ever *adds* writes). Cues fired during the replay
    /// are discarded. Returns the number of events fired.
    pub fn seek_sequence(
        &mut self,
        world: &mut FlintWorld,
        entity_id: EntityId,
        t: f64,
        step_dt: f64,
    ) -> usize {
        let step_dt = if step_dt > 0.0 { step_dt } else { 1.0 / 120.0 };
        self.skeletal_sync.reset_state(&entity_id);
        self.sequence_sync.restart(&entity_id);
        self.sequence_sync.set_playing(&entity_id, true);

        let mut elapsed = 0.0;
        let target = t.max(0.0);
        loop {
            let dt = (target - elapsed).min(step_dt);
            self.sequence_sync.advance(world, dt.max(0.0));
            self.skeletal_sync.sync_from_world(world);
            self.skeletal_sync.advance_and_compute(dt.max(0.0));
            self.skeletal_sync.write_back(world);
            elapsed += dt;
            if dt <= 0.0 || elapsed >= target {
                break;
            }
        }
        self.sequence_sync.drain_cues();
        self.sequence_sync
            .state(&entity_id)
            .map(|s| s.fired_count())
            .unwrap_or(0)
    }

    /// Seconds left and target of a layer weight ramp, if one is running
    pub fn skeletal_layer_fade(&self, entity_id: &EntityId, index: usize) -> Option<(f32, f32)> {
        self.skeletal_sync.layer_fade(entity_id, index)
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

    /// Retire finished crossfades / layer fades in the ECS. Must follow
    /// `advance_skeletal` every frame — otherwise the next sync re-arms
    /// them (a `blend_target` left set restarts the same blend forever).
    pub fn write_back_skeletal(&mut self, world: &mut FlintWorld) {
        self.skeletal_sync.write_back(world);
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

    // ── Animation layers ──

    /// Names of all registered skeletal clips (sorted)
    pub fn skeletal_clip_names(&self) -> Vec<String> {
        let mut names = self.skeletal_sync.clip_names();
        names.sort();
        names
    }

    /// The resolved layer list driving an entity
    pub fn skeletal_layers(&self, entity_id: &EntityId) -> Option<&[AnimLayer]> {
        self.skeletal_sync.layers(entity_id)
    }

    /// Per-joint composition bookkeeping from the last skeletal advance
    pub fn skeletal_layer_contribution(&self, entity_id: &EntityId) -> Option<&LayerContribution> {
        self.skeletal_sync.layer_contribution(entity_id)
    }

    /// Runtime-only mute for a layer (previewer solo/mute); not persisted
    pub fn set_skeletal_layer_mute(&mut self, entity_id: EntityId, index: usize, muted: bool) {
        self.skeletal_sync.set_layer_mute(entity_id, index, muted);
    }

    /// Clear runtime mutes for an entity
    pub fn clear_skeletal_layer_mutes(&mut self, entity_id: &EntityId) {
        self.skeletal_sync.clear_layer_mutes(entity_id);
    }

    /// Which joints a skeletal clip keys
    pub fn skeletal_clip_keyed_joints(&self, clip: &str, joint_count: usize) -> Option<Vec<bool>> {
        self.skeletal_sync.clip_keyed_joints(clip, joint_count)
    }
}

impl RuntimeSystem for AnimationSystem {
    fn initialize(&mut self, world: &mut FlintWorld) -> Result<()> {
        self.sync.sync_from_world(world, &self.player);
        self.skeletal_sync.sync_from_world(world);
        self.node_sync.sync_from_world(world);
        self.sprite_sync.sync_from_world(world);
        self.sequence_sync.sync_from_world(world);
        println!(
            "Animation system initialized ({} property clips, {} skeletal clips, {} node clips, {} sprite clips, {} sequences, {} property entities, {} skeletal entities, {} node entities, {} sprite entities, {} sequence entities)",
            self.player.clip_count(),
            self.skeletal_sync.clip_count(),
            self.node_sync.clip_count(),
            self.sprite_sync.clip_count(),
            self.sequence_sync.sequence_count(),
            self.sync.active_count(),
            self.skeletal_sync.active_count(),
            self.node_sync.active_count(),
            self.sprite_sync.active_count(),
            self.sequence_sync.active_count()
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

        // Sequences write animator fields; run them first so Tier 2 sees
        // the writes this frame.
        self.sequence_sync.sync_from_world(world);
        self.sequence_sync.advance(world, dt);

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
