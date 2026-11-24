//! Flint's mixer in the browser's audio thread (#136).
//!
//! The web player mixed on the main thread: cpal's WebAudio host asks for
//! each buffer from an `onended` callback, so on a phone the mix took about
//! a third of the frame and every long frame made a buffer late. This crate
//! is a second, small wasm that runs kira inside an `AudioWorkletProcessor`
//! instead. It knows nothing of Flint: the page posts it sounds (as samples)
//! and one batch of [`protocol::Command`]s a frame, and the processor asks
//! it for 128 frames at a time.
//!
//! [`Mixer`] is plain Rust, tested natively; `exports` is the C ABI the
//! processor's JS calls (`crates/flint-audio/src/worklet.js`). The wasm is
//! built on its own, with only kira: `trunk` runs
//! `cargo build -p flint-audio-worklet --target wasm32-unknown-unknown
//! --profile worklet` before each build (crates/flint-player/Trunk.toml).

pub mod bus_filter;
pub mod protocol;

#[cfg(target_arch = "wasm32")]
mod exports;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kira::backend::{Backend, Renderer};
use kira::clock::{ClockHandle, ClockSpeed, ClockTime};
use kira::effect::filter::{FilterBuilder, FilterHandle, FilterMode};
use kira::listener::ListenerHandle;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::sound::{PlaybackPosition, PlaybackState};
use kira::track::{
    SpatialTrackBuilder, SpatialTrackDistances, SpatialTrackHandle, TrackBuilder, TrackHandle,
};
use kira::{
    AudioManager, AudioManagerSettings, Decibels, Easing, Frame, Panning, PlaybackRate, StartTime,
    Tween,
};

use bus_filter::{BusFilterBuilder, BusFilterHandle};
use protocol::{At, Command, Reader, Target};

/// A kira backend with no device: it hands the renderer back so the
/// worklet's `process` can drive it.
struct Capture {
    renderer: Arc<Mutex<Option<Renderer>>>,
}

impl Backend for Capture {
    type Settings = (u32, Arc<Mutex<Option<Renderer>>>);
    type Error = ();

    fn setup((rate, renderer): Self::Settings, _: usize) -> Result<(Self, u32), ()> {
        Ok((Self { renderer }, rate))
    }

    fn start(&mut self, renderer: Renderer) -> Result<(), ()> {
        *self.renderer.lock().map_err(|_| ())? = Some(renderer);
        Ok(())
    }
}

fn tween(secs: f32) -> Tween {
    Tween {
        duration: Duration::from_secs_f32(secs.max(0.0)),
        easing: Easing::Linear,
        ..Default::default()
    }
}

fn vec3(p: [f32; 3]) -> glam::Vec3 {
    glam::Vec3::from_array(p)
}

/// A bus's track: at `db`, through its own low-pass at `lowpass` Hz
/// (#1218), as `flint_audio`'s kira mixer makes one.
fn bus_track(db: f32, lowpass: f32) -> (TrackBuilder, BusFilterHandle) {
    let mut builder = TrackBuilder::new().volume(Decibels(db));
    let filter = builder.add_effect(BusFilterBuilder {
        cutoff: lowpass as f64,
    });
    (builder, filter)
}

/// A bus low-pass's cutoff when it is open, Hz.
pub const LOWPASS_OPEN: f32 = bus_filter::OPEN as f32;

/// One-shots are swept for finished ones every this many renders (128
/// frames each, so about every 0.1 s).
const SWEEP_EVERY: u32 = 32;

/// kira, set up as `flint_audio::AudioEngine` sets it up on the desktop (a
/// low-pass on the main track, music and SFX buses under it, and the
/// game's own buses after them, each with its own low-pass), driven by
/// commands instead of calls.
pub struct Mixer {
    manager: AudioManager<Capture>,
    renderer: Renderer,
    filter: FilterHandle,
    /// By number: [`protocol::BUS_SFX`], [`protocol::BUS_MUSIC`], then each
    /// [`Command::BusNew`] in turn (#730), with each bus's low-pass (#1218).
    buses: Vec<(TrackHandle, BusFilterHandle)>,
    listener: Option<ListenerHandle>,
    sounds: HashMap<u32, StaticSoundData>,
    tracks: HashMap<u32, SpatialTrackHandle>,
    voices: HashMap<u32, StaticSoundHandle>,
    one_shots: Vec<(SpatialTrackHandle, StaticSoundHandle)>,
    /// Clocked content's clocks and stem buses (#1216), in the order made
    /// (a report lists the clocks so).
    clocks: Vec<(u32, ClockHandle)>,
    stem_buses: HashMap<u32, (TrackHandle, FilterHandle)>,
    /// Voices whose places are reported (#658), in the order asked for.
    watched: Vec<u32>,
    report: Vec<f64>,
    renders: u32,
    /// Commands that failed (an unknown id, a full track, an unreadable
    /// batch). The processor reports the count with its buffer stats.
    pub errors: u32,
    interleaved: Vec<f32>,
    planar: Vec<f32>,
}

impl Mixer {
    pub fn new(sample_rate: u32) -> Option<Self> {
        let slot = Arc::new(Mutex::new(None));
        let mut settings = AudioManagerSettings::<Capture> {
            backend_settings: (sample_rate, slot.clone()),
            ..Default::default()
        };
        let filter = settings.main_track_builder.add_effect(
            FilterBuilder::new()
                .mode(FilterMode::LowPass)
                .cutoff(20_000.0),
        );
        let mut manager = AudioManager::new(settings).ok()?;
        let mut builtin = || {
            let (track, filter) = bus_track(0.0, LOWPASS_OPEN);
            Some((manager.add_sub_track(track).ok()?, filter))
        };
        let sfx = builtin()?;
        let music = builtin()?;
        let renderer = slot.lock().ok()?.take()?;
        Some(Self {
            manager,
            renderer,
            filter,
            buses: vec![sfx, music],
            listener: None,
            sounds: HashMap::new(),
            tracks: HashMap::new(),
            voices: HashMap::new(),
            one_shots: Vec::new(),
            clocks: Vec::new(),
            stem_buses: HashMap::new(),
            watched: Vec::new(),
            report: Vec::new(),
            renders: 0,
            errors: 0,
            interleaved: Vec::new(),
            planar: Vec::new(),
        })
    }

    /// Keep a sound under `id`, from one channel of samples (played on both
    /// sides) or two interleaved. Replaces any sound with that id; voices
    /// already playing keep the old samples.
    pub fn load_sound(&mut self, id: u32, sample_rate: u32, channels: u32, samples: &[f32]) {
        let frames: Arc<[Frame]> = match channels {
            1 => samples.iter().map(|&s| Frame::from_mono(s)).collect(),
            2 => samples
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&[left, right]| Frame { left, right })
                .collect(),
            _ => {
                self.errors += 1;
                return;
            }
        };
        self.sounds.insert(
            id,
            StaticSoundData {
                sample_rate,
                frames,
                settings: Default::default(),
                slice: None,
            },
        );
    }

    /// Apply one batch of commands, in order.
    pub fn apply(&mut self, batch: &[u8]) {
        let mut reader = Reader::new(batch);
        for command in reader.by_ref() {
            if self.command(command).is_none() {
                self.errors += 1;
            }
        }
        if reader.error.is_some() {
            self.errors += 1;
        }
    }

    fn sound(&self, id: u32, db: f32, rate: f32, looping: bool) -> Option<StaticSoundData> {
        let mut data = self
            .sounds
            .get(&id)?
            .volume(Decibels(db))
            .playback_rate(PlaybackRate(rate as f64));
        if looping {
            data = data.loop_region(..);
        }
        Some(data)
    }

    fn bus(&mut self, bus: u8) -> Option<&mut TrackHandle> {
        self.buses.get_mut(bus as usize).map(|(track, _)| track)
    }

    /// A clocked change's start (#1216): at once, or on a tick of a clock
    /// made here (`None`: no such clock).
    fn start(&self, at: At) -> Option<StartTime> {
        if at.clock == 0 {
            return Some(StartTime::Immediate);
        }
        let (_, clock) = self.clocks.iter().find(|(id, _)| *id == at.clock)?;
        Some(StartTime::ClockTime(ClockTime::from_ticks_u64(
            clock.id(),
            at.tick,
        )))
    }

    fn timed(&self, at: At, secs: f32) -> Option<Tween> {
        Some(Tween {
            start_time: self.start(at)?,
            ..tween(secs)
        })
    }

    fn stem_bus(&mut self, track: u32) -> Option<&mut (TrackHandle, FilterHandle)> {
        self.stem_buses.get_mut(&track)
    }

    fn spatial_track(
        &mut self,
        pos: [f32; 3],
        (min, max): (f32, f32),
        rolloff: f32,
        bus: u8,
    ) -> Option<SpatialTrackHandle> {
        let listener = self.listener.as_ref()?.id();
        let builder = SpatialTrackBuilder::new()
            .distances(SpatialTrackDistances {
                min_distance: min,
                max_distance: max,
            })
            .attenuation_function(Some(Easing::OutPowf(rolloff.max(0.01) as f64)));
        self.bus(bus)?
            .add_spatial_sub_track(listener, vec3(pos), builder)
            .ok()
    }

    fn command(&mut self, command: Command) -> Option<()> {
        match command {
            Command::FilterCutoff { hz, tween: t } => {
                self.filter.set_cutoff(hz as f64, tween(t));
            }
            Command::BusVolume { bus, db, tween: t } => {
                self.bus(bus)?.set_volume(Decibels(db), tween(t));
            }
            Command::BusNew {
                bus,
                parent,
                db,
                lowpass,
            } => {
                // Numbered in order, so a number is never two buses.
                if bus as usize != self.buses.len() {
                    return None;
                }
                let (builder, filter) = bus_track(db, lowpass);
                let track = match parent {
                    protocol::BUS_MAIN => self.manager.add_sub_track(builder).ok()?,
                    parent => self.bus(parent)?.add_sub_track(builder).ok()?,
                };
                self.buses.push((track, filter));
            }
            Command::BusLowpass { bus, hz, tween: t } => {
                let (_, filter) = self.buses.get_mut(bus as usize)?;
                filter.set_cutoff(hz as f64, tween(t));
            }
            Command::Listener { pos, quat } => {
                let orientation = glam::Quat::from_array(quat);
                match &mut self.listener {
                    Some(l) => {
                        l.set_position(vec3(pos), tween(0.0));
                        l.set_orientation(orientation, tween(0.0));
                    }
                    None => {
                        self.listener =
                            Some(self.manager.add_listener(vec3(pos), orientation).ok()?);
                    }
                }
            }
            Command::TrackNew {
                track,
                pos,
                min,
                max,
                rolloff,
                bus,
            } => {
                let handle = self.spatial_track(pos, (min, max), rolloff, bus)?;
                self.tracks.insert(track, handle);
            }
            Command::TrackPosition { track, pos } => {
                self.tracks
                    .get_mut(&track)?
                    .set_position(vec3(pos), tween(0.0));
            }
            Command::TrackDrop { track } => {
                self.tracks.remove(&track)?;
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
                let data = self.sound(sound, db, rate, looping)?.panning(Panning(pan));
                let handle = match target {
                    Target::Bus(bus) => self.bus(bus)?.play(data).ok()?,
                    Target::Track(track) => self.tracks.get_mut(&track)?.play(data).ok()?,
                };
                self.voices.insert(voice, handle);
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
                let data = self.sound(sound, db, rate, false)?;
                let mut track = self.spatial_track(pos, (min, max), rolloff, bus)?;
                let handle = track.play(data).ok()?;
                self.one_shots.push((track, handle));
            }
            Command::VoiceVolume {
                voice,
                db,
                tween: t,
            } => {
                self.voices
                    .get_mut(&voice)?
                    .set_volume(Decibels(db), tween(t));
            }
            Command::VoiceRate {
                voice,
                rate,
                tween: t,
            } => {
                self.voices
                    .get_mut(&voice)?
                    .set_playback_rate(PlaybackRate(rate as f64), tween(t));
            }
            Command::VoiceStop { voice, tween: t } => {
                self.voices.get_mut(&voice)?.stop(tween(t));
            }
            Command::VoiceDrop { voice } => {
                self.watched.retain(|&v| v != voice);
                self.voices.remove(&voice)?;
            }
            Command::VoiceWatch { voice } => {
                self.voices.get(&voice)?;
                if !self.watched.contains(&voice) {
                    self.watched.push(voice);
                }
            }
            Command::VoicePanning {
                voice,
                pan,
                tween: t,
            } => {
                self.voices
                    .get_mut(&voice)?
                    .set_panning(Panning(pan), tween(t));
            }
            Command::VoicePause { voice, tween: t } => {
                self.voices.get_mut(&voice)?.pause(tween(t));
            }
            Command::VoiceResume { voice, tween: t } => {
                self.voices.get_mut(&voice)?.resume(tween(t));
            }
            Command::ClearOneShots => self.one_shots.clear(),
            Command::ClockNew {
                clock,
                ticks_per_second,
            } => {
                if clock == 0 || self.clocks.iter().any(|(id, _)| *id == clock) {
                    return None;
                }
                let handle = self
                    .manager
                    .add_clock(ClockSpeed::TicksPerSecond(ticks_per_second))
                    .ok()?;
                self.clocks.push((clock, handle));
            }
            Command::ClockStart { clock } => {
                let (_, handle) = self.clocks.iter_mut().find(|(id, _)| *id == clock)?;
                handle.start();
            }
            Command::ClockDrop { clock } => {
                let at = self.clocks.iter().position(|(id, _)| *id == clock)?;
                self.clocks.remove(at);
            }
            Command::StemBusNew { track } => {
                let mut builder = TrackBuilder::new();
                let filter = builder.add_effect(
                    FilterBuilder::new()
                        .mode(FilterMode::LowPass)
                        .cutoff(20_000.0),
                );
                let handle = self.manager.add_sub_track(builder).ok()?;
                self.stem_buses.insert(track, (handle, filter));
            }
            Command::StemBusVolume {
                track,
                db,
                at,
                tween: t,
            } => {
                let tween = self.timed(at, t)?;
                self.stem_bus(track)?.0.set_volume(Decibels(db), tween);
            }
            Command::StemBusCutoff {
                track,
                hz,
                at,
                tween: t,
            } => {
                let tween = self.timed(at, t)?;
                self.stem_bus(track)?.1.set_cutoff(hz as f64, tween);
            }
            Command::StemBusDrop { track } => {
                self.stem_buses.remove(&track)?;
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
                let start = self.start(at)?;
                let data = self
                    .sounds
                    .get(&sound)?
                    .start_position(PlaybackPosition::Samples(from as usize))
                    .start_time(start)
                    .volume(Decibels(db))
                    .playback_rate(PlaybackRate(rate as f64));
                let handle = self.stem_bus(track)?.0.play(data).ok()?;
                self.voices.insert(voice, handle);
            }
            Command::VoiceVolumeAt {
                voice,
                db,
                at,
                tween: t,
            } => {
                let tween = self.timed(at, t)?;
                self.voices.get_mut(&voice)?.set_volume(Decibels(db), tween);
            }
            Command::VoiceRateAt {
                voice,
                rate,
                at,
                tween: t,
            } => {
                let tween = self.timed(at, t)?;
                self.voices
                    .get_mut(&voice)?
                    .set_playback_rate(PlaybackRate(rate as f64), tween);
            }
            Command::VoiceStopAt {
                voice,
                at,
                tween: t,
            } => {
                let tween = self.timed(at, t)?;
                self.voices.get_mut(&voice)?.stop(tween);
            }
            Command::SoundDrop { sound } => {
                self.sounds.remove(&sound)?;
            }
        }
        Some(())
    }

    /// Mix the next `frames` frames: left channel, then right.
    pub fn render(&mut self, frames: usize) -> &[f32] {
        self.renders = self.renders.wrapping_add(1);
        if self.renders.is_multiple_of(SWEEP_EVERY) {
            self.one_shots
                .retain(|(_, sound)| sound.state() != PlaybackState::Stopped);
        }
        self.interleaved.resize(frames * 2, 0.0);
        self.renderer.on_start_processing();
        self.renderer.process(&mut self.interleaved, 2);
        self.planar.resize(frames * 2, 0.0);
        let (left, right) = self.planar.split_at_mut(frames);
        for (i, &[l, r]) in self.interleaved.as_chunks::<2>().0.iter().enumerate() {
            left[i] = l;
            right[i] = r;
        }
        &self.planar
    }

    /// Where each watched voice is now, [`protocol::REPORT_STRIDE`] `f64`s
    /// a voice (#658): its id, seconds into its sound and its playback
    /// state. Read after a [`render`](Self::render), it is where the next
    /// render starts. Then each clock (#1216): its id, its ticks with the
    /// fraction and [`protocol::state::CLOCK`] (`CLOCK_STOPPED` before it
    /// starts). Empty when nothing is watched and there is no clock.
    pub fn report(&mut self) -> &[f64] {
        self.report.clear();
        for (id, clock) in &self.clocks {
            let time = clock.time();
            let state = if clock.ticking() {
                protocol::state::CLOCK
            } else {
                protocol::state::CLOCK_STOPPED
            };
            self.report.extend_from_slice(&[
                *id as f64,
                time.ticks as f64 + time.fraction,
                state as f64,
            ]);
        }
        for &voice in &self.watched {
            let Some(handle) = self.voices.get(&voice) else {
                continue;
            };
            self.report.extend_from_slice(&[
                voice as f64,
                handle.position(),
                handle.state() as u8 as f64,
            ]);
        }
        &self.report
    }

    /// How many `f64`s the last [`report`](Self::report) wrote.
    pub fn report_len(&self) -> usize {
        self.report.len()
    }

    /// Voices the page still holds handles to, and placed one-shots playing.
    pub fn counts(&self) -> (usize, usize, usize) {
        (self.voices.len(), self.tracks.len(), self.one_shots.len())
    }
}

#[cfg(test)]
mod tests {
    use super::protocol::*;
    use super::*;

    const RATE: u32 = 48_000;
    const QUANTUM: usize = 128;

    fn batch(commands: &[Command]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for c in commands {
            c.encode(&mut bytes);
        }
        bytes
    }

    /// A 440 Hz tone, mono, a second long.
    fn tone() -> Vec<f32> {
        (0..44_100)
            .map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 44_100.0).sin() * 0.5)
            .collect()
    }

    /// RMS of each side over `quanta` renders.
    fn loudness(m: &mut Mixer, quanta: usize) -> (f32, f32) {
        let (mut l, mut r) = (0.0, 0.0);
        for _ in 0..quanta {
            let out = m.render(QUANTUM);
            l += out[..QUANTUM].iter().map(|s| s * s).sum::<f32>();
            r += out[QUANTUM..].iter().map(|s| s * s).sum::<f32>();
        }
        let n = (quanta * QUANTUM) as f32;
        ((l / n).sqrt(), (r / n).sqrt())
    }

    fn mixer_with_tone() -> Mixer {
        let mut m = Mixer::new(RATE).expect("a mixer without a device");
        m.load_sound(1, 44_100, 1, &tone());
        m.apply(&batch(&[Command::Listener {
            pos: [0.0, 0.0, 0.0],
            quat: [0.0, 0.0, 0.0, 1.0],
        }]));
        m
    }

    #[test]
    fn a_looping_sound_on_a_bus_plays_fades_and_stops() {
        let mut m = mixer_with_tone();
        assert_eq!(loudness(&mut m, 4), (0.0, 0.0), "silent before a play");
        m.apply(&batch(&[Command::Play {
            voice: 9,
            sound: 1,
            target: Target::Bus(BUS_MUSIC),
            db: 0.0,
            rate: 1.0,
            pan: 0.0,
            looping: true,
        }]));
        let (l, r) = loudness(&mut m, 40);
        assert!(l > 0.3 && (l - r).abs() < 1e-4, "{l} {r}");
        // Past the tone's one second, it loops.
        let (l, _) = loudness(&mut m, 400);
        assert!(l > 0.3, "{l}");

        m.apply(&batch(&[Command::BusVolume {
            bus: BUS_MUSIC,
            db: -20.0,
            tween: 0.0,
        }]));
        loudness(&mut m, 2);
        let (quiet, _) = loudness(&mut m, 40);
        assert!(
            (quiet / l - 0.1).abs() < 0.02,
            "-20 dB is a tenth: {quiet} vs {l}"
        );

        m.apply(&batch(&[Command::VoiceStop {
            voice: 9,
            tween: 0.0,
        }]));
        loudness(&mut m, 4);
        assert_eq!(loudness(&mut m, 4), (0.0, 0.0));
        assert_eq!(m.errors, 0);
    }

    /// A game's buses (#730): made in number order, under the main track or
    /// another bus, whose volume scales theirs.
    #[test]
    fn a_games_bus_hangs_off_its_parent_and_is_made_in_order() {
        let mut m = mixer_with_tone();
        m.apply(&batch(&[
            // A number out of turn is refused, and makes nothing.
            Command::BusNew {
                bus: 3,
                parent: BUS_MAIN,
                db: 0.0,
                lowpass: LOWPASS_OPEN,
            },
            Command::BusNew {
                bus: 2,
                parent: BUS_SFX,
                db: -6.0,
                lowpass: LOWPASS_OPEN,
            },
            Command::Play {
                voice: 1,
                sound: 1,
                target: Target::Bus(2),
                db: 0.0,
                rate: 1.0,
                pan: 0.0,
                looping: true,
            },
        ]));
        assert_eq!(m.errors, 1, "bus 3 before bus 2");
        loudness(&mut m, 2);
        let (l, _) = loudness(&mut m, 40);
        // The tone's RMS is 0.5/sqrt(2); -6 dB is about half.
        assert!((l / 0.3536 - 0.501).abs() < 0.03, "-6 dB: {l}");

        // Its parent, sfx, turned down: the child goes with it.
        m.apply(&batch(&[Command::BusVolume {
            bus: BUS_SFX,
            db: -60.0,
            tween: 0.0,
        }]));
        loudness(&mut m, 2);
        let (quiet, _) = loudness(&mut m, 40);
        assert!(quiet < l * 0.01, "under a silent parent: {quiet}");
        // A bus the mixer doesn't have is an error, never a panic.
        m.apply(&batch(&[Command::BusVolume {
            bus: 9,
            db: 0.0,
            tween: 0.0,
        }]));
        assert_eq!(m.errors, 2);
    }

    /// A 3 kHz tone, mono, a second long: bright enough that an 800 Hz
    /// low-pass takes most of it.
    fn bright() -> Vec<f32> {
        (0..44_100)
            .map(|i| (i as f32 * 3000.0 * std::f32::consts::TAU / 44_100.0).sin() * 0.5)
            .collect()
    }

    /// #1218: each bus has its own low-pass. Turned down on sfx it
    /// muffles sfx (and a bus under it) and leaves music alone; a bus made
    /// with a cutoff starts there.
    #[test]
    fn a_bus_lowpass_muffles_its_bus_and_not_the_others() {
        let mut m = Mixer::new(RATE).unwrap();
        m.load_sound(1, 44_100, 1, &bright());
        let play = |voice, bus| Command::Play {
            voice,
            sound: 1,
            target: Target::Bus(bus),
            db: 0.0,
            rate: 1.0,
            pan: 0.0,
            looping: true,
        };
        let stop = |voice| Command::VoiceStop { voice, tween: 0.0 };
        // Each bus alone: what it sounds like open.
        let alone = |m: &mut Mixer, voice, bus| {
            m.apply(&batch(&[play(voice, bus)]));
            loudness(m, 4);
            let (l, _) = loudness(m, 40);
            m.apply(&batch(&[stop(voice)]));
            loudness(m, 4);
            l
        };
        m.apply(&batch(&[Command::BusNew {
            bus: 2,
            parent: BUS_SFX,
            db: 0.0,
            lowpass: LOWPASS_OPEN,
        }]));
        let open = alone(&mut m, 1, BUS_SFX);
        assert!((open - 0.3536).abs() < 0.01, "open, the tone: {open}");
        assert!((alone(&mut m, 2, BUS_MUSIC) - open).abs() < 0.005);

        m.apply(&batch(&[Command::BusLowpass {
            bus: BUS_SFX,
            hz: 800.0,
            tween: 0.0,
        }]));
        let sfx = alone(&mut m, 3, BUS_SFX);
        let child = alone(&mut m, 4, 2);
        let music = alone(&mut m, 5, BUS_MUSIC);
        // Two poles at 800 Hz: 3 kHz is down about 23 dB.
        assert!(sfx < open * 0.1, "sfx muffled: {sfx} vs {open}");
        assert!(child < open * 0.1, "and the bus under it: {child}");
        assert!((music - open).abs() < 0.005, "music untouched: {music}");

        // Open again, and a bus made muffled.
        m.apply(&batch(&[
            Command::BusLowpass {
                bus: BUS_SFX,
                hz: LOWPASS_OPEN,
                tween: 0.0,
            },
            Command::BusNew {
                bus: 3,
                parent: BUS_MAIN,
                db: 0.0,
                lowpass: 800.0,
            },
        ]));
        assert!((alone(&mut m, 6, BUS_SFX) - open).abs() < 0.005);
        assert!(alone(&mut m, 7, 3) < open * 0.1);
        // A bus the mixer doesn't have is an error.
        m.apply(&batch(&[Command::BusLowpass {
            bus: 9,
            hz: 800.0,
            tween: 0.0,
        }]));
        assert_eq!(m.errors, 1);
    }

    #[test]
    fn a_spatial_track_pans_to_its_side_and_falls_silent_past_max() {
        let mut m = mixer_with_tone();
        let track = |track, x: f32| Command::TrackNew {
            track,
            pos: [x, 0.0, 0.0],
            min: 1.0,
            max: 25.0,
            rolloff: 2.0,
            bus: BUS_SFX,
        };
        let play = |voice, track| Command::Play {
            voice,
            sound: 1,
            target: Target::Track(track),
            db: 0.0,
            rate: 1.0,
            pan: 0.0,
            looping: true,
        };
        m.apply(&batch(&[track(1, 3.0), play(1, 1)]));
        let (l, r) = loudness(&mut m, 40);
        assert!(r > l * 1.5, "+x is on the right: {l} {r}");
        m.apply(&batch(&[Command::TrackPosition {
            track: 1,
            pos: [-3.0, 0.0, 0.0],
        }]));
        loudness(&mut m, 4);
        let (l, r) = loudness(&mut m, 40);
        assert!(l > r * 1.5, "moved to -x: {l} {r}");

        m.apply(&batch(&[Command::TrackPosition {
            track: 1,
            pos: [0.0, 0.0, 40.0],
        }]));
        loudness(&mut m, 4);
        let (l, r) = loudness(&mut m, 40);
        assert!(l < 1e-3 && r < 1e-3, "past max_distance: {l} {r}");

        // Dropping the track takes its sound with it.
        m.apply(&batch(&[
            Command::TrackPosition {
                track: 1,
                pos: [2.0, 0.0, 0.0],
            },
            Command::VoiceDrop { voice: 1 },
            Command::TrackDrop { track: 1 },
        ]));
        loudness(&mut m, 4);
        assert_eq!(loudness(&mut m, 4), (0.0, 0.0));
        assert_eq!((m.counts(), m.errors), ((0, 0, 0), 0));
    }

    #[test]
    fn a_one_shot_plays_once_and_its_track_is_swept() {
        let mut m = mixer_with_tone();
        m.load_sound(2, 44_100, 1, &tone()[..4410]);
        m.apply(&batch(&[Command::OneShot {
            sound: 2,
            pos: [0.0, 0.0, -2.0],
            db: 0.0,
            rate: 1.0,
            min: 1.0,
            max: 25.0,
            rolloff: 2.0,
            bus: BUS_SFX,
        }]));
        assert_eq!(m.counts().2, 1);
        let (l, _) = loudness(&mut m, 10);
        assert!(l > 0.05, "{l}");
        // 0.1 s of sound, then a sweep.
        loudness(&mut m, 200);
        assert_eq!(m.counts().2, 0);
        assert_eq!(m.errors, 0);
    }

    #[test]
    fn a_watched_voice_reports_where_it_is_wrapping_at_its_loop() {
        let mut m = mixer_with_tone();
        let play = |voice| Command::Play {
            voice,
            sound: 1,
            target: Target::Bus(BUS_MUSIC),
            db: 0.0,
            rate: 1.0,
            pan: 0.0,
            looping: true,
        };
        m.apply(&batch(&[
            play(1),
            play(2),
            Command::VoiceWatch { voice: 2 },
        ]));
        assert_eq!(m.report()[..2], [2.0, 0.0], "not yet mixed");
        // 375 renders of 128 frames at 48 kHz: one second, the tone's
        // length; 75 more is 0.2 s into its second time round.
        for _ in 0..450 {
            m.render(QUANTUM);
        }
        let report: Vec<VoiceReport> = read_report(m.report()).collect();
        assert_eq!(report.len(), 1, "only the watched voice");
        assert_eq!(report[0].voice, 2);
        assert!(
            (report[0].position - 0.2).abs() < 0.01,
            "wrapped: {}",
            report[0].position
        );
        assert!(state::moving(report[0].state));

        m.apply(&batch(&[Command::VoicePause {
            voice: 2,
            tween: 0.0,
        }]));
        for _ in 0..10 {
            m.render(QUANTUM);
        }
        let held = m.report()[1];
        for _ in 0..40 {
            m.render(QUANTUM);
        }
        assert_eq!(m.report()[1], held, "a paused voice stays put");
        assert!(!state::moving(m.report()[2] as u8));

        // A dropped voice leaves the report, and an unknown one can't join.
        m.apply(&batch(&[
            Command::VoiceDrop { voice: 2 },
            Command::VoiceWatch { voice: 7 },
        ]));
        assert!(m.report().is_empty());
        assert_eq!(m.errors, 1);
    }

    /// Clocked content (#1216): a stem started on a tick of a clock that
    /// counts samples lands on that sample, a bus change timed on a later
    /// tick lands there, and the report carries the clock's ticks.
    #[test]
    fn a_stem_on_a_clock_tick_starts_on_that_sample() {
        let mut m = Mixer::new(RATE).unwrap();
        // A constant: every frame it plays is 0.5 on both sides
        m.load_sound(5, RATE, 1, &vec![0.5; RATE as usize]);
        m.apply(&batch(&[
            Command::ClockNew {
                clock: 1,
                ticks_per_second: RATE as f64,
            },
            Command::StemBusNew { track: 2 },
            Command::StemPlay {
                voice: 3,
                sound: 5,
                track: 2,
                from: 0,
                at: At::tick(1, 1000),
                db: 0.0,
                rate: 1.0,
            },
            // -inf at tick 2000, at once
            Command::StemBusVolume {
                track: 2,
                db: -100.0,
                at: At::tick(1, 2000),
                tween: 0.0,
            },
        ]));
        assert_eq!(
            read_report(m.report()).next().map(|r| (r.voice, r.state)),
            Some((1, state::CLOCK_STOPPED))
        );
        m.apply(&batch(&[Command::ClockStart { clock: 1 }]));
        let mut left = Vec::new();
        for _ in 0..24 {
            left.extend_from_slice(&m.render(QUANTUM)[..QUANTUM]);
        }
        let first = left.iter().position(|s| s.abs() > 1e-6).unwrap();
        let last = left.iter().rposition(|s| s.abs() > 1e-3).unwrap();
        // kira starts a sound and lands a tween at the start of the chunk
        // its clock reaches the tick in: within a quantum before it, and
        // the same on the desktop (flint-music's offline render is kira
        // at 128 frames too, and lands there exactly: flint-music's
        // tests/worklet_parity.rs)
        assert_eq!(first, 1000 / QUANTUM * QUANTUM, "{first}");
        // (a track's volume glides across the chunk it changes in)
        assert!((2000 - QUANTUM..2000).contains(&last), "{last}");
        let clock = read_report(m.report()).next().unwrap();
        assert_eq!(clock.state, state::CLOCK);
        assert!(
            (clock.position - (24 * QUANTUM) as f64).abs() <= QUANTUM as f64,
            "{}",
            clock.position
        );
        m.apply(&batch(&[
            Command::VoiceDrop { voice: 3 },
            Command::StemBusDrop { track: 2 },
            Command::ClockDrop { clock: 1 },
            Command::SoundDrop { sound: 5 },
            // gone: a second drop of each is an error
            Command::ClockStart { clock: 1 },
            Command::SoundDrop { sound: 5 },
        ]));
        assert!(m.report().is_empty());
        assert_eq!(m.errors, 2);
    }

    #[test]
    fn unknown_ids_and_bad_bytes_count_as_errors_and_change_nothing() {
        let mut m = Mixer::new(RATE).unwrap();
        m.apply(&batch(&[
            Command::Play {
                voice: 1,
                sound: 42,
                target: Target::Bus(BUS_SFX),
                db: 0.0,
                rate: 1.0,
                pan: 0.0,
                looping: false,
            },
            // No listener yet: a spatial track can't be made.
            Command::TrackNew {
                track: 1,
                pos: [0.0; 3],
                min: 1.0,
                max: 2.0,
                rolloff: 1.0,
                bus: BUS_SFX,
            },
            Command::VoiceStop {
                voice: 5,
                tween: 0.0,
            },
        ]));
        assert_eq!(m.errors, 3);
        m.apply(&[255]);
        assert_eq!(m.errors, 4);
        m.load_sound(1, 44_100, 3, &[0.0; 9]);
        assert_eq!(m.errors, 5);
        assert_eq!(loudness(&mut m, 2), (0.0, 0.0));
    }
}
