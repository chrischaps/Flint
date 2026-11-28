//! What clocked content asks of a mixer (#1216): a music suite's clock, its
//! stem buses and its stems, wherever the mix runs.
//!
//! A suite (`flint_music::SuiteSession`) builds a small graph of its own: a
//! clock counting samples, a bus per stem with a low-pass on it, every stem
//! started on one tick of the clock, and gain, cutoff and rate changes timed
//! on later ticks. Those few things are [`ClockedMixer`] and the handles it
//! makes. kira here (`AudioManager<B>` for any backend: the device, the
//! engine's stream, the offline renderer) is one implementation; the
//! browser's worklet ([`RemoteMixer`], over a [`Link`]) is the other, where
//! each call is a `protocol::Command` and the clock's ticks come back in the
//! worklet's reports, as watched voices' positions do (#658).
//!
//! Every change takes a [`Ramp`]: linear, over seconds, starting now or on a
//! [`Start::Tick`] of the clock its bus was made on. Handles behave like
//! kira's: dropping a voice lets it play on, dropping a bus or the clock
//! takes what is on it.

use crate::remote::protocol::{self, At};
use crate::remote::{Command, Link, RemoteVoice, SoundLoad};
use flint_core::{FlintError, Result};
use kira::backend::Backend;
use kira::clock::{ClockHandle, ClockId, ClockSpeed, ClockTime};
use kira::effect::filter::{FilterBuilder, FilterHandle, FilterMode};
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::sound::{PlaybackPosition, PlaybackState};
use kira::track::{TrackBuilder, TrackHandle};
use kira::{AudioManager, Decibels, Easing, PlaybackRate, StartTime, Tween};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// A stem bus's low-pass when open.
pub const CUTOFF_OPEN_HZ: f64 = 20_000.0;

/// How far past its last report a remote clock is carried forward, seconds:
/// reports come every 1024 frames (about 21 ms), so a gap longer than this
/// is the page asleep, not the clock running unheard.
const CARRY_LIMIT_SECS: f64 = 0.1;

/// When a change starts: at once, or on a tick of the clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Start {
    #[default]
    Now,
    Tick(u64),
}

/// A linear change over `secs`, from `start`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ramp {
    pub start: Start,
    pub secs: f64,
}

impl Default for Ramp {
    /// At once, over 10 ms: kira's `Tween::default`.
    fn default() -> Self {
        Self::now(0.01)
    }
}

impl Ramp {
    pub fn now(secs: f64) -> Self {
        Self {
            start: Start::Now,
            secs,
        }
    }

    pub fn at(start: Start, secs: f64) -> Self {
        Self { start, secs }
    }

    fn duration(&self) -> Duration {
        Duration::from_secs_f64(self.secs.max(0.0))
    }

    fn tween(&self, clock: ClockId) -> Tween {
        Tween {
            start_time: start_time(self.start, clock),
            duration: self.duration(),
            easing: Easing::Linear,
        }
    }

    fn wire(&self, clock: u32) -> (At, f32) {
        (at(self.start, clock), self.duration().as_secs_f32())
    }
}

fn start_time(start: Start, clock: ClockId) -> StartTime {
    match start {
        Start::Now => StartTime::Immediate,
        Start::Tick(tick) => StartTime::ClockTime(ClockTime::from_ticks_u64(clock, tick)),
    }
}

fn at(start: Start, clock: u32) -> At {
    match start {
        Start::Now => At::NOW,
        Start::Tick(tick) => At::tick(clock, tick),
    }
}

/// How a stem starts on its bus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StemPlay {
    /// The frame of the stem to start from.
    pub from_frame: u64,
    pub start: Start,
    pub db: f32,
    pub rate: f64,
}

impl Default for StemPlay {
    fn default() -> Self {
        Self {
            from_frame: 0,
            start: Start::Now,
            db: 0.0,
            rate: 1.0,
        }
    }
}

/// The few things a suite does with a mixer. See the module docs.
pub trait ClockedMixer {
    /// "kira" or "worklet".
    fn name(&self) -> &'static str;
    /// A clock counting `ticks_per_second`, stopped until
    /// [`MixClock::start`].
    fn add_clock(&mut self, ticks_per_second: f64) -> Result<MixClock>;
    /// A sound for stems to play: here, the data itself; remote, sent once.
    fn add_stem(&mut self, data: &StaticSoundData) -> Stem;
    /// A stem bus under the main track, its low-pass open, whose changes
    /// and stems start on `clock`'s ticks.
    fn add_bus(&mut self, clock: &MixClock) -> Result<StemBus>;
}

impl<B: Backend> ClockedMixer for AudioManager<B> {
    fn name(&self) -> &'static str {
        "kira"
    }

    fn add_clock(&mut self, ticks_per_second: f64) -> Result<MixClock> {
        self.add_clock(ClockSpeed::TicksPerSecond(ticks_per_second))
            .map(MixClock::Local)
            .map_err(|e| FlintError::AudioError(format!("failed to create clock: {e}")))
    }

    fn add_stem(&mut self, data: &StaticSoundData) -> Stem {
        Stem::Local(data.clone())
    }

    fn add_bus(&mut self, clock: &MixClock) -> Result<StemBus> {
        let MixClock::Local(clock) = clock else {
            return Err(FlintError::AudioError(
                "a stem bus on another mixer's clock".into(),
            ));
        };
        let mut builder = TrackBuilder::new();
        let filter = builder.add_effect(
            FilterBuilder::new()
                .mode(FilterMode::LowPass)
                .cutoff(CUTOFF_OPEN_HZ),
        );
        let track = self
            .add_sub_track(builder)
            .map_err(|e| FlintError::AudioError(format!("stem bus: {e}")))?;
        Ok(StemBus::Local {
            track,
            filter,
            clock: clock.id(),
        })
    }
}

/// kira in the browser's worklet, through a [`Link`] (#1216).
pub struct RemoteMixer {
    link: Link,
}

impl RemoteMixer {
    pub fn new(link: Link) -> Self {
        Self { link }
    }
}

impl ClockedMixer for RemoteMixer {
    fn name(&self) -> &'static str {
        "worklet"
    }

    fn add_clock(&mut self, ticks_per_second: f64) -> Result<MixClock> {
        let id = self.link.id();
        self.link.send(Command::ClockNew {
            clock: id,
            ticks_per_second,
        });
        Ok(MixClock::Remote(RemoteClock {
            id,
            ticks_per_second,
            link: self.link.clone(),
            seen: AtomicU64::new(0f64.to_bits()),
        }))
    }

    fn add_stem(&mut self, data: &StaticSoundData) -> Stem {
        let id = self.link.id();
        self.link
            .load(SoundLoad::new(id, data.sample_rate, &data.frames));
        Stem::Remote {
            id,
            frames: data.frames.len(),
            link: self.link.clone(),
        }
    }

    fn add_bus(&mut self, clock: &MixClock) -> Result<StemBus> {
        let MixClock::Remote(clock) = clock else {
            return Err(FlintError::AudioError(
                "a stem bus on another mixer's clock".into(),
            ));
        };
        let id = self.link.id();
        self.link.send(Command::StemBusNew { track: id });
        Ok(StemBus::Remote {
            id,
            clock: clock.id,
            link: self.link.clone(),
        })
    }
}

/// The content's clock.
pub enum MixClock {
    Local(ClockHandle),
    Remote(RemoteClock),
}

pub struct RemoteClock {
    id: u32,
    ticks_per_second: f64,
    link: Link,
    /// The most it has read, so it never reads less: a report that lands
    /// behind what was carried forward holds the reading still instead.
    seen: AtomicU64,
}

impl Drop for RemoteClock {
    fn drop(&mut self) {
        self.link.send(Command::ClockDrop { clock: self.id });
        self.link.forget_voice(self.id);
    }
}

impl MixClock {
    pub fn start(&mut self) {
        match self {
            Self::Local(c) => c.start(),
            Self::Remote(c) => c.link.send(Command::ClockStart { clock: c.id }),
        }
    }

    /// Ticks since it started, with the fraction. Remote, the last report
    /// carried forward on the audio clock (`Link::clock`) by at most
    /// [`CARRY_LIMIT_SECS`]; 0 before the worklet has said anything.
    pub fn ticks(&self) -> f64 {
        match self {
            Self::Local(c) => {
                let t = c.time();
                t.ticks as f64 + t.fraction
            }
            Self::Remote(c) => {
                let seen = f64::from_bits(c.seen.load(Ordering::Relaxed));
                let Some((report, at)) = c.link.voice_report(c.id) else {
                    return seen;
                };
                let mut ticks = report.position;
                if report.state == protocol::state::CLOCK {
                    if let Some([now, _]) = c.link.clock() {
                        ticks += (now - at).clamp(0.0, CARRY_LIMIT_SECS) * c.ticks_per_second;
                    }
                }
                let ticks = ticks.max(seen);
                c.seen.store(ticks.to_bits(), Ordering::Relaxed);
                ticks
            }
        }
    }
}

/// A sound for stems to play.
pub enum Stem {
    Local(StaticSoundData),
    Remote { id: u32, frames: usize, link: Link },
}

impl Drop for Stem {
    fn drop(&mut self) {
        if let Self::Remote { id, link, .. } = self {
            link.send(Command::SoundDrop { sound: *id });
        }
    }
}

impl Stem {
    /// How many frames it holds.
    pub fn frames(&self) -> usize {
        match self {
            Self::Local(data) => data.frames.len(),
            Self::Remote { frames, .. } => *frames,
        }
    }
}

/// A stem bus: a track with a low-pass, on its clock.
pub enum StemBus {
    Local {
        track: TrackHandle,
        filter: FilterHandle,
        clock: ClockId,
    },
    Remote {
        id: u32,
        clock: u32,
        link: Link,
    },
}

impl Drop for StemBus {
    fn drop(&mut self) {
        if let Self::Remote { id, link, .. } = self {
            link.send(Command::StemBusDrop { track: *id });
        }
    }
}

impl StemBus {
    pub fn set_volume(&mut self, db: f32, ramp: Ramp) {
        match self {
            Self::Local { track, clock, .. } => track.set_volume(Decibels(db), ramp.tween(*clock)),
            Self::Remote { id, clock, link } => {
                let (at, tween) = ramp.wire(*clock);
                link.send(Command::StemBusVolume {
                    track: *id,
                    db,
                    at,
                    tween,
                });
            }
        }
    }

    pub fn set_cutoff(&mut self, hz: f64, ramp: Ramp) {
        match self {
            Self::Local { filter, clock, .. } => filter.set_cutoff(hz, ramp.tween(*clock)),
            Self::Remote { id, clock, link } => {
                let (at, tween) = ramp.wire(*clock);
                link.send(Command::StemBusCutoff {
                    track: *id,
                    hz: hz as f32,
                    at,
                    tween,
                });
            }
        }
    }

    /// Play `stem` on this bus. A stem made by another mixer is an error.
    pub fn play(&mut self, stem: &Stem, play: StemPlay) -> Result<StemVoice> {
        match (self, stem) {
            (Self::Local { track, clock, .. }, Stem::Local(data)) => {
                let data = data
                    .start_position(PlaybackPosition::Samples(play.from_frame as usize))
                    .start_time(start_time(play.start, *clock))
                    .volume(Decibels(play.db))
                    .playback_rate(PlaybackRate(play.rate));
                let sound = track
                    .play(data)
                    .map_err(|e| FlintError::AudioError(format!("stem: {e}")))?;
                Ok(StemVoice::Local {
                    sound,
                    clock: *clock,
                })
            }
            (Self::Remote { id, clock, link }, Stem::Remote { id: sound, .. }) => {
                let voice = link.id();
                link.send(Command::StemPlay {
                    voice,
                    sound: *sound,
                    track: *id,
                    from: play.from_frame,
                    at: at(play.start, *clock),
                    db: play.db,
                    rate: play.rate as f32,
                });
                // Reported back, so its position and end can be read
                link.send(Command::VoiceWatch { voice });
                Ok(StemVoice::Remote {
                    voice: RemoteVoice {
                        id: voice,
                        link: link.clone(),
                    },
                    clock: *clock,
                })
            }
            _ => Err(FlintError::AudioError(
                "a stem played on another mixer's bus".into(),
            )),
        }
    }
}

/// A stem playing.
pub enum StemVoice {
    Local {
        sound: StaticSoundHandle,
        clock: ClockId,
    },
    Remote {
        voice: RemoteVoice,
        clock: u32,
    },
}

impl StemVoice {
    pub fn set_volume(&mut self, db: f32, ramp: Ramp) {
        match self {
            Self::Local { sound, clock } => sound.set_volume(Decibels(db), ramp.tween(*clock)),
            Self::Remote { voice, clock } => {
                let (at, tween) = ramp.wire(*clock);
                voice.link.send(Command::VoiceVolumeAt {
                    voice: voice.id,
                    db,
                    at,
                    tween,
                });
            }
        }
    }

    pub fn set_rate(&mut self, rate: f64, ramp: Ramp) {
        match self {
            Self::Local { sound, clock } => {
                sound.set_playback_rate(PlaybackRate(rate), ramp.tween(*clock))
            }
            Self::Remote { voice, clock } => {
                let (at, tween) = ramp.wire(*clock);
                voice.link.send(Command::VoiceRateAt {
                    voice: voice.id,
                    rate: rate as f32,
                    at,
                    tween,
                });
            }
        }
    }

    pub fn stop(&mut self, ramp: Ramp) {
        match self {
            Self::Local { sound, clock } => sound.stop(ramp.tween(*clock)),
            Self::Remote { voice, clock } => {
                let (at, tween) = ramp.wire(*clock);
                voice.link.send(Command::VoiceStopAt {
                    voice: voice.id,
                    at,
                    tween,
                });
            }
        }
    }

    /// Seconds into its stem: remote, as last reported (0 before).
    pub fn position(&self) -> f64 {
        match self {
            Self::Local { sound, .. } => sound.position(),
            Self::Remote { voice, .. } => voice
                .link
                .voice_report(voice.id)
                .map_or(0.0, |(r, _)| r.position),
        }
    }

    /// Whether it has played out or been stopped: remote, as last reported.
    pub fn is_stopped(&self) -> bool {
        match self {
            Self::Local { sound, .. } => sound.state() == PlaybackState::Stopped,
            Self::Remote { voice, .. } => voice
                .link
                .voice_report(voice.id)
                .is_some_and(|(r, _)| r.state == protocol::state::STOPPED),
        }
    }
}
