//! Audio engine wrapping Kira's AudioManager
//!
//! Handles sound loading, listener management, and spatial track creation.
//! Degrades gracefully when no audio device is available.
//!
//! The mix runs in one of two places (#136). On the desktop, and in the
//! browser's fallback, kira runs here on its own backend ([`Mixer::Local`]).
//! In the browser it normally runs in an `AudioWorklet`, off the main
//! thread: every call below then becomes a command to that mixer, and each
//! sound crosses to it once, as samples ([`Mixer::Remote`],
//! `crate::remote`). Handles ([`SoundHandle`], [`SpatialTrack`]) hide which.

use crate::decode::{DecodeQueue, Need};
pub use crate::handle::{SoundHandle, SpatialTrack};
use crate::remote::{Command, Link, RemoteTrack, RemoteVoice, SoundLoad, Target};
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

/// The kira backend a local mix goes through: cpal's default device on the
/// desktop, and in the browser (when the worklet can't run) a WebAudio
/// stream on the main thread that measures its own steadiness (`crate::web`).
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

    fn code(self) -> u8 {
        match self {
            Bus::Sfx => crate::remote::protocol::BUS_SFX,
            Bus::Music => crate::remote::protocol::BUS_MUSIC,
        }
    }
}

/// Where the browser mixes (#136). The worklet is the default; the main
/// thread is the fallback when it can't start, and `?mixer=main` asks for it.
#[cfg(target_arch = "wasm32")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WebMixer {
    #[default]
    Worklet,
    MainThread,
}

/// kira, here: its manager, the master filter and the two buses.
struct Local {
    manager: AudioManager<AudioBackend>,
    filter: FilterHandle,
    /// Music mixer bus (sub-track of main)
    music_bus: Option<TrackHandle>,
    /// SFX mixer bus (sub-track of main); all spatial tracks hang off this
    /// or the music bus rather than the main track directly
    sfx_bus: Option<TrackHandle>,
    listener: Option<kira::listener::ListenerHandle>,
    /// Keeps spatial track + sound handles alive until one-shot sounds finish
    oneshot_tracks: Vec<(SpatialTrackHandle, StaticSoundHandle)>,
}

impl Local {
    fn bus(&mut self, bus: Bus) -> Result<&mut TrackHandle> {
        match bus {
            Bus::Music => &mut self.music_bus,
            Bus::Sfx => &mut self.sfx_bus,
        }
        .as_mut()
        .ok_or_else(|| flint_core::FlintError::AudioError(format!("Bus {bus:?} unavailable")))
    }
}

/// kira, somewhere else (the browser's audio thread), reached through a
/// [`Link`].
struct Remote {
    link: Link,
    /// Every sound sent, by name.
    sounds: HashMap<String, u32>,
    listener: bool,
    /// The worklet behind the link, while it starts and once it runs.
    #[cfg(target_arch = "wasm32")]
    stream: Option<crate::worklet::WorkletStream>,
}

impl Remote {
    /// Send a sound, under the id it already has if it was sent before.
    fn send_sound(&mut self, name: &str, data: &StaticSoundData) {
        let id = match self.sounds.get(name) {
            Some(&id) => id,
            None => {
                let id = self.link.id();
                self.sounds.insert(name.to_string(), id);
                id
            }
        };
        self.link
            .load(SoundLoad::new(id, data.sample_rate, &data.frames));
    }

    fn sound(&self, name: &str) -> Result<u32> {
        self.sounds
            .get(name)
            .copied()
            .ok_or_else(|| flint_core::FlintError::AudioError(format!("Sound not cached: {name}")))
    }

    /// Whether sounds still need a copy here: until the worklet is known to
    /// run, the main-thread fallback may need them.
    fn keeps_cache(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        if let Some(s) = &self.stream {
            return !s.running();
        }
        false
    }
}

enum Mixer {
    Silent,
    Local(Box<Local>),
    Remote(Remote),
}

/// Wraps Kira's AudioManager with sound caching and listener management
pub struct AudioEngine {
    mixer: Mixer,
    /// Decoded sounds. A remote mix gets each one as it is cached, and this
    /// copy is let go once the mixer is known to be running.
    sound_cache: HashMap<String, StaticSoundData>,
    /// Sounds waiting to be decoded a frame's budget at a time (#124).
    decodes: DecodeQueue,
    master_volume: f64,
    /// The cutoff and bus volumes last asked for, so a mixer started
    /// later (the web's click) opens where the game already is.
    filter_cutoff: f64,
    bus_volumes: [f64; 2],
    /// Both buses quiet while the page holds the game (FlintKart#27), whatever
    /// `bus_volumes` asks for.
    held: bool,
    /// The mix moved after it started (the worklet failed and the main
    /// thread took over), so everything that was playing is gone.
    restarted: bool,
    /// The browser stream's steadiness, shared with whatever mixes.
    #[cfg(target_arch = "wasm32")]
    meter: std::sync::Arc<std::sync::Mutex<crate::meter::BufferMeter>>,
    /// Frames per WebAudio buffer (see `web::WebAudioSettings`), and per
    /// metered buffer in the worklet.
    #[cfg(target_arch = "wasm32")]
    buffer_frames: u32,
    #[cfg(target_arch = "wasm32")]
    web_mixer: WebMixer,
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn tween_ms(ms: u64) -> Tween {
    Tween {
        duration: Duration::from_millis(ms),
        ..Default::default()
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
        // The worklet's wasm is fetched now, so Begin doesn't wait for it.
        #[cfg(target_arch = "wasm32")]
        crate::worklet::preload();
        Self {
            mixer: Mixer::Silent,
            sound_cache: HashMap::new(),
            decodes: DecodeQueue::default(),
            master_volume: 1.0,
            filter_cutoff: 20000.0,
            bus_volumes: [1.0, 1.0],
            held: false,
            restarted: false,
            #[cfg(target_arch = "wasm32")]
            meter: Default::default(),
            #[cfg(target_arch = "wasm32")]
            buffer_frames: crate::web::WebAudioSettings::default().buffer_frames,
            #[cfg(target_arch = "wasm32")]
            web_mixer: crate::worklet::requested(),
        }
    }

    /// Open the audio device, if it isn't open, and say whether there is
    /// sound. In the browser, call it from the click (or key) handler that
    /// begins the game: it opens the worklet's `AudioContext` inside the
    /// gesture, and the worklet's mixer starts a moment later, with the
    /// calls made meanwhile waiting for it. Failing leaves the engine
    /// silent, as before, and a later call tries again.
    pub fn start(&mut self) -> bool {
        if !matches!(self.mixer, Mixer::Silent) {
            return true;
        }
        #[cfg(target_arch = "wasm32")]
        if self.web_mixer == WebMixer::Worklet {
            if let Some((stream, link)) =
                crate::worklet::WorkletStream::open(self.meter.clone(), self.buffer_frames)
            {
                self.start_remote(link);
                if let Mixer::Remote(r) = &mut self.mixer {
                    r.stream = Some(stream);
                }
                return true;
            }
            tracing::warn!("[flint-audio] no AudioWorklet here: mixing on the main thread");
        }
        self.start_local()
    }

    /// Mix with kira here, on [`AudioBackend`].
    fn start_local(&mut self) -> bool {
        // Main track with a low-pass filter, at the cutoff last asked for
        // (fully open at 20 kHz by default)
        #[allow(unused_mut)]
        let mut settings = kira::AudioManagerSettings::<AudioBackend>::default();
        #[cfg(target_arch = "wasm32")]
        {
            settings.backend_settings.meter = self.meter.clone();
            settings.backend_settings.buffer_frames = self.buffer_frames;
        }
        let filter = settings.main_track_builder.add_effect(
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
        let music_bus = manager
            .add_sub_track(TrackBuilder::new())
            .map_err(|e| tracing::warn!("Failed to create music bus: {e}"))
            .ok();
        let sfx_bus = manager
            .add_sub_track(TrackBuilder::new())
            .map_err(|e| tracing::warn!("Failed to create sfx bus: {e}"))
            .ok();
        self.mixer = Mixer::Local(Box::new(Local {
            manager,
            filter,
            music_bus,
            sfx_bus,
            listener: None,
            oneshot_tracks: Vec::new(),
        }));
        self.apply_bus_volumes();
        true
    }

    /// Mix somewhere else: every call from now on is written to `link`, and
    /// the sounds cached so far are sent along it. The browser's worklet is
    /// one end; a test can take the outbox itself.
    pub fn start_remote(&mut self, link: Link) {
        let mut remote = Remote {
            link,
            sounds: HashMap::new(),
            listener: false,
            #[cfg(target_arch = "wasm32")]
            stream: None,
        };
        let mut names: Vec<&String> = self.sound_cache.keys().collect();
        names.sort();
        for name in names {
            remote.send_sound(name, &self.sound_cache[name]);
        }
        remote.link.send(Command::FilterCutoff {
            hz: self.filter_cutoff as f32,
            tween: 0.0,
        });
        self.mixer = Mixer::Remote(remote);
        self.apply_bus_volumes();
        self.let_go_of_sent_sounds();
    }

    fn apply_bus_volumes(&mut self) {
        for bus in [Bus::Music, Bus::Sfx] {
            if self.held || self.bus_volumes[bus as usize] != 1.0 {
                self.send_bus_volume(bus, 0.016);
            }
        }
    }

    /// Drop the main thread's copy of sounds the remote mixer holds.
    fn let_go_of_sent_sounds(&mut self) {
        if let Mixer::Remote(r) = &self.mixer {
            if !r.keeps_cache() && !self.sound_cache.is_empty() {
                self.sound_cache.clear();
            }
        }
    }

    /// Once a frame: follow the mixer. In the browser this notices the
    /// worklet running (and lets go of the main thread's copies of the
    /// sounds it holds) or failing, in which case the main thread takes
    /// over and [`take_restarted`](Self::take_restarted) says so.
    pub fn poll(&mut self) {
        #[cfg(target_arch = "wasm32")]
        {
            let failed = match &self.mixer {
                Mixer::Remote(Remote {
                    stream: Some(s), ..
                }) => s.failed(),
                _ => false,
            };
            if failed {
                self.mixer = Mixer::Silent;
                self.web_mixer = WebMixer::MainThread;
                if self.start_local() {
                    self.restarted = true;
                }
                return;
            }
        }
        self.let_go_of_sent_sounds();
    }

    /// Whether the mix moved since the last call, losing everything that
    /// was playing: the caller starts its sounds again.
    pub fn take_restarted(&mut self) -> bool {
        std::mem::take(&mut self.restarted)
    }

    /// Frames per WebAudio buffer for the next [`start`](Self::start).
    /// Two buffers are in flight, so one buffer is how long a frame may
    /// hold the main thread before sound drops (2048 at 44.1 kHz is 46 ms).
    /// The worklet meters its steadiness over buffers this long too.
    #[cfg(target_arch = "wasm32")]
    pub fn set_buffer_frames(&mut self, frames: u32) {
        self.buffer_frames = frames;
    }

    /// Where the browser mixes, for the next [`start`](Self::start).
    #[cfg(target_arch = "wasm32")]
    pub fn set_web_mixer(&mut self, mixer: WebMixer) {
        self.web_mixer = mixer;
    }

    /// Where the sound is mixed: "worklet" (starting or running),
    /// "main thread", or "silent".
    pub fn mixer_name(&self) -> &'static str {
        match &self.mixer {
            Mixer::Silent => "silent",
            Mixer::Local(_) => "main thread",
            Mixer::Remote(_) => "worklet",
        }
    }

    /// How steadily the browser's audio stream has been fed since it
    /// opened: buffers, late ones, slack and mix time. `None` on the
    /// desktop, and before [`start`](Self::start).
    pub fn buffer_stats(&self) -> Option<crate::meter::BufferStats> {
        #[cfg(target_arch = "wasm32")]
        {
            if !matches!(self.mixer, Mixer::Silent) {
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
        self.send_bus_volume(bus, 0.016);
    }

    /// Hold the mix (FlintKart#27): both buses fade out and stay out, under any
    /// volume set meanwhile, and come back to it when let go. Sounds keep
    /// their places, so a loop resumes where the game left it.
    pub fn set_held(&mut self, held: bool) {
        if held == self.held {
            return;
        }
        self.held = held;
        for bus in [Bus::Music, Bus::Sfx] {
            self.send_bus_volume(bus, if held { 0.12 } else { 0.25 });
        }
    }

    /// Whether [`set_held`](Self::set_held) holds the mix.
    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Give the mixer a bus's volume: the one asked for, or none while held.
    fn send_bus_volume(&mut self, bus: Bus, tween: f64) {
        let volume = if self.held {
            0.0
        } else {
            self.bus_volumes[bus as usize]
        };
        let db = amplitude_to_db(volume);
        match &mut self.mixer {
            Mixer::Silent => {}
            Mixer::Local(l) => {
                if let Ok(handle) = l.bus(bus) {
                    handle.set_volume(db, tween_ms((tween * 1000.0) as u64));
                }
            }
            Mixer::Remote(r) => r.link.send(Command::BusVolume {
                bus: bus.code(),
                db: db.0,
                tween: tween as f32,
            }),
        }
    }

    /// Whether audio is actually available
    pub fn is_available(&self) -> bool {
        !matches!(self.mixer, Mixer::Silent)
    }

    /// Mutable access to the underlying kira manager, for hosts that start
    /// additional clocked content (the music suite) on the same device stream.
    /// Handles created on it must drop before this engine (ADR 0017). `None`
    /// while silent, and when the mix runs in the browser's worklet.
    pub fn manager_mut(&mut self) -> Option<&mut AudioManager<AudioBackend>> {
        match &mut self.mixer {
            Mixer::Local(l) => Some(&mut l.manager),
            _ => None,
        }
    }

    /// Load a sound file into the cache
    pub fn load_sound(&mut self, name: &str, path: &Path) -> Result<()> {
        if self.has_sound(name) {
            return Ok(());
        }

        let sound_data = static_sound_from_file(path).map_err(|e| {
            flint_core::FlintError::AudioError(format!(
                "Failed to load '{}': {}",
                path.display(),
                e
            ))
        })?;

        self.cache_sound(name, sound_data);
        Ok(())
    }

    /// Keep a decoded sound: in the cache, and with the remote mixer.
    fn cache_sound(&mut self, name: &str, data: StaticSoundData) {
        if let Mixer::Remote(r) = &mut self.mixer {
            r.send_sound(name, &data);
            if !r.keeps_cache() {
                return;
            }
        }
        self.sound_cache.insert(name.to_string(), data);
    }

    /// Queue a sound file to be decoded over the next frames by
    /// [`decode_queued`](Self::decode_queued), instead of now: a song takes
    /// 100 to 241 ms to decode in the browser. The file is read now. A sound
    /// already cached is left alone, and one already queued keeps its place
    /// unless `need` is sooner.
    pub fn queue_sound(&mut self, name: &str, path: &Path, need: Need) -> Result<()> {
        if self.has_sound(name) {
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
                self.cache_sound(&name, data);
            }
            Err(e) => tracing::warn!("Failed to decode '{name}': {e}"),
        }
    }

    /// Make sure a sound about to play is decoded: a small queued one is
    /// decoded now, a big one jumps the queue and this play is skipped.
    fn ensure_decoded(&mut self, name: &str) -> Result<()> {
        if self.has_sound(name) {
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
        self.cache_sound(name, data);
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
        self.cache_sound(
            name,
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
        let pos = to_glam_vec3(position);
        let orientation = glam::Quat::IDENTITY;
        match &mut self.mixer {
            Mixer::Silent => {}
            Mixer::Local(l) => {
                let handle = l.manager.add_listener(pos, orientation).map_err(|e| {
                    flint_core::FlintError::AudioError(format!("Failed to create listener: {e}"))
                })?;
                l.listener = Some(handle);
            }
            Mixer::Remote(r) => {
                r.link.send(Command::Listener {
                    pos: pos.to_array(),
                    quat: orientation.to_array(),
                });
                r.listener = true;
            }
        }
        Ok(())
    }

    /// Whether the listener has been created
    pub fn has_listener(&self) -> bool {
        match &self.mixer {
            Mixer::Silent => false,
            Mixer::Local(l) => l.listener.is_some(),
            Mixer::Remote(r) => r.listener,
        }
    }

    /// Update listener position and orientation from yaw/pitch angles
    pub fn update_listener(&mut self, position: Vec3, yaw: f32, pitch: f32) {
        let pos = to_glam_vec3(position);
        let orientation = glam::Quat::from_euler(glam::EulerRot::YXZ, -yaw, -pitch, 0.0);
        match &mut self.mixer {
            Mixer::Local(l) => {
                if let Some(listener) = &mut l.listener {
                    listener.set_position(pos, tween_ms(0));
                    listener.set_orientation(orientation, tween_ms(0));
                }
            }
            Mixer::Remote(r) if r.listener => r.link.send(Command::Listener {
                pos: pos.to_array(),
                quat: orientation.to_array(),
            }),
            _ => {}
        }
    }

    /// Create a spatial track for an entity's audio source on the given bus
    pub fn create_spatial_track(
        &mut self,
        position: Vec3,
        min_distance: f32,
        max_distance: f32,
        bus: Bus,
    ) -> Result<SpatialTrack> {
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
    ) -> Result<SpatialTrack> {
        let no_listener = || flint_core::FlintError::AudioError("No listener created".into());
        let pos = to_glam_vec3(position);
        match &mut self.mixer {
            Mixer::Silent => Err(flint_core::FlintError::AudioError("No audio device".into())),
            Mixer::Local(l) => {
                let listener_id = l.listener.as_ref().ok_or_else(no_listener)?.id();
                let builder = SpatialTrackBuilder::new()
                    .distances(SpatialTrackDistances {
                        min_distance,
                        max_distance,
                    })
                    .attenuation_function(Some(Easing::OutPowf(rolloff.max(0.01) as f64)));
                let handle = l
                    .bus(bus)?
                    .add_spatial_sub_track(listener_id, pos, builder)
                    .map_err(|e| {
                        flint_core::FlintError::AudioError(format!(
                            "Failed to create spatial track: {e}"
                        ))
                    })?;
                Ok(SpatialTrack::Local(Box::new(handle)))
            }
            Mixer::Remote(r) => {
                if !r.listener {
                    return Err(no_listener());
                }
                let id = r.link.id();
                r.link.send(Command::TrackNew {
                    track: id,
                    pos: pos.to_array(),
                    min: min_distance,
                    max: max_distance,
                    rolloff,
                    bus: bus.code(),
                });
                Ok(SpatialTrack::Remote(RemoteTrack {
                    id,
                    link: r.link.clone(),
                }))
            }
        }
    }

    /// A cached sound with its volume, rate and loop set for one play.
    fn sound_data(
        &self,
        sound_name: &str,
        volume: f64,
        pitch: f64,
        looping: bool,
    ) -> Result<StaticSoundData> {
        let mut data = self
            .sound_cache
            .get(sound_name)
            .ok_or_else(|| {
                flint_core::FlintError::AudioError(format!("Sound not cached: {sound_name}"))
            })?
            .volume(amplitude_to_db(volume * self.master_volume))
            .playback_rate(kira::PlaybackRate(pitch));
        if looping {
            data = data.loop_region(..);
        }
        Ok(data)
    }

    /// Start a voice in the remote mixer.
    fn play_remote(
        &self,
        sound_name: &str,
        target: Target,
        volume: f64,
        pitch: f64,
        looping: bool,
    ) -> Result<SoundHandle> {
        let Mixer::Remote(r) = &self.mixer else {
            return Err(flint_core::FlintError::AudioError(
                "the mix moved; this track is gone".into(),
            ));
        };
        let sound = r.sound(sound_name)?;
        let voice = r.link.id();
        r.link.send(Command::Play {
            voice,
            sound,
            target,
            db: amplitude_to_db(volume * self.master_volume).0,
            rate: pitch as f32,
            looping,
        });
        Ok(SoundHandle::Remote(RemoteVoice {
            id: voice,
            link: r.link.clone(),
        }))
    }

    /// Play a cached sound on a spatial track (looping or one-shot)
    pub fn play_on_spatial_track(
        &mut self,
        sound_name: &str,
        track: &mut SpatialTrack,
        volume: f64,
        pitch: f64,
        looping: bool,
    ) -> Result<SoundHandle> {
        let play_error = |e: kira::PlaySoundError<()>| {
            flint_core::FlintError::AudioError(format!("Failed to play '{sound_name}': {e}"))
        };
        match track {
            SpatialTrack::Local(handle) => {
                let data = self.sound_data(sound_name, volume, pitch, looping)?;
                Ok(SoundHandle::Local(handle.play(data).map_err(play_error)?))
            }
            SpatialTrack::Remote(t) => {
                self.play_remote(sound_name, Target::Track(t.id), volume, pitch, looping)
            }
        }
    }

    /// Play a cached sound on a mixer bus (non-spatial, e.g. ambient/music)
    pub fn play_non_spatial(
        &mut self,
        sound_name: &str,
        volume: f64,
        pitch: f64,
        looping: bool,
        bus: Bus,
    ) -> Result<SoundHandle> {
        if !self.is_available() {
            return Err(flint_core::FlintError::AudioError("No audio device".into()));
        }
        self.ensure_decoded(sound_name)?;
        if matches!(self.mixer, Mixer::Remote(_)) {
            return self.play_remote(sound_name, Target::Bus(bus.code()), volume, pitch, looping);
        }
        let data = self.sound_data(sound_name, volume, pitch, looping)?;
        let Mixer::Local(l) = &mut self.mixer else {
            unreachable!("checked available and not remote");
        };
        let handle = l.bus(bus)?.play(data).map_err(|e| {
            flint_core::FlintError::AudioError(format!("Failed to play '{sound_name}': {e}"))
        })?;
        Ok(SoundHandle::Local(handle))
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
        self.ensure_decoded(sound_name)?;
        if let Mixer::Remote(r) = &self.mixer {
            // The worklet keeps the track until the sound ends.
            if !r.listener {
                return Err(flint_core::FlintError::AudioError(
                    "No listener created".into(),
                ));
            }
            r.link.send(Command::OneShot {
                sound: r.sound(sound_name)?,
                pos: to_glam_vec3(position).to_array(),
                db: amplitude_to_db(volume * self.master_volume).0,
                rate: pitch as f32,
                min: distances.0,
                max: distances.1,
                rolloff,
                bus: bus.code(),
            });
            return Ok(());
        }
        let SpatialTrack::Local(track) =
            self.create_spatial_track_with(position, distances, rolloff, bus)?
        else {
            unreachable!("a local mixer makes local tracks");
        };
        let mut track = *track;
        let data = self.sound_data(sound_name, volume, pitch, false)?;
        let handle = track.play(data).map_err(|e| {
            flint_core::FlintError::AudioError(format!("Failed to play '{sound_name}': {e}"))
        })?;
        if let Mixer::Local(l) = &mut self.mixer {
            l.oneshot_tracks.push((track, handle));
        }
        Ok(())
    }

    /// Remove finished one-shot spatial sounds. Call once per frame. (A
    /// remote mixer sweeps its own.)
    pub fn cleanup_finished_oneshots(&mut self) {
        if let Mixer::Local(l) = &mut self.mixer {
            l.oneshot_tracks
                .retain(|(_track, sound)| sound.state() != kira::sound::PlaybackState::Stopped);
        }
    }

    /// Drop all one-shot spatial handles (for scene transitions).
    pub fn clear_oneshots(&mut self) {
        match &mut self.mixer {
            Mixer::Silent => {}
            Mixer::Local(l) => l.oneshot_tracks.clear(),
            Mixer::Remote(r) => r.link.send(Command::ClearOneShots),
        }
    }

    /// Check if a sound is already loaded
    pub fn has_sound(&self, name: &str) -> bool {
        self.sound_cache.contains_key(name)
            || matches!(&self.mixer, Mixer::Remote(r) if r.sounds.contains_key(name))
    }

    /// Set the low-pass filter cutoff frequency (Hz). 20000 = fully open, lower = more muffled.
    pub fn set_filter_cutoff(&mut self, hz: f32) {
        self.filter_cutoff = hz as f64;
        match &mut self.mixer {
            Mixer::Silent => {}
            Mixer::Local(l) => l.filter.set_cutoff(hz as f64, tween_ms(16)),
            Mixer::Remote(r) => r.link.send(Command::FilterCutoff { hz, tween: 0.016 }),
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
        // A hold keeps the volumes asked for, to come back to.
        engine.set_held(true);
        engine.set_bus_volume(Bus::Sfx, 0.5);
        assert!(engine.is_held());
        assert_eq!(engine.bus_volumes, [0.5, 0.25]);
        engine.set_held(false);
        assert!(!engine.is_held());
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

    /// The engine driven remotely, against the worklet's own mixer: what it
    /// writes is what the browser's audio thread would apply.
    #[test]
    fn a_remote_engine_drives_the_worklet_mixer() {
        use flint_audio_worklet::Mixer;

        fn deliver(link: &Link, mixer: &mut Mixer) {
            let out = link.take();
            for l in &out.loads {
                mixer.load_sound(l.id, l.sample_rate, l.channels, &l.samples);
            }
            mixer.apply(&out.batch);
        }
        fn loudness(mixer: &mut Mixer) -> (f32, f32) {
            let (mut l, mut r) = (0.0f32, 0.0f32);
            for _ in 0..40 {
                let out = mixer.render(128);
                l = l.max(out[..128].iter().fold(0.0, |a, s| a.max(s.abs())));
                r = r.max(out[128..].iter().fold(0.0, |a, s| a.max(s.abs())));
            }
            (l, r)
        }

        let tone: Vec<f32> = (0..44_100)
            .map(|i| (i as f32 * 0.0627).sin() * 0.5)
            .collect();
        let mut engine = AudioEngine::silent();
        // Cached while silent (behind the web's gate), sent at start.
        engine
            .load_sound_from_samples("sea", 44_100, &[&tone])
            .unwrap();
        engine.set_bus_volume(Bus::Music, 0.5);
        let link = Link::default();
        engine.start_remote(link.clone());
        assert!(engine.is_available() && engine.manager_mut().is_none());
        assert_eq!(engine.mixer_name(), "worklet");
        assert!(engine.has_sound("sea"), "known by name once sent");
        assert!(
            engine.sound_cache.is_empty(),
            "the main thread let go of it"
        );

        let mut mixer = Mixer::new(48_000).unwrap();
        engine.create_listener(Vec3::new(0.0, 0.0, 0.0)).unwrap();
        let mut music = engine
            .play_non_spatial("sea", 1.0, 1.0, true, Bus::Music)
            .unwrap();
        deliver(&link, &mut mixer);
        loudness(&mut mixer); // past the bus's 16 ms tween
        let (l, r) = loudness(&mut mixer);
        assert!(
            (l - 0.25).abs() < 0.02 && (l - r).abs() < 1e-3,
            "half volume: {l} {r}"
        );
        music.stop(Tween::default());
        drop(music);

        // A placed source to the right, then moved to the left.
        let mut track = engine
            .create_spatial_track(Vec3::new(3.0, 0.0, 0.0), 1.0, 25.0, Bus::Sfx)
            .unwrap();
        let _voice = engine
            .play_on_spatial_track("sea", &mut track, 1.0, 1.0, true)
            .unwrap();
        deliver(&link, &mut mixer);
        loudness(&mut mixer);
        let (l, r) = loudness(&mut mixer);
        assert!(r > l * 1.5, "on the right: {l} {r}");
        track.set_position(glam::Vec3::new(-3.0, 0.0, 0.0), Tween::default());
        deliver(&link, &mut mixer);
        loudness(&mut mixer);
        let (l, r) = loudness(&mut mixer);
        assert!(l > r * 1.5, "on the left: {l} {r}");

        // Dropping the track removes it, and a one-shot needs no handle.
        drop(track);
        engine
            .play_oneshot_at(
                "sea",
                Vec3::new(0.0, 0.0, -2.0),
                1.0,
                1.0,
                (1.0, 25.0),
                2.0,
                Bus::Sfx,
            )
            .unwrap();
        deliver(&link, &mut mixer);
        assert_eq!(
            mixer.counts(),
            (1, 0, 1),
            "the voice handle lives; the track went"
        );
        assert_eq!(mixer.errors, 0);
        assert!(engine
            .play_non_spatial("nope", 1.0, 1.0, false, Bus::Sfx)
            .is_err());
    }
}
