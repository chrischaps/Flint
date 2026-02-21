//! Bridges ECS entities to node-level animation playback
//!
//! Manages per-entity node animation state, clip playback, and transform writes.
//! Analogous to `SkeletalSync` but writes to child entity transforms instead of
//! computing bone matrices for GPU skinning.

use crate::node_clip::NodeClip;
use crate::skeletal_clip::{JointProperty, JointTrack};
use crate::skeletal_sampler::sample_joint_track;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::HashMap;

/// Per-entity node animation playback state
#[derive(Debug, Clone)]
pub struct NodePlaybackState {
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

impl NodePlaybackState {
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

/// Registry of node clips and per-entity playback state
pub struct NodeSync {
    clips: HashMap<String, NodeClip>,
    states: HashMap<EntityId, NodePlaybackState>,
    /// Per root entity: node_name → child EntityId
    node_maps: HashMap<EntityId, HashMap<String, EntityId>>,
}

impl NodeSync {
    pub fn new() -> Self {
        Self {
            clips: HashMap::new(),
            states: HashMap::new(),
            node_maps: HashMap::new(),
        }
    }

    /// Clear all node animation state for a scene transition.
    pub fn clear(&mut self) {
        self.clips.clear();
        self.states.clear();
        self.node_maps.clear();
    }

    /// Register a node clip by name
    pub fn add_clip(&mut self, clip: NodeClip) {
        self.clips.insert(clip.name.clone(), clip);
    }

    /// Register a root entity's child node map
    pub fn register_entity(&mut self, entity_id: EntityId, node_map: HashMap<String, EntityId>) {
        self.node_maps.insert(entity_id, node_map);
    }

    /// Number of registered node clips
    pub fn clip_count(&self) -> usize {
        self.clips.len()
    }

    /// Number of active node-animated entities
    pub fn active_count(&self) -> usize {
        self.states.len()
    }

    /// Scan the world for entities with `animator` (but no `skeleton`) that are
    /// registered in `node_maps`. Creates playback states for newly discovered entities.
    /// Detects clip/speed/blend changes for existing entities.
    pub fn sync_from_world(&mut self, world: &FlintWorld) {
        for entity in world.all_entities() {
            // Must be registered in our node_maps
            if !self.node_maps.contains_key(&entity.id) {
                continue;
            }

            let Some(components) = world.get_components(entity.id) else {
                continue;
            };

            // Must have animator but NOT skeleton (skeletal handled by SkeletalSync)
            let Some(animator) = components.get("animator") else {
                continue;
            };
            if components.get("skeleton").is_some() {
                continue;
            }

            // If already tracked, check for clip changes and blend_target changes
            if let Some(state) = self.states.get_mut(&entity.id) {
                // Check if clip name changed (e.g. script called play_clip)
                let ecs_clip = animator
                    .get("clip")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let ecs_playing = animator
                    .get("playing")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let ecs_speed = animator
                    .get("speed")
                    .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                    .unwrap_or(1.0);
                let ecs_looping = animator
                    .get("loop")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                // Clip changed — reset playback
                if !ecs_clip.is_empty() && ecs_clip != state.clip_name && self.clips.contains_key(&ecs_clip) {
                    state.clip_name = ecs_clip;
                    state.time = 0.0;
                    state.playing = ecs_playing;
                    state.speed = ecs_speed;
                    state.looping = ecs_looping;
                    state.blend_target.clear();
                    state.blend_elapsed = 0.0;
                    continue;
                }

                // Sync playing/speed/loop state
                state.playing = ecs_playing;
                state.speed = ecs_speed;
                state.looping = ecs_looping;

                // Check for blend_target changes
                let ecs_blend_target = animator
                    .get("blend_target")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let ecs_blend_duration = animator
                    .get("blend_duration")
                    .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                    .unwrap_or(0.3) as f32;

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

            // New entity — create playback state
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

            let mut state = NodePlaybackState::new(clip_name, speed, looping, playing);

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

    /// Advance all node animation playbacks and write transforms to child entities.
    pub fn advance_and_apply(&mut self, world: &mut FlintWorld, dt: f64) {
        let entity_ids: Vec<EntityId> = self.states.keys().copied().collect();

        for entity_id in entity_ids {
            let state = self.states.get_mut(&entity_id).unwrap();
            let Some(clip) = self.clips.get(&state.clip_name) else {
                continue;
            };
            let Some(node_map) = self.node_maps.get(&entity_id) else {
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

            // Sample and apply each node track
            for track in &clip.node_tracks {
                let Some(&child_entity_id) = node_map.get(&track.node_name) else {
                    continue;
                };

                // Convert NodeTrack to JointTrack for sampling (reuse existing sampler)
                let joint_track = JointTrack {
                    joint_index: 0, // unused for sampling
                    property: track.property.clone(),
                    interpolation: track.interpolation.clone(),
                    keyframes: track.keyframes.clone(),
                };

                let value = sample_joint_track(&joint_track, state.time);

                // Write to child entity's transform
                apply_sampled_value(world, child_entity_id, &track.property, &value);
            }
        }
    }
}

/// Write a sampled animation value to an entity's transform component
fn apply_sampled_value(
    world: &mut FlintWorld,
    entity_id: EntityId,
    property: &JointProperty,
    value: &[f32],
) {
    let Some(components) = world.get_components_mut(entity_id) else {
        return;
    };

    // Ensure transform component exists
    if components.get("transform").is_none() {
        components.data.insert(
            "transform".to_string(),
            toml::Value::Table(toml::map::Map::new()),
        );
    }

    let Some(transform) = components.get_mut("transform") else {
        return;
    };

    let table = match transform {
        toml::Value::Table(t) => t,
        _ => return,
    };

    match property {
        JointProperty::Translation => {
            if value.len() >= 3 {
                table.insert(
                    "position".to_string(),
                    toml::Value::Array(vec![
                        toml::Value::Float(value[0] as f64),
                        toml::Value::Float(value[1] as f64),
                        toml::Value::Float(value[2] as f64),
                    ]),
                );
            }
        }
        JointProperty::Rotation => {
            if value.len() >= 4 {
                // Write as quaternion to avoid gimbal lock
                table.insert(
                    "rotation_quat".to_string(),
                    toml::Value::Array(vec![
                        toml::Value::Float(value[0] as f64),
                        toml::Value::Float(value[1] as f64),
                        toml::Value::Float(value[2] as f64),
                        toml::Value::Float(value[3] as f64),
                    ]),
                );
            }
        }
        JointProperty::Scale => {
            if value.len() >= 3 {
                table.insert(
                    "scale".to_string(),
                    toml::Value::Array(vec![
                        toml::Value::Float(value[0] as f64),
                        toml::Value::Float(value[1] as f64),
                        toml::Value::Float(value[2] as f64),
                    ]),
                );
            }
        }
    }
}

impl Default for NodeSync {
    fn default() -> Self {
        Self::new()
    }
}
