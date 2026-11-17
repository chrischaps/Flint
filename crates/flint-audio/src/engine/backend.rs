//! Where the mix runs, as one trait (#730).
//!
//! [`AudioEngine`](super::AudioEngine) keeps what is the same everywhere
//! (the sound cache, the decode queue, placed one-shots and keyed sounds,
//! the listener's place, the bus table and volumes) and hands everything a
//! mixer does differently to its [`MixerBackend`]: [`Silent`] before there
//! is a device, [`Local`](super::local::Local) (kira on this thread: the
//! desktop, the browser's fallback) and [`Remote`](super::remote_mix::Remote)
//! (kira in the browser's `AudioWorklet`, reached by commands).
//!
//! No method has a default body, so a new one is written for all three or
//! nothing compiles: a call that works on the desktop can't be silently a
//! no-op in the browser. The worklet's side is held the same way by the
//! protocol (`Command::encode` and `Mixer::command` match every command,
//! and `every_op_reads_back` checks the reader).

use crate::bus::Bus;
use crate::handle::{SoundHandle, SpatialTrack};
use crate::pan::Falloff;
use flint_core::Result;
use kira::sound::static_sound::StaticSoundData;
use kira::{AudioManager, Decibels};
use web_time::Instant;

use super::{AudioBackend, MixCounts};

/// One sound to start: the engine's decoded copy of it when it keeps one,
/// and how it plays. `db` already has the master volume in it.
pub(crate) struct Voice<'a> {
    pub name: &'a str,
    pub data: Option<&'a StaticSoundData>,
    pub db: Decibels,
    pub rate: f64,
    pub pan: f32,
    pub looping: bool,
}

/// Where a voice plays.
pub(crate) enum On<'a> {
    Bus(Bus),
    Track(&'a mut SpatialTrack),
}

/// What the engine holds that a mixer may count as its own voices.
pub(crate) struct Held {
    /// Placed one-shots on spatial tracks of their own (the old mix).
    pub on_tracks: usize,
    /// Every placed one-shot, and every keyed sound on a bus.
    pub placed: usize,
    pub keyed: usize,
}

/// Why a mixer can't host content that builds its own kira graph (a music
/// suite, ADR 0017).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unhosted {
    /// No device yet (or none at all).
    Silent,
    /// The mix runs in the browser's worklet, which has no command for a
    /// clock or a stem; the scene mixes on the main thread instead.
    Worklet,
}

impl std::fmt::Display for Unhosted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Unhosted::Silent => "no audio device",
            Unhosted::Worklet => "the browser's worklet mixer hosts no clocked content",
        })
    }
}

/// Everything a mixer does that another does differently. See the module
/// docs; no method has a default.
pub(crate) trait MixerBackend {
    /// "silent", "main thread" or "worklet".
    fn name(&self) -> &'static str;
    /// Whether it plays sound at all.
    fn is_available(&self) -> bool;
    /// Whether it runs elsewhere and says nothing back unless asked
    /// (`SoundHandle::watch`).
    fn is_remote(&self) -> bool;
    /// Whether it gave up after it started (the worklet failing), so the
    /// engine moves the mix.
    fn failed(&self) -> bool;

    /// Whether the engine must keep its own decoded copy of each sound:
    /// everywhere but a remote mixer known to be running.
    fn needs_cache(&self) -> bool;
    /// A sound was decoded, or replaced: a mixer that holds its own copies
    /// takes one.
    fn sound_cached(&mut self, name: &str, data: &StaticSoundData);
    /// Whether it holds a sound of its own under `name`.
    fn holds_sound(&self, name: &str) -> bool;
    /// The sounds it holds of its own, for an error's hint.
    fn sound_names(&self) -> Vec<&str>;
    /// How long a sound it holds plays at its own rate, seconds.
    fn sound_length(&self, name: &str) -> Option<f64>;

    /// Make a bus the engine's table declares, under `parent` (`None`: the
    /// main track). The built-in two exist from the start; this comes for
    /// each other bus, in number order.
    fn add_bus(&mut self, bus: Bus, parent: Option<Bus>, db: Decibels) -> Result<()>;
    fn set_bus_volume(&mut self, bus: Bus, db: Decibels, tween_secs: f64);
    fn set_filter_cutoff(&mut self, hz: f32, tween_secs: f64);

    fn create_listener(&mut self, pos: glam::Vec3, orientation: glam::Quat) -> Result<()>;
    fn has_listener(&self) -> bool;
    fn move_listener(&mut self, pos: glam::Vec3, orientation: glam::Quat);

    fn create_spatial_track(
        &mut self,
        pos: glam::Vec3,
        falloff: Falloff,
        bus: Bus,
    ) -> Result<SpatialTrack>;
    fn play(&mut self, voice: Voice<'_>, on: On<'_>) -> Result<SoundHandle>;
    /// A one-shot on a spatial track of its own (`?mix=old`): the handle
    /// and track to keep when the engine must, `None` when the mixer keeps
    /// them itself.
    fn play_on_own_track(
        &mut self,
        voice: Voice<'_>,
        pos: glam::Vec3,
        falloff: Falloff,
        bus: Bus,
    ) -> Result<Option<(SoundHandle, SpatialTrack)>>;
    /// When a sound started now will have played out, for a mixer whose
    /// handles can't say; `None` where they can.
    fn play_ends(&self, name: &str, rate: f64, looping: bool, now: Instant) -> Option<Instant>;
    /// Stop the placed one-shots the mixer keeps itself.
    fn clear_oneshots(&mut self);

    /// Add what it mixes to `counts`; `None` when it mixes nothing.
    fn add_counts(&self, counts: &mut MixCounts, held: &Held) -> Option<()>;
    /// Now, on the clock sounds' positions are stamped with (#658).
    fn clock_now(&self, epoch: Instant) -> Option<f64>;
    /// How long after it is mixed a sound is heard, seconds, if known.
    fn output_latency(&self) -> Option<f64>;

    /// The kira manager content with its own clock and tracks starts on
    /// (a music suite), or why there is none here.
    fn host(&mut self) -> std::result::Result<&mut AudioManager<AudioBackend>, Unhosted>;
}

/// No mixer: before the device opens (the web's Begin), or with none.
pub(crate) struct Silent;

fn no_device() -> flint_core::FlintError {
    flint_core::FlintError::AudioError("No audio device".into())
}

impl MixerBackend for Silent {
    fn name(&self) -> &'static str {
        "silent"
    }
    fn is_available(&self) -> bool {
        false
    }
    fn is_remote(&self) -> bool {
        false
    }
    fn failed(&self) -> bool {
        false
    }
    fn needs_cache(&self) -> bool {
        true
    }
    fn sound_cached(&mut self, _: &str, _: &StaticSoundData) {}
    fn holds_sound(&self, _: &str) -> bool {
        false
    }
    fn sound_names(&self) -> Vec<&str> {
        Vec::new()
    }
    fn sound_length(&self, _: &str) -> Option<f64> {
        None
    }
    // The engine remembers the table and the volumes for the mixer to come.
    fn add_bus(&mut self, _: Bus, _: Option<Bus>, _: Decibels) -> Result<()> {
        Ok(())
    }
    fn set_bus_volume(&mut self, _: Bus, _: Decibels, _: f64) {}
    fn set_filter_cutoff(&mut self, _: f32, _: f64) {}
    fn create_listener(&mut self, _: glam::Vec3, _: glam::Quat) -> Result<()> {
        Ok(())
    }
    fn has_listener(&self) -> bool {
        false
    }
    fn move_listener(&mut self, _: glam::Vec3, _: glam::Quat) {}
    fn create_spatial_track(&mut self, _: glam::Vec3, _: Falloff, _: Bus) -> Result<SpatialTrack> {
        Err(no_device())
    }
    fn play(&mut self, _: Voice<'_>, _: On<'_>) -> Result<SoundHandle> {
        Err(no_device())
    }
    fn play_on_own_track(
        &mut self,
        _: Voice<'_>,
        _: glam::Vec3,
        _: Falloff,
        _: Bus,
    ) -> Result<Option<(SoundHandle, SpatialTrack)>> {
        Err(no_device())
    }
    fn play_ends(&self, _: &str, _: f64, _: bool, _: Instant) -> Option<Instant> {
        None
    }
    fn clear_oneshots(&mut self) {}
    fn add_counts(&self, _: &mut MixCounts, _: &Held) -> Option<()> {
        None
    }
    fn clock_now(&self, epoch: Instant) -> Option<f64> {
        Some(epoch.elapsed().as_secs_f64())
    }
    fn output_latency(&self) -> Option<f64> {
        None
    }
    fn host(&mut self) -> std::result::Result<&mut AudioManager<AudioBackend>, Unhosted> {
        Err(Unhosted::Silent)
    }
}
