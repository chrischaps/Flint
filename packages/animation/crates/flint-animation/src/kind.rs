//! The clip-kind seam (ADR 0139 section 5): what a package that animates
//! something the core doesn't name plugs into [`crate::AnimationSystem`].
//!
//! The core plays property clips itself (and, until #1621 moves them to
//! `rig`, node and skeletal clips). A registered kind is the part that knows
//! what its clips drive: the `flipbook` package's writes `sprite.source_rect`.
//! Every kind follows the one `animator` rule set ([`crate::Animator::start`],
//! [`crate::Animator::apply_to`], [`crate::animator::publish`]) on the one
//! clock ([`crate::ClipPlaybackState::advance_time`]), so a sprite pauses,
//! resumes, loops and replays as a skinned character does.
//!
//! Registered kinds run after the core's own, in the order they were added,
//! which is the flipbook's slot in today's frame (property, sequences,
//! skeletal, node, flipbook): a game with every package on steps exactly as
//! before.

use crate::playback_state::ClipPlaybackState;
use flint_core::EntityId;
use flint_ecs::FlintWorld;

/// A one-shot that reached its end this frame, of any kind: scripts hear it
/// as `on_animation_end(clip)` on the entity's script (ADR 0139 section 8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipEnd {
    pub entity_id: EntityId,
    pub clip_name: String,
}

/// A kind of clip a package plays on the core's clock.
pub trait ClipKind: Send {
    /// For logs and the load line: `"flipbook"`.
    fn name(&self) -> &'static str;

    /// Clips it has registered.
    fn clip_count(&self) -> usize;

    /// Entities it plays.
    fn active_count(&self) -> usize;

    /// Whether it knows a clip of this name.
    fn has_clip(&self, clip: &str) -> bool;

    /// Follow the world's animators: start a playback for an entity whose
    /// clip is one of this kind's and whose shape the kind drives (a sprite
    /// for a flipbook), and apply each animator to the playbacks it holds.
    fn sync_from_world(&mut self, world: &FlintWorld);

    /// Move every playback `dt` seconds on, write what the kind drives, tell
    /// each animator what the runtime changed, and push each one-shot that
    /// ended to `ends`.
    fn advance(&mut self, world: &mut FlintWorld, dt: f64, ends: &mut Vec<ClipEnd>);

    /// An entity's playback, if this kind plays it.
    fn state(&self, entity: EntityId) -> Option<&ClipPlaybackState>;

    /// Forget one entity's playback: seeking replays it from the start.
    fn reset(&mut self, entity: EntityId);

    /// Forget every playback (its clips stay registered).
    fn clear(&mut self);
}
