//! Synchronization between FlintWorld audio_source components and Kira sounds
//!
//! Follows the same pattern as PhysicsSync: discovers entities with audio components,
//! starts their sounds, and steers placed ones to the listener each frame.
//! Also propagates per-frame pitch/volume changes from ECS to Kira sound handles.

use crate::engine::{amplitude_to_db, AudioEngine, Bus, FRAME_TWEEN};
use crate::pan::{volume_and_pan, Ears, Falloff};
use flint_core::components as comp;
use flint_core::toml_util::{toml_f32, toml_f64};
use flint_core::{EntityId, Vec3};
use flint_ecs::FlintWorld;
use kira::sound::static_sound::StaticSoundHandle;
use kira::track::SpatialTrackHandle;
use kira::Tween;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// Per-entity audio state for a placed sound: its sound handles, cached
/// parameters and where it is
struct SpatialEntry {
    /// Its own kira spatial track (`?mix=old`). Without one, its sounds are
    /// panned voices on the bus that [`AudioSync::pan_sources`] steers.
    track: Option<SpatialTrackHandle>,
    handles: Vec<StaticSoundHandle>,
    last_volume: f64,
    last_pitch: f64,
    looping: bool,
    /// Where it was last put, and how it falls off with distance
    position: Vec3,
    falloff: Falloff,
    /// The amplitude and pan last sent to a panned voice
    sent: (f32, f32),
    /// Paused because nobody can hear it (#138)
    culled: bool,
}

/// Non-spatial entry: sound handles + cached parameters
struct NonSpatialEntry {
    handles: Vec<StaticSoundHandle>,
    last_volume: f64,
    last_pitch: f64,
    looping: bool,
    /// Paused because it's at volume 0 (#138)
    culled: bool,
}

/// At or below this `audio_source.volume` a loop is paused: `amplitude_to_db`
/// makes it -60 dB, which kira would still mix sample by sample.
pub const SILENT_VOLUME: f64 = 0.001;

/// A spatial loop pauses once the listener is this share past its
/// `max_distance`, and resumes inside [`RESUME_REACH`] of it, so walking
/// along the edge doesn't flutter it on and off. At `max` it's silent, so it
/// is back before it can be heard.
pub const CULL_REACH: f32 = 1.1;
pub const RESUME_REACH: f32 = 1.05;

/// Culled loops fade out and back in over this, so neither end clicks. A
/// paused loop picks up where it stopped, not from its start.
pub const CULL_FADE: Duration = Duration::from_millis(100);

const CULL_TWEEN: Tween = Tween {
    duration: CULL_FADE,
    easing: kira::Easing::Linear,
    start_time: kira::StartTime::Immediate,
};

/// A panned voice's amplitude or pan has to move this much to be sent again.
const PAN_EPSILON: f32 = 1e-3;

/// Tracks which entities have been synced to Kira and holds their sound handles
#[derive(Default)]
pub struct AudioSync {
    /// EntityId → spatial audio entry for 3D-positioned sounds
    spatial_map: HashMap<EntityId, SpatialEntry>,
    /// EntityId → non-spatial audio entry (ambient/UI sounds)
    non_spatial_map: HashMap<EntityId, NonSpatialEntry>,
    /// Entities that have already been discovered and synced
    synced_entities: HashSet<EntityId>,
    /// Autoplay sources that found their sound not yet loaded (a late
    /// bundle, a decode queue) and are waiting for it.
    waiting: HashSet<EntityId>,
    /// Mix as before #138 (the web's `?mix=old`): every loop on, heard or
    /// not, and new placed sources on kira spatial tracks
    old_mix: bool,
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
            waiting: HashSet::new(),
            old_mix: false,
        }
    }

    /// Mix as before #138, for comparing: [`cull_inaudible`](Self::cull_inaudible)
    /// resumes what it paused and pauses nothing more, and sources synced
    /// from now on get kira spatial tracks.
    pub fn set_old_mix(&mut self, old: bool) {
        self.old_mix = old;
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
        self.waiting.clear();
    }

    /// Discover entities with `audio_source` components and start their sounds
    pub fn sync_new_sources(&mut self, world: &FlintWorld, engine: &mut AudioEngine) {
        if !engine.is_available() {
            return;
        }

        for &entity_id in world.entities_with_component(comp::AUDIO_SOURCE) {
            if self.synced_entities.contains(&entity_id) {
                continue;
            }

            let components = match world.get_components(entity_id) {
                Some(c) => c,
                None => continue,
            };

            let audio_data = match components.get(comp::AUDIO_SOURCE) {
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

            let volume = audio_data.get("volume").and_then(toml_f64).unwrap_or(1.0);

            let pitch = audio_data.get("pitch").and_then(toml_f64).unwrap_or(1.0);

            let file = audio_data
                .get("file")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let bus = audio_data
                .get("bus")
                .and_then(|v| v.as_str())
                .map(Bus::from_name)
                .unwrap_or_default();

            if file.is_empty() {
                self.synced_entities.insert(entity_id);
                continue;
            }

            // An autoplay source whose sound isn't loaded yet waits, unsynced,
            // and starts once it is: on the web the audio arrives in a late
            // bundle after the first frame (#98).
            if autoplay && !engine.has_sound(file) {
                self.waiting.insert(entity_id);
                continue;
            }
            if self.waiting.remove(&entity_id) {
                tracing::info!("audio_source {file} starts: its sound has loaded");
            }

            if spatial {
                let position = world
                    .get_world_position(entity_id)
                    .unwrap_or_else(|| world.get_transform(entity_id).unwrap_or_default().position);

                let min_distance = audio_data
                    .get("min_distance")
                    .and_then(toml_f32)
                    .unwrap_or(1.0);

                let max_distance = audio_data
                    .get("max_distance")
                    .and_then(toml_f32)
                    .unwrap_or(25.0);

                let falloff = Falloff {
                    min: min_distance,
                    max: max_distance,
                    rolloff: crate::engine::DEFAULT_ROLLOFF,
                };
                let mut entry = SpatialEntry {
                    track: None,
                    handles: Vec::new(),
                    last_volume: volume,
                    last_pitch: pitch,
                    looping,
                    position,
                    falloff,
                    sent: (0.0, 0.0),
                    culled: false,
                };
                if self.old_mix {
                    match engine.create_spatial_track(position, min_distance, max_distance, bus) {
                        Ok(mut track) => {
                            if autoplay {
                                match engine
                                    .play_on_spatial_track(file, &mut track, volume, pitch, looping)
                                {
                                    Ok(handle) => entry.handles.push(handle),
                                    Err(e) => tracing::warn!("Failed to play '{}': {:?}", file, e),
                                }
                            }
                            entry.track = Some(track);
                        }
                        Err(e) => {
                            tracing::warn!("Failed to create spatial track: {:?}", e);
                            continue;
                        }
                    }
                } else if autoplay {
                    let (gain, pan) =
                        volume_and_pan(engine.ears().gains(to_glam_vec3(position), &falloff));
                    let amplitude = volume as f32 * gain;
                    match engine.play_panned(file, amplitude as f64, pan, pitch, looping, bus) {
                        Ok(handle) => {
                            entry.handles.push(handle);
                            entry.sent = (amplitude, pan);
                        }
                        Err(e) => tracing::warn!("Failed to play '{}': {:?}", file, e),
                    }
                }
                self.spatial_map.insert(entity_id, entry);
            } else {
                // Non-spatial: play directly on the main track
                let mut handles = Vec::new();
                if autoplay && engine.has_sound(file) {
                    match engine.play_non_spatial(file, volume, pitch, looping, bus) {
                        Ok(handle) => handles.push(handle),
                        Err(e) => {
                            tracing::warn!("Failed to play non-spatial '{}': {:?}", file, e)
                        }
                    }
                }
                self.non_spatial_map.insert(
                    entity_id,
                    NonSpatialEntry {
                        handles,
                        last_volume: volume,
                        last_pitch: pitch,
                        looping,
                        culled: false,
                    },
                );
            }

            self.synced_entities.insert(entity_id);
        }
    }

    /// Update spatial positions for entities that have moved
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

            entry.position = position;
            if let Some(track) = &mut entry.track {
                track.set_position(to_glam_vec3(position), tween);
            }
        }
    }

    /// Steer placed sources' panned voices to the listener: each one's
    /// volume and pan from its distance and side, as kira's spatial track
    /// would work them out per sample (#138, [`crate::pan`]). Call after
    /// [`update_parameters`](Self::update_parameters) and
    /// [`cull_inaudible`](Self::cull_inaudible).
    pub fn pan_sources(&mut self, ears: &Ears) {
        for entry in self.spatial_map.values_mut() {
            if entry.track.is_some() || entry.culled || entry.handles.is_empty() {
                continue;
            }
            let (gain, pan) =
                volume_and_pan(ears.gains(to_glam_vec3(entry.position), &entry.falloff));
            let amplitude = entry.last_volume as f32 * gain;
            if (amplitude - entry.sent.0).abs() < PAN_EPSILON
                && (pan - entry.sent.1).abs() < PAN_EPSILON
            {
                continue;
            }
            for handle in &mut entry.handles {
                handle.set_volume(amplitude_to_db(amplitude as f64), FRAME_TWEEN);
                handle.set_panning(kira::Panning(pan), FRAME_TWEEN);
            }
            entry.sent = (amplitude, pan);
        }
    }

    /// Pause looping sources nobody can hear, and resume them when someone
    /// can (#138): a spatial loop past its `max_distance` from `listener`,
    /// and any loop at volume 0 (Float starts its songs, weather beds and
    /// wash loops that way and only ever fades them). kira skips a paused
    /// sound's resampling, and a paused track's sounds and spatialisation.
    /// Call after [`update_positions`](Self::update_positions) and
    /// [`update_parameters`](Self::update_parameters).
    pub fn cull_inaudible(&mut self, listener: Option<Vec3>) {
        for entry in self.spatial_map.values_mut() {
            if !entry.looping || entry.handles.is_empty() {
                continue;
            }
            let reach = if entry.culled {
                RESUME_REACH
            } else {
                CULL_REACH
            };
            let far =
                listener.is_some_and(|l| (l - entry.position).length() > entry.falloff.max * reach);
            let cull = !self.old_mix && (far || entry.last_volume <= SILENT_VOLUME);
            if cull != entry.culled {
                match &mut entry.track {
                    Some(track) if cull => track.pause(CULL_TWEEN),
                    Some(track) => track.resume(CULL_TWEEN),
                    None => {
                        for handle in &mut entry.handles {
                            if cull {
                                handle.pause(CULL_TWEEN);
                            } else {
                                handle.resume(CULL_TWEEN);
                            }
                        }
                    }
                }
                entry.culled = cull;
            }
        }
        for entry in self.non_spatial_map.values_mut() {
            if !entry.looping || entry.handles.is_empty() {
                continue;
            }
            let cull = !self.old_mix && entry.last_volume <= SILENT_VOLUME;
            if cull != entry.culled {
                for handle in &mut entry.handles {
                    if cull {
                        handle.pause(CULL_TWEEN);
                    } else {
                        handle.resume(CULL_TWEEN);
                    }
                }
                entry.culled = cull;
            }
        }
    }

    /// Add the `audio_source`s' share to `counts`, which
    /// [`AudioEngine::mix_counts`] filled from the buses: the voices on
    /// their own spatial tracks (`?mix=old`), which a bus doesn't count as
    /// its sounds, and which tracks and voices are paused.
    pub fn add_mix_counts(&self, counts: &mut crate::engine::MixCounts) {
        for entry in self.spatial_map.values() {
            let on_track = entry.track.as_ref().map_or(0, |t| t.num_sounds());
            counts.voices += on_track;
            if entry.culled {
                counts.paused_voices += entry.handles.len();
                counts.paused_tracks += usize::from(entry.track.is_some());
            }
        }
        for entry in self.non_spatial_map.values().filter(|e| e.culled) {
            counts.paused_voices += entry.handles.len();
        }
    }

    /// How many sources are paused because nobody can hear them (#138).
    pub fn culled_count(&self) -> usize {
        self.spatial_map.values().filter(|e| e.culled).count()
            + self.non_spatial_map.values().filter(|e| e.culled).count()
    }

    /// Read audio_source pitch/volume from ECS and push changes to Kira sound
    /// handles. A panned source's volume goes out with its pan, from
    /// [`pan_sources`](Self::pan_sources).
    pub fn update_parameters(&mut self, world: &FlintWorld) {
        // Update spatial entries
        for (entity_id, entry) in &mut self.spatial_map {
            let (volume, pitch) = read_audio_params(world, *entity_id);

            let vol_changed = entry.track.is_some() && (volume - entry.last_volume).abs() > 0.001;
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
            }
            entry.last_volume = volume;
            entry.last_pitch = pitch;
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

    /// Number of placed (spatial) sources synced
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

    let audio_data = match components.get(comp::AUDIO_SOURCE) {
        Some(v) => v,
        None => return (1.0, 1.0),
    };

    let volume = audio_data.get("volume").and_then(toml_f64).unwrap_or(1.0);

    let pitch = audio_data.get("pitch").and_then(toml_f64).unwrap_or(1.0);

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
            t.insert(
                "file".into(),
                toml::Value::String("fire_crackle.ogg".into()),
            );
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

    /// A world with a far spatial loop, a near one at volume 0, a near
    /// audible one and a silent non-spatial bed, synced on a real device.
    /// `None` where there's no audio device (CI). `old` mixes as before #138.
    fn culling_scene(old: bool) -> Option<(FlintWorld, AudioSync, AudioEngine, [EntityId; 4])> {
        let mut engine = AudioEngine::new();
        if !engine.is_available() {
            eprintln!("no audio device: culling test skipped");
            return None;
        }
        let tone: Vec<f32> = (0..4410).map(|i| (i as f32 * 0.05).sin() * 0.1).collect();
        engine
            .load_sound_from_samples("hum", 44_100, &[&tone])
            .ok()?;
        engine.create_listener(Vec3::new(0.0, 0.0, 0.0)).ok()?;

        let mut world = FlintWorld::new();
        let mut spawn = |name: &str, at: [f32; 3], volume: f64, spatial: bool| {
            let id = world.spawn(name).unwrap();
            let position = at.iter().map(|&v| toml::Value::Float(v as f64)).collect();
            world
                .set_field(id, "transform", "position", toml::Value::Array(position))
                .unwrap();
            for (field, value) in [
                ("file", toml::Value::String("hum".into())),
                ("loop", toml::Value::Boolean(true)),
                ("spatial", toml::Value::Boolean(spatial)),
                ("volume", toml::Value::Float(volume)),
                ("max_distance", toml::Value::Float(12.0)),
            ] {
                world.set_field(id, "audio_source", field, value).unwrap();
            }
            id
        };
        let far = spawn("far", [40.0, 0.0, 0.0], 0.8, true);
        let quiet = spawn("quiet", [2.0, 0.0, 0.0], 0.0, true);
        let near = spawn("near", [3.0, 0.0, 0.0], 0.8, true);
        let bed = spawn("bed", [0.0, 0.0, 0.0], 0.0, false);
        let mut sync = AudioSync::new();
        engine.set_old_mix(old);
        sync.set_old_mix(old);
        sync.sync_new_sources(&world, &mut engine);
        sync.update_positions(&world);
        Some((world, sync, engine, [far, quiet, near, bed]))
    }

    #[test]
    fn loops_nobody_can_hear_pause_and_come_back() {
        let Some((mut world, mut sync, engine, [far, quiet, near, _bed])) = culling_scene(false)
        else {
            return;
        };
        let culled = |s: &AudioSync, id| {
            s.spatial_map
                .get(&id)
                .map(|e| e.culled)
                .or_else(|| s.non_spatial_map.get(&id).map(|e| e.culled))
                .unwrap()
        };
        sync.cull_inaudible(engine.listener_position());
        // Out of earshot, at volume 0 and the silent bed pause; the one you
        // can hear plays on.
        assert!(culled(&sync, far) && culled(&sync, quiet) && !culled(&sync, near));
        assert_eq!(sync.culled_count(), 3);
        let mut counts = engine.mix_counts().unwrap();
        sync.add_mix_counts(&mut counts);
        // Placed sources are panned voices now, not tracks.
        assert_eq!((counts.tracks, counts.paused_tracks), (0, 0));
        assert_eq!((counts.voices, counts.paused_voices), (4, 3));

        // The near one, 3 m to the listener's right, is steered right.
        sync.pan_sources(&engine.ears());
        let (amplitude, pan) = sync.spatial_map[&near].sent;
        assert!(amplitude > 0.3 && pan > 0.5, "{amplitude} {pan}");

        // Fading the quiet one up brings it back...
        world
            .set_field(quiet, "audio_source", "volume", toml::Value::Float(0.4))
            .unwrap();
        sync.update_parameters(&world);
        sync.cull_inaudible(engine.listener_position());
        assert!(!culled(&sync, quiet));

        // ...and the far one resumes only once well inside reach, not at
        // the edge it was culled past.
        let listener = |x: f32| Some(Vec3::new(x, 0.0, 0.0));
        sync.cull_inaudible(listener(40.0 - 12.0 * 1.08));
        assert!(
            culled(&sync, far),
            "between resume and cull reach: stays paused"
        );
        sync.cull_inaudible(listener(40.0 - 12.0));
        assert!(!culled(&sync, far));
        sync.cull_inaudible(listener(40.0 - 12.0 * 1.08));
        assert!(
            !culled(&sync, far),
            "and stays playing until past cull reach"
        );
        sync.cull_inaudible(listener(40.0 - 12.0 * 1.2));
        assert!(culled(&sync, far));
    }

    #[test]
    fn the_old_mix_puts_placed_sources_on_tracks_and_pauses_nothing() {
        let Some((_world, mut sync, engine, _)) = culling_scene(true) else {
            return;
        };
        sync.cull_inaudible(engine.listener_position());
        assert_eq!(sync.culled_count(), 0);
        let mut counts = engine.mix_counts().unwrap();
        sync.add_mix_counts(&mut counts);
        assert_eq!(
            (counts.tracks, counts.voices, counts.paused_voices),
            (3, 4, 0)
        );
    }

    #[test]
    fn test_read_audio_params_defaults() {
        let world = FlintWorld::new();
        let (vol, pitch) = read_audio_params(&world, EntityId::new());
        assert_eq!(vol, 1.0);
        assert_eq!(pitch, 1.0);
    }
}
