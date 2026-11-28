//! The rig's two clip kinds on the animation package's clock (ADR 0139
//! section 5, #1621): [`SkeletalKind`] poses a skeleton's joints into bone
//! matrices, and [`NodeKind`] moves a model's child nodes found by name. Each
//! is what `SkeletalSync` and `NodeSync` were inside `AnimationSystem`,
//! behind [`ClipKind`], at the slots they ran at (skeletal, then node, then
//! the flipbook's), so a frame steps exactly as before the split.
//!
//! The skeletal kind also holds what reads its pose: bone probes, then
//! two-bone IK (ADR 0070), in [`ClipKind::pose`], and the skinned entities
//! whose bones go to the GPU ([`ClipKind::skins`]).
//!
//! [`Rig`] is the rig's calls on an [`AnimationSystem`]: what the system
//! answered for skeletons and nodes before #1621, spelled the same, found
//! through the kinds ([`AnimationSystem::kind_of`]). Registering a clip, a
//! skeleton or a node map adds the kind it needs if the system has none.

use crate::ik_pass::IkPass;
use crate::node_clip::NodeClip;
use crate::node_sync::{NodePlaybackState, NodeSync};
use crate::skeletal_clip::SkeletalClip;
use crate::skeletal_sync::{LayerContribution, SkeletalPlaybackState, SkeletalSync};
use crate::skeleton::Skeleton;
use flint_animation::{
    AnimLayer, AnimationSystem, ClipEnd, ClipKind, ClipPlaybackState, EachSkin, SLOT_NODE,
    SLOT_SKELETAL,
};
use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::{BTreeMap, HashMap};

/// The skeletal kind's name.
pub const SKELETAL: &str = "skeletal";
/// The node kind's name.
pub const NODE: &str = "node";

/// Skeletal clips on skinned models: joints to bone matrices, layers and
/// crossfades composed (`blend.rs`), then bone probes and IK.
#[derive(Default)]
pub struct SkeletalKind {
    sync: SkeletalSync,
    /// Skinned entities and their model's asset, in id order: whose bones
    /// go to the GPU. Every skinned model the scene loaded, whether a
    /// skeleton could be built for it or not.
    skinned: Vec<(EntityId, String)>,
    ik: IkPass,
}

impl SkeletalKind {
    pub fn new() -> Self {
        Self::default()
    }

    /// The playback and poses.
    pub fn sync(&self) -> &SkeletalSync {
        &self.sync
    }

    /// [`Self::sync`], to change it.
    pub fn sync_mut(&mut self) -> &mut SkeletalSync {
        &mut self.sync
    }

    /// Skinned entities and their asset, in id order.
    pub fn skinned(&self) -> &[(EntityId, String)] {
        &self.skinned
    }

    /// Say these entities are skinned, drawing these assets.
    pub fn add_skinned(&mut self, skinned: impl IntoIterator<Item = (EntityId, String)>) {
        self.skinned.extend(skinned);
        self.skinned.sort();
    }
}

impl ClipKind for SkeletalKind {
    fn name(&self) -> &'static str {
        SKELETAL
    }

    fn slot(&self) -> u32 {
        SLOT_SKELETAL
    }

    fn clip_count(&self) -> usize {
        self.sync.clip_count()
    }

    fn active_count(&self) -> usize {
        self.sync.active_count()
    }

    fn has_clip(&self, clip: &str) -> bool {
        self.sync.get_clip_duration(clip).is_some()
    }

    fn sync_from_world(&mut self, world: &FlintWorld) {
        self.sync.sync_from_world(world);
    }

    /// The clips on, the bone matrices computed, then the finished
    /// crossfades retired in the ECS (or they re-arm forever).
    fn advance(&mut self, world: &mut FlintWorld, dt: f64, _ends: &mut Vec<ClipEnd>) {
        flint_core::scope!("animation::skeletal");
        self.sync.advance_and_compute(dt);
        self.sync.write_back(world);
    }

    /// Bone probes, then two-bone IK (ADR 0070): the last writer of
    /// transforms the renderer sees.
    fn pose(&mut self, world: &mut FlintWorld) {
        {
            flint_core::scope!("publish_bone_probes");
            publish_bone_probes(&self.sync, world);
        }
        flint_core::scope!("animation::ik");
        self.ik.run(world);
    }

    fn skins(&self, each: &mut EachSkin) {
        for (id, asset) in &self.skinned {
            each(*id, asset, self.sync.bone_matrices(id));
        }
    }

    fn state(&self, entity: EntityId) -> Option<&ClipPlaybackState> {
        self.sync.get_playback_state(&entity)
    }

    fn reset(&mut self, entity: EntityId) {
        self.sync.reset_state(&entity);
    }

    fn clear(&mut self) {
        self.sync.clear();
        self.ik.clear();
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// Node clips: a model's child nodes, found by name, moved by the joint
/// tracks skeletal clips use (until #1664).
#[derive(Default)]
pub struct NodeKind {
    sync: NodeSync,
}

impl NodeKind {
    pub fn new() -> Self {
        Self::default()
    }

    /// The playback.
    pub fn sync(&self) -> &NodeSync {
        &self.sync
    }

    /// [`Self::sync`], to change it.
    pub fn sync_mut(&mut self) -> &mut NodeSync {
        &mut self.sync
    }
}

impl ClipKind for NodeKind {
    fn name(&self) -> &'static str {
        NODE
    }

    fn slot(&self) -> u32 {
        SLOT_NODE
    }

    fn clip_count(&self) -> usize {
        self.sync.clip_count()
    }

    fn active_count(&self) -> usize {
        self.sync.active_count()
    }

    fn has_clip(&self, clip: &str) -> bool {
        self.sync.get_clip_duration(clip).is_some()
    }

    fn sync_from_world(&mut self, world: &FlintWorld) {
        self.sync.sync_from_world(world);
    }

    fn advance(&mut self, world: &mut FlintWorld, dt: f64, _ends: &mut Vec<ClipEnd>) {
        flint_core::scope!("animation::node");
        self.sync.advance_and_apply(world, dt);
    }

    fn state(&self, entity: EntityId) -> Option<&ClipPlaybackState> {
        self.sync.get_playback_state(&entity)
    }

    /// A seek replays it from the start with the sequence, as the skeletal
    /// kind's (before #1621 a seek stepped skeletal clips alone).
    fn reset(&mut self, entity: EntityId) {
        self.sync.reset_state(&entity);
    }

    fn clear(&mut self) {
        self.sync.clear();
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// Write each `bone_probe`'s joint, in model space, into its component, right
/// after the pose is computed, so scripts read this frame's pose (a seat camera
/// following an eye, say). Moved from the player by #1620, to the rig by #1621.
pub fn publish_bone_probes(skeletal: &SkeletalSync, world: &mut FlintWorld) {
    let probe_ids: Vec<EntityId> = world
        .entities_with_component(comp::BONE_PROBE)
        .iter()
        .copied()
        .collect();
    for id in probe_ids {
        let joint = world
            .view(id, comp::BONE_PROBE)
            .opt_str("joint")
            .map(String::from);
        let Some(joint) = joint else {
            continue;
        };
        if let Some(pos) = skeletal.joint_position(&id, &joint) {
            for (field, value) in [("x", pos[0]), ("y", pos[1]), ("z", pos[2])] {
                let _ = world.set_field(
                    id,
                    comp::BONE_PROBE,
                    field,
                    toml::Value::Float(value as f64),
                );
            }
        }
    }
}

/// The rig's calls on an animation system, as it answered them before
/// #1621: through its [`SkeletalKind`] and [`NodeKind`].
pub trait Rig {
    /// The skeletal kind, if the system has one.
    fn skeletal(&self) -> Option<&SkeletalKind>;
    /// The skeletal kind, added if the system has none.
    fn skeletal_mut(&mut self) -> &mut SkeletalKind;
    /// The node kind, if the system has one.
    fn nodes(&self) -> Option<&NodeKind>;
    /// The node kind, added if the system has none.
    fn nodes_mut(&mut self) -> &mut NodeKind;

    // ── Registration ──

    /// Register a skeletal animation clip
    fn add_skeletal_clip(&mut self, clip: SkeletalClip) {
        self.skeletal_mut().sync.add_clip(clip);
    }

    /// Register a skeleton for an entity
    fn add_skeleton(&mut self, entity_id: EntityId, skeleton: Skeleton) {
        self.skeletal_mut().sync.add_skeleton(entity_id, skeleton);
    }

    /// Register a node transform animation clip
    fn add_node_clip(&mut self, clip: NodeClip) {
        self.nodes_mut().sync.add_clip(clip);
    }

    /// Register an entity's node name → EntityId mapping for node animation
    fn register_node_entity(&mut self, entity_id: EntityId, node_map: HashMap<String, EntityId>) {
        self.nodes_mut().sync.register_entity(entity_id, node_map);
    }

    // ── Queries ──

    /// Get computed bone matrices for an entity (skeletal animation)
    fn bone_matrices(&self, entity_id: &EntityId) -> Option<&[[[f32; 4]; 4]]> {
        self.skeletal()?.sync.bone_matrices(entity_id)
    }

    /// The skeleton driving an entity (joint hierarchy + model-space globals
    /// from the last pose computation, e.g. for a debug armature overlay)
    fn skeleton(&self, entity_id: &EntityId) -> Option<&Skeleton> {
        self.skeletal()?.sync.skeleton(entity_id)
    }

    /// Model-space position of a named joint (bone_probe: camera anchors,
    /// attachment points)
    fn joint_position(&self, entity_id: &EntityId, joint: &str) -> Option<[f32; 3]> {
        self.skeletal()?.sync.joint_position(entity_id, joint)
    }

    /// Turn a joint by hand, in its own frame, on top of what the clips say;
    /// `None` gives it back. Takes effect at once, with or without a clip.
    fn set_joint_offset(&mut self, entity_id: EntityId, joint: usize, rotation: Option<[f32; 4]>) {
        self.skeletal_mut()
            .sync
            .set_joint_offset(entity_id, joint, rotation);
    }

    /// The joints turned by [`Self::set_joint_offset`].
    fn joint_offsets(&self, entity_id: &EntityId) -> Option<&BTreeMap<usize, [f32; 4]>> {
        self.skeletal()?.sync.joint_offsets(entity_id)
    }

    /// Seconds left and target of a layer weight ramp, if one is running
    fn skeletal_layer_fade(&self, entity_id: &EntityId, index: usize) -> Option<(f32, f32)> {
        self.skeletal()?.sync.layer_fade(entity_id, index)
    }

    /// Get skeletal playback state for an entity
    fn skeletal_playback_state(&self, entity_id: &EntityId) -> Option<&SkeletalPlaybackState> {
        self.skeletal()?.sync.get_playback_state(entity_id)
    }

    /// Get node playback state for an entity
    fn node_playback_state(&self, entity_id: &EntityId) -> Option<&NodePlaybackState> {
        self.nodes()?.sync.get_playback_state(entity_id)
    }

    /// Get the duration of a skeletal clip by name
    fn skeletal_clip_duration(&self, clip_name: &str) -> Option<f64> {
        self.skeletal()?.sync.get_clip_duration(clip_name)
    }

    /// Get the duration of a node clip by name
    fn node_clip_duration(&self, clip_name: &str) -> Option<f64> {
        self.nodes()?.sync.get_clip_duration(clip_name)
    }

    /// Names of all registered skeletal clips (sorted)
    fn skeletal_clip_names(&self) -> Vec<String> {
        let mut names = self
            .skeletal()
            .map(|k| k.sync.clip_names())
            .unwrap_or_default();
        names.sort();
        names
    }

    /// The resolved layer list driving an entity
    fn skeletal_layers(&self, entity_id: &EntityId) -> Option<&[AnimLayer]> {
        self.skeletal()?.sync.layers(entity_id)
    }

    /// Per-joint composition bookkeeping from the last skeletal advance
    fn skeletal_layer_contribution(&self, entity_id: &EntityId) -> Option<&LayerContribution> {
        self.skeletal()?.sync.layer_contribution(entity_id)
    }

    /// Which joints a skeletal clip keys
    fn skeletal_clip_keyed_joints(&self, clip: &str, joint_count: usize) -> Option<Vec<bool>> {
        self.skeletal()?.sync.clip_keyed_joints(clip, joint_count)
    }

    // ── Stepping by hand (the model previewer) ──

    /// Sync skeletal animation state from ECS world
    fn sync_skeletal_from_world(&mut self, world: &FlintWorld) {
        self.skeletal_mut().sync.sync_from_world(world);
    }

    /// Advance skeletal animations and compute bone matrices
    fn advance_skeletal(&mut self, dt: f64) {
        self.skeletal_mut().sync.advance_and_compute(dt);
    }

    /// Retire finished crossfades / layer fades in the ECS. Must follow
    /// `advance_skeletal` every frame, or the next sync re-arms them (a
    /// `blend_target` left set restarts the same blend forever).
    fn write_back_skeletal(&mut self, world: &mut FlintWorld) {
        self.skeletal_mut().sync.write_back(world);
    }

    /// Sync node animation state from ECS world
    fn sync_node_from_world(&mut self, world: &FlintWorld) {
        self.nodes_mut().sync.sync_from_world(world);
    }

    /// Advance node animations and apply transforms to ECS
    fn advance_node_and_apply(&mut self, world: &mut FlintWorld, dt: f64) {
        self.nodes_mut().sync.advance_and_apply(world, dt);
    }

    /// Reset skeletal playback state for an entity
    fn reset_skeletal_state(&mut self, entity_id: &EntityId) {
        self.skeletal_mut().sync.reset_state(entity_id);
    }

    /// Set skeletal playback time for an entity (scrubbing)
    fn set_skeletal_playback_time(&mut self, entity_id: &EntityId, time: f64) {
        self.skeletal_mut().sync.set_playback_time(entity_id, time);
    }

    /// Set node playback time for an entity (scrubbing)
    fn set_node_playback_time(&mut self, entity_id: &EntityId, time: f64) {
        self.nodes_mut().sync.set_playback_time(entity_id, time);
    }

    /// Runtime-only mute for a layer (previewer solo/mute); not persisted
    fn set_skeletal_layer_mute(&mut self, entity_id: EntityId, index: usize, muted: bool) {
        self.skeletal_mut()
            .sync
            .set_layer_mute(entity_id, index, muted);
    }

    /// Clear runtime mutes for an entity
    fn clear_skeletal_layer_mutes(&mut self, entity_id: &EntityId) {
        self.skeletal_mut().sync.clear_layer_mutes(entity_id);
    }

    /// Two-bone IK (ADR 0070) over the world as it stands, with the
    /// skeletal kind's pass (its rest captures kept between calls).
    fn run_ik(&mut self, world: &mut FlintWorld) {
        flint_core::scope!("animation::ik");
        self.skeletal_mut().ik.run(world);
    }
}

impl Rig for AnimationSystem {
    fn skeletal(&self) -> Option<&SkeletalKind> {
        self.kind_of::<SkeletalKind>()
    }

    fn skeletal_mut(&mut self) -> &mut SkeletalKind {
        if self.kind_of::<SkeletalKind>().is_none() {
            self.add_kind(Box::new(SkeletalKind::new()));
        }
        self.kind_of_mut::<SkeletalKind>()
            .expect("the skeletal kind was just added")
    }

    fn nodes(&self) -> Option<&NodeKind> {
        self.kind_of::<NodeKind>()
    }

    fn nodes_mut(&mut self) -> &mut NodeKind {
        if self.kind_of::<NodeKind>().is_none() {
            self.add_kind(Box::new(NodeKind::new()));
        }
        self.kind_of_mut::<NodeKind>()
            .expect("the node kind was just added")
    }
}
