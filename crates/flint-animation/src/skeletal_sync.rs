//! Bridges ECS entities to skeletal animation playback
//!
//! Manages per-entity skeleton state, skeletal clip playback, and bone matrix computation.

use crate::blend::{
    blend_joint, blend_poses, quat_conjugate, quat_mul, quat_normalize, quat_slerp,
};
use crate::playback_state::{AnimLayer, ClipPlaybackState, LayerMode};
use crate::skeletal_clip::{JointProperty, SkeletalClip};
use crate::skeletal_sampler::sample_joint_track;
use crate::skeleton::{JointPose, Skeleton};
use flint_core::components as comp;
use flint_core::toml_util::{toml_f32, toml_f64};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::HashMap;

/// Per-entity skeletal playback state (alias for the shared [`ClipPlaybackState`])
pub type SkeletalPlaybackState = ClipPlaybackState;

/// `last_writer` value for a joint nothing animated this frame (rest pose).
pub const WRITER_REST: u8 = u8::MAX;
/// `last_writer` value for a joint written by the base clip / crossfade.
pub const WRITER_BASE: u8 = 0;

/// Per-joint bookkeeping of how the last pose was composed — the data the
/// previewer colours its skeleton overlay with.
#[derive(Debug, Clone, Default)]
pub struct LayerContribution {
    /// Per joint: [`WRITER_BASE`], `n` = layer `n-1` wrote it last, or
    /// [`WRITER_REST`] if nothing keyed it.
    pub last_writer: Vec<u8>,
    /// `[layer][joint]` effective weight applied (0 when masked, un-keyed,
    /// muted or inactive).
    pub weights: Vec<Vec<f32>>,
    /// `[layer][joint]` whether the layer's clip keys the joint at all.
    pub keyed: Vec<Vec<bool>>,
    /// `[layer][joint]` resolved mask (all `true` when no mask is set).
    pub mask_bits: Vec<Vec<bool>>,
    /// Mask strings the `mask_bits` were resolved from (cache key).
    mask_keys: Vec<String>,
}

impl LayerContribution {
    fn reset(&mut self, layer_count: usize, joint_count: usize) {
        self.last_writer.clear();
        self.last_writer.resize(joint_count, WRITER_REST);
        self.weights.resize_with(layer_count, Vec::new);
        self.keyed.resize_with(layer_count, Vec::new);
        self.mask_bits.resize_with(layer_count, Vec::new);
        self.mask_keys.resize_with(layer_count, String::new);
        self.weights.truncate(layer_count);
        self.keyed.truncate(layer_count);
        self.mask_bits.truncate(layer_count);
        self.mask_keys.truncate(layer_count);
        for li in 0..layer_count {
            self.weights[li].clear();
            self.weights[li].resize(joint_count, 0.0);
            self.keyed[li].clear();
            self.keyed[li].resize(joint_count, false);
        }
    }

    /// Weight layer `li` applied to `joint`, 0 if out of range.
    pub fn weight(&self, li: usize, joint: usize) -> f32 {
        self.weights
            .get(li)
            .and_then(|w| w.get(joint))
            .copied()
            .unwrap_or(0.0)
    }

    pub fn is_keyed(&self, li: usize, joint: usize) -> bool {
        self.keyed
            .get(li)
            .and_then(|k| k.get(joint))
            .copied()
            .unwrap_or(false)
    }

    pub fn in_mask(&self, li: usize, joint: usize) -> bool {
        self.mask_bits
            .get(li)
            .and_then(|m| m.get(joint))
            .copied()
            .unwrap_or(false)
    }
}

/// Registry of skeletal clips and per-entity skeleton state
#[derive(Default)]
pub struct SkeletalSync {
    clips: HashMap<String, SkeletalClip>,
    skeletons: HashMap<EntityId, Skeleton>,
    states: HashMap<EntityId, SkeletalPlaybackState>,
    /// Entities whose crossfade finished this frame. The ECS `blend_target`
    /// field must be cleared for them (see [`Self::write_back`]) — otherwise
    /// the next `sync_from_world` sees a target that no longer matches the
    /// (now-cleared) runtime target and re-arms the same crossfade forever.
    completed_blends: Vec<EntityId>,
    /// Per-joint composition bookkeeping from the last `advance_and_compute`.
    contributions: HashMap<EntityId, LayerContribution>,
    /// Runtime-only per-layer mute flags (previewer solo/mute). Kept out of
    /// `ClipPlaybackState` so `sync_from_world` never clobbers them.
    mutes: HashMap<EntityId, Vec<bool>>,
    /// In-flight layer weight ramps, `[entity][layer]`. Like `mutes`, kept
    /// outside the playback state: `sync_from_world` copies every layer's
    /// weight from the ECS each frame, so the ramp is *driven* by the ECS
    /// (`fade_target` + `fade_duration`) and the ramped weight is written
    /// back each frame (see [`Self::write_back`]) — the same contract as
    /// `blend_target`.
    fades: HashMap<EntityId, Vec<Option<LayerFade>>>,
    /// `(entity, layer)` fades that finished this frame; `write_back`
    /// zeroes their ECS `fade_duration` so the next sync doesn't re-arm.
    completed_fades: Vec<(EntityId, usize)>,
}

/// One layer weight ramp
#[derive(Debug, Clone, Copy, PartialEq)]
struct LayerFade {
    from: f32,
    to: f32,
    duration: f32,
    elapsed: f32,
}

impl LayerFade {
    fn weight(&self) -> f32 {
        let t = (self.elapsed / self.duration).clamp(0.0, 1.0);
        self.from + (self.to - self.from) * t
    }
    fn done(&self) -> bool {
        self.elapsed >= self.duration
    }
}

impl SkeletalSync {
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all skeletal animation state for a scene transition.
    pub fn clear(&mut self) {
        self.clips.clear();
        self.skeletons.clear();
        self.states.clear();
        self.completed_blends.clear();
        self.contributions.clear();
        self.mutes.clear();
        self.fades.clear();
        self.completed_fades.clear();
    }

    /// Parse the `animator.layers` array, falling back to the legacy
    /// `layer_clip` / `layer_weight` pair as a single additive layer.
    pub fn read_layers(animator: &toml::Value) -> Vec<AnimLayer> {
        if let Some(arr) = animator.get("layers").and_then(|v| v.as_array()) {
            if !arr.is_empty() {
                return arr.iter().map(AnimLayer::from_toml).collect();
            }
        }
        let legacy = animator
            .get("layer_clip")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if legacy.is_empty() {
            return Vec::new();
        }
        let weight = animator
            .get("layer_weight")
            .and_then(toml_f32)
            .unwrap_or(1.0);
        vec![AnimLayer::new(legacy, weight)]
    }

    /// Copy the ECS layer list onto the runtime state. Weight/mode/mask/
    /// speed are live dials; a clip change restarts that layer's clock.
    ///
    /// A layer with `fade_duration > 0` starts (or continues) a weight
    /// ramp in `fades` instead of copying its weight: the ramp owns the
    /// weight until it completes. Any other weight write cancels the ramp.
    fn apply_ecs_layers(
        state: &mut SkeletalPlaybackState,
        animator: &toml::Value,
        fades: &mut Vec<Option<LayerFade>>,
    ) {
        let ecs = Self::read_layers(animator);
        state.layers.truncate(ecs.len());
        fades.resize(ecs.len(), None);
        for (i, incoming) in ecs.into_iter().enumerate() {
            let fading = incoming.fade_duration > 0.0;
            if let Some(cur) = state.layers.get_mut(i) {
                if cur.clip != incoming.clip {
                    cur.clip = incoming.clip;
                    cur.time = 0.0;
                }
                cur.mode = incoming.mode;
                cur.mask = incoming.mask;
                cur.speed = incoming.speed;
                cur.fade_target = incoming.fade_target;
                cur.fade_duration = incoming.fade_duration;
                if fading {
                    let same = fades[i].is_some_and(|f| {
                        f.to == incoming.fade_target && f.duration == incoming.fade_duration
                    });
                    if !same {
                        fades[i] = Some(LayerFade {
                            from: cur.weight,
                            to: incoming.fade_target,
                            duration: incoming.fade_duration,
                            elapsed: 0.0,
                        });
                    }
                } else {
                    cur.weight = incoming.weight;
                    fades[i] = None;
                }
            } else {
                if fading {
                    fades[i] = Some(LayerFade {
                        from: incoming.weight,
                        to: incoming.fade_target,
                        duration: incoming.fade_duration,
                        elapsed: 0.0,
                    });
                }
                state.layers.push(incoming);
            }
        }
        // Legacy mirror
        state.layer_clip = state
            .layers
            .first()
            .map(|l| l.clip.clone())
            .unwrap_or_default();
        state.layer_weight = state.layers.first().map(|l| l.weight).unwrap_or(1.0);
        state.layer_time = state.layers.first().map(|l| l.time).unwrap_or(0.0);
    }

    /// Register a skeletal clip by name
    pub fn add_clip(&mut self, clip: SkeletalClip) {
        self.clips.insert(clip.name.clone(), clip);
    }

    /// Register a skeleton for an entity
    pub fn add_skeleton(&mut self, entity_id: EntityId, skeleton: Skeleton) {
        self.skeletons.insert(entity_id, skeleton);
    }

    /// Number of registered skeletal clips
    pub fn clip_count(&self) -> usize {
        self.clips.len()
    }

    /// Number of active skeletal entities
    pub fn active_count(&self) -> usize {
        self.states.len()
    }

    /// Return the names of all registered skeletal clips
    pub fn clip_names(&self) -> Vec<String> {
        self.clips.keys().cloned().collect()
    }

    /// Reset playback state for an entity (used when externally switching clips)
    pub fn reset_state(&mut self, entity_id: &EntityId) {
        self.states.remove(entity_id);
        self.fades.remove(entity_id);
        self.completed_fades.retain(|(e, _)| e != entity_id);
    }

    /// Check if an entity has a skeleton registered
    pub fn has_skeleton(&self, entity_id: &EntityId) -> bool {
        self.skeletons.contains_key(entity_id)
    }

    /// Scan the world for entities with `animator` + `skeleton` components.
    /// Creates playback states for newly discovered entities.
    /// Updates blend_target/blend_duration for existing entities if changed in ECS.
    pub fn sync_from_world(&mut self, world: &FlintWorld) {
        for entity_id in world.entities_with_components(&[comp::ANIMATOR, comp::SKELETON]) {
            let Some(components) = world.get_components(entity_id) else {
                continue;
            };

            let Some(animator) = components.get(comp::ANIMATOR) else {
                continue;
            };
            // Must also have a skeleton registered for this entity
            if !self.skeletons.contains_key(&entity_id) {
                continue;
            }

            // If already tracked, check for blend_target changes from ECS
            if let Some(state) = self.states.get_mut(&entity_id) {
                let ecs_blend_target = animator
                    .get("blend_target")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let ecs_blend_duration = animator
                    .get("blend_duration")
                    .and_then(toml_f32)
                    .unwrap_or(0.3);

                // Start a new crossfade if ECS sets a new blend_target
                if !ecs_blend_target.is_empty()
                    && ecs_blend_target != state.blend_target
                    && self.clips.contains_key(&ecs_blend_target)
                {
                    state.blend_target = ecs_blend_target;
                    state.blend_duration = ecs_blend_duration;
                    state.blend_elapsed = 0.0;
                }

                // Speed and loop are live dials too (previewer Speed
                // field, `set_anim_speed` from scripts).
                state.speed = animator.get("speed").and_then(toml_f64).unwrap_or(1.0);
                state.looping = animator
                    .get("loop")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                // Layers follow the ECS every frame (weights are live
                // dials; changing a clip restarts that layer's clock).
                let fades = self.fades.entry(entity_id).or_default();
                Self::apply_ecs_layers(state, animator, fades);
                continue;
            }

            let clip_name = animator
                .get("clip")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if clip_name.is_empty() || !self.clips.contains_key(&clip_name) {
                continue;
            }

            let speed = animator.get("speed").and_then(toml_f64).unwrap_or(1.0);

            let looping = animator
                .get("loop")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            let autoplay = animator
                .get("autoplay")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let playing = animator
                .get("playing")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                || autoplay;

            // A resolved clip that is not playing holds its first frame
            // forever. Both flags default to false, and TOML makes it easy to
            // strand them on a neighbouring component table (a key after the
            // next [table] header belongs to THAT table) — which reads as
            // "the animation is stuck on one frame" with nothing else to
            // explain it. Say so rather than render a statue in silence.
            if !playing {
                println!(
                    "WARNING: entity {:?} has animator clip '{}' but playing=false and \
                     autoplay=false — it will hold its first frame. Check that `playing`/\
                     `autoplay` are under [entities.<name>.animator].",
                    entity_id, clip_name
                );
            }

            let mut state = SkeletalPlaybackState::new(clip_name, speed, looping, playing);

            // Read initial blend fields
            state.blend_target = animator
                .get("blend_target")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            state.blend_duration = animator
                .get("blend_duration")
                .and_then(toml_f32)
                .unwrap_or(0.3);
            let fades = self.fades.entry(entity_id).or_default();
            Self::apply_ecs_layers(&mut state, animator, fades);

            self.states.insert(entity_id, state);
        }
    }

    /// Advance all skeletal playbacks and compute bone matrices.
    /// Handles crossfade blending when `blend_target` is set.
    pub fn advance_and_compute(&mut self, dt: f64) {
        let entity_ids: Vec<EntityId> = self.states.keys().copied().collect();

        for entity_id in entity_ids {
            let state = self.states.get_mut(&entity_id).unwrap();
            let Some(clip) = self.clips.get(&state.clip_name) else {
                continue;
            };
            let Some(skeleton) = self.skeletons.get_mut(&entity_id) else {
                continue;
            };

            // Advance time
            if state.playing {
                state.time += dt * state.speed;

                if state.looping {
                    if clip.duration > 0.0 {
                        if state.time >= clip.duration {
                            state.time %= clip.duration;
                        } else if state.time < 0.0 {
                            state.time = clip.duration - (-state.time % clip.duration);
                        }
                    }
                } else if state.time >= clip.duration {
                    state.time = clip.duration;
                    state.playing = false;
                } else if state.time < 0.0 {
                    state.time = 0.0;
                    state.playing = false;
                }
            }

            // Start from the rest pose every frame. Clips are sparse (an
            // exporter drops tracks that hold the rest value), so a joint
            // the current clip doesn't key must read as "rest" — not as
            // whatever the *previous* clip left in the persistent buffer.
            // Otherwise walk → idle leaves the legs frozen mid-stride.
            if skeleton.rest_poses.len() == skeleton.local_poses.len() {
                skeleton.local_poses.clone_from(&skeleton.rest_poses);
            }

            // Sample current clip into local_poses
            Self::sample_clip_into_poses(clip, state.time, &mut skeleton.local_poses);

            // Handle crossfade blending
            let is_blending = !state.blend_target.is_empty() && state.blend_duration > 0.0;

            if is_blending {
                let target_clip_name = state.blend_target.clone();
                if let Some(target_clip) = self.clips.get(&target_clip_name) {
                    state.blend_elapsed += dt as f32;
                    let blend_weight = (state.blend_elapsed / state.blend_duration).min(1.0);

                    // Sample the target clip over the REST pose (same sparse-
                    // clip rule as above) so joints the target doesn't key
                    // blend toward rest instead of sticking at the old clip.
                    let joint_count = skeleton.joint_count();
                    let mut target_poses: Vec<JointPose> =
                        if skeleton.rest_poses.len() == joint_count {
                            skeleton.rest_poses.clone()
                        } else {
                            skeleton.local_poses.clone()
                        };
                    // Compute target clip's time (starts from 0 for the blend-in clip)
                    let target_time = state.blend_elapsed as f64 * state.speed;
                    Self::sample_clip_into_poses(target_clip, target_time, &mut target_poses);

                    // Blend current poses (already in skeleton.local_poses) with target
                    let current_poses: Vec<JointPose> = skeleton.local_poses.clone();
                    blend_poses(
                        &current_poses,
                        &target_poses,
                        blend_weight,
                        &mut skeleton.local_poses,
                    );

                    // Check if blend is complete
                    if blend_weight >= 1.0 {
                        // Transition to target clip
                        state.clip_name = target_clip_name;
                        state.time = target_time;
                        state.blend_target.clear();
                        state.blend_elapsed = 0.0;
                        self.completed_blends.push(entity_id);
                    }
                } else {
                    // Target clip not found, clear blend
                    state.blend_target.clear();
                    state.blend_elapsed = 0.0;
                    self.completed_blends.push(entity_id);
                }
            }

            // ── Animation layers ────────────────────────────────────
            // Each loops on its own clock and is composed AFTER base +
            // blend (so layers survive crossfades), in array order.
            //   Additive: keyed joints contribute delta-from-REST × weight.
            //   Override: keyed joints blend toward the sampled pose by
            //             weight.
            // Both honour an optional subtree mask. Un-keyed joints are
            // untouched (composing identity would corrupt joints whose
            // rest rotation is non-identity). The pre-layer pose is
            // RESTORED after matrix computation: local_poses persists
            // across frames and base clips only overwrite the joints they
            // key, so leaving the deltas in would compound them every
            // frame (feet were windmilling at 120°+).
            let joint_count = skeleton.joint_count();
            let pre_layer: Vec<JointPose> = skeleton.local_poses.clone();

            let contrib = self.contributions.entry(entity_id).or_default();
            contrib.reset(state.layers.len(), joint_count);
            for track in &clip.joint_tracks {
                if let Some(w) = contrib.last_writer.get_mut(track.joint_index) {
                    *w = WRITER_BASE;
                }
            }
            if let Some(target) = self.clips.get(&state.blend_target) {
                for track in &target.joint_tracks {
                    if let Some(w) = contrib.last_writer.get_mut(track.joint_index) {
                        *w = WRITER_BASE;
                    }
                }
            }
            let mutes = self.mutes.get(&entity_id);
            let mut fades = self.fades.get_mut(&entity_id);

            for (li, layer) in state.layers.iter_mut().enumerate() {
                // Advance an in-flight weight ramp. Done before the mute /
                // zero-weight early-outs so a fade-in from 0 actually starts.
                if let Some(fade) = fades.as_deref_mut().and_then(|f| f.get_mut(li)) {
                    if let Some(f) = fade {
                        f.elapsed += dt as f32;
                        layer.weight = f.weight();
                        if f.done() {
                            layer.weight = f.to;
                            layer.fade_duration = 0.0;
                            *fade = None;
                            self.completed_fades.push((entity_id, li));
                        }
                    }
                }
                // Resolve mask (cached by mask string) and keyed set even
                // for inactive layers so the previewer can show them.
                if contrib.mask_keys[li] != layer.mask || contrib.mask_bits[li].len() != joint_count
                {
                    contrib.mask_bits[li] = match layer.mask.as_str() {
                        "" => vec![true; joint_count],
                        root => skeleton
                            .subtree_mask(root)
                            .unwrap_or_else(|| vec![true; joint_count]),
                    };
                    contrib.mask_keys[li] = layer.mask.clone();
                }
                let Some(layer_clip) = self.clips.get(&layer.clip) else {
                    continue;
                };
                for track in &layer_clip.joint_tracks {
                    if let Some(k) = contrib.keyed[li].get_mut(track.joint_index) {
                        *k = true;
                    }
                }
                let muted = mutes.and_then(|m| m.get(li)).copied().unwrap_or(false);
                if muted || layer.weight <= 0.001 {
                    continue;
                }

                layer.time += dt * state.speed * layer.speed;
                if layer_clip.duration > 0.0 {
                    layer.time %= layer_clip.duration;
                    if layer.time < 0.0 {
                        layer.time += layer_clip.duration;
                    }
                }
                let w = layer.weight.clamp(0.0, 1.0);
                let mask = &contrib.mask_bits[li];

                match layer.mode {
                    LayerMode::Additive => {
                        for track in &layer_clip.joint_tracks {
                            let idx = track.joint_index;
                            if idx >= joint_count || !mask[idx] {
                                continue;
                            }
                            let value = sample_joint_track(track, layer.time);
                            let pose = &mut skeleton.local_poses[idx];
                            let rest = &skeleton.rest_poses[idx];
                            match track.property {
                                JointProperty::Rotation => {
                                    if value.len() < 4 {
                                        continue;
                                    }
                                    let sampled = [value[0], value[1], value[2], value[3]];
                                    // delta = rest⁻¹ * sampled, faded toward
                                    // identity by weight, applied on the base.
                                    let delta = quat_mul(&quat_conjugate(&rest.rotation), &sampled);
                                    let faded = quat_slerp(&[0.0, 0.0, 0.0, 1.0], &delta, w);
                                    pose.rotation =
                                        quat_normalize(&quat_mul(&pose.rotation, &faded));
                                }
                                JointProperty::Translation => {
                                    if value.len() < 3 {
                                        continue;
                                    }
                                    for (c, v) in value.iter().take(3).enumerate() {
                                        pose.translation[c] += (v - rest.translation[c]) * w;
                                    }
                                }
                                JointProperty::Scale => {
                                    if value.len() < 3 {
                                        continue;
                                    }
                                    for (c, v) in value.iter().take(3).enumerate() {
                                        let ratio = v / rest.scale[c].abs().max(1e-6);
                                        pose.scale[c] *= 1.0 + (ratio - 1.0) * w;
                                    }
                                }
                            }
                            contrib.weights[li][idx] = w;
                            contrib.last_writer[idx] = (li + 1) as u8;
                        }
                    }
                    LayerMode::Override => {
                        // Sample onto a copy of the current pose so
                        // un-keyed properties of a keyed joint stay put.
                        let mut sampled = skeleton.local_poses.clone();
                        Self::sample_clip_into_poses(layer_clip, layer.time, &mut sampled);
                        for idx in 0..joint_count {
                            if !mask[idx] || !contrib.keyed[li][idx] {
                                continue;
                            }
                            skeleton.local_poses[idx] =
                                blend_joint(&skeleton.local_poses[idx], &sampled[idx], w);
                            contrib.weights[li][idx] = w;
                            contrib.last_writer[idx] = (li + 1) as u8;
                        }
                    }
                }
            }

            // Legacy mirror of layer 0's clock
            state.layer_time = state.layers.first().map(|l| l.time).unwrap_or(0.0);

            // Compute final bone matrices
            skeleton.compute_bone_matrices();

            // Un-apply the layers from the persistent pose buffer (see
            // the accumulation note above).
            skeleton.local_poses = pre_layer;
        }
    }

    // ── Layer queries / runtime-only controls ───────────────────────

    /// The resolved layer list driving an entity
    pub fn layers(&self, entity_id: &EntityId) -> Option<&[AnimLayer]> {
        self.states.get(entity_id).map(|s| s.layers.as_slice())
    }

    /// Per-joint composition bookkeeping from the last pose computation
    pub fn layer_contribution(&self, entity_id: &EntityId) -> Option<&LayerContribution> {
        self.contributions.get(entity_id)
    }

    /// Mute/unmute a layer at runtime without touching its ECS weight
    /// (previewer solo/mute). Survives `sync_from_world`.
    pub fn set_layer_mute(&mut self, entity_id: EntityId, index: usize, muted: bool) {
        let m = self.mutes.entry(entity_id).or_default();
        if m.len() <= index {
            m.resize(index + 1, false);
        }
        m[index] = muted;
    }

    /// Clear all runtime mute flags for an entity
    pub fn clear_layer_mutes(&mut self, entity_id: &EntityId) {
        self.mutes.remove(entity_id);
    }

    /// Which joints (by index) a clip keys, as a per-joint bitset
    pub fn clip_keyed_joints(&self, clip_name: &str, joint_count: usize) -> Option<Vec<bool>> {
        let clip = self.clips.get(clip_name)?;
        let mut keyed = vec![false; joint_count];
        for t in &clip.joint_tracks {
            if let Some(k) = keyed.get_mut(t.joint_index) {
                *k = true;
            }
        }
        Some(keyed)
    }

    /// Retire finished crossfades in the ECS.
    ///
    /// `blend_to` sets the `blend_target` field and nothing else ever cleared
    /// it, while `advance_and_compute` cleared only the runtime mirror. The
    /// next `sync_from_world` therefore saw a non-empty ECS target that no
    /// longer matched the empty runtime one and started the SAME crossfade
    /// again — a self-sustaining loop with a period of `blend_duration`, so
    /// every clip permanently replayed only its first `blend_duration`
    /// seconds (0.3 s default ≈ 3-4 restarts/second). Clearing the field
    /// here closes the loop and keeps `blend_to(e, same_clip, t)` meaningful
    /// as an explicit restart.
    pub fn write_back(&mut self, world: &mut FlintWorld) {
        for entity_id in self.completed_blends.drain(..) {
            let Some(components) = world.get_components_mut(entity_id) else {
                continue;
            };
            components.set_field(
                comp::ANIMATOR,
                "blend_target",
                toml::Value::String(String::new()),
            );
        }

        // Layer fades: publish the ramped weight every frame (so the next
        // sync copies the same value and UI sliders visibly move), and
        // retire finished ramps by zeroing `fade_duration`.
        let completed = std::mem::take(&mut self.completed_fades);
        for (entity_id, fades) in &self.fades {
            let active: Vec<usize> = fades
                .iter()
                .enumerate()
                .filter_map(|(i, f)| f.is_some().then_some(i))
                .collect();
            let done: Vec<usize> = completed
                .iter()
                .filter(|(e, _)| e == entity_id)
                .map(|(_, i)| *i)
                .collect();
            if active.is_empty() && done.is_empty() {
                continue;
            }
            let Some(state) = self.states.get(entity_id) else {
                continue;
            };
            let Some(components) = world.get_components_mut(*entity_id) else {
                continue;
            };
            let Some(layers) = components
                .get_mut(comp::ANIMATOR)
                .and_then(|a| a.get_mut("layers"))
                .and_then(|l| l.as_array_mut())
            else {
                continue;
            };
            for li in active.iter().chain(done.iter()) {
                let (Some(layer), Some(table)) = (
                    state.layers.get(*li),
                    layers.get_mut(*li).and_then(|v| v.as_table_mut()),
                ) else {
                    continue;
                };
                table.insert("weight".into(), toml::Value::Float(layer.weight as f64));
                if done.contains(li) {
                    table.insert("fade_duration".into(), toml::Value::Float(0.0));
                }
            }
        }
    }

    /// Whether a layer weight ramp is in flight, as `(target, seconds left)`
    pub fn layer_fade(&self, entity_id: &EntityId, index: usize) -> Option<(f32, f32)> {
        self.fades
            .get(entity_id)?
            .get(index)?
            .map(|f| (f.to, (f.duration - f.elapsed).max(0.0)))
    }

    /// Sample a clip's joint tracks into a pose array
    fn sample_clip_into_poses(clip: &SkeletalClip, time: f64, poses: &mut [JointPose]) {
        for track in &clip.joint_tracks {
            let value = sample_joint_track(track, time);
            let idx = track.joint_index;
            if idx >= poses.len() {
                continue;
            }

            match track.property {
                JointProperty::Translation => {
                    if value.len() >= 3 {
                        poses[idx].translation = [value[0], value[1], value[2]];
                    }
                }
                JointProperty::Rotation => {
                    if value.len() >= 4 {
                        poses[idx].rotation = [value[0], value[1], value[2], value[3]];
                    }
                }
                JointProperty::Scale => {
                    if value.len() >= 3 {
                        poses[idx].scale = [value[0], value[1], value[2]];
                    }
                }
            }
        }
    }

    /// Get the playback state for a given entity
    pub fn get_playback_state(&self, entity_id: &EntityId) -> Option<&SkeletalPlaybackState> {
        self.states.get(entity_id)
    }

    /// Set the playback time for a given entity (used for timeline scrubbing)
    pub fn set_playback_time(&mut self, entity_id: &EntityId, time: f64) {
        if let Some(state) = self.states.get_mut(entity_id) {
            state.time = time;
        }
    }

    /// Get the duration of a clip by name
    pub fn get_clip_duration(&self, clip_name: &str) -> Option<f64> {
        self.clips.get(clip_name).map(|c| c.duration)
    }

    /// Model-space position of a named joint for an entity (bone_probe)
    pub fn joint_position(&self, entity_id: &EntityId, joint: &str) -> Option<[f32; 3]> {
        self.skeletons.get(entity_id)?.joint_position(joint)
    }

    /// The skeleton driving an entity (joint hierarchy + last computed globals)
    pub fn skeleton(&self, entity_id: &EntityId) -> Option<&Skeleton> {
        self.skeletons.get(entity_id)
    }

    /// Get bone matrices for a given entity (for GPU upload)
    pub fn bone_matrices(&self, entity_id: &EntityId) -> Option<&[[[f32; 4]; 4]]> {
        self.skeletons
            .get(entity_id)
            .map(|s| s.bone_matrices.as_slice())
    }

    /// Iterate over all entities with computed bone matrices
    pub fn all_bone_matrices(&self) -> impl Iterator<Item = (EntityId, &[[[f32; 4]; 4]])> {
        self.skeletons
            .iter()
            .filter(|(id, _)| self.states.contains_key(id))
            .map(|(id, skel)| (*id, skel.bone_matrices.as_slice()))
    }

    /// Get the skin index for a given entity's skeleton
    pub fn skin_index(&self, _entity_id: &EntityId) -> usize {
        // Currently we only support one skin per entity; always 0
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::Interpolation;
    use crate::skeletal_clip::{JointKeyframe, JointTrack};

    const IDENT: [[f32; 4]; 4] = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];

    fn one_joint_skeleton() -> Skeleton {
        Skeleton {
            joint_names: vec!["root".into()],
            parents: vec![None],
            inverse_bind_matrices: vec![IDENT],
            local_poses: vec![JointPose::default()],
            rest_poses: vec![JointPose::default()],
            bone_matrices: vec![IDENT],
            global_matrices: vec![IDENT],
        }
    }

    /// A clip that slides the single joint along +X over `duration`.
    fn slide_clip(name: &str, duration: f64) -> SkeletalClip {
        SkeletalClip {
            name: name.to_string(),
            duration,
            joint_tracks: vec![JointTrack {
                joint_index: 0,
                property: JointProperty::Translation,
                interpolation: Interpolation::Linear,
                keyframes: vec![
                    JointKeyframe {
                        time: 0.0,
                        value: vec![0.0, 0.0, 0.0],
                        in_tangent: vec![],
                        out_tangent: vec![],
                    },
                    JointKeyframe {
                        time: duration,
                        value: vec![duration as f32, 0.0, 0.0],
                        in_tangent: vec![],
                        out_tangent: vec![],
                    },
                ],
            }],
        }
    }

    fn world_with_animator(
        clip: &str,
        blend_target: &str,
        blend_duration: f64,
    ) -> (FlintWorld, EntityId) {
        let mut world = FlintWorld::new();
        let eid = world.spawn("rig").unwrap();
        world
            .set_field(
                eid,
                comp::ANIMATOR,
                "clip",
                toml::Value::String(clip.into()),
            )
            .unwrap();
        world
            .set_field(eid, comp::ANIMATOR, "playing", toml::Value::Boolean(true))
            .unwrap();
        world
            .set_field(eid, comp::ANIMATOR, "loop", toml::Value::Boolean(true))
            .unwrap();
        world
            .set_field(
                eid,
                comp::ANIMATOR,
                "blend_target",
                toml::Value::String(blend_target.into()),
            )
            .unwrap();
        world
            .set_field(
                eid,
                comp::ANIMATOR,
                "blend_duration",
                toml::Value::Float(blend_duration),
            )
            .unwrap();
        world
            .set_field(
                eid,
                comp::SKELETON,
                "skin",
                toml::Value::String("rig".into()),
            )
            .unwrap();
        (world, eid)
    }

    fn ecs_blend_target(world: &FlintWorld, eid: EntityId) -> String {
        world
            .get_components(eid)
            .and_then(|c| c.get(comp::ANIMATOR))
            .and_then(|a| a.get("blend_target"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    }

    /// A completed crossfade must be retired in the ECS, not just in the
    /// runtime mirror. Before the fix, `blend_target` stayed set forever and
    /// `sync_from_world` re-armed the same blend every frame — the clip
    /// replayed only its first `blend_duration` seconds, on loop.
    #[test]
    fn completed_blend_clears_ecs_target_and_does_not_rearm() {
        let mut sync = SkeletalSync::new();
        sync.add_clip(slide_clip("idle", 4.0));
        sync.add_clip(slide_clip("scoot", 4.0));

        let (mut world, eid) = world_with_animator("idle", "scoot", 0.2);
        sync.add_skeleton(eid, one_joint_skeleton());

        // Run 1 second at 60 Hz — far past the 0.2 s crossfade.
        for _ in 0..60 {
            sync.sync_from_world(&world);
            sync.advance_and_compute(1.0 / 60.0);
            sync.write_back(&mut world);
        }

        assert_eq!(
            ecs_blend_target(&world, eid),
            "",
            "finished crossfade must clear the ECS blend_target"
        );

        let state = sync.states.get(&eid).unwrap();
        assert_eq!(
            state.clip_name, "scoot",
            "should have settled on the target clip"
        );
        assert!(
            state.blend_target.is_empty(),
            "runtime blend must not be re-armed"
        );
        // The real symptom: time kept resetting to ~blend_duration. After
        // ~1 s of playback the clip must be well past that.
        assert!(
            state.time > 0.5,
            "clip time {} suggests the blend is restarting (stuck near blend_duration)",
            state.time
        );
    }

    fn ecs_layer0(world: &FlintWorld, eid: EntityId) -> toml::Value {
        world
            .get_components(eid)
            .and_then(|c| c.get(comp::ANIMATOR))
            .and_then(|a| a.get("layers"))
            .and_then(|l| l.as_array())
            .and_then(|l| l.first())
            .cloned()
            .unwrap()
    }

    /// A layer `fade_duration` ramps the runtime weight, publishes it to
    /// the ECS each frame, and retires itself so the next sync doesn't
    /// re-arm the ramp.
    #[test]
    fn layer_fade_ramps_and_retires() {
        let mut sync = SkeletalSync::new();
        sync.add_clip(slide_clip("idle", 4.0));
        sync.add_clip(slide_clip("pose", 4.0));

        let (mut world, eid) = world_with_animator("idle", "", 0.1);
        sync.add_skeleton(eid, one_joint_skeleton());
        let mut layer = toml::map::Map::new();
        layer.insert("clip".into(), toml::Value::String("pose".into()));
        layer.insert("weight".into(), toml::Value::Float(0.0));
        layer.insert("fade_target".into(), toml::Value::Float(1.0));
        layer.insert("fade_duration".into(), toml::Value::Float(0.5));
        world
            .set_field(
                eid,
                comp::ANIMATOR,
                "layers",
                toml::Value::Array(vec![toml::Value::Table(layer)]),
            )
            .unwrap();

        let step = |sync: &mut SkeletalSync, world: &mut FlintWorld, dt: f64| {
            sync.sync_from_world(world);
            sync.advance_and_compute(dt);
            sync.write_back(world);
        };

        // 0.25 s in → halfway
        for _ in 0..5 {
            step(&mut sync, &mut world, 0.05);
        }
        let w = sync.layers(&eid).unwrap()[0].weight;
        assert!((w - 0.5).abs() < 1e-4, "weight {w} should be mid-ramp");
        let ecs = ecs_layer0(&world, eid);
        assert!((toml_f32(&ecs["weight"]).unwrap() - 0.5).abs() < 1e-4);
        assert_eq!(sync.layer_fade(&eid, 0).map(|f| f.0), Some(1.0));

        // Finish, then keep syncing: stays at 1.0, fade retired
        for _ in 0..10 {
            step(&mut sync, &mut world, 0.05);
        }
        let ecs = ecs_layer0(&world, eid);
        assert_eq!(toml_f32(&ecs["weight"]), Some(1.0));
        assert_eq!(toml_f32(&ecs["fade_duration"]), Some(0.0));
        assert!(sync.layer_fade(&eid, 0).is_none());
        assert_eq!(sync.layers(&eid).unwrap()[0].weight, 1.0);

        // A plain weight write now wins instantly
        let mut layer = ecs.as_table().unwrap().clone();
        layer.insert("weight".into(), toml::Value::Float(0.25));
        world
            .set_field(
                eid,
                comp::ANIMATOR,
                "layers",
                toml::Value::Array(vec![toml::Value::Table(layer)]),
            )
            .unwrap();
        step(&mut sync, &mut world, 0.05);
        assert_eq!(sync.layers(&eid).unwrap()[0].weight, 0.25);
    }

    /// A clip that doesn't key a joint must leave it at REST, not at
    /// whatever the previous clip wrote (exporters drop held-at-rest
    /// tracks, so "un-keyed" means "rest"). Walk → idle used to freeze the
    /// legs mid-stride because the pose buffer persisted across clips.
    #[test]
    fn unkeyed_joints_return_to_rest_after_clip_change() {
        let mut sync = SkeletalSync::new();
        sync.add_clip(slide_clip("scoot", 4.0));
        sync.add_clip(SkeletalClip {
            name: "still".into(),
            duration: 1.0,
            joint_tracks: vec![], // keys nothing
        });

        let (mut world, eid) = world_with_animator("scoot", "", 0.1);
        sync.add_skeleton(eid, one_joint_skeleton());
        for _ in 0..30 {
            sync.sync_from_world(&world);
            sync.advance_and_compute(1.0 / 30.0);
            sync.write_back(&mut world);
        }
        let moved = sync.skeleton(&eid).unwrap().local_poses[0].translation[0];
        assert!(
            moved > 0.5,
            "scoot should have displaced the joint, got {moved}"
        );

        // Crossfade to the clip that keys nothing
        world
            .set_field(
                eid,
                comp::ANIMATOR,
                "blend_target",
                toml::Value::String("still".into()),
            )
            .unwrap();
        for _ in 0..30 {
            sync.sync_from_world(&world);
            sync.advance_and_compute(1.0 / 30.0);
            sync.write_back(&mut world);
        }
        let x = sync.skeleton(&eid).unwrap().local_poses[0].translation[0];
        assert!(
            x.abs() < 1e-5,
            "un-keyed joint must be back at rest, got {x}"
        );
    }

    /// Re-issuing the SAME clip must still restart it — body.rhai chains
    /// held-key scoot steps that way.
    #[test]
    fn reissuing_same_clip_restarts_it() {
        let mut sync = SkeletalSync::new();
        sync.add_clip(slide_clip("scoot", 4.0));

        let (mut world, eid) = world_with_animator("scoot", "", 0.1);
        sync.add_skeleton(eid, one_joint_skeleton());

        for _ in 0..60 {
            sync.sync_from_world(&world);
            sync.advance_and_compute(1.0 / 60.0);
            sync.write_back(&mut world);
        }
        let before = sync.states.get(&eid).unwrap().time;
        assert!(before > 0.5);

        // blend_to(me, "scoot", 0.1) — same clip, explicit restart.
        world
            .set_field(
                eid,
                comp::ANIMATOR,
                "blend_target",
                toml::Value::String("scoot".into()),
            )
            .unwrap();
        for _ in 0..12 {
            sync.sync_from_world(&world);
            sync.advance_and_compute(1.0 / 60.0);
            sync.write_back(&mut world);
        }

        let after = sync.states.get(&eid).unwrap().time;
        assert!(
            after < before,
            "re-issuing the playing clip must restart it (before {before}, after {after})"
        );
    }

    // ── Layer tests ─────────────────────────────────────────────────

    /// root -> child, both at rest identity.
    fn two_joint_skeleton() -> Skeleton {
        Skeleton {
            joint_names: vec!["root".into(), "child".into()],
            parents: vec![None, Some(0)],
            inverse_bind_matrices: vec![IDENT; 2],
            local_poses: vec![JointPose::default(); 2],
            rest_poses: vec![JointPose::default(); 2],
            bone_matrices: vec![IDENT; 2],
            global_matrices: vec![IDENT; 2],
        }
    }

    /// A constant clip that places `joint` at translation `(x,0,0)`.
    fn hold_clip(name: &str, joint: usize, x: f32) -> SkeletalClip {
        SkeletalClip {
            name: name.to_string(),
            duration: 1.0,
            joint_tracks: vec![JointTrack {
                joint_index: joint,
                property: JointProperty::Translation,
                interpolation: Interpolation::Step,
                keyframes: vec![JointKeyframe {
                    time: 0.0,
                    value: vec![x, 0.0, 0.0],
                    in_tangent: vec![],
                    out_tangent: vec![],
                }],
            }],
        }
    }

    fn set_layers(world: &mut FlintWorld, eid: EntityId, layers: &[(&str, f64, &str, &str)]) {
        let arr = layers
            .iter()
            .map(|(clip, w, mode, mask)| {
                let mut t = toml::map::Map::new();
                t.insert("clip".into(), toml::Value::String(clip.to_string()));
                t.insert("weight".into(), toml::Value::Float(*w));
                t.insert("mode".into(), toml::Value::String(mode.to_string()));
                t.insert("mask".into(), toml::Value::String(mask.to_string()));
                toml::Value::Table(t)
            })
            .collect();
        world
            .set_field(eid, comp::ANIMATOR, "layers", toml::Value::Array(arr))
            .unwrap();
    }

    fn step(sync: &mut SkeletalSync, world: &mut FlintWorld, frames: usize) {
        for _ in 0..frames {
            sync.sync_from_world(world);
            sync.advance_and_compute(1.0 / 60.0);
            sync.write_back(world);
        }
    }

    fn global_x(sync: &SkeletalSync, eid: EntityId, joint: usize) -> f32 {
        sync.skeletons[&eid].global_matrices[joint][3][0]
    }

    #[test]
    fn legacy_layer_fields_map_to_layer0() {
        let mut sync = SkeletalSync::new();
        sync.add_clip(hold_clip("base", 0, 0.0));
        sync.add_clip(hold_clip("lay", 0, 2.0));
        let (mut world, eid) = world_with_animator("base", "", 0.3);
        world
            .set_field(
                eid,
                comp::ANIMATOR,
                "layer_clip",
                toml::Value::String("lay".into()),
            )
            .unwrap();
        world
            .set_field(eid, comp::ANIMATOR, "layer_weight", toml::Value::Float(0.5))
            .unwrap();
        sync.add_skeleton(eid, one_joint_skeleton());
        step(&mut sync, &mut world, 1);
        let layers = sync.layers(&eid).unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].clip, "lay");
        assert_eq!(layers[0].mode, LayerMode::Additive);
        assert!((layers[0].weight - 0.5).abs() < 1e-6);
        // additive: base 0 + (2 - rest 0) * 0.5 = 1
        assert!((global_x(&sync, eid, 0) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn layers_array_beats_legacy_fields() {
        let mut sync = SkeletalSync::new();
        sync.add_clip(hold_clip("base", 0, 0.0));
        sync.add_clip(hold_clip("a", 0, 1.0));
        sync.add_clip(hold_clip("b", 0, 3.0));
        let (mut world, eid) = world_with_animator("base", "", 0.3);
        world
            .set_field(
                eid,
                comp::ANIMATOR,
                "layer_clip",
                toml::Value::String("a".into()),
            )
            .unwrap();
        set_layers(&mut world, eid, &[("b", 1.0, "additive", "")]);
        sync.add_skeleton(eid, one_joint_skeleton());
        step(&mut sync, &mut world, 1);
        assert_eq!(sync.layers(&eid).unwrap()[0].clip, "b");
        assert!((global_x(&sync, eid, 0) - 3.0).abs() < 1e-5);
    }

    #[test]
    fn additive_weight_zero_is_noop_and_full_adds_delta() {
        let mut sync = SkeletalSync::new();
        sync.add_clip(hold_clip("base", 0, 1.0));
        sync.add_clip(hold_clip("lay", 0, 4.0));
        let (mut world, eid) = world_with_animator("base", "", 0.3);
        sync.add_skeleton(eid, one_joint_skeleton());

        set_layers(&mut world, eid, &[("lay", 0.0, "additive", "")]);
        step(&mut sync, &mut world, 1);
        assert!((global_x(&sync, eid, 0) - 1.0).abs() < 1e-5);

        set_layers(&mut world, eid, &[("lay", 1.0, "additive", "")]);
        step(&mut sync, &mut world, 1);
        assert!(
            (global_x(&sync, eid, 0) - 5.0).abs() < 1e-5,
            "base 1 + delta 4"
        );
    }

    #[test]
    fn override_layer_replaces_within_mask_only() {
        let mut sync = SkeletalSync::new();
        // base holds both joints at local x=1, override keys both to x=10
        let mut base = hold_clip("base", 0, 1.0);
        base.joint_tracks
            .push(hold_clip("_", 1, 1.0).joint_tracks.remove(0));
        let mut over = hold_clip("over", 0, 10.0);
        over.joint_tracks
            .push(hold_clip("_", 1, 10.0).joint_tracks.remove(0));
        sync.add_clip(base);
        sync.add_clip(over);
        let (mut world, eid) = world_with_animator("base", "", 0.3);
        sync.add_skeleton(eid, two_joint_skeleton());

        set_layers(&mut world, eid, &[("over", 1.0, "override", "child")]);
        step(&mut sync, &mut world, 1);
        let root = sync.skeletons[&eid].global_matrices[0][3][0];
        let child_local = sync.skeletons[&eid].global_matrices[1][3][0] - root;
        assert!((root - 1.0).abs() < 1e-5, "root outside mask stays base");
        assert!((child_local - 10.0).abs() < 1e-5, "child fully overridden");

        set_layers(&mut world, eid, &[("over", 0.5, "override", "child")]);
        step(&mut sync, &mut world, 1);
        let root = sync.skeletons[&eid].global_matrices[0][3][0];
        let child_local = sync.skeletons[&eid].global_matrices[1][3][0] - root;
        assert!((child_local - 5.5).abs() < 1e-5, "midpoint of 1 and 10");

        let c = sync.layer_contribution(&eid).unwrap();
        assert_eq!(c.last_writer, vec![WRITER_BASE, 1]);
        assert!(c.in_mask(0, 1) && !c.in_mask(0, 0));
        assert!(c.is_keyed(0, 0) && c.is_keyed(0, 1));
        assert!((c.weight(0, 1) - 0.5).abs() < 1e-6 && c.weight(0, 0) == 0.0);
    }

    #[test]
    fn layer_order_matters() {
        let mut sync = SkeletalSync::new();
        sync.add_clip(hold_clip("base", 0, 1.0));
        sync.add_clip(hold_clip("add", 0, 4.0)); // +4 additive
        sync.add_clip(hold_clip("over", 0, 10.0)); // override to 10
        let (mut world, eid) = world_with_animator("base", "", 0.3);
        sync.add_skeleton(eid, one_joint_skeleton());

        set_layers(
            &mut world,
            eid,
            &[("add", 1.0, "additive", ""), ("over", 1.0, "override", "")],
        );
        step(&mut sync, &mut world, 1);
        assert!(
            (global_x(&sync, eid, 0) - 10.0).abs() < 1e-5,
            "override last wins"
        );

        set_layers(
            &mut world,
            eid,
            &[("over", 1.0, "override", ""), ("add", 1.0, "additive", "")],
        );
        step(&mut sync, &mut world, 1);
        assert!(
            (global_x(&sync, eid, 0) - 14.0).abs() < 1e-5,
            "additive on top of override"
        );
    }

    #[test]
    fn layer_does_not_accumulate_across_frames() {
        let mut sync = SkeletalSync::new();
        sync.add_clip(hold_clip("base", 0, 1.0));
        sync.add_clip(hold_clip("lay", 0, 2.0));
        let (mut world, eid) = world_with_animator("base", "", 0.3);
        sync.add_skeleton(eid, one_joint_skeleton());
        set_layers(&mut world, eid, &[("lay", 1.0, "additive", "")]);
        step(&mut sync, &mut world, 1);
        let first = global_x(&sync, eid, 0);
        step(&mut sync, &mut world, 120);
        assert!((global_x(&sync, eid, 0) - first).abs() < 1e-5);
        assert!((first - 3.0).abs() < 1e-5);
    }

    #[test]
    fn layer_survives_crossfade() {
        let mut sync = SkeletalSync::new();
        sync.add_clip(hold_clip("a", 0, 1.0));
        sync.add_clip(hold_clip("b", 0, 2.0));
        sync.add_clip(hold_clip("lay", 0, 10.0));
        let (mut world, eid) = world_with_animator("a", "b", 0.1);
        sync.add_skeleton(eid, one_joint_skeleton());
        set_layers(&mut world, eid, &[("lay", 1.0, "additive", "")]);
        step(&mut sync, &mut world, 60);
        assert_eq!(sync.states[&eid].clip_name, "b");
        assert!(
            (global_x(&sync, eid, 0) - 12.0).abs() < 1e-4,
            "b (2) + layer (10)"
        );
    }

    #[test]
    fn muted_layer_is_skipped_but_weight_kept() {
        let mut sync = SkeletalSync::new();
        sync.add_clip(hold_clip("base", 0, 1.0));
        sync.add_clip(hold_clip("lay", 0, 2.0));
        let (mut world, eid) = world_with_animator("base", "", 0.3);
        sync.add_skeleton(eid, one_joint_skeleton());
        set_layers(&mut world, eid, &[("lay", 1.0, "additive", "")]);
        sync.set_layer_mute(eid, 0, true);
        step(&mut sync, &mut world, 1);
        assert!((global_x(&sync, eid, 0) - 1.0).abs() < 1e-5);
        assert!((sync.layers(&eid).unwrap()[0].weight - 1.0).abs() < 1e-6);
        sync.set_layer_mute(eid, 0, false);
        step(&mut sync, &mut world, 1);
        assert!((global_x(&sync, eid, 0) - 3.0).abs() < 1e-5);
    }

    #[test]
    fn inactive_slot_keeps_indices_stable() {
        let mut sync = SkeletalSync::new();
        sync.add_clip(hold_clip("base", 0, 1.0));
        sync.add_clip(hold_clip("lay", 0, 2.0));
        let (mut world, eid) = world_with_animator("base", "", 0.3);
        sync.add_skeleton(eid, one_joint_skeleton());
        set_layers(
            &mut world,
            eid,
            &[("", 1.0, "additive", ""), ("lay", 1.0, "additive", "")],
        );
        step(&mut sync, &mut world, 1);
        let layers = sync.layers(&eid).unwrap();
        assert_eq!(layers.len(), 2);
        assert!(!layers[0].is_active() && layers[1].clip == "lay");
        assert_eq!(sync.layer_contribution(&eid).unwrap().last_writer, vec![2]);
    }

    #[test]
    fn speed_change_in_ecs_applies_to_tracked_entity() {
        let mut sync = SkeletalSync::new();
        sync.add_clip(slide_clip("walk", 4.0));
        let (mut world, eid) = world_with_animator("walk", "", 0.3);
        sync.add_skeleton(eid, one_joint_skeleton());
        step(&mut sync, &mut world, 1);
        world
            .set_field(eid, comp::ANIMATOR, "speed", toml::Value::Float(3.0))
            .unwrap();
        step(&mut sync, &mut world, 1);
        assert!((sync.states[&eid].speed - 3.0).abs() < 1e-9);
    }
}
