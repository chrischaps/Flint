//! What the main thread tells the worklet's mixer (#136).
//!
//! The page and its `AudioWorklet` share no memory (GitHub Pages can't send
//! the headers `SharedArrayBuffer` needs), so the engine's calls become
//! [`Command`]s, written into one byte batch a frame and posted to the
//! worklet, which applies them to its own kira. Sounds cross separately,
//! once each, as samples. Every id is made on the main thread, so a call
//! returns its handle at once and never waits for an answer.
//!
//! This file has no dependencies: `flint-audio` includes it with `#[path]`
//! to write batches, and the worklet crate reads them, so both sides are
//! always built from the same encoding.

/// The mixer bus a sound or a spatial track hangs off. These two are
/// every mixer's from the start; a game's own buses (#730) follow them,
/// numbered in the order [`Command::BusNew`] makes them.
pub const BUS_SFX: u8 = 0;
pub const BUS_MUSIC: u8 = 1;
/// The parent a bus names when it hangs off the main track (where the
/// master low-pass is) rather than off another bus.
pub const BUS_MAIN: u8 = u8::MAX;

/// Where a voice plays: a bus, or a spatial track made earlier.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Bus(u8),
    Track(u32),
}

/// One engine call. Tween lengths are seconds, linear.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// The main track's low-pass cutoff, Hz.
    FilterCutoff {
        hz: f32,
        tween: f32,
    },
    BusVolume {
        bus: u8,
        db: f32,
        tween: f32,
    },
    /// A game's bus (#730): `bus` is the next number after the buses there
    /// are, under `parent` (a bus made earlier, or [`BUS_MAIN`]), starting
    /// at `db` with its low-pass at `lowpass` Hz (#1218; 20000 is open).
    BusNew {
        bus: u8,
        parent: u8,
        db: f32,
        lowpass: f32,
    },
    /// A bus's own low-pass cutoff, Hz (#1218): every bus has one, the
    /// built-in two included.
    BusLowpass {
        bus: u8,
        hz: f32,
        tween: f32,
    },
    /// Place the listener, making it if there is none yet.
    Listener {
        pos: [f32; 3],
        quat: [f32; 4],
    },
    TrackNew {
        track: u32,
        pos: [f32; 3],
        min: f32,
        max: f32,
        rolloff: f32,
        bus: u8,
    },
    TrackPosition {
        track: u32,
        pos: [f32; 3],
    },
    /// The track's handle was dropped: the track goes, with its sounds.
    TrackDrop {
        track: u32,
    },
    /// `pan` is kira's `Panning`: -1 left, 1 right, at constant power. A
    /// placed sound plays on a bus this way, steered once a frame (#138).
    Play {
        voice: u32,
        sound: u32,
        target: Target,
        db: f32,
        rate: f32,
        pan: f32,
        looping: bool,
    },
    /// A placed one-shot. The worklet keeps its track until it ends.
    OneShot {
        sound: u32,
        pos: [f32; 3],
        db: f32,
        rate: f32,
        min: f32,
        max: f32,
        rolloff: f32,
        bus: u8,
    },
    VoiceVolume {
        voice: u32,
        db: f32,
        tween: f32,
    },
    VoiceRate {
        voice: u32,
        rate: f32,
        tween: f32,
    },
    VoiceStop {
        voice: u32,
        tween: f32,
    },
    VoicePanning {
        voice: u32,
        pan: f32,
        tween: f32,
    },
    /// Paused because nobody can hear it (#138); resumed where it stopped.
    VoicePause {
        voice: u32,
        tween: f32,
    },
    VoiceResume {
        voice: u32,
        tween: f32,
    },
    /// The voice's handle was dropped; the sound plays on to its end.
    VoiceDrop {
        voice: u32,
    },
    ClearOneShots,
    /// Report where the voice is in its sound (#658): from now until it is
    /// dropped, the mixer's report names it, and the
    /// processor posts that back to the page.
    VoiceWatch {
        voice: u32,
    },

    // Clocked content (#1216): a music suite's clock, its stem buses and
    // its stems, every change of them able to start on a tick of the clock.
    /// A clock counting `ticks_per_second`, stopped until
    /// [`Command::ClockStart`]. Every report names it from now on.
    ClockNew {
        clock: u32,
        ticks_per_second: f64,
    },
    ClockStart {
        clock: u32,
    },
    /// The clock's handle was dropped; what waits on it never starts.
    ClockDrop {
        clock: u32,
    },
    /// A stem bus: a track under the main one (where the master low-pass
    /// is) with a low-pass of its own, open.
    StemBusNew {
        track: u32,
    },
    StemBusVolume {
        track: u32,
        db: f32,
        at: At,
        tween: f32,
    },
    StemBusCutoff {
        track: u32,
        hz: f32,
        at: At,
        tween: f32,
    },
    /// The bus's handle was dropped: it goes, with its sounds.
    StemBusDrop {
        track: u32,
    },
    /// A stem played on a stem bus from frame `from` of its sound, starting
    /// at `at`. The voice is like any other ([`Command::VoiceWatch`],
    /// [`Command::VoiceDrop`]).
    StemPlay {
        voice: u32,
        sound: u32,
        track: u32,
        from: u64,
        at: At,
        db: f32,
        rate: f32,
    },
    VoiceVolumeAt {
        voice: u32,
        db: f32,
        at: At,
        tween: f32,
    },
    VoiceRateAt {
        voice: u32,
        rate: f32,
        at: At,
        tween: f32,
    },
    VoiceStopAt {
        voice: u32,
        at: At,
        tween: f32,
    },
    /// The sound's last holder let go of it; voices playing it play on.
    SoundDrop {
        sound: u32,
    },
}

/// When a clocked change starts (#1216): on tick `tick` of clock `clock`,
/// or at once when `clock` is 0 ([`At::NOW`]; ids start at 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct At {
    pub clock: u32,
    pub tick: u64,
}

impl At {
    pub const NOW: At = At { clock: 0, tick: 0 };

    pub fn tick(clock: u32, tick: u64) -> Self {
        Self { clock, tick }
    }
}

/// What the mixer reports back about each watched voice (#658), as
/// `f64`s: the voice's id, seconds into its sound (wrapped, for a loop)
/// and kira's `PlaybackState` as a number, [`REPORT_STRIDE`] to a voice.
/// A clock (#1216) is reported the same way: its id, its ticks with the
/// fraction, and [`state::CLOCK`] (or [`state::CLOCK_STOPPED`]).
pub const REPORT_STRIDE: usize = 3;

/// kira's `PlaybackState`, numbered as kira numbers it.
pub mod state {
    pub const PLAYING: u8 = 0;
    pub const PAUSING: u8 = 1;
    pub const PAUSED: u8 = 2;
    pub const WAITING_TO_RESUME: u8 = 3;
    pub const RESUMING: u8 = 4;
    pub const STOPPING: u8 = 5;
    pub const STOPPED: u8 = 6;
    /// Not a voice: a clock (#1216), running, whose "position" is ticks.
    pub const CLOCK: u8 = 100;
    /// A clock not started yet.
    pub const CLOCK_STOPPED: u8 = 101;

    /// The sound's position is moving: it is being mixed.
    pub fn moving(state: u8) -> bool {
        matches!(state, PLAYING | PAUSING | RESUMING | STOPPING)
    }
}

/// One watched voice, as a report gives it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceReport {
    pub voice: u32,
    pub position: f64,
    pub state: u8,
}

/// Read a report's voices from its `f64`s.
pub fn read_report(values: &[f64]) -> impl Iterator<Item = VoiceReport> + '_ {
    values
        .as_chunks::<REPORT_STRIDE>()
        .0
        .iter()
        .map(|v| VoiceReport {
            voice: v[0] as u32,
            position: v[1],
            state: v[2] as u8,
        })
}

mod op {
    pub const FILTER_CUTOFF: u8 = 1;
    pub const BUS_VOLUME: u8 = 2;
    pub const LISTENER: u8 = 3;
    pub const TRACK_NEW: u8 = 4;
    pub const TRACK_POSITION: u8 = 5;
    pub const TRACK_DROP: u8 = 6;
    pub const PLAY_ON_BUS: u8 = 7;
    pub const PLAY_ON_TRACK: u8 = 8;
    pub const ONE_SHOT: u8 = 9;
    pub const VOICE_VOLUME: u8 = 10;
    pub const VOICE_RATE: u8 = 11;
    pub const VOICE_STOP: u8 = 12;
    pub const VOICE_DROP: u8 = 13;
    pub const CLEAR_ONE_SHOTS: u8 = 14;
    pub const VOICE_PANNING: u8 = 15;
    pub const VOICE_PAUSE: u8 = 16;
    pub const VOICE_RESUME: u8 = 17;
    pub const VOICE_WATCH: u8 = 18;
    pub const BUS_NEW: u8 = 19;
    pub const BUS_LOWPASS: u8 = 20;
    pub const CLOCK_NEW: u8 = 21;
    pub const CLOCK_START: u8 = 22;
    pub const CLOCK_DROP: u8 = 23;
    pub const STEM_BUS_NEW: u8 = 24;
    pub const STEM_BUS_VOLUME: u8 = 25;
    pub const STEM_BUS_CUTOFF: u8 = 26;
    pub const STEM_BUS_DROP: u8 = 27;
    pub const STEM_PLAY: u8 = 28;
    pub const VOICE_VOLUME_AT: u8 = 29;
    pub const VOICE_RATE_AT: u8 = 30;
    pub const VOICE_STOP_AT: u8 = 31;
    pub const SOUND_DROP: u8 = 32;
    /// The highest op: every one from 1 to this reads back
    /// (`every_op_reads_back`).
    #[cfg_attr(not(test), allow(dead_code))]
    pub const LAST: u8 = SOUND_DROP;
}

struct Writer<'a>(&'a mut Vec<u8>);

impl Writer<'_> {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn f32(&mut self, v: f32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn f32s(&mut self, v: &[f32]) -> &mut Self {
        for &x in v {
            self.f32(x);
        }
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn f64(&mut self, v: f64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn at(&mut self, at: At) -> &mut Self {
        self.u32(at.clock).u64(at.tick)
    }
}

impl Command {
    /// Append this command to a batch.
    pub fn encode(&self, out: &mut Vec<u8>) {
        let mut w = Writer(out);
        match *self {
            Command::FilterCutoff { hz, tween } => {
                w.u8(op::FILTER_CUTOFF).f32(hz).f32(tween);
            }
            Command::BusVolume { bus, db, tween } => {
                w.u8(op::BUS_VOLUME).u8(bus).f32(db).f32(tween);
            }
            Command::BusNew {
                bus,
                parent,
                db,
                lowpass,
            } => {
                w.u8(op::BUS_NEW).u8(bus).u8(parent).f32(db).f32(lowpass);
            }
            Command::BusLowpass { bus, hz, tween } => {
                w.u8(op::BUS_LOWPASS).u8(bus).f32(hz).f32(tween);
            }
            Command::Listener { pos, quat } => {
                w.u8(op::LISTENER).f32s(&pos).f32s(&quat);
            }
            Command::TrackNew {
                track,
                pos,
                min,
                max,
                rolloff,
                bus,
            } => {
                w.u8(op::TRACK_NEW)
                    .u32(track)
                    .f32s(&pos)
                    .f32(min)
                    .f32(max)
                    .f32(rolloff)
                    .u8(bus);
            }
            Command::TrackPosition { track, pos } => {
                w.u8(op::TRACK_POSITION).u32(track).f32s(&pos);
            }
            Command::TrackDrop { track } => {
                w.u8(op::TRACK_DROP).u32(track);
            }
            Command::Play {
                voice,
                sound,
                target,
                db,
                rate,
                pan,
                looping,
            } => {
                match target {
                    Target::Bus(bus) => w.u8(op::PLAY_ON_BUS).u8(bus),
                    Target::Track(track) => w.u8(op::PLAY_ON_TRACK).u32(track),
                };
                w.u32(voice)
                    .u32(sound)
                    .f32(db)
                    .f32(rate)
                    .f32(pan)
                    .u8(looping as u8);
            }
            Command::OneShot {
                sound,
                pos,
                db,
                rate,
                min,
                max,
                rolloff,
                bus,
            } => {
                w.u8(op::ONE_SHOT)
                    .u32(sound)
                    .f32s(&pos)
                    .f32(db)
                    .f32(rate)
                    .f32(min)
                    .f32(max)
                    .f32(rolloff)
                    .u8(bus);
            }
            Command::VoiceVolume { voice, db, tween } => {
                w.u8(op::VOICE_VOLUME).u32(voice).f32(db).f32(tween);
            }
            Command::VoiceRate { voice, rate, tween } => {
                w.u8(op::VOICE_RATE).u32(voice).f32(rate).f32(tween);
            }
            Command::VoiceStop { voice, tween } => {
                w.u8(op::VOICE_STOP).u32(voice).f32(tween);
            }
            Command::VoiceDrop { voice } => {
                w.u8(op::VOICE_DROP).u32(voice);
            }
            Command::VoicePanning { voice, pan, tween } => {
                w.u8(op::VOICE_PANNING).u32(voice).f32(pan).f32(tween);
            }
            Command::VoicePause { voice, tween } => {
                w.u8(op::VOICE_PAUSE).u32(voice).f32(tween);
            }
            Command::VoiceResume { voice, tween } => {
                w.u8(op::VOICE_RESUME).u32(voice).f32(tween);
            }
            Command::ClearOneShots => {
                w.u8(op::CLEAR_ONE_SHOTS);
            }
            Command::VoiceWatch { voice } => {
                w.u8(op::VOICE_WATCH).u32(voice);
            }
            Command::ClockNew {
                clock,
                ticks_per_second,
            } => {
                w.u8(op::CLOCK_NEW).u32(clock).f64(ticks_per_second);
            }
            Command::ClockStart { clock } => {
                w.u8(op::CLOCK_START).u32(clock);
            }
            Command::ClockDrop { clock } => {
                w.u8(op::CLOCK_DROP).u32(clock);
            }
            Command::StemBusNew { track } => {
                w.u8(op::STEM_BUS_NEW).u32(track);
            }
            Command::StemBusVolume {
                track,
                db,
                at,
                tween,
            } => {
                w.u8(op::STEM_BUS_VOLUME)
                    .u32(track)
                    .f32(db)
                    .at(at)
                    .f32(tween);
            }
            Command::StemBusCutoff {
                track,
                hz,
                at,
                tween,
            } => {
                w.u8(op::STEM_BUS_CUTOFF)
                    .u32(track)
                    .f32(hz)
                    .at(at)
                    .f32(tween);
            }
            Command::StemBusDrop { track } => {
                w.u8(op::STEM_BUS_DROP).u32(track);
            }
            Command::StemPlay {
                voice,
                sound,
                track,
                from,
                at,
                db,
                rate,
            } => {
                w.u8(op::STEM_PLAY)
                    .u32(voice)
                    .u32(sound)
                    .u32(track)
                    .u64(from)
                    .at(at)
                    .f32(db)
                    .f32(rate);
            }
            Command::VoiceVolumeAt {
                voice,
                db,
                at,
                tween,
            } => {
                w.u8(op::VOICE_VOLUME_AT)
                    .u32(voice)
                    .f32(db)
                    .at(at)
                    .f32(tween);
            }
            Command::VoiceRateAt {
                voice,
                rate,
                at,
                tween,
            } => {
                w.u8(op::VOICE_RATE_AT)
                    .u32(voice)
                    .f32(rate)
                    .at(at)
                    .f32(tween);
            }
            Command::VoiceStopAt { voice, at, tween } => {
                w.u8(op::VOICE_STOP_AT).u32(voice).at(at).f32(tween);
            }
            Command::SoundDrop { sound } => {
                w.u8(op::SOUND_DROP).u32(sound);
            }
        }
    }
}

/// Reads the commands of one batch in order. It stops at the first byte it
/// can't read (a truncated or unknown command) and keeps it in
/// [`error`](Self::error).
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    pub error: Option<usize>,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            at: 0,
            error: None,
        }
    }

    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let end = self.at.checked_add(N)?;
        let b = self.bytes.get(self.at..end)?.try_into().ok()?;
        self.at = end;
        Some(b)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take::<1>().map(|b| b[0])
    }
    fn u32(&mut self) -> Option<u32> {
        self.take().map(u32::from_le_bytes)
    }
    fn f32(&mut self) -> Option<f32> {
        self.take().map(f32::from_le_bytes)
    }
    fn vec3(&mut self) -> Option<[f32; 3]> {
        Some([self.f32()?, self.f32()?, self.f32()?])
    }
    fn u64(&mut self) -> Option<u64> {
        self.take().map(u64::from_le_bytes)
    }
    fn f64(&mut self) -> Option<f64> {
        self.take().map(f64::from_le_bytes)
    }
    fn at(&mut self) -> Option<At> {
        Some(At {
            clock: self.u32()?,
            tick: self.u64()?,
        })
    }

    fn command(&mut self) -> Option<Command> {
        Some(match self.u8()? {
            op::FILTER_CUTOFF => Command::FilterCutoff {
                hz: self.f32()?,
                tween: self.f32()?,
            },
            op::BUS_VOLUME => Command::BusVolume {
                bus: self.u8()?,
                db: self.f32()?,
                tween: self.f32()?,
            },
            op::BUS_NEW => Command::BusNew {
                bus: self.u8()?,
                parent: self.u8()?,
                db: self.f32()?,
                lowpass: self.f32()?,
            },
            op::BUS_LOWPASS => Command::BusLowpass {
                bus: self.u8()?,
                hz: self.f32()?,
                tween: self.f32()?,
            },
            op::LISTENER => Command::Listener {
                pos: self.vec3()?,
                quat: [self.f32()?, self.f32()?, self.f32()?, self.f32()?],
            },
            op::TRACK_NEW => Command::TrackNew {
                track: self.u32()?,
                pos: self.vec3()?,
                min: self.f32()?,
                max: self.f32()?,
                rolloff: self.f32()?,
                bus: self.u8()?,
            },
            op::TRACK_POSITION => Command::TrackPosition {
                track: self.u32()?,
                pos: self.vec3()?,
            },
            op::TRACK_DROP => Command::TrackDrop { track: self.u32()? },
            code @ (op::PLAY_ON_BUS | op::PLAY_ON_TRACK) => {
                let target = if code == op::PLAY_ON_BUS {
                    Target::Bus(self.u8()?)
                } else {
                    Target::Track(self.u32()?)
                };
                Command::Play {
                    voice: self.u32()?,
                    sound: self.u32()?,
                    target,
                    db: self.f32()?,
                    rate: self.f32()?,
                    pan: self.f32()?,
                    looping: self.u8()? != 0,
                }
            }
            op::ONE_SHOT => Command::OneShot {
                sound: self.u32()?,
                pos: self.vec3()?,
                db: self.f32()?,
                rate: self.f32()?,
                min: self.f32()?,
                max: self.f32()?,
                rolloff: self.f32()?,
                bus: self.u8()?,
            },
            op::VOICE_VOLUME => Command::VoiceVolume {
                voice: self.u32()?,
                db: self.f32()?,
                tween: self.f32()?,
            },
            op::VOICE_RATE => Command::VoiceRate {
                voice: self.u32()?,
                rate: self.f32()?,
                tween: self.f32()?,
            },
            op::VOICE_STOP => Command::VoiceStop {
                voice: self.u32()?,
                tween: self.f32()?,
            },
            op::VOICE_DROP => Command::VoiceDrop { voice: self.u32()? },
            op::VOICE_PANNING => Command::VoicePanning {
                voice: self.u32()?,
                pan: self.f32()?,
                tween: self.f32()?,
            },
            op::VOICE_PAUSE => Command::VoicePause {
                voice: self.u32()?,
                tween: self.f32()?,
            },
            op::VOICE_RESUME => Command::VoiceResume {
                voice: self.u32()?,
                tween: self.f32()?,
            },
            op::CLEAR_ONE_SHOTS => Command::ClearOneShots,
            op::VOICE_WATCH => Command::VoiceWatch { voice: self.u32()? },
            op::CLOCK_NEW => Command::ClockNew {
                clock: self.u32()?,
                ticks_per_second: self.f64()?,
            },
            op::CLOCK_START => Command::ClockStart { clock: self.u32()? },
            op::CLOCK_DROP => Command::ClockDrop { clock: self.u32()? },
            op::STEM_BUS_NEW => Command::StemBusNew { track: self.u32()? },
            op::STEM_BUS_VOLUME => Command::StemBusVolume {
                track: self.u32()?,
                db: self.f32()?,
                at: self.at()?,
                tween: self.f32()?,
            },
            op::STEM_BUS_CUTOFF => Command::StemBusCutoff {
                track: self.u32()?,
                hz: self.f32()?,
                at: self.at()?,
                tween: self.f32()?,
            },
            op::STEM_BUS_DROP => Command::StemBusDrop { track: self.u32()? },
            op::STEM_PLAY => Command::StemPlay {
                voice: self.u32()?,
                sound: self.u32()?,
                track: self.u32()?,
                from: self.u64()?,
                at: self.at()?,
                db: self.f32()?,
                rate: self.f32()?,
            },
            op::VOICE_VOLUME_AT => Command::VoiceVolumeAt {
                voice: self.u32()?,
                db: self.f32()?,
                at: self.at()?,
                tween: self.f32()?,
            },
            op::VOICE_RATE_AT => Command::VoiceRateAt {
                voice: self.u32()?,
                rate: self.f32()?,
                at: self.at()?,
                tween: self.f32()?,
            },
            op::VOICE_STOP_AT => Command::VoiceStopAt {
                voice: self.u32()?,
                at: self.at()?,
                tween: self.f32()?,
            },
            op::SOUND_DROP => Command::SoundDrop { sound: self.u32()? },
            _ => return None,
        })
    }
}

impl Iterator for Reader<'_> {
    type Item = Command;

    fn next(&mut self) -> Option<Command> {
        if self.at >= self.bytes.len() || self.error.is_some() {
            return None;
        }
        let start = self.at;
        let command = self.command();
        if command.is_none() {
            self.error = Some(start);
        }
        command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_command() -> Vec<Command> {
        vec![
            Command::FilterCutoff {
                hz: 800.0,
                tween: 0.016,
            },
            Command::BusVolume {
                bus: BUS_MUSIC,
                db: -6.0,
                tween: 0.016,
            },
            Command::BusNew {
                bus: 2,
                parent: BUS_MAIN,
                db: -3.0,
                lowpass: 20_000.0,
            },
            Command::BusNew {
                bus: 3,
                parent: BUS_SFX,
                db: 0.0,
                lowpass: 2_500.0,
            },
            Command::BusLowpass {
                bus: BUS_SFX,
                hz: 800.0,
                tween: 0.016,
            },
            Command::Listener {
                pos: [1.0, 2.0, 3.0],
                quat: [0.0, 0.0, 0.0, 1.0],
            },
            Command::TrackNew {
                track: 7,
                pos: [-5.0, 1.0, -3.0],
                min: 1.0,
                max: 25.0,
                rolloff: 2.0,
                bus: BUS_SFX,
            },
            Command::TrackPosition {
                track: 7,
                pos: [0.5, 0.5, 0.5],
            },
            Command::Play {
                voice: 3,
                sound: 1,
                target: Target::Track(7),
                db: -1.5,
                rate: 1.25,
                pan: 0.0,
                looping: true,
            },
            Command::Play {
                voice: 4,
                sound: 2,
                target: Target::Bus(BUS_MUSIC),
                db: 0.0,
                rate: 1.0,
                pan: -0.4,
                looping: false,
            },
            Command::VoicePanning {
                voice: 4,
                pan: 0.7,
                tween: 0.016,
            },
            Command::VoicePause {
                voice: 3,
                tween: 0.1,
            },
            Command::VoiceResume {
                voice: 3,
                tween: 0.1,
            },
            Command::OneShot {
                sound: 2,
                pos: [9.0, 0.0, 9.0],
                db: -3.0,
                rate: 0.9,
                min: 2.0,
                max: 80.0,
                rolloff: 0.6,
                bus: BUS_SFX,
            },
            Command::VoiceVolume {
                voice: 3,
                db: -60.0,
                tween: 5.0,
            },
            Command::VoiceRate {
                voice: 3,
                rate: 0.5,
                tween: 0.016,
            },
            Command::VoiceStop {
                voice: 4,
                tween: 0.016,
            },
            Command::VoiceWatch { voice: 3 },
            Command::ClockNew {
                clock: 20,
                ticks_per_second: 48_000.0,
            },
            Command::ClockStart { clock: 20 },
            Command::StemBusNew { track: 21 },
            Command::StemBusVolume {
                track: 21,
                db: -6.0,
                at: At::tick(20, 96_000),
                tween: 0.5,
            },
            Command::StemBusCutoff {
                track: 21,
                hz: 800.0,
                at: At::NOW,
                tween: 0.01,
            },
            Command::StemPlay {
                voice: 22,
                sound: 2,
                track: 21,
                from: u64::from(u32::MAX) + 7,
                at: At::tick(20, 1 << 40),
                db: -80.0,
                rate: 1.0,
            },
            Command::VoiceVolumeAt {
                voice: 22,
                db: 0.0,
                at: At::tick(20, 9),
                tween: 2.0,
            },
            Command::VoiceRateAt {
                voice: 22,
                rate: 0.75,
                at: At::NOW,
                tween: 0.1,
            },
            Command::VoiceStopAt {
                voice: 22,
                at: At::tick(20, 10),
                tween: 0.05,
            },
            Command::StemBusDrop { track: 21 },
            Command::ClockDrop { clock: 20 },
            Command::SoundDrop { sound: 2 },
            Command::VoiceDrop { voice: 4 },
            Command::TrackDrop { track: 7 },
            Command::ClearOneShots,
        ]
    }

    #[test]
    fn a_batch_reads_back_as_written() {
        let commands = every_command();
        let mut bytes = Vec::new();
        for c in &commands {
            c.encode(&mut bytes);
        }
        let mut reader = Reader::new(&bytes);
        let read: Vec<Command> = reader.by_ref().collect();
        assert_eq!(read, commands);
        assert_eq!(reader.error, None);
    }

    /// Every op the encoder writes is one the reader takes: a new `Command`
    /// needs its arm in `encode` and in the worklet's `Mixer::command` (or
    /// nothing compiles), and its op, its arm in the reader and an example
    /// above, or this fails.
    #[test]
    fn every_op_reads_back() {
        let mut ops: Vec<u8> = every_command()
            .iter()
            .map(|c| {
                let mut bytes = Vec::new();
                c.encode(&mut bytes);
                bytes[0]
            })
            .collect();
        ops.sort();
        ops.dedup();
        assert_eq!(ops, (1..=op::LAST).collect::<Vec<_>>());
    }

    #[test]
    fn a_cut_or_unknown_command_stops_the_batch_where_it_starts() {
        let mut bytes = Vec::new();
        Command::ClearOneShots.encode(&mut bytes);
        let good = bytes.len();
        Command::TrackDrop { track: 1 }.encode(&mut bytes);
        bytes.pop();
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.by_ref().count(), 1);
        assert_eq!(reader.error, Some(good));

        let mut reader = Reader::new(&[200, 1, 2]);
        assert_eq!(reader.by_ref().count(), 0);
        assert_eq!(reader.error, Some(0));
    }
}
