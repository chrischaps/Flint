//! The animation package's core (ADR 0139, #1620): clips on one clock.
//!
//! - **Property clips** (`.anim.toml`): any TOML component field via keyframes
//! - **Sequences** (`.sequence.toml`): timed `animator` writes and cues
//! - **Clip kinds** ([`kind::ClipKind`]) other packages register: the `rig`
//!   package's skeletal and node clips (#1621), the `flipbook` package's
//!   sprite-sheet clips (#1620)
//! - **`tween`**'s field tweens ([`tween::FieldTweens`])
//!
//! Its `player` feature is the package's player part (`player::PACKAGE`):
//! the subsystem, the `animation` and `tween` script modules, and the
//! `player::ClipKinds` point.

pub mod animator;
pub mod clip;
pub mod kind;
pub mod loader;
pub mod playback_state;
pub mod property;
pub mod sampler;
pub mod sequence;
pub mod sync;
pub mod tween;

#[cfg(feature = "player")]
pub mod player;

pub use animator::{Animator, LAYER_MODES};
pub use kind::{
    BoneMatrices, ClipEnd, ClipKind, EachSkin, SLOT_FLIPBOOK, SLOT_NODE, SLOT_SKELETAL,
};
pub use playback_state::{AnimLayer, ClipPlaybackState, LayerMode};
pub use sequence::{
    load_sequence_from_file, AnimSequence, SequenceCueEvent, SequenceEvent, SequenceRuntime,
    SequenceStep,
};

use flint_core::{EntityId, Result};
use flint_ecs::FlintWorld;
use flint_runtime::RuntimeSystem;

use clip::AnimationClip;
use property::AnimationPlayer;
use sequence::SequenceSync;
use sync::AnimationSync;

/// The game state's policy name for animation (ADR 0138 decision 7): a
/// state that pauses it holds every clip, sequence and flipbook. Registered
/// by the package; its manifest's `[without] policies` keeps it a known
/// name in a build without it.
pub const POLICY: &str = "animation";

/// Top-level animation system integrating clip playback with the ECS world.
///
/// Plays property clips and sequences itself, and the clip kinds other
/// packages register ([`AnimationSystem::add_kind`]) after them, each at its
/// slot. Implements `RuntimeSystem`.
#[derive(Default)]
pub struct AnimationSystem {
    pub(crate) player: AnimationPlayer,
    pub(crate) sync: AnimationSync,
    /// The registered kinds, in the order they run (by slot)
    pub(crate) kinds: Vec<Box<dyn ClipKind>>,
    /// One-shots of the registered kinds that ended since the last drain
    pub(crate) ends: Vec<ClipEnd>,
    pub(crate) sequence_sync: SequenceSync,
}

impl AnimationSystem {
    pub fn new() -> Self {
        Self {
            player: AnimationPlayer::new(),
            sync: AnimationSync::new(),
            kinds: Vec::new(),
            ends: Vec::new(),
            sequence_sync: SequenceSync::new(),
        }
    }

    /// Clear all animation state for a scene transition.
    /// Preserves clip registries (clips are reloadable).
    pub fn clear(&mut self) {
        self.sync.clear();
        for kind in &mut self.kinds {
            kind.clear();
        }
        self.ends.clear();
        self.sequence_sync.clear();
    }

    /// What reads the finished pose, each kind's in its slot's order: the
    /// rig's bone probes, then its two-bone IK (ADR 0070). Call after
    /// scripts, physics and `update` have written their transforms and
    /// before the renderer reads them.
    pub fn pose(&mut self, world: &mut FlintWorld) {
        flint_core::scope!("animation::pose");
        for kind in &mut self.kinds {
            kind.pose(world);
        }
    }

    /// Every skinned entity the kinds pose, with its asset and its bone
    /// matrices once computed, in the kinds' order.
    pub fn skins(&self, each: &mut EachSkin) {
        for kind in &self.kinds {
            kind.skins(each);
        }
    }

    // ── Sequences (timestamped animator events) ──

    /// Register a sequence by name
    pub fn add_sequence(&mut self, seq: AnimSequence) {
        self.sequence_sync.add_sequence(seq);
    }

    /// The folders the host read `*.sequence.toml` files from, which the
    /// warning about an unknown sequence then lists.
    pub fn set_sequence_search(&mut self, dirs: Vec<std::path::PathBuf>) {
        self.sequence_sync.set_searched(dirs);
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
    /// the kinds so the writes land this frame.
    pub fn advance_sequences(&mut self, world: &mut FlintWorld, dt: f64) {
        self.sequence_sync.advance(world, dt);
    }

    /// Seek an entity's sequence to `t` by deterministic replay: reset the
    /// registered kinds' playback, restart the sequence, and step the
    /// sequence with every registered kind together in `step_dt` increments
    /// (ADR 0139 section 5). The caller must first restore the animator
    /// component to its pre-sequence state (the sequence only ever *adds*
    /// writes). Cues fired during the replay are discarded. Returns the
    /// number of events fired.
    pub fn seek_sequence(
        &mut self,
        world: &mut FlintWorld,
        entity_id: EntityId,
        t: f64,
        step_dt: f64,
    ) -> usize {
        let step_dt = if step_dt > 0.0 { step_dt } else { 1.0 / 120.0 };
        for kind in &mut self.kinds {
            kind.reset(entity_id);
        }
        self.sequence_sync.restart(&entity_id);
        self.sequence_sync.set_playing(&entity_id, true);

        let mut elapsed = 0.0;
        let target = t.max(0.0);
        loop {
            let dt = (target - elapsed).min(step_dt);
            self.sequence_sync.advance(world, dt.max(0.0));
            for kind in &mut self.kinds {
                kind.sync_from_world(world);
                kind.advance(world, dt.max(0.0), &mut self.ends);
            }
            elapsed += dt;
            if dt <= 0.0 || elapsed >= target {
                break;
            }
        }
        self.sequence_sync.drain_cues();
        self.ends.clear();
        self.sequence_sync
            .state(&entity_id)
            .map(|s| s.fired_count())
            .unwrap_or(0)
    }

    // ── Clip registration (scene loading) ──

    /// Register a property tween clip
    pub fn add_property_clip(&mut self, clip: AnimationClip) {
        self.player.add_clip(clip);
    }

    /// Play a package's kind of clip (ADR 0139 section 5): the rig
    /// package's skeletal and node clips, the flipbook package's
    /// sprite-sheet clips. It runs at its [`ClipKind::slot`], after the kinds
    /// already there of the same slot.
    pub fn add_kind(&mut self, kind: Box<dyn ClipKind>) {
        let slot = kind.slot();
        let at = self.kinds.partition_point(|k| k.slot() <= slot);
        self.kinds.insert(at, kind);
    }

    /// The registered kinds, in the order they run.
    pub fn kinds(&self) -> impl Iterator<Item = &dyn ClipKind> {
        self.kinds.iter().map(|k| k.as_ref())
    }

    /// The registered kind called `name`.
    pub fn kind(&self, name: &str) -> Option<&dyn ClipKind> {
        self.kinds().find(|k| k.name() == name)
    }

    /// The registered kind of type `K`, for its package's own calls (the
    /// rig's skeletons and bone matrices).
    pub fn kind_of<K: ClipKind>(&self) -> Option<&K> {
        self.kinds
            .iter()
            .find_map(|k| k.as_any().downcast_ref::<K>())
    }

    /// [`Self::kind_of`], to change it.
    pub fn kind_of_mut<K: ClipKind>(&mut self) -> Option<&mut K> {
        self.kinds
            .iter_mut()
            .find_map(|k| k.as_any_mut().downcast_mut::<K>())
    }

    // ── Runtime queries ──

    /// The one-shots of the registered kinds (a flipbook's) that ended
    /// since the last drain: scripts' `on_animation_end(clip)`.
    pub fn drain_clip_ends(&mut self) -> Vec<ClipEnd> {
        std::mem::take(&mut self.ends)
    }

    /// The playback driving an entity's property clip
    pub fn property_state(&self, entity_id: &EntityId) -> Option<&ClipPlaybackState> {
        self.sync.playback_state(entity_id)
    }

    // ── Sync & advance (player/preview update loops) ──

    /// Sync property animation state from ECS world
    pub fn sync_property_from_world(&mut self, world: &FlintWorld) {
        self.sync.sync_from_world(world, &self.player);
    }

    /// Advance property animations and write results back to ECS
    pub fn advance_property_and_write(&mut self, world: &mut FlintWorld, dt: f64) {
        self.sync.advance_and_write(world, &self.player, dt);
    }

    /// Follow the world's animators and step every registered kind `dt`
    /// seconds on, in slot order: the kinds' part of [`RuntimeSystem::update`],
    /// for a host that steps property clips and sequences itself (the model
    /// previewer).
    pub fn advance_kinds(&mut self, world: &mut FlintWorld, dt: f64) {
        for kind in &mut self.kinds {
            flint_core::scope!("animation::kind", kind.name());
            kind.sync_from_world(world);
            kind.advance(world, dt, &mut self.ends);
        }
    }
}

impl RuntimeSystem for AnimationSystem {
    fn initialize(&mut self, world: &mut FlintWorld) -> Result<()> {
        self.sync.sync_from_world(world, &self.player);
        for kind in &mut self.kinds {
            kind.sync_from_world(world);
        }
        self.sequence_sync.sync_from_world(world);
        let kinds: String = self
            .kinds
            .iter()
            .map(|k| {
                format!(
                    ", {} {} clips, {} {} entities",
                    k.clip_count(),
                    k.name(),
                    k.active_count(),
                    k.name()
                )
            })
            .collect();
        log::debug!(target: "load",
            "Animation system initialized ({} property clips, {} sequences, {} property entities, {} sequence entities{kinds})",
            self.player.clip_count(),
            self.sequence_sync.sequence_count(),
            self.sync.active_count(),
            self.sequence_sync.active_count()
        );
        Ok(())
    }

    fn update(&mut self, world: &mut FlintWorld, dt: f64) -> Result<()> {
        flint_core::scope!("animation::update");
        // Property animation
        {
            flint_core::scope!("animation::property");
            self.sync.sync_from_world(world, &self.player);
            self.sync.advance_and_write(world, &self.player, dt);
        }

        // Sequences write animator fields; run them first so the kinds see
        // the writes this frame.
        {
            flint_core::scope!("animation::sequences");
            self.sequence_sync.sync_from_world(world);
            self.sequence_sync.advance(world, dt);
        }

        // The registered kinds, by slot: skeletal, node, flipbook
        self.advance_kinds(world, dt);

        Ok(())
    }
}
