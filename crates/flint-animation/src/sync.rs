//! Bridges ECS `animator` components to the animation player

use crate::player::{advance, AnimationPlayer, PlaybackState};
use flint_core::EntityId;
use flint_ecs::FlintWorld;
use std::collections::HashMap;

/// Manages per-entity playback states, syncing between TOML components and the player.
pub struct AnimationSync {
    states: HashMap<EntityId, PlaybackState>,
}

impl AnimationSync {
    pub fn new() -> Self {
        Self {
            states: HashMap::new(),
        }
    }

    /// Number of active animated entities.
    pub fn active_count(&self) -> usize {
        self.states.len()
    }

    /// Scan the world for entities with an `animator` component and create
    /// `PlaybackState` entries for any new ones.
    pub fn sync_from_world(&mut self, world: &FlintWorld, player: &AnimationPlayer) {
        for entity in world.all_entities() {
            // Skip entities we're already tracking
            if self.states.contains_key(&entity.id) {
                continue;
            }

            let Some(components) = world.get_components(entity.id) else {
                continue;
            };
            let Some(animator) = components.get("animator") else {
                continue;
            };

            let clip_name = animator
                .get("clip")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if clip_name.is_empty() || !player.has_clip(&clip_name) {
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

            self.states.insert(
                entity.id,
                PlaybackState::new(clip_name, speed, looping, playing),
            );
        }
    }

    /// Advance all playbacks and write sampled values back to ECS components.
    pub fn advance_and_write(
        &mut self,
        world: &mut FlintWorld,
        player: &AnimationPlayer,
        dt: f64,
    ) {
        for (entity_id, state) in &mut self.states {
            let Some(clip) = player.get_clip(&state.clip_name) else {
                continue;
            };

            let Some(result) = advance(state, clip, dt) else {
                continue;
            };

            // Write sampled values back to the entity's components
            let Some(components) = world.get_components_mut(*entity_id) else {
                continue;
            };

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
                        components.set_field("transform", "position", arr);
                    }
                    crate::clip::TrackTarget::Rotation => {
                        let arr = toml::Value::Array(vec![
                            toml::Value::Float(value[0] as f64),
                            toml::Value::Float(value[1] as f64),
                            toml::Value::Float(value[2] as f64),
                        ]);
                        components.set_field("transform", "rotation", arr);
                    }
                    crate::clip::TrackTarget::Scale => {
                        let arr = toml::Value::Array(vec![
                            toml::Value::Float(value[0] as f64),
                            toml::Value::Float(value[1] as f64),
                            toml::Value::Float(value[2] as f64),
                        ]);
                        components.set_field("transform", "scale", arr);
                    }
                    crate::clip::TrackTarget::CustomFloat { component, field } => {
                        components.set_field(
                            component,
                            field,
                            toml::Value::Float(value[0] as f64),
                        );
                    }
                }
            }
        }
    }
}

impl Default for AnimationSync {
    fn default() -> Self {
        Self::new()
    }
}
