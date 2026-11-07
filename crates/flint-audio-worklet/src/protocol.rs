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

/// The mixer bus a sound or a spatial track hangs off.
pub const BUS_SFX: u8 = 0;
pub const BUS_MUSIC: u8 = 1;

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
