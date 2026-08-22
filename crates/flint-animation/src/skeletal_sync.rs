//! Bridges ECS entities to skeletal animation playback
//!
//! Manages per-entity skeleton state, skeletal clip playback, and bone matrix computation.

use crate::blend::blend_poses;
use crate::playback_state::ClipPlaybackState;
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
}

impl SkeletalSync {
    pub fn new() -> Self {
        Self {
            clips: HashMap::new(),
            skeletons: HashMap::new(),
            states: HashMap::new(),
            completed_blends: Vec::new(),
        }
    }

    /// Clear all skeletal animation state for a scene transition.
    pub fn clear(&mut self) {
        self.clips.clear();
        self.skeletons.clear();
        self.states.clear();
        self.completed_blends.clear();
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

                // Additive layer follows the ECS every frame (weight is a
                // live dial; changing the clip restarts its own clock).
                let ecs_layer = animator
                    .get("layer_clip")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if ecs_layer != state.layer_clip {
                    state.layer_clip = ecs_layer;
                    state.layer_time = 0.0;
                }
                state.layer_weight = animator
                    .get("layer_weight")
                    .and_then(toml_f32)
                    .unwrap_or(1.0);
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
            state.layer_clip = animator
                .get("layer_clip")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            state.layer_weight = animator
                .get("layer_weight")
                .and_then(toml_f32)
                .unwrap_or(1.0);

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

            // Sample current clip into local_poses
            Self::sample_clip_into_poses(clip, state.time, &mut skeleton.local_poses);

            // Handle crossfade blending
            let is_blending = !state.blend_target.is_empty() && state.blend_duration > 0.0;

            if is_blending {
                let target_clip_name = state.blend_target.clone();
                if let Some(target_clip) = self.clips.get(&target_clip_name) {
                    state.blend_elapsed += dt as f32;
                    let blend_weight = (state.blend_elapsed / state.blend_duration).min(1.0);

                    // Sample target clip into temporary pose array
                    let joint_count = skeleton.joint_count();
                    let mut target_poses = vec![JointPose::default(); joint_count];
                    // Initialize target poses from skeleton defaults
                    for (i, pose) in target_poses.iter_mut().enumerate() {
                        *pose = skeleton.local_poses[i].clone();
                    }
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

            // ── Additive layer ──────────────────────────────────────
            // Loops on its own clock, composed AFTER base + blend so it
            // survives crossfades: each keyed joint contributes its
            // delta-from-REST, scaled by layer_weight. Un-keyed joints
            // are untouched (composing identity would corrupt joints
            // whose rest rotation is non-identity). The pre-layer pose
            // is RESTORED after matrix computation: local_poses persists
            // across frames and base clips only overwrite the joints
            // they key, so leaving the deltas in would compound them
            // every frame (feet were windmilling at 120°+).
            let mut layer_saved: Vec<(usize, JointPose)> = Vec::new();
            if !state.layer_clip.is_empty() && state.layer_weight > 0.001 {
                if let Some(layer_clip) = self.clips.get(&state.layer_clip) {
                    state.layer_time += dt * state.speed;
                    if layer_clip.duration > 0.0 {
                        state.layer_time %= layer_clip.duration;
                        if state.layer_time < 0.0 {
                            state.layer_time += layer_clip.duration;
                        }
                    }
                    let w = state.layer_weight.clamp(0.0, 1.0);
                    for track in &layer_clip.joint_tracks {
                        let idx = track.joint_index;
                        if idx >= skeleton.local_poses.len() {
                            continue;
                        }
                        if !layer_saved.iter().any(|(i, _)| *i == idx) {
                            layer_saved.push((idx, skeleton.local_poses[idx].clone()));
                        }
                        let value = sample_joint_track(track, state.layer_time);
                        match track.property {
                            JointProperty::Rotation => {
                                if value.len() < 4 {
                                    continue;
                                }
                                let rest = skeleton.rest_poses[idx].rotation;
                                let sampled = [value[0], value[1], value[2], value[3]];
                                // delta = rest⁻¹ * sampled, faded toward
                                // identity by weight, applied on the base.
                                let delta = quat_mul(quat_conj(rest), sampled);
                                let faded = quat_nlerp([0.0, 0.0, 0.0, 1.0], delta, w);
                                let base = skeleton.local_poses[idx].rotation;
                                skeleton.local_poses[idx].rotation =
                                    quat_normalize(quat_mul(base, faded));
                            }
                            JointProperty::Translation => {
                                if value.len() < 3 {
                                    continue;
                                }
                                let rest = skeleton.rest_poses[idx].translation;
                                for c in 0..3 {
                                    skeleton.local_poses[idx].translation[c] +=
                                        (value[c] - rest[c]) * w;
                                }
                            }
                            JointProperty::Scale => {}
                        }
                    }
                }
            }

            // Compute final bone matrices
            skeleton.compute_bone_matrices();

            // Un-apply the additive layer from the persistent pose
            // buffer (see the accumulation note above).
            for (idx, pose) in layer_saved {
                skeleton.local_poses[idx] = pose;
            }
        }
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

// ── Quaternion helpers for additive layer composition (xyzw) ────────────

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// Conjugate = inverse for unit quaternions
fn quat_conj(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

fn quat_normalize(q: [f32; 4]) -> [f32; 4] {
    let len = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if len < 1e-10 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    [q[0] / len, q[1] / len, q[2] / len, q[3] / len]
}

/// Normalized lerp with shortest-path correction — fine for the small
/// angles a layer weight fades through.
fn quat_nlerp(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let mut b = b;
    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    if dot < 0.0 {
        b = [-b[0], -b[1], -b[2], -b[3]];
    }
    quat_normalize([
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ])
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
}
