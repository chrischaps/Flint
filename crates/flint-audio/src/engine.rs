//! Audio engine wrapping Kira's AudioManager
//!
//! Handles sound loading, listener management, and spatial track creation.
//! Degrades gracefully when no audio device is available.

use crate::decode::{DecodeQueue, Need};
use crate::pan::{volume_and_pan, Ears, Falloff};
use flint_core::{vfs, Result, Vec3};
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

/// `StaticSoundData::from_file`, read through the vfs: the bytes come from
/// the active file source (the disk, or the web's bundle) and kira decodes
/// them from memory, which is all its `from_file` does after opening the
/// file. kira leaves `from_file` out of wasm32 builds anyway.
fn static_sound_from_file(
    path: impl AsRef<Path>,
) -> std::result::Result<StaticSoundData, kira::sound::FromFileError> {
    let bytes = vfs::read(path)?;
    StaticSoundData::from_cursor(std::io::Cursor::new(bytes))
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

/// A queued sound asked for before its turn is decoded on the spot when its
/// file is this small (a one-shot: about 12 ms in the browser at most), and
/// skipped while it moves to the front of the queue when it's bigger.
pub const DECODE_NOW_BYTES: usize = 64 * 1024;

/// How many placed one-shots of one sound may play at once. One more steals
/// the oldest, which fades out over [`STEAL_FADE`]: raft creaks and drips
/// come in bursts, and by then the oldest has mostly died away (#138).
pub const MAX_ONESHOTS_PER_SOUND: usize = 4;

/// The fade a stolen one-shot stops with.
pub const STEAL_FADE: Duration = Duration::from_millis(50);

/// A placed sound's gains change over a frame, so a step is never heard.
pub(crate) const FRAME_TWEEN: Tween = Tween {
    duration: Duration::from_millis(16),
    easing: Easing::Linear,
    start_time: kira::StartTime::Immediate,
};

/// What the mixer is doing, for the perf readout (#138): the sounds and
/// spatial tracks kira holds, and how many of each are paused because
/// nobody can hear them. A paused track skips its sounds and its
/// spatialisation; a paused sound skips its resampling.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MixCounts {
    /// Sounds on the buses and on spatial tracks.
    pub voices: usize,
    /// Of those, paused ones, and ones on a paused track.
    pub paused_voices: usize,
    /// Spatial tracks: `audio_source`s' and placed one-shots'.
    pub tracks: usize,
    /// Of those, paused ones.
    pub paused_tracks: usize,
    /// Placed one-shots never started because the listener was past their
    /// `max` distance.
    pub skipped_oneshots: u64,
    /// One-shots stopped early to make room under
    /// [`MAX_ONESHOTS_PER_SOUND`].
    pub stolen_oneshots: u64,
}

impl MixCounts {
    /// Voices the mixer runs.
    pub fn live_voices(&self) -> usize {
        self.voices.saturating_sub(self.paused_voices)
    }

    /// Spatial tracks the mixer runs.
    pub fn live_tracks(&self) -> usize {
        self.tracks.saturating_sub(self.paused_tracks)
    }
}

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
    /// Sounds waiting to be decoded a frame's budget at a time (#124).
    decodes: DecodeQueue,
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
    /// Placed one-shots that are playing, oldest first
    oneshots: Vec<OneShot>,
    /// Where the listener was last put and which way it faces
    listener_position: Option<Vec3>,
    listener_orientation: glam::Quat,
    skipped_oneshots: u64,
    stolen_oneshots: u64,
    /// Mix as before #138 (the web's `?mix=old`): every one-shot on a kira
    /// spatial track, none skipped or stolen
    old_mix: bool,
}

/// A placed one-shot that is playing, and the sound it plays.
struct OneShot {
    sound: String,
    handle: StaticSoundHandle,
    /// Its own kira spatial track (`?mix=old`); without one it's a panned
    /// voice on its bus that [`AudioEngine::pan_oneshots`] steers
    track: Option<SpatialTrackHandle>,
    position: Vec3,
    falloff: Falloff,
    volume: f64,
    /// Stopped to make room. The handle says `Playing` until the audio
    /// thread reads the stop, which may be a buffer away.
    stolen: bool,
}

impl OneShot {
    /// Heard, or about to be: not stolen, stopping or stopped.
    fn is_sounding(&self) -> bool {
        use kira::sound::PlaybackState::*;
        !self.stolen && !matches!(self.handle.state(), Stopping | Stopped)
    }
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
            decodes: DecodeQueue::default(),
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
            oneshots: Vec::new(),
            listener_position: None,
            listener_orientation: glam::Quat::IDENTITY,
            skipped_oneshots: 0,
            stolen_oneshots: 0,
            old_mix: false,
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

    /// Queue a sound file to be decoded over the next frames by
    /// [`decode_queued`](Self::decode_queued), instead of now: a song takes
    /// 100 to 241 ms to decode in the browser. The file is read now. A sound
    /// already cached is left alone, and one already queued keeps its place
    /// unless `need` is sooner.
    pub fn queue_sound(&mut self, name: &str, path: &Path, need: Need) -> Result<()> {
        if self.sound_cache.contains_key(name) {
            return Ok(());
        }
        if self.decodes.contains(name) {
            self.decodes.promote(name, need);
            return Ok(());
        }
        let bytes = vfs::read(path).map_err(|e| {
            flint_core::FlintError::AudioError(format!("Failed to read '{}': {e}", path.display()))
        })?;
        self.decodes.push(name, bytes, need);
        Ok(())
    }

    /// Decode queued sounds for up to `budget`, soonest needed first, and
    /// cache the ones that finish. Call once a frame; it costs nothing when
    /// the queue is empty. Returns how many are still queued.
    pub fn decode_queued(&mut self, budget: Duration) -> usize {
        if self.decodes.is_empty() {
            return 0;
        }
        for decoded in self.decodes.run(budget) {
            self.finish_decode(decoded);
        }
        if self.decodes.is_empty() {
            tracing::info!(
                "Decoded {} queued sounds in {:.0} ms of frame time",
                self.decodes.decoded,
                self.decodes.spent.as_secs_f64() * 1000.0
            );
        }
        self.decodes.len()
    }

    /// Whether a sound is waiting in the decode queue.
    pub fn is_queued(&self, name: &str) -> bool {
        self.decodes.contains(name)
    }

    /// How many sounds are waiting in the decode queue.
    pub fn queued_count(&self) -> usize {
        self.decodes.len()
    }

    fn finish_decode(&mut self, decoded: crate::decode::Decoded) {
        let name = decoded.name;
        match decoded.result {
            Ok(data) => {
                tracing::info!(
                    "Decoded {name} ({} KB) in {:.1} ms over {} frame(s), longest step {:.1} ms",
                    decoded.size / 1024,
                    decoded.spent.as_secs_f64() * 1000.0,
                    decoded.runs,
                    decoded.longest.as_secs_f64() * 1000.0
                );
                self.sound_cache.insert(name, data);
            }
            Err(e) => tracing::warn!("Failed to decode '{name}': {e}"),
        }
    }

    /// Make sure a sound about to play is decoded: a small queued one is
    /// decoded now, a big one jumps the queue and this play is skipped.
    fn ensure_decoded(&mut self, name: &str) -> Result<()> {
        if self.sound_cache.contains_key(name) {
            return Ok(());
        }
        match self.decodes.size_of(name) {
            Some(size) if size <= DECODE_NOW_BYTES => {
                if let Some(decoded) = self.decodes.decode_now(name) {
                    self.finish_decode(decoded);
                }
                Ok(())
            }
            Some(_) => {
                self.decodes.promote(name, Need::Asked);
                Err(flint_core::FlintError::AudioError(format!(
                    "'{name}' skipped: still decoding, moved to the front of the queue"
                )))
            }
            None => Err(flint_core::FlintError::AudioError(format!(
                "Sound not cached: {name}"
            ))),
        }
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
        self.listener_position = Some(position);
        Ok(())
    }

    /// Where the listener was last put; `None` before it exists.
    pub fn listener_position(&self) -> Option<Vec3> {
        self.listener_position
    }

    /// The listener's ears, for working out a placed sound's gains
    /// ([`crate::pan`]). Before there is a listener, where `AudioSystem`
    /// first puts it.
    pub fn ears(&self) -> Ears {
        let p = self.listener_position.unwrap_or(Vec3::new(0.0, 1.0, 0.0));
        Ears {
            position: to_glam_vec3(p),
            orientation: self.listener_orientation,
        }
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
        self.listener_position = Some(position);
        self.listener_orientation = orientation;

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
        self.create_spatial_track_with(position, (min_distance, max_distance), DEFAULT_ROLLOFF, bus)
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
        self.play_panned(sound_name, volume, 0.0, pitch, looping, bus)
    }

    /// Play a cached sound on a mixer bus, panned (kira's `Panning`: -1
    /// left, 1 right, at constant power). A placed sound plays this way
    /// with its gains from [`crate::pan`], steered every frame.
    pub fn play_panned(
        &mut self,
        sound_name: &str,
        volume: f64,
        pan: f32,
        pitch: f64,
        looping: bool,
        bus: Bus,
    ) -> Result<StaticSoundHandle> {
        if self.manager.is_none() {
            return Err(flint_core::FlintError::AudioError("No audio device".into()));
        }
        self.ensure_decoded(sound_name)?;

        let sound_data = self
            .sound_cache
            .get(sound_name)
            .ok_or_else(|| {
                flint_core::FlintError::AudioError(format!("Sound not cached: {sound_name}"))
            })?
            .clone();

        let mut data = sound_data
            .volume(amplitude_to_db(volume * self.master_volume))
            .playback_rate(kira::PlaybackRate(pitch))
            .panning(kira::Panning(pan));

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

    /// Play a one-shot sound at a 3D position. Its handle is kept until the
    /// sound finishes playing.
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
        self.ensure_decoded(sound_name)?;
        let falloff = Falloff {
            min: distances.0,
            max: distances.1,
            rolloff,
        };
        let (track, handle) = if self.old_mix {
            let mut track = self.create_spatial_track_with(position, distances, rolloff, bus)?;
            let handle =
                self.play_on_spatial_track(sound_name, &mut track, volume, pitch, false)?;
            (Some(track), handle)
        } else {
            // Past `max` it would be mixed in silence, at the full price
            // of a voice.
            if self.is_out_of_earshot(position, distances.1) {
                self.skipped_oneshots += 1;
                return Ok(());
            }
            if self.manager.is_none() {
                return Err(flint_core::FlintError::AudioError("No audio device".into()));
            }
            let (gain, pan) = volume_and_pan(self.ears().gains(to_glam_vec3(position), &falloff));
            let handle =
                self.play_panned(sound_name, volume * gain as f64, pan, pitch, false, bus)?;
            self.steal_beyond_cap(sound_name);
            (None, handle)
        };
        self.oneshots.push(OneShot {
            sound: sound_name.to_string(),
            handle,
            track,
            position,
            falloff,
            volume,
            stolen: false,
        });
        Ok(())
    }

    /// Steer placed one-shots to where the listener is now. Call once a
    /// frame, after [`update_listener`](Self::update_listener).
    pub fn pan_oneshots(&mut self) {
        let ears = self.ears();
        for o in self.oneshots.iter_mut().filter(|o| o.track.is_none()) {
            let (gain, pan) = volume_and_pan(ears.gains(to_glam_vec3(o.position), &o.falloff));
            o.handle
                .set_volume(amplitude_to_db(o.volume * gain as f64), FRAME_TWEEN);
            o.handle.set_panning(kira::Panning(pan), FRAME_TWEEN);
        }
    }

    /// Mix as before #138, for comparing (the web's `?mix=old`): placed
    /// one-shots on kira spatial tracks, none skipped or stolen. Sounds
    /// already playing keep the way they started.
    pub fn set_old_mix(&mut self, old: bool) {
        self.old_mix = old;
    }

    /// Whether [`set_old_mix`](Self::set_old_mix) is on.
    pub fn old_mix(&self) -> bool {
        self.old_mix
    }

    /// Whether a sound at `position` that falls silent at `max_distance`
    /// is past it from the listener. Never before there is one.
    fn is_out_of_earshot(&self, position: Vec3, max_distance: f32) -> bool {
        self.listener_position
            .is_some_and(|l| (l - position).length() > max_distance)
    }

    /// Fade out the oldest one-shots of `sound` so one more fits under
    /// [`MAX_ONESHOTS_PER_SOUND`]. Ones already stopping don't count.
    fn steal_beyond_cap(&mut self, sound: &str) {
        let mut playing: Vec<&mut OneShot> = self
            .oneshots
            .iter_mut()
            .filter(|o| o.sound == sound && o.is_sounding())
            .collect();
        let excess = (playing.len() + 1).saturating_sub(MAX_ONESHOTS_PER_SOUND);
        for oldest in playing.iter_mut().take(excess) {
            oldest.handle.stop(Tween {
                duration: STEAL_FADE,
                ..Default::default()
            });
            oldest.stolen = true;
            self.stolen_oneshots += 1;
        }
    }

    /// Remove finished one-shot spatial sounds. Call once per frame.
    pub fn cleanup_finished_oneshots(&mut self) {
        self.oneshots
            .retain(|o| o.handle.state() != kira::sound::PlaybackState::Stopped);
    }

    /// The buses' and one-shots' share of [`MixCounts`]: every voice and
    /// spatial track kira holds. Which of the `audio_source`s' are paused
    /// is [`AudioSync::add_mix_counts`](crate::sync::AudioSync::add_mix_counts)'s
    /// to say. `None` without a device.
    pub fn mix_counts(&self) -> Option<MixCounts> {
        self.manager.as_ref()?;
        let mut counts = MixCounts {
            skipped_oneshots: self.skipped_oneshots,
            stolen_oneshots: self.stolen_oneshots,
            ..Default::default()
        };
        for bus in [&self.sfx_bus, &self.music_bus].into_iter().flatten() {
            counts.voices += bus.num_sounds();
            counts.tracks += bus.num_sub_tracks();
        }
        counts.voices += self
            .oneshots
            .iter()
            .filter_map(|o| o.track.as_ref())
            .map(|t| t.num_sounds())
            .sum::<usize>();
        Some(counts)
    }

    /// Stop and drop every placed one-shot (for scene transitions). One on
    /// its own track stopped with it; a panned one needs telling.
    pub fn clear_oneshots(&mut self) {
        for o in &mut self.oneshots {
            o.handle.stop(FRAME_TWEEN);
        }
        self.oneshots.clear();
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
    fn queued_sounds_decode_over_frames_and_a_small_one_asked_for_decodes_now() {
        use crate::decode::tests::wav;
        let mut files = vfs::MemorySource::new();
        files.insert("audio/sea.wav", wav(200_000));
        files.insert("audio/song.wav", wav(400_000));
        files.insert("audio/click.wav", wav(1_000));
        vfs::scoped(Arc::new(files), || {
            let mut engine = AudioEngine::silent();
            for (name, need) in [
                ("audio/song.wav", Need::Later),
                ("audio/click.wav", Need::Later),
                ("audio/sea.wav", Need::Audible),
            ] {
                engine.queue_sound(name, Path::new(name), need).unwrap();
            }
            assert!(engine
                .queue_sound("x", Path::new("audio/none.wav"), Need::Later)
                .is_err());
            assert_eq!(engine.queued_count(), 3);
            assert!(!engine.has_sound("audio/sea.wav"));

            // Asked for before its turn: a small sound decodes on the spot
            // (the play itself fails here only for want of a device)...
            let at = Vec3::new(0.0, 0.0, 0.0);
            let play = |e: &mut AudioEngine, name| {
                e.play_oneshot_at(name, at, 1.0, 1.0, DEFAULT_DISTANCES, 1.0, Bus::Sfx)
                    .unwrap_err()
                    .to_string()
            };
            assert!(play(&mut engine, "audio/click.wav").contains("No audio device"));
            assert!(engine.has_sound("audio/click.wav"));
            // ...and a big one is skipped, saying why.
            assert!(play(&mut engine, "audio/song.wav").contains("still decoding"));
            assert!(engine.is_queued("audio/song.wav"));

            // The song was asked for, so it now goes before the sea.
            let mut frames = 0;
            while !engine.has_sound("audio/song.wav") {
                engine.decode_queued(Duration::from_micros(500));
                frames += 1;
            }
            assert!(engine.is_queued("audio/sea.wav") && frames > 1);
            while engine.decode_queued(Duration::from_millis(1)) > 0 {}
            assert!(engine.has_sound("audio/sea.wav"));
            assert_eq!(engine.sound_cache["audio/sea.wav"].frames.len(), 200_000);
        });
    }

    #[test]
    fn oneshots_out_of_earshot_are_skipped_and_a_fifth_steals_the_oldest() {
        let mut engine = AudioEngine::new();
        if !engine.is_available() {
            eprintln!("no audio device: one-shot cap test skipped");
            return;
        }
        let tone: Vec<f32> = (0..44_100).map(|i| (i as f32 * 0.05).sin() * 0.1).collect();
        engine
            .load_sound_from_samples("creak", 44_100, &[&tone])
            .unwrap();
        engine
            .load_sound_from_samples("drip", 44_100, &[&tone])
            .unwrap();
        engine.create_listener(Vec3::new(0.0, 0.0, 0.0)).unwrap();
        let play = |e: &mut AudioEngine, name, x| {
            e.play_oneshot_at(
                name,
                Vec3::new(x, 0.0, 0.0),
                1.0,
                1.0,
                (1.0, 25.0),
                2.0,
                Bus::Sfx,
            )
            .unwrap()
        };

        play(&mut engine, "creak", 30.0);
        assert_eq!(engine.oneshots.len(), 0, "past max: never started");
        for _ in 0..6 {
            play(&mut engine, "creak", 2.0);
        }
        play(&mut engine, "drip", 2.0);
        let sounding = |e: &AudioEngine, name: &str| {
            e.oneshots
                .iter()
                .filter(|o| o.sound == name && o.is_sounding())
                .count()
        };
        assert_eq!(sounding(&engine, "creak"), MAX_ONESHOTS_PER_SOUND);
        assert_eq!(sounding(&engine, "drip"), 1, "the cap is per sound");
        let counts = engine.mix_counts().unwrap();
        assert_eq!((counts.skipped_oneshots, counts.stolen_oneshots), (1, 2));
        // The two stolen creaks still hold their tracks while they fade.
        assert_eq!(engine.oneshots.len(), 7);
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
