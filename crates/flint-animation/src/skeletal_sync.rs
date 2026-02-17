//! Bridges ECS entities to skeletal animation playback
//!
//! Manages per-entity skeleton state, skeletal clip playback, and bone matrix computation.

use crate::blend::blend_poses;
use crate::skeletal_clip::{JointProperty, SkeletalClip};
use crate::skeletal_sampler::sample_joint_track;
use crate::skeleton::{JointPose, Skeleton};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::HashMap;

/// Per-entity skeletal playback state
#[derive(Debug, Clone)]
pub struct SkeletalPlaybackState {
    pub clip_name: String,
    pub time: f64,
    pub speed: f64,
    pub looping: bool,
    pub playing: bool,
    /// Clip name to crossfade into (empty = no blend)
    pub blend_target: String,
    /// Duration of the crossfade in seconds
    pub blend_duration: f32,
    /// Time elapsed in the current blend
    pub blend_elapsed: f32,
}

impl SkeletalPlaybackState {
    pub fn new(clip_name: String, speed: f64, looping: bool, playing: bool) -> Self {
        Self {
            clip_name,
            time: 0.0,
            speed,
            looping,
            playing,
            blend_target: String::new(),
            blend_duration: 0.3,
            blend_elapsed: 0.0,
        }
    }
}

/// Registry of skeletal clips and per-entity skeleton state
pub struct SkeletalSync {
    clips: HashMap<String, SkeletalClip>,
    skeletons: HashMap<EntityId, Skeleton>,
    states: HashMap<EntityId, SkeletalPlaybackState>,
}

impl SkeletalSync {
    pub fn new() -> Self {
        Self {
            clips: HashMap::new(),
            skeletons: HashMap::new(),
            states: HashMap::new(),
        }
    }

    /// Clear all skeletal animation state for a scene transition.
    pub fn clear(&mut self) {
        self.clips.clear();
        self.skeletons.clear();
        self.states.clear();
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

    /// Check if an entity has a skeleton registered
    pub fn has_skeleton(&self, entity_id: &EntityId) -> bool {
        self.skeletons.contains_key(entity_id)
    }

    /// Scan the world for entities with `animator` + `skeleton` components.
    /// Creates playback states for newly discovered entities.
    /// Updates blend_target/blend_duration for existing entities if changed in ECS.
    pub fn sync_from_world(&mut self, world: &FlintWorld) {
        for entity in world.all_entities() {
            let Some(components) = world.get_components(entity.id) else {
                continue;
            };

            // Must have both animator and skeleton components
            let Some(animator) = components.get("animator") else {
                continue;
            };
            if components.get("skeleton").is_none() {
                continue;
            }
            // Must also have a skeleton registered for this entity
            if !self.skeletons.contains_key(&entity.id) {
                continue;
            }

            // If already tracked, check for blend_target changes from ECS
            if let Some(state) = self.states.get_mut(&entity.id) {
                let ecs_blend_target = animator
                    .get("blend_target")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let ecs_blend_duration = animator
                    .get("blend_duration")
                    .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                    .unwrap_or(0.3) as f32;

                // Start a new crossfade if ECS sets a new blend_target
                if !ecs_blend_target.is_empty()
                    && ecs_blend_target != state.blend_target
                    && self.clips.contains_key(&ecs_blend_target)
                {
                    state.blend_target = ecs_blend_target;
                    state.blend_duration = ecs_blend_duration;
                    state.blend_elapsed = 0.0;
                }
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

            let speed = animator
                .get("speed")
                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                .unwrap_or(1.0);

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

            let mut state = SkeletalPlaybackState::new(clip_name, speed, looping, playing);

            // Read initial blend fields
            state.blend_target = animator
                .get("blend_target")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            state.blend_duration = animator
                .get("blend_duration")
                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                .unwrap_or(0.3) as f32;

            self.states.insert(entity.id, state);
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
            let is_blending = !state.blend_target.is_empty()
                && state.blend_duration > 0.0;

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
                    }
                } else {
                    // Target clip not found, clear blend
                    state.blend_target.clear();
                    state.blend_elapsed = 0.0;
                }
            }

            // Compute final bone matrices
            skeleton.compute_bone_matrices();
        }
    }

    /// Sample a clip's joint tracks into a pose array
    fn sample_clip_into_poses(
        clip: &SkeletalClip,
        time: f64,
        poses: &mut [JointPose],
    ) {
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

impl Default for SkeletalSync {
    fn default() -> Self {
        Self::new()
    }
}
