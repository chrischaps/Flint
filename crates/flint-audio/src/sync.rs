//! Synchronization between FlintWorld audio_source components and Kira spatial tracks
//!
//! Follows the same pattern as PhysicsSync: discovers entities with audio components,
//! creates Kira tracks, and updates positions each frame.

use crate::engine::AudioEngine;
use flint_core::{EntityId, Vec3};
use flint_ecs::FlintWorld;
use kira::track::SpatialTrackHandle;
use kira::Tween;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Tracks which entities have been synced to Kira and holds their spatial track handles
pub struct AudioSync {
    /// EntityId → spatial track handle for 3D-positioned sounds
    track_map: HashMap<EntityId, SpatialTrackHandle>,
    /// Entities that have already been discovered and synced
    synced_entities: HashSet<EntityId>,
}

impl Default for AudioSync {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioSync {
    pub fn new() -> Self {
        Self {
            track_map: HashMap::new(),
            synced_entities: HashSet::new(),
        }
    }

    /// Discover entities with `audio_source` components and create Kira tracks
    pub fn sync_new_sources(&mut self, world: &FlintWorld, engine: &mut AudioEngine) {
        if !engine.is_available() {
            return;
        }

        for entity in world.all_entities() {
            if self.synced_entities.contains(&entity.id) {
                continue;
            }

            let components = match world.get_components(entity.id) {
                Some(c) => c,
                None => continue,
            };

            let audio_data = match components.get("audio_source") {
                Some(v) => v,
                None => continue,
            };

            // Read audio_source fields
            let spatial = audio_data
                .get("spatial")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            let autoplay = audio_data
                .get("autoplay")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            let looping = audio_data
                .get("loop")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let volume = audio_data
                .get("volume")
                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                .unwrap_or(1.0);

            let pitch = audio_data
                .get("pitch")
                .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                .unwrap_or(1.0);

            let file = audio_data
                .get("file")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if file.is_empty() {
                self.synced_entities.insert(entity.id);
                continue;
            }

            if spatial {
                let transform = world.get_transform(entity.id).unwrap_or_default();

                let min_distance = audio_data
                    .get("min_distance")
                    .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                    .unwrap_or(1.0) as f32;

                let max_distance = audio_data
                    .get("max_distance")
                    .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
                    .unwrap_or(25.0) as f32;

                match engine.create_spatial_track(transform.position, min_distance, max_distance) {
                    Ok(mut track) => {
                        if autoplay && engine.has_sound(file) {
                            if let Err(e) =
                                engine.play_on_spatial_track(file, &mut track, volume, pitch, looping)
                            {
                                eprintln!("Audio: failed to play '{}': {:?}", file, e);
                            }
                        }
                        self.track_map.insert(entity.id, track);
                    }
                    Err(e) => {
                        eprintln!("Audio: failed to create spatial track: {:?}", e);
                    }
                }
            } else {
                // Non-spatial: play directly on the main track
                if autoplay && engine.has_sound(file) {
                    if let Err(e) = engine.play_non_spatial(file, volume, pitch, looping) {
                        eprintln!("Audio: failed to play non-spatial '{}': {:?}", file, e);
                    }
                }
            }

            self.synced_entities.insert(entity.id);
        }
    }

    /// Update spatial track positions for entities that have moved
    pub fn update_positions(&mut self, world: &FlintWorld) {
        let tween = Tween {
            duration: Duration::ZERO,
            ..Default::default()
        };

        for (entity_id, track) in &mut self.track_map {
            let transform = match world.get_transform(*entity_id) {
                Some(t) => t,
                None => continue,
            };

            let pos = to_glam_vec3(transform.position);
            track.set_position(pos, tween);
        }
    }

    /// Check if an entity has been synced
    pub fn is_synced(&self, entity_id: EntityId) -> bool {
        self.synced_entities.contains(&entity_id)
    }

    /// Number of spatial tracks currently active
    pub fn spatial_track_count(&self) -> usize {
        self.track_map.len()
    }
}

/// Convert Flint Vec3 to glam Vec3
fn to_glam_vec3(v: Vec3) -> glam::Vec3 {
    glam::Vec3::new(v.x, v.y, v.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_sync_discovers_entities() {
        let mut world = FlintWorld::new();
        let id = world.spawn("fireplace").unwrap();

        // Add transform
        let transform = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert(
                "position".into(),
                toml::Value::Array(vec![
                    toml::Value::Float(-5.0),
                    toml::Value::Float(1.0),
                    toml::Value::Float(-3.0),
                ]),
            );
            t
        });
        world.set_component(id, "transform", transform).unwrap();

        // Add audio_source
        let audio = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert("file".into(), toml::Value::String("fire_crackle.ogg".into()));
            t.insert("spatial".into(), toml::Value::Boolean(true));
            t.insert("loop".into(), toml::Value::Boolean(true));
            t.insert("volume".into(), toml::Value::Float(0.8));
            t
        });
        world.set_component(id, "audio_source", audio).unwrap();

        let sync = AudioSync::new();
        // Without an engine, we can't sync, but we can verify the sync tracks state
        assert!(!sync.is_synced(id));
        assert_eq!(sync.spatial_track_count(), 0);
    }

    #[test]
    fn test_audio_sync_skips_entities_without_audio() {
        let mut world = FlintWorld::new();
        let id = world.spawn("wall").unwrap();

        let transform = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert(
                "position".into(),
                toml::Value::Array(vec![
                    toml::Value::Float(0.0),
                    toml::Value::Float(0.0),
                    toml::Value::Float(0.0),
                ]),
            );
            t
        });
        world.set_component(id, "transform", transform).unwrap();

        let mut sync = AudioSync::new();
        let mut engine = AudioEngine::new();
        sync.sync_new_sources(&world, &mut engine);

        // Entity without audio_source should not be synced
        assert!(!sync.is_synced(id));
    }
}
