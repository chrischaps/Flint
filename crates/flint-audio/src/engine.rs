//! Audio engine wrapping Kira's AudioManager
//!
//! Handles sound loading, listener management, and spatial track creation.
//! Degrades gracefully when no audio device is available.

use flint_core::{Result, Vec3};
use kira::effect::filter::{FilterBuilder, FilterHandle, FilterMode};
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::track::{SpatialTrackBuilder, SpatialTrackDistances, SpatialTrackHandle};
use kira::{AudioManager, DefaultBackend, Easing, Tween};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// Wraps Kira's AudioManager with sound caching and listener management
pub struct AudioEngine {
    manager: Option<AudioManager<DefaultBackend>>,
    listener: Option<kira::listener::ListenerHandle>,
    sound_cache: HashMap<String, StaticSoundData>,
    master_volume: f64,
    filter_handle: Option<FilterHandle>,
    /// Keeps spatial track + sound handles alive until one-shot sounds finish
    oneshot_tracks: Vec<(SpatialTrackHandle, StaticSoundHandle)>,
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioEngine {
    pub fn new() -> Self {
        // Set up main track with a low-pass filter (fully open at 20 kHz by default)
        let mut settings = kira::AudioManagerSettings::<DefaultBackend>::default();
        let filter_handle = settings.main_track_builder.add_effect(
            FilterBuilder::new()
                .mode(FilterMode::LowPass)
                .cutoff(20000.0)
        );

        // Try to create the audio manager; gracefully fail if no device
        let manager = AudioManager::<DefaultBackend>::new(settings)
            .map_err(|e| eprintln!("Audio: no device available ({e}), running silent"))
            .ok();

        let has_audio = manager.is_some();
        Self {
            manager,
            listener: None,
            sound_cache: HashMap::new(),
            master_volume: 1.0,
            filter_handle: if has_audio { Some(filter_handle) } else { None },
            oneshot_tracks: Vec::new(),
        }
    }

    /// Whether audio is actually available
    pub fn is_available(&self) -> bool {
        self.manager.is_some()
    }

    /// Load a sound file into the cache
    pub fn load_sound(&mut self, name: &str, path: &Path) -> Result<()> {
        if self.sound_cache.contains_key(name) {
            return Ok(());
        }

        let sound_data = StaticSoundData::from_file(path)
            .map_err(|e| flint_core::FlintError::AudioError(format!("Failed to load '{}': {}", path.display(), e)))?;

        self.sound_cache.insert(name.to_string(), sound_data);
        Ok(())
    }

    /// Create the listener at the given position (typically the player/camera)
    pub fn create_listener(&mut self, position: Vec3) -> Result<()> {
        let manager = match &mut self.manager {
            Some(m) => m,
            None => return Ok(()),
        };

        let pos = to_glam_vec3(position);
        let orientation = glam::Quat::IDENTITY;

        let handle = manager
            .add_listener(pos, orientation)
            .map_err(|e| flint_core::FlintError::AudioError(format!("Failed to create listener: {e}")))?;

        self.listener = Some(handle);
        Ok(())
    }

    /// Update listener position and orientation from yaw/pitch angles
    pub fn update_listener(&mut self, position: Vec3, yaw: f32, pitch: f32) {
        let Some(listener) = &mut self.listener else {
            return;
        };

        let pos = to_glam_vec3(position);
        let orientation = glam::Quat::from_euler(glam::EulerRot::YXZ, -yaw, -pitch, 0.0);

        let tween = Tween {
            duration: Duration::ZERO,
            ..Default::default()
        };

        listener.set_position(pos, tween);
        listener.set_orientation(orientation, tween);
    }

    /// Create a spatial track for an entity's audio source
    pub fn create_spatial_track(
        &mut self,
        position: Vec3,
        min_distance: f32,
        max_distance: f32,
    ) -> Result<SpatialTrackHandle> {
        let manager = match &mut self.manager {
            Some(m) => m,
            None => {
                return Err(flint_core::FlintError::AudioError(
                    "No audio device".into(),
                ));
            }
        };

        let listener_id = match &self.listener {
            Some(l) => l.id(),
            None => {
                return Err(flint_core::FlintError::AudioError(
                    "No listener created".into(),
                ));
            }
        };

        let pos = to_glam_vec3(position);

        let builder = SpatialTrackBuilder::new()
            .distances(SpatialTrackDistances {
                min_distance,
                max_distance,
            })
            .attenuation_function(Some(Easing::OutPowf(2.0)));

        let handle = manager
            .add_spatial_sub_track(listener_id, pos, builder)
            .map_err(|e| flint_core::FlintError::AudioError(format!("Failed to create spatial track: {e}")))?;

        Ok(handle)
    }

    /// Play a cached sound on a spatial track (looping or one-shot)
    pub fn play_on_spatial_track(
        &mut self,
        sound_name: &str,
        track: &mut SpatialTrackHandle,
        volume: f64,
        pitch: f64,
        looping: bool,
    ) -> Result<StaticSoundHandle> {
        let sound_data = self
            .sound_cache
            .get(sound_name)
            .ok_or_else(|| flint_core::FlintError::AudioError(format!("Sound not cached: {sound_name}")))?
            .clone();

        let mut data = sound_data
            .volume(amplitude_to_db(volume * self.master_volume))
            .playback_rate(kira::PlaybackRate(pitch));

        if looping {
            data = data.loop_region(..);
        }

        let handle = track
            .play(data)
            .map_err(|e| flint_core::FlintError::AudioError(format!("Failed to play '{sound_name}': {e}")))?;

        Ok(handle)
    }

    /// Play a cached sound directly on the main track (non-spatial, e.g. ambient)
    pub fn play_non_spatial(
        &mut self,
        sound_name: &str,
        volume: f64,
        pitch: f64,
        looping: bool,
    ) -> Result<StaticSoundHandle> {
        let manager = match &mut self.manager {
            Some(m) => m,
            None => {
                return Err(flint_core::FlintError::AudioError(
                    "No audio device".into(),
                ));
            }
        };

        let sound_data = self
            .sound_cache
            .get(sound_name)
            .ok_or_else(|| flint_core::FlintError::AudioError(format!("Sound not cached: {sound_name}")))?
            .clone();

        let mut data = sound_data
            .volume(amplitude_to_db(volume * self.master_volume))
            .playback_rate(kira::PlaybackRate(pitch));

        if looping {
            data = data.loop_region(..);
        }

        let handle = manager
            .play(data)
            .map_err(|e| flint_core::FlintError::AudioError(format!("Failed to play '{sound_name}': {e}")))?;

        Ok(handle)
    }

    /// Play a one-shot sound at a 3D position (creates a temporary spatial track).
    /// The track and sound handles are kept alive until the sound finishes playing.
    pub fn play_at_position(
        &mut self,
        sound_name: &str,
        position: Vec3,
        volume: f64,
    ) -> Result<()> {
        let mut track = self.create_spatial_track(position, 1.0, 25.0)?;
        let handle = self.play_on_spatial_track(sound_name, &mut track, volume, 1.0, false)?;
        self.oneshot_tracks.push((track, handle));
        Ok(())
    }

    /// Remove finished one-shot spatial sounds. Call once per frame.
    pub fn cleanup_finished_oneshots(&mut self) {
        self.oneshot_tracks.retain(|(_track, sound)| {
            sound.state() != kira::sound::PlaybackState::Stopped
        });
    }

    /// Drop all one-shot spatial handles (for scene transitions).
    pub fn clear_oneshots(&mut self) {
        self.oneshot_tracks.clear();
    }

    /// Check if a sound is already loaded
    pub fn has_sound(&self, name: &str) -> bool {
        self.sound_cache.contains_key(name)
    }

    /// Set the low-pass filter cutoff frequency (Hz). 20000 = fully open, lower = more muffled.
    pub fn set_filter_cutoff(&mut self, hz: f32) {
        if let Some(handle) = &mut self.filter_handle {
            let tween = Tween {
                duration: Duration::from_millis(16),
                ..Default::default()
            };
            handle.set_cutoff(hz as f64, tween);
        }
    }
}

/// Convert Flint Vec3 to glam Vec3
fn to_glam_vec3(v: Vec3) -> glam::Vec3 {
    glam::Vec3::new(v.x, v.y, v.z)
}

/// Convert linear amplitude (0.0–2.0) to decibels
pub fn amplitude_to_db(amplitude: f64) -> kira::Decibels {
    if amplitude <= 0.0 {
        kira::Decibels(-60.0) // silence
    } else {
        kira::Decibels((20.0 * (amplitude as f32).log10()).max(-60.0))
    }
}
