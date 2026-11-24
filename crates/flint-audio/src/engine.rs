//! Audio engine wrapping Kira's AudioManager
//!
//! Handles sound loading, listener management, and spatial track creation.
//! Degrades gracefully when no audio device is available.
//!
//! The mix runs in one of two places (#136). On the desktop, and in the
//! browser's fallback, kira runs here on its own backend (`local`). In the
//! browser it normally runs in an `AudioWorklet`, off the main thread:
//! every call below then becomes a command to that mixer, and each sound
//! crosses to it once, as samples (`remote_mix`, `crate::remote`). Both,
//! and silence before either, are a [`MixerBackend`](backend) (#730), whose
//! methods have no defaults, so a call can't exist for one and not the
//! other. Handles ([`SoundHandle`], [`SpatialTrack`]) hide which.
//!
//! Built without the `device` feature (#221) there is no local mix to start,
//! so the local mixer's builders go unused.

#![cfg_attr(
    not(any(feature = "device", target_arch = "wasm32")),
    allow(dead_code, unused_imports)
)]

use crate::clocked::ClockedMixer;
use crate::decode::{decode_all, DecodeQueue, Need};
pub use crate::handle::{SoundHandle, SpatialTrack};
use crate::pan::{volume_and_pan, Ears, Falloff};
use crate::remote::Link;
use backend::{Held, MixerBackend, On, Silent, Voice};
use flint_core::project::{AssetKind, AssetScope};
use flint_core::{vfs, Result, Vec3};
use kira::sound::static_sound::StaticSoundData;
use kira::{Easing, Tween};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use web_time::Instant;

mod backend;
mod duck;
mod keyed;
mod local;
mod remote_mix;
pub use crate::bus::{Bus, BusDecl, Buses};
pub use backend::Unhosted;
pub use keyed::KeyedPlay;

/// `StaticSoundData::from_file`, read through the vfs: the bytes come from
/// the active file source (the disk, or the web's bundle) and are decoded
/// from memory. Not through kira's loader, which keeps a Vorbis stream's
/// end padding (#632): [`decode_all`] trims it. kira leaves `from_file` out
/// of wasm32 builds anyway.
fn static_sound_from_file(path: impl AsRef<Path>) -> std::result::Result<StaticSoundData, String> {
    let bytes = vfs::read(path).map_err(|e| e.to_string())?;
    decode_all(bytes)
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

/// Where the browser mixes (#136). The worklet is the default; the main
/// thread is the fallback when it can't start, and `?mixer=main` asks for it.
#[cfg(target_arch = "wasm32")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WebMixer {
    #[default]
    Worklet,
    MainThread,
}

/// A placed one-shot that is playing, and the sound it plays.
struct OneShot {
    sound: String,
    /// The bus it plays on, which it counts as sounding on (#1218).
    bus: Bus,
    handle: SoundHandle,
    /// Its own kira spatial track (`?mix=old`, mixed here); without one
    /// it's a panned voice on its bus that [`AudioEngine::pan_oneshots`]
    /// steers
    track: Option<SpatialTrack>,
    position: Vec3,
    falloff: Falloff,
    volume: f64,
    /// When a remote one has played out: the worklet reports no one-shots
    ends_at: Option<Instant>,
    /// Stopped to make room, or by its owner. A local handle says `Playing`
    /// until the audio thread reads the stop, which may be a buffer away.
    stolen: bool,
    /// The caller's key for it (a script's handle, #699), which
    /// [`stop_keyed`](AudioEngine::stop_keyed) and
    /// [`set_keyed_volume`](AudioEngine::set_keyed_volume) find it by.
    key: Option<u64>,
    /// Plays until stopped: never skipped for being out of earshot, never
    /// stolen to make room.
    looping: bool,
    /// A fade to a new volume its owner asked for, which
    /// [`pan_oneshots`](AudioEngine::pan_oneshots) follows.
    ramp: Option<keyed::Ramp>,
}

impl OneShot {
    /// Its volume now, before distance: along its ramp, if it has one.
    fn volume_now(&mut self, now: Instant) -> f64 {
        if let Some(ramp) = self.ramp {
            let (volume, done) = ramp.at(now);
            self.volume = volume;
            if done {
                self.ramp = None;
            }
        }
        self.volume
    }

    /// Heard, or about to be: not stolen, stopping or stopped.
    fn is_sounding(&self, now: Instant) -> bool {
        use kira::sound::PlaybackState::*;
        !self.stolen
            && match self.handle.local_state() {
                Some(state) => !matches!(state, Stopping | Stopped),
                None => self.ends_at.is_none_or(|t| now < t),
            }
    }

    fn is_finished(&self, now: Instant) -> bool {
        match self.handle.local_state() {
            Some(state) => state == kira::sound::PlaybackState::Stopped,
            None => self.ends_at.is_some_and(|t| now >= t),
        }
    }
}

/// Wraps Kira's AudioManager with sound caching and listener management
pub struct AudioEngine {
    /// Where the mix runs (#730): [`Silent`] until a device opens.
    mixer: Box<dyn MixerBackend>,
    /// Decoded sounds. A remote mix gets each one as it is cached, and this
    /// copy is let go once the mixer is known to be running.
    sound_cache: HashMap<String, StaticSoundData>,
    /// Sounds waiting to be decoded a frame's budget at a time (#124).
    decodes: DecodeQueue,
    master_volume: f64,
    /// The cutoff and bus volumes last asked for, so a mixer started
    /// later (the web's click) opens where the game already is.
    filter_cutoff: f64,
    /// Every bus, by number: the built-in two and the game's (#730).
    buses: Buses,
    /// Each bus's volume as asked for, by number.
    bus_volumes: Vec<f64>,
    /// The player's own volume for each bus they set, by name (#1217): a
    /// second factor under the game's, so a fade never overwrites a
    /// player's "music at 40%". By name, so a saved setting read before
    /// the game declares its buses finds its bus when it is declared.
    user_volumes: BTreeMap<String, f64>,
    /// Each bus's low-pass cutoff as asked for, Hz, by number (#1218).
    bus_lowpass: Vec<f64>,
    /// Which buses are ducked now, by number (#1218): their volume goes to
    /// the mixer times their duck's gain.
    ducked: Vec<bool>,
    /// When each sound played on a bus with no key and no loop ends, and
    /// its bus (#1218): an extension's or a trigger's one-shot, kept only
    /// to say what is sounding where.
    tails: Vec<(Bus, Instant)>,
    /// Bus names asked for that aren't buses, said once each.
    unknown_buses: HashSet<String>,
    /// The table [`set_buses`](Self::set_buses) applied last.
    applied_buses: Option<Buses>,
    /// Every top-level bus quiet while the page holds the game
    /// (FlintKart#27), whatever `bus_volumes` asks for.
    held: bool,
    /// The mix moved after it started (the worklet failed and the main
    /// thread took over), so everything that was playing is gone.
    restarted: bool,
    /// Bumped each time a mixer is installed (#1216): clocked content
    /// started on the last one notices it is gone.
    mixer_epoch: u32,
    /// The browser stream's steadiness, shared with whatever mixes.
    #[cfg(target_arch = "wasm32")]
    meter: std::sync::Arc<std::sync::Mutex<crate::meter::BufferMeter>>,
    /// Frames per WebAudio buffer (see `web::WebAudioSettings`), and per
    /// metered buffer in the worklet.
    #[cfg(target_arch = "wasm32")]
    buffer_frames: u32,
    #[cfg(target_arch = "wasm32")]
    web_mixer: WebMixer,
    /// Placed one-shots that are playing, oldest first
    oneshots: Vec<OneShot>,
    /// Sounds on a bus the caller keeps a key for (#699), oldest first;
    /// the placed ones are in `oneshots`.
    keyed: Vec<keyed::Keyed>,
    /// Where the listener was last put and which way it faces
    listener_position: Option<Vec3>,
    listener_orientation: glam::Quat,
    skipped_oneshots: u64,
    stolen_oneshots: u64,
    /// Mix as before #138 (the web's `?mix=old`): placed one-shots on kira
    /// spatial tracks, none skipped or stolen
    old_mix: bool,
    /// Where [`clock_now`](Self::clock_now) counts from, mixing here.
    epoch: Instant,
    /// The scene's search for a sound played by a name nobody cached
    /// (#975); `None` until a host sets one.
    sound_search: Option<AssetScope>,
    /// Names that search found nothing for, since it was set.
    searched_missing: HashSet<String>,
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
        {
            crate::worklet::preload();
            // A hidden page's sound sleeps (#162).
            crate::page::listen();
        }
        let buses = Buses::new();
        Self {
            mixer: Box::new(Silent),
            sound_cache: HashMap::new(),
            decodes: DecodeQueue::default(),
            master_volume: 1.0,
            filter_cutoff: 20000.0,
            bus_volumes: buses.iter().map(|(_, d)| d.volume).collect(),
            user_volumes: BTreeMap::new(),
            bus_lowpass: buses.iter().map(|(_, d)| d.lowpass).collect(),
            ducked: vec![false; buses.len()],
            tails: Vec::new(),
            buses,
            unknown_buses: HashSet::new(),
            applied_buses: None,
            held: false,
            restarted: false,
            mixer_epoch: 0,
            #[cfg(target_arch = "wasm32")]
            meter: Default::default(),
            #[cfg(target_arch = "wasm32")]
            buffer_frames: crate::web::WebAudioSettings::default().buffer_frames,
            #[cfg(target_arch = "wasm32")]
            web_mixer: crate::worklet::requested(),
            oneshots: Vec::new(),
            keyed: Vec::new(),
            listener_position: None,
            listener_orientation: glam::Quat::IDENTITY,
            skipped_oneshots: 0,
            stolen_oneshots: 0,
            old_mix: false,
            epoch: Instant::now(),
            sound_search: None,
            searched_missing: HashSet::new(),
        }
    }

    /// Where a sound nobody cached is looked for when it is first played
    /// (#975): the scene's project and directory, searched as an
    /// `audio_source.file` is (`AssetKind::Audio`: a path from the scene or
    /// from any directory above it, then each `audio/`, then the engine's).
    /// So `play_sound("../sfx/door.ogg")` plays whatever `flint check`
    /// accepted, not only the files the scene's preload cached. The host
    /// sets it at each scene load and when files arrive (a late bundle);
    /// either forgets the names a search found nothing for.
    pub fn set_sound_search(&mut self, scope: Option<AssetScope>) {
        self.sound_search = scope;
        self.searched_missing.clear();
    }

    /// The file the project's search finds for a sound nobody cached under
    /// `name`, if a search is set and finds one. A miss is remembered until
    /// the next [`set_sound_search`](Self::set_sound_search), so a script
    /// asking for it every frame walks the folders once.
    fn search_for_sound(&mut self, name: &str) -> Option<PathBuf> {
        let scope = self.sound_search.as_ref()?;
        if self.searched_missing.contains(name) {
            return None;
        }
        match scope.resolve(AssetKind::Audio, name) {
            Ok(path) => Some(path),
            Err(_) => {
                self.searched_missing.insert(name.to_string());
                None
            }
        }
    }

    /// Open the audio device, if it isn't open, and say whether there is
    /// sound. In the browser, call it from the click (or key) handler that
    /// begins the game: it opens the worklet's `AudioContext` inside the
    /// gesture, and the worklet's mixer starts a moment later, with the
    /// calls made meanwhile waiting for it. Failing leaves the engine
    /// silent, as before, and a later call tries again.
    pub fn start(&mut self) -> bool {
        if self.mixer.is_available() {
            return true;
        }
        #[cfg(target_arch = "wasm32")]
        if self.web_mixer == WebMixer::Worklet {
            if let Some((stream, link)) =
                crate::worklet::WorkletStream::open(self.meter.clone(), self.buffer_frames)
            {
                let mut remote = remote_mix::Remote::new(link);
                remote.stream = Some(stream);
                self.install(Box::new(remote));
                return true;
            }
            tracing::warn!("[flint-audio] no AudioWorklet here: mixing on the main thread");
        }
        self.start_local()
    }

    /// Mix with kira here, on [`AudioBackend`].
    ///
    /// Built without the sound card (#221), kira's DefaultBackend is its
    /// MockBackend, which would take sounds and never play them, so this
    /// runs silent instead, exactly as on a machine with no device.
    #[cfg(not(any(feature = "device", target_arch = "wasm32")))]
    fn start_local(&mut self) -> bool {
        tracing::info!("[flint-audio] built without the `device` feature, running silent");
        false
    }

    #[cfg(any(feature = "device", target_arch = "wasm32"))]
    fn start_local(&mut self) -> bool {
        #[allow(unused_mut)]
        let mut settings = kira::AudioManagerSettings::<AudioBackend>::default();
        #[cfg(target_arch = "wasm32")]
        {
            settings.backend_settings.meter = self.meter.clone();
            settings.backend_settings.buffer_frames = self.buffer_frames;
        }
        // The main track's low-pass at the cutoff last asked for (fully
        // open at 20 kHz by default); no device, and the engine stays silent.
        let Some(local) = local::Local::open(settings, self.filter_cutoff) else {
            return false;
        };
        self.install(Box::new(local));
        true
    }

    /// Mix somewhere else: every call from now on is written to `link`, and
    /// the sounds cached so far are sent along it. The browser's worklet is
    /// one end; a test can take the outbox itself.
    pub fn start_remote(&mut self, link: Link) {
        self.install(Box::new(remote_mix::Remote::new(link)));
    }

    /// Mix on `mixer` from now on, opened where the game already is: the
    /// sounds cached so far (in name order), the cutoff, the game's buses
    /// and every volume asked for.
    fn install(&mut self, mixer: Box<dyn MixerBackend>) {
        self.mixer = mixer;
        self.mixer_epoch = self.mixer_epoch.wrapping_add(1);
        let mut names: Vec<&String> = self.sound_cache.keys().collect();
        names.sort();
        for name in names {
            self.mixer.sound_cached(name, &self.sound_cache[name]);
        }
        if self.mixer.is_remote() {
            // The worklet's mixer opens fully open; a local one opened at it.
            self.mixer.set_filter_cutoff(self.filter_cutoff as f32, 0.0);
        }
        self.make_buses(Bus::BUILT_IN.len());
        let buses: Vec<Bus> = self.buses.iter().map(|(bus, _)| bus).collect();
        for bus in buses {
            let i = bus.index();
            if self.held || self.ducked[i] || self.mixed_volume(bus) != 1.0 {
                self.send_bus_volume(bus, 0.016);
            }
            // The rest were made at theirs.
            if i < Bus::BUILT_IN.len() && self.bus_lowpass[i] < crate::bus::LOWPASS_OPEN {
                self.mixer
                    .set_bus_lowpass(bus, self.bus_lowpass[i] as f32, 0.0);
            }
        }
        self.let_go_of_sent_sounds();
    }

    /// Make the buses numbered `from` on in the mixer, each at its volume.
    fn make_buses(&mut self, from: usize) {
        for (bus, decl) in self.buses.iter().skip(from) {
            let db = amplitude_to_db(self.mixed_volume(bus));
            let lowpass = self.bus_lowpass[bus.index()] as f32;
            if let Err(e) = self.mixer.add_bus(bus, decl.parent, db, lowpass) {
                tracing::warn!("[flint-audio] bus '{}': {e}", decl.name);
            }
        }
    }

    /// Mix through these buses (#730): `sfx`, `music` and the game's, as a
    /// manifest's `[audio.buses]` declares them ([`Buses::from_manifest`]).
    /// The first table sets every bus's starting volume, low-pass and duck
    /// (#1218), the built-in two's included. A later, different one
    /// (another project's) only adds the buses it names that aren't here,
    /// each as it says: a bus already here keeps its number and what the
    /// game has given it. A new bus is made in the mixer now, or when it
    /// starts. None is removed. Applying the table applied last changes
    /// nothing, so a host can hand it over at every scene.
    pub fn set_buses(&mut self, table: &Buses) {
        if self.applied_buses.as_ref() == Some(table) {
            return;
        }
        let first = self.applied_buses.replace(table.clone()).is_none();
        let before = self.buses.len();
        let mut taken = Vec::new();
        for (_, decl) in table.iter() {
            // A parent always comes before its children in a table.
            let parent = decl
                .parent
                .and_then(|p| table.decl(p))
                .and_then(|p| self.buses.get(&p.name));
            if !first && self.buses.get(&decl.name).is_some() {
                continue;
            }
            match self.buses.declare(&decl.name, parent, decl.volume) {
                Ok(bus) if bus.index() >= self.bus_volumes.len() => {
                    self.bus_volumes.push(decl.volume);
                    self.bus_lowpass.push(decl.lowpass);
                    self.ducked.push(false);
                    self.buses.set_lowpass(bus, decl.lowpass);
                    taken.push((bus, decl));
                }
                Ok(bus) => {
                    if self.bus_volumes[bus.index()] != decl.volume {
                        self.bus_volumes[bus.index()] = decl.volume;
                        self.send_bus_volume(bus, 0.016);
                    }
                    self.buses.set_lowpass(bus, decl.lowpass);
                    self.set_bus_lowpass(bus, decl.lowpass);
                    taken.push((bus, decl));
                }
                Err(e) => tracing::warn!("[flint-audio] {e}"),
            }
        }
        // Every bus is here now: each duck's `under`, numbered as this
        // engine numbers it.
        for (bus, decl) in taken {
            let duck = decl.duck.and_then(|d| {
                let under = self.buses.get(&table.decl(d.under)?.name)?;
                Some(crate::bus::Duck { under, ..d })
            });
            if let Err(e) = self.buses.set_duck(bus, duck) {
                tracing::warn!("[flint-audio] bus '{}': {e}", decl.name);
            }
        }
        self.make_buses(before);
    }

    /// Every bus, by number.
    pub fn buses(&self) -> &Buses {
        &self.buses
    }

    /// The bus with this name: `sfx`, `music` or one the game declared.
    pub fn bus(&self, name: &str) -> Option<Bus> {
        self.buses.get(name)
    }

    /// The bus with this name, or `sfx` with a warning the first time a
    /// name isn't one (a script's or a scene's `bus`).
    pub fn bus_or_sfx(&mut self, name: &str) -> Bus {
        if let Some(bus) = self.buses.get(name) {
            return bus;
        }
        if self.unknown_buses.insert(name.to_string()) {
            let names: Vec<&str> = self.buses.names().collect();
            tracing::warn!(
                "[flint-audio] no bus '{name}' (the buses are {}): playing on sfx",
                names.join(", ")
            );
        }
        Bus::SFX
    }

    /// Drop the main thread's copy of sounds the remote mixer holds.
    fn let_go_of_sent_sounds(&mut self) {
        if !self.mixer.needs_cache() && !self.sound_cache.is_empty() {
            self.sound_cache.clear();
        }
    }

    /// Once a frame: follow the mixer. In the browser this notices the
    /// worklet running (and lets go of the main thread's copies of the
    /// sounds it holds) or failing, in which case the main thread takes
    /// over and [`take_restarted`](Self::take_restarted) says so.
    pub fn poll(&mut self) {
        if self.mixer.failed() {
            // The worklet's one-shots went with it.
            self.oneshots.clear();
            self.keyed.clear();
            self.mixer = Box::new(Silent);
            #[cfg(target_arch = "wasm32")]
            {
                self.web_mixer = WebMixer::MainThread;
            }
            if self.start_local() {
                self.restarted = true;
            }
            return;
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
        self.mixer.name()
    }

    /// How steadily the browser's audio stream has been fed since it
    /// opened: buffers, late ones, slack and mix time. `None` on the
    /// desktop, and before [`start`](Self::start).
    pub fn buffer_stats(&self) -> Option<crate::meter::BufferStats> {
        #[cfg(target_arch = "wasm32")]
        {
            if self.mixer.is_available() {
                return self.meter.lock().ok().map(|m| m.stats());
            }
        }
        None
    }

    /// Set a mixer bus volume (linear amplitude, 0.0 = mute, 1.0 = unity).
    /// Applies multiplicatively on top of per-sound volumes, so script-driven
    /// fades keep working underneath, and over its children's.
    pub fn set_bus_volume(&mut self, bus: Bus, volume: f64) {
        let Some(slot) = self.bus_volumes.get_mut(bus.index()) else {
            tracing::warn!("[flint-audio] set_bus_volume: no bus {}", bus.index());
            return;
        };
        *slot = volume;
        self.send_bus_volume(bus, 0.016);
    }

    /// [`set_bus_volume`](Self::set_bus_volume) by name (a script's
    /// `set_bus_volume("voice", 0.5)`). A name that is no bus changes
    /// nothing, is warned about the first time, and is an error naming the
    /// buses there are.
    pub fn set_bus_volume_named(&mut self, name: &str, volume: f64) -> Result<()> {
        let Some(bus) = self.buses.get(name) else {
            let names: Vec<&str> = self.buses.names().collect();
            let message = format!("no bus '{name}' (the buses are {})", names.join(", "));
            if self.unknown_buses.insert(name.to_string()) {
                tracing::warn!("[flint-audio] set_bus_volume: {message}");
            }
            return Err(flint_core::FlintError::AudioError(message));
        };
        self.set_bus_volume(bus, volume);
        Ok(())
    }

    /// A bus's volume as last asked for (not while held, which is silence).
    /// The game's alone: the player's own is [`user_bus_volume`](Self::user_bus_volume).
    pub fn bus_volume(&self, bus: Bus) -> Option<f64> {
        self.bus_volumes.get(bus.index()).copied()
    }

    /// The player's own volume for a bus (#1217), by name: a second factor
    /// the mixer multiplies with the game's [`set_bus_volume`](Self::set_bus_volume),
    /// so a script's fade to 0.5 under a player's 0.4 is heard at 0.2, and
    /// the fade never overwrites the setting. Clamped to 0..=1 (a settings
    /// slider's range; the game's own volume is where a boost belongs). A
    /// name that is no bus changes nothing and is an error naming the
    /// buses there are, as [`set_bus_volume_named`](Self::set_bus_volume_named) is.
    pub fn set_user_bus_volume(&mut self, name: &str, volume: f64) -> Result<()> {
        let Some(bus) = self.buses.get(name) else {
            let names: Vec<&str> = self.buses.names().collect();
            let message = format!("no bus '{name}' (the buses are {})", names.join(", "));
            if self.unknown_buses.insert(name.to_string()) {
                tracing::warn!("[flint-audio] set_user_bus_volume: {message}");
            }
            return Err(flint_core::FlintError::AudioError(message));
        };
        let volume = user_volume(volume);
        if self.user_volumes.get(name) == Some(&volume) {
            return Ok(());
        }
        self.user_volumes.insert(name.to_string(), volume);
        self.send_bus_volume(bus, 0.016);
        Ok(())
    }

    /// The player's own volume for the bus `name`: 1 for one they never
    /// set (and for a name that is no bus).
    pub fn user_bus_volume(&self, name: &str) -> f64 {
        self.user_volumes.get(name).copied().unwrap_or(1.0)
    }

    /// Every bus the player has set a volume for, by name.
    pub fn user_bus_volumes(&self) -> &BTreeMap<String, f64> {
        &self.user_volumes
    }

    /// Take the player's saved volumes (a host's settings file, read at
    /// start), in place of any set before. A name that is no bus yet is
    /// kept for the bus the game may declare later; each bus here moves to
    /// its new mix now, or when the mixer starts.
    pub fn load_user_bus_volumes(&mut self, volumes: impl IntoIterator<Item = (String, f64)>) {
        self.user_volumes = volumes
            .into_iter()
            .map(|(name, v)| (name, user_volume(v)))
            .collect();
        let buses: Vec<Bus> = self.buses.iter().map(|(bus, _)| bus).collect();
        for bus in buses {
            self.send_bus_volume(bus, 0.016);
        }
    }

    /// What the mixer is given for a bus: the game's volume times the
    /// player's (before a hold, which silences the top-level buses).
    fn mixed_volume(&self, bus: Bus) -> f64 {
        let game = self.bus_volumes[bus.index()];
        let user = self
            .buses
            .decl(bus)
            .and_then(|d| self.user_volumes.get(&d.name))
            .copied()
            .unwrap_or(1.0);
        game * user
    }

    /// Hold the mix (FlintKart#27): every top-level bus fades out and stays
    /// out, under any volume set meanwhile, and comes back to it when let
    /// go. Sounds keep their places, so a loop resumes where the game left
    /// it.
    pub fn set_held(&mut self, held: bool) {
        if held == self.held {
            return;
        }
        self.held = held;
        let roots: Vec<Bus> = self
            .buses
            .iter()
            .filter(|(_, d)| d.parent.is_none())
            .map(|(bus, _)| bus)
            .collect();
        for bus in roots {
            self.send_bus_volume(bus, if held { 0.12 } else { 0.25 });
        }
    }

    /// Whether [`set_held`](Self::set_held) holds the mix.
    pub fn is_held(&self) -> bool {
        self.held
    }

    /// Give the mixer a bus's volume: the one asked for, times its duck's
    /// gain while ducked (#1218), or none while held (a child bus is held
    /// through its parent).
    fn send_bus_volume(&mut self, bus: Bus, tween: f64) {
        let decl = self.buses.decl(bus);
        let top = decl.is_some_and(|d| d.parent.is_none());
        let duck = match decl.and_then(|d| d.duck) {
            Some(d) if self.ducked[bus.index()] => d.gain(),
            _ => 1.0,
        };
        let volume = if self.held && top {
            0.0
        } else {
            self.mixed_volume(bus) * duck
        };
        self.mixer
            .set_bus_volume(bus, amplitude_to_db(volume), tween);
    }

    /// Whether audio is actually available
    pub fn is_available(&self) -> bool {
        self.mixer.is_available()
    }

    /// Whether the mix runs elsewhere (the browser's worklet), which says
    /// nothing back about what it plays.
    pub fn is_remote(&self) -> bool {
        self.mixer.is_remote()
    }

    /// Start content with its own clock and stem buses on the engine's
    /// mixer (a music suite, ADR 0017): `start` gets the mixer's
    /// [`ClockedMixer`] for the call, and the handles it makes are the
    /// caller's (#1216): kira's here, the worklet's in the browser. Handles
    /// made here must drop before this engine. Not while silent (#730).
    /// When the mix moves (the worklet failing), the handles' mixer is gone:
    /// [`mixer_epoch`](Self::mixer_epoch) changes, and the content starts
    /// again on the new one.
    pub fn host<T>(
        &mut self,
        start: impl FnOnce(&mut dyn ClockedMixer) -> T,
    ) -> std::result::Result<T, Unhosted> {
        self.mixer.host().map(start)
    }

    /// Whether [`host`](Self::host) would run now, and why not.
    pub fn can_host(&mut self) -> std::result::Result<(), Unhosted> {
        self.mixer.host().map(|_| ())
    }

    /// Which mixer this is, counted: it changes when the engine starts or
    /// moves its mix, so content [`host`](Self::host)ed on the one before
    /// knows its handles went with it.
    pub fn mixer_epoch(&self) -> u32 {
        self.mixer_epoch
    }

    /// Load a sound file into the cache
    pub fn load_sound(&mut self, name: &str, path: &Path) -> Result<()> {
        if self.has_sound(name) {
            return Ok(());
        }
        flint_core::scope!("audio::decode", path);
        // A whole sound decoded on the frame (#1077)
        let _note =
            flint_core::frame_notes::begin(flint_core::frame_notes::NoteKind::AudioDecode, || {
                flint_core::project::shown(path)
            });

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

    /// Keep a decoded sound: with a mixer that holds its own copies, and in
    /// the cache while one is needed here.
    fn cache_sound(&mut self, name: &str, data: StaticSoundData) {
        self.mixer.sound_cached(name, &data);
        if self.mixer.needs_cache() {
            self.sound_cache.insert(name.to_string(), data);
        }
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
        // The frame's share of the queue (#1077): budgeted, but on the frame
        let queued = self.decodes.len();
        flint_core::scope!("audio::decode_queue", queued);
        let _note =
            flint_core::frame_notes::begin(flint_core::frame_notes::NoteKind::AudioDecode, || {
                format!("queue, {queued} waiting")
            });
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
    /// decoded now, a big one jumps the queue and this play is skipped. A
    /// sound neither cached nor queued is looked for by the project's
    /// search (#975), queued under the name as given, and then treated the
    /// same way.
    fn ensure_decoded(&mut self, name: &str) -> Result<()> {
        if self.has_sound(name) {
            return Ok(());
        }
        if !self.decodes.contains(name) {
            if let Some(path) = self.search_for_sound(name) {
                tracing::info!(
                    target: "load",
                    "Sound '{name}' was not preloaded: found by the project's search, {}",
                    flint_core::project::shown(&path)
                );
                self.queue_sound(name, &path, Need::Asked)?;
            }
        }
        match self.decodes.size_of(name) {
            Some(size) if size <= DECODE_NOW_BYTES => {
                let _note = flint_core::frame_notes::begin(
                    flint_core::frame_notes::NoteKind::AudioDecode,
                    || format!("{name}, now, to play it"),
                );
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
            None => Err(self.not_cached(name)),
        }
    }

    /// Whether a sound is cached or waiting to be decoded under `name`.
    fn knows_sound(&self, name: &str) -> bool {
        self.has_sound(name) || self.decodes.contains(name)
    }

    /// The name a sound is kept under (#629). A scene's preload keys each
    /// file by its path from the game root, `audio/whoosh.ogg`, so a name
    /// that isn't one tries `audio/<name>`, then `audio/<name>.ogg`,
    /// `.wav`, `.flac` and `.mp3`: `play_sound("whoosh")` finds it. A name
    /// cached as given always wins, so a sound loaded as "blip" stays
    /// "blip". Nothing found: the name as given, for the error to name.
    pub fn resolve_sound<'a>(&self, name: &'a str) -> std::borrow::Cow<'a, str> {
        if self.knows_sound(name) {
            return name.into();
        }
        let bare = name.strip_prefix("audio/").unwrap_or(name);
        let mut candidates = vec![format!("audio/{bare}")];
        candidates.extend(
            SOUND_EXTENSIONS
                .iter()
                .map(|ext| format!("audio/{bare}.{ext}")),
        );
        candidates
            .into_iter()
            .find(|c| self.knows_sound(c))
            .map_or(name.into(), Into::into)
    }

    /// "Sound not cached", naming the nearest sound there is, since in play
    /// the failure is otherwise only a warning.
    pub fn not_cached(&self, name: &str) -> flint_core::FlintError {
        let names = self
            .sound_cache
            .keys()
            .map(String::as_str)
            .chain(self.mixer.sound_names())
            .chain(self.decodes.names());
        let hint = match nearest_sound(name, names) {
            Some(near) => format!(" (nearest: \"{near}\")"),
            None => " (no sounds are cached)".to_string(),
        };
        flint_core::FlintError::AudioError(format!("Sound not cached: {name}{hint}"))
    }

    /// Cache an encoded sound held in memory (WAV, OGG, FLAC or MP3 bytes,
    /// e.g. from `include_bytes!`) under `name`. Replaces any sound already
    /// cached under that name; sounds already playing keep the old data.
    pub fn load_sound_from_bytes(&mut self, name: &str, bytes: Vec<u8>) -> Result<()> {
        let data = decode_all(bytes).map_err(|e| {
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
        let orientation = glam::Quat::IDENTITY;
        if self.is_available() {
            self.listener_position = Some(position);
            self.listener_orientation = orientation;
        }
        self.mixer
            .create_listener(to_glam_vec3(position), orientation)
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
        self.mixer.has_listener()
    }

    /// Update listener position and orientation from yaw/pitch angles
    pub fn update_listener(&mut self, position: Vec3, yaw: f32, pitch: f32) {
        let orientation = glam::Quat::from_euler(glam::EulerRot::YXZ, -yaw, -pitch, 0.0);
        if self.has_listener() {
            self.listener_position = Some(position);
            self.listener_orientation = orientation;
        }
        self.mixer
            .move_listener(to_glam_vec3(position), orientation);
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
        (min, max): (f32, f32),
        rolloff: f32,
        bus: Bus,
    ) -> Result<SpatialTrack> {
        self.mixer
            .create_spatial_track(to_glam_vec3(position), Falloff { min, max, rolloff }, bus)
    }

    /// One sound to start, at `volume` under the master volume, with the
    /// cache's copy if the engine keeps one: "not cached" (naming the
    /// nearest) when neither the engine nor the mixer has it. Takes the
    /// cache, not `self`, so the mixer can be borrowed beside it.
    #[allow(clippy::too_many_arguments)]
    fn voice<'a>(
        &self,
        cache: &'a HashMap<String, StaticSoundData>,
        name: &'a str,
        volume: f64,
        pan: f32,
        pitch: f64,
        looping: bool,
    ) -> Result<Voice<'a>> {
        if !self.has_sound(name) {
            return Err(self.not_cached(name));
        }
        Ok(Voice {
            name,
            data: cache.get(name),
            db: amplitude_to_db(volume * self.master_volume),
            rate: pitch,
            pan,
            looping,
        })
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
        let sound_name = &*self.resolve_sound(sound_name);
        let voice = self.voice(&self.sound_cache, sound_name, volume, 0.0, pitch, looping)?;
        self.mixer.play(voice, On::Track(track))
    }

    /// Play a cached sound on a mixer bus (non-spatial, e.g. ambient/music).
    /// One that ends by itself counts as sounding on its bus until it has
    /// (a bus ducking under that one dips, #1218); a loop played this way
    /// is the caller's to know of.
    pub fn play_non_spatial(
        &mut self,
        sound_name: &str,
        volume: f64,
        pitch: f64,
        looping: bool,
        bus: Bus,
    ) -> Result<SoundHandle> {
        self.play_panned(sound_name, volume, 0.0, pitch, looping, bus)
    }

    /// Play a cached sound on a mixer bus, panned (kira's `Panning`: -1
    /// left, 1 right, at constant power). A placed sound plays this way
    /// with its gains from [`crate::pan`], steered every frame (#138). As
    /// [`play_non_spatial`](Self::play_non_spatial), a one-shot counts as
    /// sounding on its bus until it ends.
    pub fn play_panned(
        &mut self,
        sound_name: &str,
        volume: f64,
        pan: f32,
        pitch: f64,
        looping: bool,
        bus: Bus,
    ) -> Result<SoundHandle> {
        let handle = self.start_panned(sound_name, volume, pan, pitch, looping, bus)?;
        if !looping {
            if let Some(secs) = self.sound_length(sound_name) {
                let secs = secs / pitch.abs().max(1e-3);
                self.tails
                    .push((bus, Instant::now() + Duration::from_secs_f64(secs)));
            }
        }
        Ok(handle)
    }

    /// [`play_panned`](Self::play_panned) for a caller that says itself
    /// when the sound is sounding: a keyed sound, a placed one-shot, an
    /// `audio_source` ([`AudioSync::sounding_buses`](crate::sync::AudioSync::sounding_buses)).
    pub(crate) fn start_panned(
        &mut self,
        sound_name: &str,
        volume: f64,
        pan: f32,
        pitch: f64,
        looping: bool,
        bus: Bus,
    ) -> Result<SoundHandle> {
        if !self.is_available() {
            return Err(flint_core::FlintError::AudioError("No audio device".into()));
        }
        let sound_name = &*self.resolve_sound(sound_name);
        self.ensure_decoded(sound_name)?;
        let voice = self.voice(&self.sound_cache, sound_name, volume, pan, pitch, looping)?;
        self.mixer.play(voice, On::Bus(bus))
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
            Bus::SFX,
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
        let falloff = Falloff {
            min: distances.0,
            max: distances.1,
            rolloff,
        };
        let play = KeyedPlay {
            volume,
            pitch,
            looping: false,
            bus,
        };
        self.place(sound_name, position, falloff, play, None)
    }

    /// Place a sound as a panned voice on its bus, steered each frame by
    /// [`pan_oneshots`](Self::pan_oneshots): a one-shot when `key` is
    /// `None`, else one its caller can stop or fade (#699), which never
    /// takes the old mix (`?mix=old` has no handle in the worklet).
    pub(crate) fn place(
        &mut self,
        sound_name: &str,
        position: Vec3,
        falloff: Falloff,
        play: KeyedPlay,
        key: Option<u64>,
    ) -> Result<()> {
        let sound_name = &*self.resolve_sound(sound_name);
        self.ensure_decoded(sound_name)?;
        let KeyedPlay {
            volume,
            pitch,
            looping,
            bus,
        } = play;
        if self.old_mix && key.is_none() {
            return self.play_oneshot_on_track(sound_name, position, volume, pitch, falloff, bus);
        }
        if !self.is_available() {
            return Err(flint_core::FlintError::AudioError("No audio device".into()));
        }
        // Past `max` it would be mixed in silence, at the full price of a
        // voice. A loop plays on, for when the listener comes nearer.
        if !looping && self.is_out_of_earshot(position, falloff.max) {
            self.skipped_oneshots += 1;
            return Ok(());
        }
        let (gain, pan) = volume_and_pan(self.ears().gains(to_glam_vec3(position), &falloff));
        let handle =
            self.start_panned(sound_name, volume * gain as f64, pan, pitch, looping, bus)?;
        let now = Instant::now();
        if !looping {
            self.steal_beyond_cap(sound_name, now);
        }
        let ends_at = self.remote_end(sound_name, pitch, looping, now);
        self.oneshots.push(OneShot {
            sound: sound_name.to_string(),
            bus,
            handle,
            track: None,
            position,
            falloff,
            volume,
            ends_at,
            stolen: false,
            key,
            looping,
            ramp: None,
        });
        Ok(())
    }

    /// When a sound started now in the remote mixer will have played out,
    /// which that mixer doesn't say; `None` mixing here, or for a loop.
    pub(crate) fn remote_end(
        &self,
        sound_name: &str,
        pitch: f64,
        looping: bool,
        now: Instant,
    ) -> Option<Instant> {
        self.mixer.play_ends(sound_name, pitch, looping, now)
    }

    /// A one-shot as before #138 (`?mix=old`): its own kira spatial track,
    /// here, or the worklet's, which keeps it until the sound ends.
    fn play_oneshot_on_track(
        &mut self,
        sound_name: &str,
        position: Vec3,
        volume: f64,
        pitch: f64,
        falloff: Falloff,
        bus: Bus,
    ) -> Result<()> {
        let voice = self.voice(&self.sound_cache, sound_name, volume, 0.0, pitch, false)?;
        let Some((handle, track)) =
            self.mixer
                .play_on_own_track(voice, to_glam_vec3(position), falloff, bus)?
        else {
            return Ok(());
        };
        self.oneshots.push(OneShot {
            sound: sound_name.to_string(),
            bus,
            handle,
            track: Some(track),
            position,
            falloff,
            volume,
            ends_at: None,
            stolen: false,
            key: None,
            looping: false,
            ramp: None,
        });
        Ok(())
    }

    /// Steer placed one-shots to where the listener is now. Call once a
    /// frame, after [`update_listener`](Self::update_listener).
    pub fn pan_oneshots(&mut self) {
        let ears = self.ears();
        let now = Instant::now();
        for o in self.oneshots.iter_mut().filter(|o| o.track.is_none()) {
            let (gain, pan) = volume_and_pan(ears.gains(to_glam_vec3(o.position), &o.falloff));
            let volume = o.volume_now(now);
            o.handle
                .set_volume(amplitude_to_db(volume * gain as f64), FRAME_TWEEN);
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
    fn steal_beyond_cap(&mut self, sound: &str, now: Instant) {
        let mut playing: Vec<&mut OneShot> = self
            .oneshots
            .iter_mut()
            .filter(|o| o.sound == sound && !o.looping && o.is_sounding(now))
            .collect();
        let excess = (playing.len() + 1).saturating_sub(MAX_ONESHOTS_PER_SOUND);
        for oldest in playing.iter_mut().take(excess) {
            oldest.handle.stop(Tween {
                duration: STEAL_FADE,
                ..Default::default()
            });
            oldest.stolen = true;
            let faded = now + STEAL_FADE;
            oldest.ends_at = Some(oldest.ends_at.map_or(faded, |t| t.min(faded)));
            self.stolen_oneshots += 1;
        }
    }

    /// Remove finished one-shot spatial sounds. Call once per frame. (A
    /// remote mixer sweeps the old mix's own.)
    pub fn cleanup_finished_oneshots(&mut self) {
        let now = Instant::now();
        self.oneshots.retain(|o| !o.is_finished(now));
        self.keyed.retain(|k| !k.is_finished(now));
        self.tails.retain(|&(_, end)| now < end);
    }

    /// What the mixer holds, for the perf readout: every voice and spatial
    /// track kira holds here; in the browser's worklet, which says nothing
    /// back, the one-shots this engine started. Which of the
    /// `audio_source`s' are paused, and the worklet's other voices, are
    /// [`AudioSync::add_mix_counts`](crate::sync::AudioSync::add_mix_counts)'s
    /// to say. `None` without a device.
    pub fn mix_counts(&self) -> Option<MixCounts> {
        let mut counts = MixCounts {
            skipped_oneshots: self.skipped_oneshots,
            stolen_oneshots: self.stolen_oneshots,
            ..Default::default()
        };
        let held = Held {
            on_tracks: self.oneshots.iter().filter(|o| o.track.is_some()).count(),
            placed: self.oneshots.len(),
            keyed: self.keyed.len(),
        };
        self.mixer.add_counts(&mut counts, &held)?;
        Some(counts)
    }

    /// Stop and drop every placed one-shot (for scene transitions). One on
    /// its own track stops with it; a panned one needs telling.
    pub fn clear_oneshots(&mut self) {
        for o in &mut self.oneshots {
            o.handle.stop(FRAME_TWEEN);
        }
        self.oneshots.clear();
        self.tails.clear();
        self.mixer.clear_oneshots();
    }

    /// Now, on the clock sounds' positions are stamped with (#658): the
    /// remote mixer's audio time (the browser's `AudioContext` time),
    /// or seconds since this engine was made when mixing here. `None` for a
    /// remote mixer whose clock can't be read (a test's bare link).
    pub fn clock_now(&self) -> Option<f64> {
        self.mixer.clock_now(self.epoch)
    }

    /// How long after it is mixed a sound is heard, seconds (#658): the
    /// browser's `baseLatency + outputLatency`. `None` where it isn't known:
    /// kira's desktop stream doesn't say what its device adds.
    pub fn output_latency(&self) -> Option<f64> {
        self.mixer.output_latency()
    }

    /// How long a loaded sound plays at its own rate, seconds.
    pub fn sound_length(&self, name: &str) -> Option<f64> {
        let name = &*self.resolve_sound(name);
        self.mixer.sound_length(name).or_else(|| {
            self.sound_cache
                .get(name)
                .map(|d| d.frames.len() as f64 / d.sample_rate.max(1) as f64)
        })
    }

    /// Check if a sound is already loaded
    pub fn has_sound(&self, name: &str) -> bool {
        self.sound_cache.contains_key(name) || self.mixer.holds_sound(name)
    }

    /// Set the low-pass filter cutoff frequency (Hz). 20000 = fully open, lower = more muffled.
    pub fn set_filter_cutoff(&mut self, hz: f32) {
        self.filter_cutoff = hz as f64;
        self.mixer.set_filter_cutoff(hz, 0.016);
    }
}

/// Convert Flint Vec3 to glam Vec3
fn to_glam_vec3(v: Vec3) -> glam::Vec3 {
    glam::Vec3::new(v.x, v.y, v.z)
}

/// The extensions a bare sound name tries, in order (#629).
pub const SOUND_EXTENSIONS: [&str; 4] = ["ogg", "wav", "flac", "mp3"];

/// A name's file stem: `audio/sfx/whoosh.ogg` -> `whoosh`.
fn sound_stem(name: &str) -> &str {
    let file = name.rsplit('/').next().unwrap_or(name);
    file.rsplit_once('.').map_or(file, |(stem, _)| stem)
}

/// Edit distance between two short names, case folded.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut diag = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = (diag + usize::from(ca != cb))
                .min(row[j] + 1)
                .min(row[j + 1] + 1);
            diag = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

/// The known name nearest `name`, by its stem first and then the whole
/// name, ties to the first in name order so the hint is the same each run.
fn nearest_sound<'a>(name: &str, known: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let stem = sound_stem(name);
    known.min_by_key(|k| {
        (
            edit_distance(stem, sound_stem(k)),
            edit_distance(name, k),
            *k,
        )
    })
}

/// A player's bus volume as kept: 0..=1, and 1 for a value that is no
/// number.
fn user_volume(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        1.0
    }
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
    fn a_bare_stem_finds_the_preloaded_file_and_a_miss_names_the_nearest() {
        // #629: the scene preload keys files by path, extension and all.
        let mut engine = AudioEngine::silent();
        let tick = [0.0, 0.1, 0.0];
        for name in ["audio/whoosh.ogg", "audio/thud.wav", "blip"] {
            engine
                .load_sound_from_samples(name, 22_050, &[&tick])
                .unwrap();
        }
        // A queued sound counts too, before it is decoded.
        engine
            .decodes
            .push("audio/rain.flac", crate::decode::tests::wav(8), Need::Later);
        let resolve = |n: &str| engine.resolve_sound(n).into_owned();
        assert_eq!(resolve("audio/whoosh.ogg"), "audio/whoosh.ogg");
        assert_eq!(resolve("whoosh"), "audio/whoosh.ogg");
        assert_eq!(resolve("audio/whoosh"), "audio/whoosh.ogg");
        assert_eq!(resolve("thud"), "audio/thud.wav");
        assert_eq!(resolve("thud.wav"), "audio/thud.wav");
        assert_eq!(resolve("blip"), "blip", "a name cached as given wins");
        assert_eq!(resolve("whosh"), "whosh", "a miss stays as written");
        assert_eq!(resolve("rain"), "audio/rain.flac");

        let message = |n: &str| engine.not_cached(n).to_string();
        assert!(
            message("whosh").contains("nearest: \"audio/whoosh.ogg\""),
            "{}",
            message("whosh")
        );
        assert!(message("audio/thudd.ogg").contains("nearest: \"audio/thud.wav\""));
        assert!(AudioEngine::silent()
            .not_cached("x")
            .to_string()
            .contains("no sounds are cached"));
    }

    #[test]
    fn a_silent_engine_remembers_its_mix_for_start() {
        let mut engine = AudioEngine::silent();
        assert!(!engine.is_available() && !engine.has_listener());
        engine.set_filter_cutoff(800.0);
        engine.set_bus_volume(Bus::MUSIC, 0.25);
        // Sounds load and cache without a device, ready for the click.
        engine
            .load_sound_from_samples("sea", 22_050, &[&[0.0, 0.1, 0.0]])
            .unwrap();
        assert!(engine.has_sound("sea"));
        assert_eq!(engine.filter_cutoff, 800.0);
        assert_eq!(engine.bus_volumes, vec![1.0, 0.25]);
        // A hold keeps the volumes asked for, to come back to.
        engine.set_held(true);
        engine.set_bus_volume(Bus::SFX, 0.5);
        assert!(engine.is_held());
        assert_eq!(engine.bus_volumes, vec![0.5, 0.25]);
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
                e.play_oneshot_at(name, at, 1.0, 1.0, DEFAULT_DISTANCES, 1.0, Bus::SFX)
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

    /// #975: a name nobody cached is looked for by the scene's search on its
    /// first play and kept under that name; a small sound plays at once, a
    /// big one skips the play and goes to the front of the queue, and a
    /// name the search can't find is asked of the folders only once until
    /// the search is set again.
    #[test]
    fn a_sound_nobody_cached_is_found_by_the_scenes_search_when_first_played() {
        use crate::decode::tests::wav;
        let mut files = vfs::MemorySource::new();
        files.insert("game/flint.toml", "[project]\nid = \"g\"\n");
        files.insert("game/sfx/door.wav", wav(1_000));
        files.insert("game/sfx/song.wav", wav(400_000));
        vfs::scoped(Arc::new(files), || {
            let mut engine = AudioEngine::silent();
            engine.start_remote(Link::default());
            let play = |e: &mut AudioEngine, name| {
                e.play_non_spatial(name, 1.0, 1.0, false, Bus::SFX)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            };
            // No search set: as before, only what was cached plays
            let before = play(&mut engine, "../sfx/door.wav").unwrap_err();
            assert!(before.contains("not cached"), "{before}");

            let scene = Path::new("game/levels/a.scene.toml");
            engine.set_sound_search(Some(AssetScope::for_scene(scene)));
            play(&mut engine, "../sfx/door.wav").unwrap();
            assert!(engine.has_sound("../sfx/door.wav"), "kept as written");

            let big = play(&mut engine, "../sfx/song.wav").unwrap_err();
            assert!(big.contains("still decoding"), "{big}");
            assert!(engine.is_queued("../sfx/song.wav"));

            assert!(play(&mut engine, "gone.wav").is_err());
            assert!(engine.searched_missing.contains("gone.wav"));
            assert!(play(&mut engine, "gone.wav").is_err());
            assert_eq!(engine.searched_missing.len(), 1);
            engine.set_sound_search(Some(AssetScope::for_scene(scene)));
            assert!(engine.searched_missing.is_empty(), "a new search forgets");
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
                Bus::SFX,
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
                .filter(|o| o.sound == name && o.is_sounding(Instant::now()))
                .count()
        };
        assert_eq!(sounding(&engine, "creak"), MAX_ONESHOTS_PER_SOUND);
        assert_eq!(sounding(&engine, "drip"), 1, "the cap is per sound");
        let counts = engine.mix_counts().unwrap();
        assert_eq!((counts.skipped_oneshots, counts.stolen_oneshots), (1, 2));
        // The two stolen creaks are kept while they fade.
        assert_eq!(engine.oneshots.len(), 7);
    }

    /// A panned one-shot in the worklet: louder on its side, steered as the
    /// listener turns, and swept once it has played out.
    #[test]
    fn a_remote_one_shot_is_a_panned_voice_steered_each_frame() {
        use flint_audio_worklet::Mixer;
        let link = Link::default();
        let mut mixer = Mixer::new(48_000).unwrap();
        let mut engine = AudioEngine::silent();
        let tone: Vec<f32> = (0..4_410)
            .map(|i| (i as f32 * 0.0627).sin() * 0.5)
            .collect();
        engine
            .load_sound_from_samples("creak", 44_100, &[&tone])
            .unwrap();
        engine.start_remote(link.clone());
        engine.create_listener(Vec3::new(0.0, 0.0, 0.0)).unwrap();
        let loudness = |link: &Link, mixer: &mut Mixer| {
            let out = link.take();
            for l in &out.loads {
                mixer.load_sound(l.id, l.sample_rate, l.channels, &l.samples);
            }
            mixer.apply(&out.batch);
            let (mut l, mut r) = (0.0f32, 0.0f32);
            for _ in 0..8 {
                let out = mixer.render(128);
                l = l.max(out[..128].iter().fold(0.0, |a, s| a.max(s.abs())));
                r = r.max(out[128..].iter().fold(0.0, |a, s| a.max(s.abs())));
            }
            (l, r)
        };
        engine
            .play_oneshot_at(
                "creak",
                Vec3::new(3.0, 0.0, 0.0),
                1.0,
                1.0,
                (1.0, 25.0),
                2.0,
                Bus::SFX,
            )
            .unwrap();
        let (l, r) = loudness(&link, &mut mixer);
        assert!(r > l * 1.5, "on the right: {l} {r}");
        // Turned to face it, it comes from ahead.
        engine.update_listener(Vec3::new(0.0, 0.0, 0.0), std::f32::consts::FRAC_PI_2, 0.0);
        engine.pan_oneshots();
        loudness(&link, &mut mixer);
        let (l, r) = loudness(&link, &mut mixer);
        assert!((l - r).abs() < 0.05 * r.max(l), "ahead: {l} {r}");
        assert_eq!(mixer.errors, 0);
        // A 0.1 s sound has played out well before 1 s.
        engine.oneshots[0].ends_at = Some(Instant::now() - Duration::from_secs(1));
        engine.cleanup_finished_oneshots();
        assert!(engine.oneshots.is_empty());
    }

    #[test]
    fn an_unknown_sound_is_an_error_not_a_panic() {
        let mut engine = AudioEngine::new();
        let at = Vec3::new(0.0, 0.0, 0.0);
        assert!(engine
            .play_oneshot_at("nope", at, 1.0, 1.0, (2.0, 80.0), 0.7, Bus::SFX)
            .is_err());
        assert!(engine
            .play_non_spatial("nope", 1.0, 1.0, false, Bus::MUSIC)
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
        engine.set_bus_volume(Bus::MUSIC, 0.5);
        let link = Link::default();
        engine.start_remote(link.clone());
        assert!(engine.is_available());
        assert_eq!(
            engine.host(|mixer| mixer.name()),
            Ok("worklet"),
            "the worklet hosts a music suite (#1216)"
        );
        assert_eq!(engine.mixer_name(), "worklet");
        assert!(engine.has_sound("sea"), "known by name once sent");
        assert!(
            engine.sound_cache.is_empty(),
            "the main thread let go of it"
        );

        let mut mixer = Mixer::new(48_000).unwrap();
        engine.create_listener(Vec3::new(0.0, 0.0, 0.0)).unwrap();
        let mut music = engine
            .play_non_spatial("sea", 1.0, 1.0, true, Bus::MUSIC)
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
            .create_spatial_track(Vec3::new(3.0, 0.0, 0.0), 1.0, 25.0, Bus::SFX)
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
                Bus::SFX,
            )
            .unwrap();
        deliver(&link, &mut mixer);
        assert_eq!(
            mixer.counts(),
            (2, 0, 0),
            "the voice handle lives; the track went; the one-shot is a panned voice (#138)"
        );
        let counts = engine.mix_counts().unwrap();
        assert_eq!(
            (counts.voices, counts.tracks),
            (1, 0),
            "the one-shot it holds"
        );

        // As before #138, a one-shot gets the worklet's own spatial track.
        engine.set_old_mix(true);
        engine
            .play_oneshot_at(
                "sea",
                Vec3::new(0.0, 0.0, -2.0),
                1.0,
                1.0,
                (1.0, 25.0),
                2.0,
                Bus::SFX,
            )
            .unwrap();
        deliver(&link, &mut mixer);
        assert_eq!(mixer.counts(), (2, 0, 1));
        assert_eq!(mixer.errors, 0);
        assert!(engine
            .play_non_spatial("nope", 1.0, 1.0, false, Bus::SFX)
            .is_err());
    }

    /// #730: a game's buses, declared before the mixer starts and after,
    /// reach the worklet as `BusNew`s in number order; the same table again
    /// changes nothing; a hold quiets only the top-level buses.
    #[test]
    fn a_games_buses_reach_the_mixer_whenever_they_are_declared() {
        use crate::remote::protocol::{Command, Reader, BUS_MAIN, BUS_SFX};
        fn sent(link: &Link) -> Vec<Command> {
            let out = link.take();
            let mut reader = Reader::new(&out.batch);
            let commands = reader.by_ref().collect();
            assert_eq!(reader.error, None);
            commands
        }
        let table = |text: &str| Buses::from_manifest(&text.parse().unwrap(), "flint.toml");

        let mut engine = AudioEngine::silent();
        let first = table("[audio.buses.voice]\nvolume = 0.5\n[audio.buses.music]\nvolume = 0.8\n")
            .unwrap();
        engine.set_buses(&first);
        let voice = engine.bus("voice").unwrap();
        assert_eq!(engine.bus_volume(voice), Some(0.5));
        let link = Link::default();
        engine.start_remote(link.clone());
        let opened = sent(&link);
        assert!(opened.contains(&Command::BusNew {
            bus: 2,
            parent: BUS_MAIN,
            db: amplitude_to_db(0.5).0,
            lowpass: 20_000.0,
        }));
        assert!(
            opened
                .iter()
                .any(|c| matches!(c, Command::BusVolume { bus: 1, .. })),
            "music opens at 0.8"
        );

        // The game turns voice down; the same table at the next scene
        // leaves it there.
        engine.set_bus_volume_named("voice", 0.1).unwrap();
        engine.set_buses(&first);
        assert_eq!(engine.bus_volume(voice), Some(0.1));
        sent(&link);

        // Another project's table adds its bus, live, after the others.
        let second = table("[audio.buses.ui]\nparent = \"sfx\"\n").unwrap();
        engine.set_buses(&second);
        let ui = engine.bus("ui").unwrap();
        assert_eq!(ui.index(), 3);
        assert_eq!(
            sent(&link),
            [Command::BusNew {
                bus: 3,
                parent: BUS_SFX,
                db: 0.0,
                lowpass: 20_000.0,
            }]
        );

        // Held: sfx, music and voice go silent; ui goes with sfx.
        engine.set_held(true);
        let held: Vec<u8> = sent(&link)
            .iter()
            .filter_map(|c| match c {
                Command::BusVolume { bus, db, .. } if *db <= -60.0 => Some(*bus),
                _ => None,
            })
            .collect();
        assert_eq!(held, [0, 1, 2]);

        // A name that is no bus: an error, sfx for a sound, and nothing sent.
        assert!(engine
            .set_bus_volume_named("chorus", 0.5)
            .unwrap_err()
            .to_string()
            .contains("the buses are sfx, music, voice, ui"));
        assert_eq!(engine.bus_or_sfx("chorus"), Bus::SFX);
        assert!(sent(&link).is_empty());
    }

    /// #1217: the player's bus volume is a second factor under the game's.
    /// A fade the game makes is heard times the player's setting and never
    /// overwrites it; a saved setting read before the game declares its
    /// bus finds the bus when it is declared.
    #[test]
    fn a_players_bus_volume_multiplies_the_games_and_outlives_its_fades() {
        use crate::remote::protocol::{Command, Reader};
        fn last_db(link: &Link, of: u8) -> Option<f32> {
            let out = link.take();
            let mut reader = Reader::new(&out.batch);
            let db = reader
                .by_ref()
                .filter_map(|c| match c {
                    Command::BusVolume { bus, db, .. } if bus == of => Some(db),
                    Command::BusNew { bus, db, .. } if bus == of => Some(db),
                    _ => None,
                })
                .last();
            assert_eq!(reader.error, None);
            db
        }
        let db = |v: f64| amplitude_to_db(v).0;

        let mut engine = AudioEngine::silent();
        // Read from the settings file at start: music, and a bus the game
        // hasn't declared yet.
        engine.load_user_bus_volumes([("music".to_string(), 0.4), ("voice".to_string(), 0.5)]);
        let link = Link::default();
        engine.start_remote(link.clone());
        assert_eq!(
            last_db(&link, 1),
            Some(db(0.4)),
            "music opens at the player's 0.4"
        );

        // The game fades music to 0.5: heard at 0.2, the setting kept.
        engine.set_bus_volume(Bus::MUSIC, 0.5);
        assert_eq!(last_db(&link, 1), Some(db(0.2)));
        assert_eq!(engine.bus_volume(Bus::MUSIC), Some(0.5));
        assert_eq!(engine.user_bus_volume("music"), 0.4);

        // The player moves the slider: the game's 0.5 is kept under it.
        engine.set_user_bus_volume("music", 0.8).unwrap();
        assert_eq!(last_db(&link, 1), Some(db(0.4)));
        assert_eq!(engine.bus_volume(Bus::MUSIC), Some(0.5));

        // The game declares voice at 0.8: made at 0.8 x the saved 0.5.
        let table = Buses::from_manifest(
            &"[audio.buses.voice]\nvolume = 0.8\n".parse().unwrap(),
            "flint.toml",
        )
        .unwrap();
        engine.set_buses(&table);
        let voice = engine.bus("voice").unwrap();
        assert_eq!(last_db(&link, voice.index() as u8), Some(db(0.4)));

        // Clamped to a slider's range; not a number is unity; no bus is an
        // error and changes nothing.
        engine.set_user_bus_volume("sfx", 3.0).unwrap();
        assert_eq!(engine.user_bus_volume("sfx"), 1.0);
        engine.set_user_bus_volume("sfx", -1.0).unwrap();
        assert_eq!(engine.user_bus_volume("sfx"), 0.0);
        engine.load_user_bus_volumes([("sfx".to_string(), f64::NAN)]);
        assert_eq!(engine.user_bus_volume("sfx"), 1.0);
        assert_eq!(
            engine.user_bus_volume("music"),
            1.0,
            "a load replaces the set"
        );
        assert!(engine.set_user_bus_volume("chorus", 0.5).is_err());
        assert_eq!(engine.user_bus_volume("chorus"), 1.0);

        // A hold still silences the bus under both, and lets go to both.
        engine.set_user_bus_volume("music", 0.4).unwrap();
        link.take();
        engine.set_held(true);
        assert!(last_db(&link, 1).unwrap() <= -60.0);
        engine.set_held(false);
        // The voice table was the first, so it set music's game volume
        // back to its default 1 (#730's rule): heard at the player's 0.4.
        assert_eq!(engine.bus_volume(Bus::MUSIC), Some(1.0));
        assert_eq!(last_db(&link, 1), Some(db(0.4)));
    }

    /// #730, #1216: content with its own clock (a music suite) is hosted
    /// wherever the engine mixes, the browser's worklet too, and not while
    /// silent; a new mixer is a new epoch.
    #[test]
    fn every_mixer_with_a_device_hosts_clocked_content() {
        let mut engine = AudioEngine::silent();
        assert_eq!(engine.can_host(), Err(Unhosted::Silent));
        assert_eq!(engine.host(|_| 1), Err(Unhosted::Silent));
        let before = engine.mixer_epoch();
        let link = Link::default();
        engine.start_remote(link.clone());
        assert_ne!(engine.mixer_epoch(), before);
        link.take(); // the cutoff a remote mixer opens with
        fn sent(link: &Link) -> Vec<crate::remote::Command> {
            let out = link.take();
            crate::remote::protocol::Reader::new(&out.batch).collect()
        }
        use crate::remote::Command;
        let clock = engine.host(|mixer| mixer.add_clock(48_000.0)).unwrap();
        let mut clock = clock.unwrap();
        clock.start();
        assert_eq!(
            sent(&link),
            vec![
                Command::ClockNew {
                    clock: 1,
                    ticks_per_second: 48_000.0
                },
                Command::ClockStart { clock: 1 },
            ]
        );
        assert!(Unhosted::Silent.to_string().contains("device"));
    }
}
