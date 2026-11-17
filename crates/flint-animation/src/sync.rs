//! Bridges ECS `animator` components to the animation player

use crate::animator::{self, Animator};
use crate::playback_state::ClipPlaybackState;
use crate::player::{advance, AnimationPlayer};
use flint_core::components as comp;
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::HashMap;

/// Manages per-entity playback states, syncing between TOML components and the player.
#[derive(Default)]
pub struct AnimationSync {
    states: HashMap<EntityId, ClipPlaybackState>,
}

impl AnimationSync {
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
        }
    }

    /// Clear all playback states for a scene transition.
    pub fn clear(&mut self) {
        self.states.clear();
    }

    /// Number of active animated entities.
    pub fn active_count(&self) -> usize {
        self.states.len()
    }

    /// The playback driving an entity's property clip
    pub fn playback_state(&self, entity_id: &EntityId) -> Option<&ClipPlaybackState> {
        self.states.get(entity_id)
    }

    /// Scan the world for entities with an `animator` component: start a
    /// playback for each new one whose clip is a property clip, and follow
    /// the animator on the tracked ones ([`Animator::apply_to`], the rules
    /// node and skeletal clips follow too).
    pub fn sync_from_world(&mut self, world: &FlintWorld, player: &AnimationPlayer) {
        let duration_of = |name: &str| player.get_clip(name).map(|c| c.duration);
        for &entity_id in world.entities_with_component(comp::ANIMATOR) {
            let animator = Animator::of(world, entity_id);
            if let Some(state) = self.states.get_mut(&entity_id) {
                animator.apply_to(state, duration_of);
            } else if let Some(state) = animator.start(duration_of) {
                self.states.insert(entity_id, state);
            }
        }
    }

    /// Advance all playbacks and write sampled values back to ECS components.
    pub fn advance_and_write(&mut self, world: &mut FlintWorld, player: &AnimationPlayer, dt: f64) {
        for (entity_id, state) in &mut self.states {
            let Some(clip) = player.get_clip(&state.clip_name) else {
                continue;
            };

            let Some(result) = advance(state, clip, dt) else {
                continue;
            };

            // Write sampled values back to the entity's components
            let Some(mut components) = world.get_components_mut(*entity_id) else {
                continue;
            };

            // A one-shot that just ended, an autoplay's start: say so, or
            // the next sync reads the stale `playing` as a script's.
            animator::publish(state, &mut components);

            for (i, track) in clip.tracks.iter().enumerate() {
                if i >= result.samples.len() {
                    break;
                }
                let value = result.samples[i];

                match &track.target {
                    crate::clip::TrackTarget::Position => {
                        let arr = toml::Value::Array(vec![
                            toml::Value::Float(value[0] as f64),
                            toml::Value::Float(value[1] as f64),
                            toml::Value::Float(value[2] as f64),
                        ]);
                        components.set_field(comp::TRANSFORM, "position", arr);
                    }
                    crate::clip::TrackTarget::Rotation => {
                        let arr = toml::Value::Array(vec![
                            toml::Value::Float(value[0] as f64),
                            toml::Value::Float(value[1] as f64),
                            toml::Value::Float(value[2] as f64),
                        ]);
                        components.set_field(comp::TRANSFORM, "rotation", arr);
                    }
                    crate::clip::TrackTarget::Scale => {
                        let arr = toml::Value::Array(vec![
                            toml::Value::Float(value[0] as f64),
                            toml::Value::Float(value[1] as f64),
                            toml::Value::Float(value[2] as f64),
                        ]);
                        components.set_field(comp::TRANSFORM, "scale", arr);
                    }
                    crate::clip::TrackTarget::CustomFloat { component, field } => {
                        components.set_field(component, field, toml::Value::Float(value[0] as f64));
                    }
                }
            }
        }
    }
}
