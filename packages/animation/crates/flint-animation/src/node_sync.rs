//! Bridges ECS entities to node-level animation playback
//!
//! Manages per-entity node animation state, clip playback, and transform writes.
//! Analogous to `SkeletalSync` but writes to child entity transforms instead of
//! computing bone matrices for GPU skinning.

use crate::animator::{self, Animator};
use crate::node_clip::NodeClip;
use crate::playback_state::ClipPlaybackState;
use crate::skeletal_clip::{JointProperty, JointTrack};
use crate::skeletal_sampler::sample_joint_track;
use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::HashMap;

/// Per-entity node animation playback state (alias for the shared [`ClipPlaybackState`])
pub type NodePlaybackState = ClipPlaybackState;

/// Registry of node clips and per-entity playback state
#[derive(Default)]
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

    /// Return the names of all registered node clips
    pub fn clip_names(&self) -> Vec<String> {
        self.clips.keys().cloned().collect()
    }

    /// Scan the world for entities with `animator` (but no `skeleton`) that are
    /// registered in `node_maps`: start a playback for each new one, and
    /// follow the animator on the tracked ones ([`Animator::apply_to`], the
    /// rules property and skeletal clips follow too).
    pub fn sync_from_world(&mut self, world: &FlintWorld) {
        for &entity_id in world.entities_with_component(comp::ANIMATOR) {
            // Must be registered in our node_maps
            if !self.node_maps.contains_key(&entity_id) {
                continue;
            }

            let Some(components) = world.get_components(entity_id) else {
                continue;
            };

            // Must have animator but NOT skeleton (skeletal handled by SkeletalSync)
            if components.get(comp::ANIMATOR).is_none() || components.get(comp::SKELETON).is_some()
            {
                continue;
            }
            let animator = Animator::of(world, entity_id);
            let clips = &self.clips;
            let duration_of = |name: &str| clips.get(name).map(|c| c.duration);
            if let Some(state) = self.states.get_mut(&entity_id) {
                animator.apply_to(state, duration_of);
            } else if let Some(state) = animator.start(duration_of) {
                self.states.insert(entity_id, state);
            }
        }
    }

    /// Get the playback state for a given entity
    pub fn get_playback_state(&self, entity_id: &EntityId) -> Option<&NodePlaybackState> {
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

            state.advance_time(clip.duration, dt);
            // A one-shot's end, an autoplay's start: say so, or the next
            // sync reads the animator's stale `playing` as a script's.
            if let Some(mut components) = world.get_components_mut(entity_id) {
                animator::publish(state, &mut components);
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

/// Write a sampled animation value to an entity's transform component.
///
/// Uses `world.set_field()` which creates the component if needed and
/// maintains the component index.
fn apply_sampled_value(
    world: &mut FlintWorld,
    entity_id: EntityId,
    property: &JointProperty,
    value: &[f32],
) {
    match property {
        JointProperty::Translation => {
            if value.len() >= 3 {
                let _ = world.set_field(
                    entity_id,
                    comp::TRANSFORM,
                    "position",
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
                let _ = world.set_field(
                    entity_id,
                    comp::TRANSFORM,
                    "rotation_quat",
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
                let _ = world.set_field(
                    entity_id,
                    comp::TRANSFORM,
                    "scale",
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
