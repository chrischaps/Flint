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

pub mod protocol;

#[cfg(target_arch = "wasm32")]
mod exports;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kira::backend::{Backend, Renderer};
use kira::effect::filter::{FilterBuilder, FilterHandle, FilterMode};
use kira::listener::ListenerHandle;
use kira::sound::static_sound::{StaticSoundData, StaticSoundHandle};
use kira::sound::PlaybackState;
use kira::track::{
    SpatialTrackBuilder, SpatialTrackDistances, SpatialTrackHandle, TrackBuilder, TrackHandle,
};
use kira::{AudioManager, AudioManagerSettings, Decibels, Easing, Frame, PlaybackRate, Tween};

use protocol::{Command, Reader, Target};

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

/// One-shots are swept for finished ones every this many renders (128
/// frames each, so about every 0.1 s).
const SWEEP_EVERY: u32 = 32;

/// kira, set up as `flint_audio::AudioEngine` sets it up on the desktop (a
/// low-pass on the main track, music and SFX buses under it), driven by
/// commands instead of calls.
pub struct Mixer {
    manager: AudioManager<Capture>,
    renderer: Renderer,
    filter: FilterHandle,
    buses: [TrackHandle; 2],
    listener: Option<ListenerHandle>,
    sounds: HashMap<u32, StaticSoundData>,
    tracks: HashMap<u32, SpatialTrackHandle>,
    voices: HashMap<u32, StaticSoundHandle>,
    one_shots: Vec<(SpatialTrackHandle, StaticSoundHandle)>,
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
        let sfx = manager.add_sub_track(TrackBuilder::new()).ok()?;
        let music = manager.add_sub_track(TrackBuilder::new()).ok()?;
        let renderer = slot.lock().ok()?.take()?;
        Some(Self {
            manager,
            renderer,
            filter,
            buses: [sfx, music],
            listener: None,
            sounds: HashMap::new(),
            tracks: HashMap::new(),
            voices: HashMap::new(),
            one_shots: Vec::new(),
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
        self.buses.get_mut(bus as usize)
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
                looping,
            } => {
                let data = self.sound(sound, db, rate, looping)?;
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
                self.voices.remove(&voice)?;
            }
            Command::ClearOneShots => self.one_shots.clear(),
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
    fn unknown_ids_and_bad_bytes_count_as_errors_and_change_nothing() {
        let mut m = Mixer::new(RATE).unwrap();
        m.apply(&batch(&[
            Command::Play {
                voice: 1,
                sound: 42,
                target: Target::Bus(BUS_SFX),
                db: 0.0,
                rate: 1.0,
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
