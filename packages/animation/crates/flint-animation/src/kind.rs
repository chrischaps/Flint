//! The clip-kind seam (ADR 0139 section 5): what a package that animates
//! something the core doesn't name plugs into [`crate::AnimationSystem`].
//!
//! The core plays property clips itself. A registered kind is the part that
//! knows what its clips drive: the `rig` package's skeletal kind writes a
//! skeleton's joints and its node kind a model's child nodes (#1621), the
//! `flipbook` package's writes `sprite.source_rect` (#1620). Every kind
//! follows the one `animator` rule set ([`crate::Animator::start`],
//! [`crate::Animator::apply_to`], [`crate::animator::publish`]) on the one
//! clock ([`crate::ClipPlaybackState::advance_time`]), so a sprite pauses,
//! resumes, loops and replays as a skinned character does.
//!
//! Kinds run after the core's property clips and sequences at fixed slots
//! ([`ClipKind::slot`]), whatever order their packages registered in, so a
//! game with every package on steps exactly as before the split: property,
//! sequences, skeletal ([`SLOT_SKELETAL`]), node ([`SLOT_NODE`]), flipbook
//! ([`SLOT_FLIPBOOK`]).

use crate::playback_state::ClipPlaybackState;
use flint_core::EntityId;
use flint_ecs::FlintWorld;

/// The skeletal kind's place in the frame: first after the sequences, which
/// write the animator fields it reads that frame.
pub const SLOT_SKELETAL: u32 = 10;
/// The node kind's: after the skeletal kind, as the node tier always ran.
pub const SLOT_NODE: u32 = 20;
/// The flipbook kind's: last.
pub const SLOT_FLIPBOOK: u32 = 30;

/// A skinned entity's bone matrices, one a joint.
pub type BoneMatrices = [[[f32; 4]; 4]];

/// What [`ClipKind::skins`] hands each skinned entity to: its id, its
/// model's asset and its bones once computed.
pub type EachSkin<'a> = dyn FnMut(EntityId, &str, Option<&BoneMatrices>) + 'a;

/// A one-shot that reached its end this frame, of any kind: scripts hear it
/// as `on_animation_end(clip)` on the entity's script (ADR 0139 section 8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipEnd {
    pub entity_id: EntityId,
    pub clip_name: String,
}

/// A kind of clip a package plays on the core's clock.
pub trait ClipKind: Send + std::any::Any {
    /// For logs and the load line: `"flipbook"`.
    fn name(&self) -> &'static str;

    /// Where it runs in the frame, lowest first ([`SLOT_SKELETAL`],
    /// [`SLOT_NODE`], [`SLOT_FLIPBOOK`]); kinds of one slot run in the
    /// order they were added.
    fn slot(&self) -> u32;

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

    /// After every kind has advanced (the player's `Stage::Pose`, a
    /// capture's step): what reads the finished pose and writes the world.
    /// The rig's skeletal kind publishes bone probes and solves two-bone IK
    /// here. Nothing, by default.
    fn pose(&mut self, _world: &mut FlintWorld) {}

    /// Each skinned entity it poses, with its model's asset and its bone
    /// matrices once computed: what the host hands the renderer. None, by
    /// default.
    fn skins(&self, _each: &mut EachSkin) {}

    /// An entity's playback, if this kind plays it.
    fn state(&self, entity: EntityId) -> Option<&ClipPlaybackState>;

    /// Forget one entity's playback: seeking replays it from the start.
    fn reset(&mut self, entity: EntityId);

    /// Forget every playback (its clips stay registered).
    fn clear(&mut self);

    /// The kind itself, for its package's own calls
    /// ([`crate::AnimationSystem::kind_of`]).
    fn as_any(&self) -> &dyn std::any::Any;

    /// [`Self::as_any`], to change it.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}
