//! Audio engine wrapping Kira's AudioManager
//!
//! Handles sound loading, listener management, and spatial track creation.
//! Degrades gracefully when no audio device is available.

use flint_core::{Result, Vec3};
use kira::effect::filter::{FilterBuilder, FilterHandle, FilterMode};
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::track::{
    SpatialTrackBuilder, SpatialTrackDistances, SpatialTrackHandle, TrackBuilder, TrackHandle,
};
use kira::{AudioManager, Easing, Tween};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// `StaticSoundData::from_file`, which kira leaves out of wasm32 builds. The
/// web reads the bytes and decodes them from memory instead; `std::fs` fails
/// there at run time until the vfs lands (docs/design/web.md).
fn static_sound_from_file(
    path: impl AsRef<Path>,
) -> std::result::Result<StaticSoundData, kira::sound::FromFileError> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        StaticSoundData::from_file(path)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let bytes = std::fs::read(path)?;
        StaticSoundData::from_cursor(std::io::Cursor::new(bytes))
    }
}

/// The kira backend sound goes through: cpal's default device on the
/// desktop, and in the browser a WebAudio stream that measures its own
/// steadiness (`crate::web`).
#[cfg(not(target_arch = "wasm32"))]
pub type AudioBackend = kira::DefaultBackend;
#[cfg(target_arch = "wasm32")]
pub type AudioBackend = crate::web::WebAudioBackend;

/// Distances for a placed one-shot when the caller names none: full volume
/// within 1 m, silent past 25 m.
pub const DEFAULT_DISTANCES: (f32, f32) = (1.0, 25.0);

/// How a placed sound fades between its distances when the caller names no
/// curve. With `r` the listener's fraction of the way from `min` to `max`,
/// the sound is `-60·r^rolloff` dB: at 2 it holds up most of the way and
/// drops near `max`; below 1 it falls fast past `min` and tails off slowly,
/// near an inverse-distance law (0.6 between 32 and 1250 m is within 1.5 dB
/// of `(32/d)^1.2` out to 300 m).
pub const DEFAULT_ROLLOFF: f32 = 2.0;

/// Mixer bus a sound routes through. Both buses are children of the main
/// track, so the master low-pass filter still applies to everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Bus {
    #[default]
    Sfx,
    Music,
}

impl Bus {
    /// Parse an `audio_source.bus` field value ("music"/"sfx"); unknown → Sfx
    pub fn from_name(s: &str) -> Self {
        match s {
            "music" => Bus::Music,
            _ => Bus::Sfx,
        }
    }
}

/// Wraps Kira's AudioManager with sound caching and listener management
pub struct AudioEngine {
    manager: Option<AudioManager<AudioBackend>>,
    listener: Option<kira::listener::ListenerHandle>,
    sound_cache: HashMap<String, StaticSoundData>,
    master_volume: f64,
    filter_handle: Option<FilterHandle>,
    /// The cutoff and bus volumes last asked for, so a manager started
    /// later (the web's click) opens where the game already is.
    filter_cutoff: f64,
    bus_volumes: [f64; 2],
    /// The WebAudio stream's steadiness, shared with its backend.
    #[cfg(target_arch = "wasm32")]
    meter: std::sync::Arc<std::sync::Mutex<crate::meter::BufferMeter>>,
    /// Frames per WebAudio buffer (see `web::WebAudioSettings`).
    #[cfg(target_arch = "wasm32")]
    buffer_frames: u32,
    /// Music mixer bus (sub-track of main)
    music_bus: Option<TrackHandle>,
    /// SFX mixer bus (sub-track of main); all spatial tracks hang off this
    /// or the music bus rather than the main track directly
    sfx_bus: Option<TrackHandle>,
    /// Keeps spatial track + sound handles alive until one-shot sounds finish
    oneshot_tracks: Vec<(SpatialTrackHandle, StaticSoundHandle)>,
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioEngine {
    /// An engine with sound. On the desktop it opens the default device
    /// now (silent if there is none). In the browser it stays silent until
    /// [`start`](Self::start) is called from a user gesture, since an
    /// `AudioContext` made before one is kept suspended.
    pub fn new() -> Self {
        #[allow(unused_mut)]
        let mut engine = Self::silent();
        #[cfg(not(target_arch = "wasm32"))]
        engine.start();
        engine
    }

    /// An engine that loads and caches sounds but plays none until
    /// [`start`](Self::start).
    pub fn silent() -> Self {
        Self {
            manager: None,
            listener: None,
            sound_cache: HashMap::new(),
            master_volume: 1.0,
            filter_handle: None,
            filter_cutoff: 20000.0,
            bus_volumes: [1.0, 1.0],
            #[cfg(target_arch = "wasm32")]
            meter: Default::default(),
            #[cfg(target_arch = "wasm32")]
            buffer_frames: crate::web::WebAudioSettings::default().buffer_frames,
            music_bus: None,
            sfx_bus: None,
            oneshot_tracks: Vec::new(),
        }
    }

    /// Open the audio device, if it isn't open, and say whether there is
    /// sound. In the browser, call it from the click (or key) handler that
    /// begins the game. Failing leaves the engine silent, as before, and a
    /// later call tries again.
    pub fn start(&mut self) -> bool {
        if self.manager.is_some() {
            return true;
        }
        // Main track with a low-pass filter, at the cutoff last asked for
        // (fully open at 20 kHz by default)
        #[allow(unused_mut)]
        let mut settings = kira::AudioManagerSettings::<AudioBackend>::default();
        #[cfg(target_arch = "wasm32")]
        {
            settings.backend_settings.meter = self.meter.clone();
            settings.backend_settings.buffer_frames = self.buffer_frames;
        }
        let filter_handle = settings.main_track_builder.add_effect(
            FilterBuilder::new()
                .mode(FilterMode::LowPass)
                .cutoff(self.filter_cutoff),
        );

        // Try to create the audio manager; gracefully fail if no device
        let Some(mut manager) = AudioManager::<AudioBackend>::new(settings)
            .map_err(|e| tracing::warn!("No audio device available ({e}), running silent"))
            .ok()
        else {
            return false;
        };

        // Create the music and SFX buses as children of the main track
        self.music_bus = manager
            .add_sub_track(TrackBuilder::new())
            .map_err(|e| tracing::warn!("Failed to create music bus: {e}"))
            .ok();
        self.sfx_bus = manager
            .add_sub_track(TrackBuilder::new())
            .map_err(|e| tracing::warn!("Failed to create sfx bus: {e}"))
            .ok();
        self.manager = Some(manager);
        self.filter_handle = Some(filter_handle);
        let [sfx, music] = self.bus_volumes;
        if music != 1.0 {
            self.set_bus_volume(Bus::Music, music);
        }
        if sfx != 1.0 {
            self.set_bus_volume(Bus::Sfx, sfx);
        }
        true
    }

    /// Frames per WebAudio buffer for the next [`start`](Self::start).
    /// Two buffers are in flight, so one buffer is how long a frame may
    /// hold the main thread before sound drops (2048 at 44.1 kHz is 46 ms).
    #[cfg(target_arch = "wasm32")]
    pub fn set_buffer_frames(&mut self, frames: u32) {
        self.buffer_frames = frames;
    }

    /// How steadily the browser's audio stream has been fed since it
    /// opened: buffers, late ones, slack and mix time. `None` on the
    /// desktop, and before [`start`](Self::start).
    pub fn buffer_stats(&self) -> Option<crate::meter::BufferStats> {
        #[cfg(target_arch = "wasm32")]
        {
            if self.manager.is_some() {
                return self.meter.lock().ok().map(|m| m.stats());
            }
        }
        None
    }

    /// Set a mixer bus volume (linear amplitude, 0.0 = mute, 1.0 = unity).
    /// Applies multiplicatively on top of per-sound volumes, so script-driven
    /// fades keep working underneath.
    pub fn set_bus_volume(&mut self, bus: Bus, volume: f64) {
        self.bus_volumes[bus as usize] = volume;
        let handle = match bus {
            Bus::Music => &mut self.music_bus,
            Bus::Sfx => &mut self.sfx_bus,
        };
        if let Some(handle) = handle {
            let tween = Tween {
                duration: Duration::from_millis(16),
                ..Default::default()
            };
            handle.set_volume(amplitude_to_db(volume), tween);
        }
    }

    /// Whether audio is actually available
    pub fn is_available(&self) -> bool {
        self.manager.is_some()
    }

    /// Mutable access to the underlying kira manager, for hosts that start
    /// additional clocked content (the music suite) on the same device stream.
    /// Handles created on it must drop before this engine (ADR 0017).
    pub fn manager_mut(&mut self) -> Option<&mut AudioManager<AudioBackend>> {
        self.manager.as_mut()
    }

    /// Load a sound file into the cache
    pub fn load_sound(&mut self, name: &str, path: &Path) -> Result<()> {
        if self.sound_cache.contains_key(name) {
            return Ok(());
        }

        let sound_data = static_sound_from_file(path).map_err(|e| {
            flint_core::FlintError::AudioError(format!(
                "Failed to load '{}': {}",
                path.display(),
                e
            ))
        })?;

        self.sound_cache.insert(name.to_string(), sound_data);
        Ok(())
    }

    /// Cache an encoded sound held in memory (WAV, OGG, FLAC or MP3 bytes,
    /// e.g. from `include_bytes!`) under `name`. Replaces any sound already
    /// cached under that name; sounds already playing keep the old data.
    pub fn load_sound_from_bytes(&mut self, name: &str, bytes: Vec<u8>) -> Result<()> {
        let data = StaticSoundData::from_cursor(std::io::Cursor::new(bytes)).map_err(|e| {
            flint_core::FlintError::AudioError(format!("Failed to decode '{name}': {e}"))
        })?;
        self.sound_cache.insert(name.to_string(), data);
        Ok(())
    }

    /// Cache a sound made in memory under `name`, from its samples: one
    /// channel (played on both sides) or two (left, right), each in -1..1 at
    /// `sample_rate` Hz. Unequal channels are cut to the shorter. Replaces
    /// any sound already cached under that name.
    pub fn load_sound_from_samples(
        &mut self,
        name: &str,
        sample_rate: u32,
        channels: &[&[f32]],
    ) -> Result<()> {
        let frames: Arc<[kira::Frame]> = match channels {
            [mono] => mono.iter().map(|&s| kira::Frame::from_mono(s)).collect(),
            [left, right] => left
                .iter()
                .zip(right.iter())
                .map(|(&l, &r)| kira::Frame { left: l, right: r })
                .collect(),
            _ => {
                return Err(flint_core::FlintError::AudioError(format!(
                    "'{name}': {} channels; a sound has one or two",
                    channels.len()
                )))
            }
        };
        if sample_rate == 0 {
            return Err(flint_core::FlintError::AudioError(format!(
                "'{name}': sample rate 0"
            )));
        }
        self.sound_cache.insert(
            name.to_string(),
            StaticSoundData {
                sample_rate,
                frames,
                settings: Default::default(),
                slice: None,
            },
        );
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

        let handle = manager.add_listener(pos, orientation).map_err(|e| {
            flint_core::FlintError::AudioError(format!("Failed to create listener: {e}"))
        })?;

        self.listener = Some(handle);
        Ok(())
    }

    /// Whether the listener has been created
    pub fn has_listener(&self) -> bool {
        self.listener.is_some()
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

    /// Create a spatial track for an entity's audio source on the given bus
    pub fn create_spatial_track(
        &mut self,
        position: Vec3,
        min_distance: f32,
        max_distance: f32,
        bus: Bus,
    ) -> Result<SpatialTrackHandle> {
        self.create_spatial_track_with(
            position,
            (min_distance, max_distance),
            DEFAULT_ROLLOFF,
            bus,
        )
    }

    /// Create a spatial track with its own falloff curve (see
    /// [`DEFAULT_ROLLOFF`]).
    pub fn create_spatial_track_with(
        &mut self,
        position: Vec3,
        (min_distance, max_distance): (f32, f32),
        rolloff: f32,
        bus: Bus,
    ) -> Result<SpatialTrackHandle> {
        if self.manager.is_none() {
            return Err(flint_core::FlintError::AudioError("No audio device".into()));
        }

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
            .attenuation_function(Some(Easing::OutPowf(rolloff.max(0.01) as f64)));

        let bus_handle = match bus {
            Bus::Music => &mut self.music_bus,
            Bus::Sfx => &mut self.sfx_bus,
        };
        let bus_handle = bus_handle.as_mut().ok_or_else(|| {
            flint_core::FlintError::AudioError(format!("Bus {bus:?} unavailable"))
        })?;

        let handle = bus_handle
            .add_spatial_sub_track(listener_id, pos, builder)
            .map_err(|e| {
                flint_core::FlintError::AudioError(format!("Failed to create spatial track: {e}"))
            })?;

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
            .ok_or_else(|| {
                flint_core::FlintError::AudioError(format!("Sound not cached: {sound_name}"))
            })?
            .clone();

        let mut data = sound_data
            .volume(amplitude_to_db(volume * self.master_volume))
            .playback_rate(kira::PlaybackRate(pitch));

        if looping {
            data = data.loop_region(..);
        }

        let handle = track.play(data).map_err(|e| {
            flint_core::FlintError::AudioError(format!("Failed to play '{sound_name}': {e}"))
        })?;

        Ok(handle)
    }

    /// Play a cached sound on a mixer bus (non-spatial, e.g. ambient/music)
    pub fn play_non_spatial(
        &mut self,
        sound_name: &str,
        volume: f64,
        pitch: f64,
        looping: bool,
        bus: Bus,
    ) -> Result<StaticSoundHandle> {
        if self.manager.is_none() {
            return Err(flint_core::FlintError::AudioError("No audio device".into()));
        }

        let sound_data = self
            .sound_cache
            .get(sound_name)
            .ok_or_else(|| {
                flint_core::FlintError::AudioError(format!("Sound not cached: {sound_name}"))
            })?
            .clone();

        let mut data = sound_data
            .volume(amplitude_to_db(volume * self.master_volume))
            .playback_rate(kira::PlaybackRate(pitch));

        if looping {
            data = data.loop_region(..);
        }

        let bus_handle = match bus {
            Bus::Music => &mut self.music_bus,
            Bus::Sfx => &mut self.sfx_bus,
        };
        let bus_handle = bus_handle.as_mut().ok_or_else(|| {
            flint_core::FlintError::AudioError(format!("Bus {bus:?} unavailable"))
        })?;

        let handle = bus_handle.play(data).map_err(|e| {
            flint_core::FlintError::AudioError(format!("Failed to play '{sound_name}': {e}"))
        })?;

        Ok(handle)
    }

    /// Play a one-shot sound at a 3D position (creates a temporary spatial track).
    /// The track and sound handles are kept alive until the sound finishes playing.
    pub fn play_at_position(
        &mut self,
        sound_name: &str,
        position: Vec3,
        volume: f64,
        pitch: f64,
    ) -> Result<()> {
        self.play_oneshot_at(
            sound_name,
            position,
            volume,
            pitch,
            DEFAULT_DISTANCES,
            DEFAULT_ROLLOFF,
            Bus::Sfx,
        )
    }

    /// Play a one-shot at a 3D position with its own falloff, `(min, max)`
    /// metres from the listener: full volume within `min`, silent past
    /// `max`, along the curve `rolloff` (see [`DEFAULT_ROLLOFF`]). Routed
    /// through `bus`.
    #[allow(clippy::too_many_arguments)]
    pub fn play_oneshot_at(
        &mut self,
        sound_name: &str,
        position: Vec3,
        volume: f64,
        pitch: f64,
        distances: (f32, f32),
        rolloff: f32,
        bus: Bus,
    ) -> Result<()> {
        if !self.sound_cache.contains_key(sound_name) {
            return Err(flint_core::FlintError::AudioError(format!(
                "Sound not cached: {sound_name}"
            )));
        }
        let mut track = self.create_spatial_track_with(position, distances, rolloff, bus)?;
        let handle = self.play_on_spatial_track(sound_name, &mut track, volume, pitch, false)?;
        self.oneshot_tracks.push((track, handle));
        Ok(())
    }

    /// Remove finished one-shot spatial sounds. Call once per frame.
    pub fn cleanup_finished_oneshots(&mut self) {
        self.oneshot_tracks
            .retain(|(_track, sound)| sound.state() != kira::sound::PlaybackState::Stopped);
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
        self.filter_cutoff = hz as f64;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_load_from_samples_and_bytes_without_a_device() {
        let mut engine = AudioEngine::new();
        let tone: Vec<f32> = (0..441).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
        engine
            .load_sound_from_samples("mono", 44_100, &[&tone])
            .unwrap();
        engine
            .load_sound_from_samples("stereo", 22_050, &[&tone, &tone[..400]])
            .unwrap();
        assert!(engine.has_sound("mono") && engine.has_sound("stereo"));
        assert_eq!(engine.sound_cache["mono"].frames.len(), 441);
        assert_eq!(
            engine.sound_cache["stereo"].frames.len(),
            400,
            "cut to the shorter"
        );
        assert_eq!(
            engine.sound_cache["mono"].frames[3].left,
            engine.sound_cache["mono"].frames[3].right
        );
        assert!(engine.load_sound_from_samples("none", 44_100, &[]).is_err());
        assert!(engine
            .load_sound_from_samples("three", 44_100, &[&tone, &tone, &tone])
            .is_err());
        assert!(engine
            .load_sound_from_samples("still", 0, &[&tone])
            .is_err());

        // A 16-bit mono WAV of the same tone, decoded from memory.
        let mut wav = Vec::new();
        let data_len = tone.len() as u32 * 2;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        for v in [
            16u32.to_le_bytes(),
            [1, 0, 1, 0],
            44_100u32.to_le_bytes(),
            88_200u32.to_le_bytes(),
            [2, 0, 16, 0],
        ] {
            wav.extend_from_slice(&v);
        }
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for s in &tone {
            wav.extend_from_slice(&((s * 32767.0) as i16).to_le_bytes());
        }
        engine.load_sound_from_bytes("wav", wav).unwrap();
        assert_eq!(engine.sound_cache["wav"].frames.len(), 441);
        assert!(engine.load_sound_from_bytes("junk", vec![1, 2, 3]).is_err());
    }

    #[test]
    fn a_silent_engine_remembers_its_mix_for_start() {
        let mut engine = AudioEngine::silent();
        assert!(!engine.is_available() && !engine.has_listener());
        engine.set_filter_cutoff(800.0);
        engine.set_bus_volume(Bus::Music, 0.25);
        // Sounds load and cache without a device, ready for the click.
        engine
            .load_sound_from_samples("sea", 22_050, &[&[0.0, 0.1, 0.0]])
            .unwrap();
        assert!(engine.has_sound("sea"));
        assert_eq!(engine.filter_cutoff, 800.0);
        assert_eq!(engine.bus_volumes, [1.0, 0.25]);
        assert!(engine.create_listener(Vec3::new(0.0, 0.0, 0.0)).is_ok());
        assert!(!engine.has_listener(), "no listener without a device");
        assert!(engine.buffer_stats().is_none());
    }

    #[test]
    fn an_unknown_sound_is_an_error_not_a_panic() {
        let mut engine = AudioEngine::new();
        let at = Vec3::new(0.0, 0.0, 0.0);
        assert!(engine
            .play_oneshot_at("nope", at, 1.0, 1.0, (2.0, 80.0), 0.7, Bus::Sfx)
            .is_err());
        assert!(engine
            .play_non_spatial("nope", 1.0, 1.0, false, Bus::Music)
            .is_err());
    }
}
