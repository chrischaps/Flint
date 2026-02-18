//! Synchronization between FlintWorld audio_source components and Kira spatial tracks
//!
//! Follows the same pattern as PhysicsSync: discovers entities with audio components,
//! creates Kira tracks, and updates positions each frame.
//! Also propagates per-frame pitch/volume changes from ECS to Kira sound handles.

use crate::engine::{amplitude_to_db, AudioEngine};
use flint_core::{EntityId, Vec3};
use flint_ecs::FlintWorld;
use kira::sound::static_sound::StaticSoundHandle;
use kira::track::SpatialTrackHandle;
use kira::Tween;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Per-entity audio state: spatial track handle + active sound handles + cached parameters
struct SpatialEntry {
    track: SpatialTrackHandle,
    handles: Vec<StaticSoundHandle>,
    last_volume: f64,
    last_pitch: f64,
}

/// Non-spatial entry: sound handles + cached parameters
struct NonSpatialEntry {
    handles: Vec<StaticSoundHandle>,
    last_volume: f64,
    last_pitch: f64,
}

/// Tracks which entities have been synced to Kira and holds their spatial track handles
pub struct AudioSync {
    /// EntityId → spatial audio entry for 3D-positioned sounds
    spatial_map: HashMap<EntityId, SpatialEntry>,
    /// EntityId → non-spatial audio entry (ambient/UI sounds)
    non_spatial_map: HashMap<EntityId, NonSpatialEntry>,
    /// Entities that have already been discovered and synced
    synced_entities: HashSet<EntityId>,
}

impl Default for AudioSync {
    fn default() -> Self {
        Self::new()
    }
}

/// Smooth tween duration for parameter changes (avoids clicks)
const PARAM_TWEEN: Tween = Tween {
    duration: Duration::from_millis(16),
    easing: kira::Easing::Linear,
    start_time: kira::StartTime::Immediate,
};

impl AudioSync {
    pub fn new() -> Self {
        Self {
            spatial_map: HashMap::new(),
            non_spatial_map: HashMap::new(),
            synced_entities: HashSet::new(),
        }
    }

    /// Clear all sync state for a scene transition.
    /// Stops all playing sounds before dropping handles, since Kira sounds
    /// continue playing even after their handles are dropped.
    pub fn clear(&mut self) {
        let stop_tween = Tween {
            duration: Duration::from_millis(16),
            ..Default::default()
        };
        for entry in self.spatial_map.values_mut() {
            for handle in &mut entry.handles {
                let _ = handle.stop(stop_tween);
            }
        }
        for entry in self.non_spatial_map.values_mut() {
            for handle in &mut entry.handles {
                let _ = handle.stop(stop_tween);
            }
        }
        self.spatial_map.clear();
        self.non_spatial_map.clear();
        self.synced_entities.clear();
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
                        let mut handles = Vec::new();
                        if autoplay && engine.has_sound(file) {
                            match engine.play_on_spatial_track(file, &mut track, volume, pitch, looping) {
                                Ok(handle) => handles.push(handle),
                                Err(e) => eprintln!("Audio: failed to play '{}': {:?}", file, e),
                            }
                        }
                        self.spatial_map.insert(entity.id, SpatialEntry {
                            track,
                            handles,
                            last_volume: volume,
                            last_pitch: pitch,
                        });
                    }
                    Err(e) => {
                        eprintln!("Audio: failed to create spatial track: {:?}", e);
                    }
                }
            } else {
                // Non-spatial: play directly on the main track
                let mut handles = Vec::new();
                if autoplay && engine.has_sound(file) {
                    match engine.play_non_spatial(file, volume, pitch, looping) {
                        Ok(handle) => handles.push(handle),
                        Err(e) => eprintln!("Audio: failed to play non-spatial '{}': {:?}", file, e),
                    }
                }
                self.non_spatial_map.insert(entity.id, NonSpatialEntry {
                    handles,
                    last_volume: volume,
                    last_pitch: pitch,
                });
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

        for (entity_id, entry) in &mut self.spatial_map {
            // Use world position to account for transform hierarchy (e.g. sounds parented to kart)
            let position = match world.get_world_position(*entity_id) {
                Some(p) => p,
                None => continue,
            };

            let pos = to_glam_vec3(position);
            entry.track.set_position(pos, tween);
        }
    }

    /// Read audio_source pitch/volume from ECS and push changes to Kira sound handles
    pub fn update_parameters(&mut self, world: &FlintWorld) {
        // Update spatial entries
        for (entity_id, entry) in &mut self.spatial_map {
            let (volume, pitch) = read_audio_params(world, *entity_id);

            let vol_changed = (volume - entry.last_volume).abs() > 0.001;
            let pitch_changed = (pitch - entry.last_pitch).abs() > 0.001;

            if vol_changed || pitch_changed {
                for handle in &mut entry.handles {
                    if vol_changed {
                        handle.set_volume(amplitude_to_db(volume), PARAM_TWEEN);
                    }
                    if pitch_changed {
                        handle.set_playback_rate(kira::PlaybackRate(pitch), PARAM_TWEEN);
                    }
                }
                entry.last_volume = volume;
                entry.last_pitch = pitch;
            }
        }

        // Update non-spatial entries
        for (entity_id, entry) in &mut self.non_spatial_map {
            let (volume, pitch) = read_audio_params(world, *entity_id);

            let vol_changed = (volume - entry.last_volume).abs() > 0.001;
            let pitch_changed = (pitch - entry.last_pitch).abs() > 0.001;

            if vol_changed || pitch_changed {
                for handle in &mut entry.handles {
                    if vol_changed {
                        handle.set_volume(amplitude_to_db(volume), PARAM_TWEEN);
                    }
                    if pitch_changed {
                        handle.set_playback_rate(kira::PlaybackRate(pitch), PARAM_TWEEN);
                    }
                }
                entry.last_volume = volume;
                entry.last_pitch = pitch;
            }
        }
    }

    /// Check if an entity has been synced
    pub fn is_synced(&self, entity_id: EntityId) -> bool {
        self.synced_entities.contains(&entity_id)
    }

    /// Number of spatial tracks currently active
    pub fn spatial_track_count(&self) -> usize {
        self.spatial_map.len()
    }
}

/// Read volume and pitch from an entity's audio_source component
fn read_audio_params(world: &FlintWorld, entity_id: EntityId) -> (f64, f64) {
    let components = match world.get_components(entity_id) {
        Some(c) => c,
        None => return (1.0, 1.0),
    };

    let audio_data = match components.get("audio_source") {
        Some(v) => v,
        None => return (1.0, 1.0),
    };

    let volume = audio_data
        .get("volume")
        .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
        .unwrap_or(1.0);

    let pitch = audio_data
        .get("pitch")
        .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
        .unwrap_or(1.0);

    (volume, pitch)
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

    #[test]
    fn test_read_audio_params_defaults() {
        let world = FlintWorld::new();
        let (vol, pitch) = read_audio_params(&world, EntityId::new());
        assert_eq!(vol, 1.0);
        assert_eq!(pitch, 1.0);
    }
}
