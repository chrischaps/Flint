//! Sounds a caller keeps a key for (#699): a script's `play_sound` handle,
//! which `stop_sound` and `set_sound_volume` name later.
//!
//! The engine holds each one's handle until it ends, so the caller holds
//! only a number. A sound on a bus is a [`Keyed`]; a placed one is a
//! [`OneShot`](super::OneShot) with its key, steered with the rest. The
//! same calls reach kira here and the worklet's mixer (`VoiceStop`,
//! `VoiceVolume`) through [`SoundHandle`].

use super::{amplitude_to_db, AudioEngine, Bus};
use crate::handle::SoundHandle;
use crate::pan::Falloff;
use flint_core::{Result, Vec3};
use kira::{Easing, Tween};
use std::time::Duration;
use web_time::Instant;

/// The shortest fade a stop or a volume change takes, so a cut never
/// clicks.
pub const MIN_FADE: Duration = Duration::from_millis(10);

/// How a keyed sound plays: its volume (0..1), pitch multiplier, whether it
/// loops until stopped, and its bus.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyedPlay {
    pub volume: f64,
    pub pitch: f64,
    pub looping: bool,
    pub bus: Bus,
}

impl Default for KeyedPlay {
    fn default() -> Self {
        Self {
            volume: 1.0,
            pitch: 1.0,
            looping: false,
            bus: Bus::SFX,
        }
    }
}

/// A sound on a bus, kept by its key until it ends.
pub(crate) struct Keyed {
    key: u64,
    sound: String,
    /// Its bus, which it counts as sounding on (#1218).
    pub(crate) bus: Bus,
    handle: SoundHandle,
    /// When a remote one has played out, or will have faded once stopped:
    /// the worklet reports no voice it isn't asked to watch.
    ends_at: Option<Instant>,
    looping: bool,
    stopped: bool,
}

impl Keyed {
    pub(crate) fn is_finished(&self, now: Instant) -> bool {
        match self.handle.local_state() {
            Some(state) => state == kira::sound::PlaybackState::Stopped,
            None => self.ends_at.is_some_and(|t| now >= t),
        }
    }

    /// Heard, or about to be: not stopped and not played out (#1218).
    pub(crate) fn is_sounding(&self, now: Instant) -> bool {
        !self.stopped && !self.is_finished(now)
    }

    fn stop(&mut self, fade: Duration, now: Instant) {
        self.handle.stop(tween(fade));
        self.stopped = true;
        let faded = now + fade;
        self.ends_at = Some(self.ends_at.map_or(faded, |t| t.min(faded)));
    }
}

/// A placed sound's fade to a new volume, followed once a frame.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Ramp {
    from: f64,
    to: f64,
    start: Instant,
    secs: f64,
}

impl Ramp {
    /// The volume at `now`, and whether the ramp is done.
    pub(crate) fn at(&self, now: Instant) -> (f64, bool) {
        let t = now.saturating_duration_since(self.start).as_secs_f64() / self.secs.max(1e-6);
        if t >= 1.0 {
            (self.to, true)
        } else {
            (self.from + (self.to - self.from) * t, false)
        }
    }
}

fn tween(fade: Duration) -> Tween {
    Tween {
        duration: fade.max(MIN_FADE),
        easing: Easing::Linear,
        ..Default::default()
    }
}

impl AudioEngine {
    /// Play a sound on its bus under `key`, the caller's name for it, to
    /// stop or fade later. A key already playing is stopped first, so one
    /// key is one sound.
    pub fn play_keyed(&mut self, key: u64, sound_name: &str, play: KeyedPlay) -> Result<()> {
        self.stop_keyed(key, Duration::ZERO);
        let handle = self.start_panned(
            sound_name,
            play.volume,
            0.0,
            play.pitch,
            play.looping,
            play.bus,
        )?;
        let sound = self.resolve_sound(sound_name).into_owned();
        let now = Instant::now();
        let ends_at = self.remote_end(&sound, play.pitch, play.looping, now);
        self.keyed.push(Keyed {
            key,
            sound,
            bus: play.bus,
            handle,
            ends_at,
            looping: play.looping,
            stopped: false,
        });
        Ok(())
    }

    /// Play a sound at a point under `key`: a panned voice on its bus,
    /// steered to the listener each frame like a one-shot, `distances`
    /// `(min, max)` metres and `rolloff` as for
    /// [`play_oneshot_at`](Self::play_oneshot_at). A one-shot out of
    /// earshot isn't started (stopping its key then does nothing); a loop
    /// always is.
    pub fn play_keyed_at(
        &mut self,
        key: u64,
        sound_name: &str,
        position: Vec3,
        distances: (f32, f32),
        rolloff: f32,
        play: KeyedPlay,
    ) -> Result<()> {
        self.stop_keyed(key, Duration::ZERO);
        let falloff = Falloff {
            min: distances.0,
            max: distances.1,
            rolloff,
        };
        self.place(sound_name, position, falloff, play, Some(key))
    }

    /// Stop the sound playing under `key`, fading over `fade` (at least
    /// [`MIN_FADE`]). Whether one was playing: a key that ended, was never
    /// started or is unknown does nothing.
    pub fn stop_keyed(&mut self, key: u64, fade: Duration) -> bool {
        let now = Instant::now();
        let fade = fade.max(MIN_FADE);
        if let Some(k) = self.keyed.iter_mut().find(|k| k.key == key && !k.stopped) {
            k.stop(fade, now);
            return true;
        }
        if let Some(o) = self
            .oneshots
            .iter_mut()
            .find(|o| o.key == Some(key) && !o.stolen)
        {
            o.handle.stop(tween(fade));
            o.stolen = true;
            let faded = now + fade;
            o.ends_at = Some(o.ends_at.map_or(faded, |t| t.min(faded)));
            return true;
        }
        false
    }

    /// Stop every keyed sound playing `sound_name` (as named to
    /// [`play_keyed`](Self::play_keyed): a bare stem finds its file the
    /// same way), fading over `fade`. How many were stopped. Sounds played
    /// with no key (`audio_source`s, triggers, one-shots) are left alone.
    pub fn stop_keyed_named(&mut self, sound_name: &str, fade: Duration) -> usize {
        let sound = self.resolve_sound(sound_name).into_owned();
        let keys: Vec<u64> = self
            .keyed
            .iter()
            .filter(|k| k.sound == sound && !k.stopped)
            .map(|k| k.key)
            .chain(
                self.oneshots
                    .iter()
                    .filter(|o| o.sound == sound && !o.stolen)
                    .filter_map(|o| o.key),
            )
            .collect();
        keys.into_iter()
            .filter(|&key| self.stop_keyed(key, fade))
            .count()
    }

    /// Fade the sound playing under `key` to `volume` (0..1) over `fade`
    /// (at least [`MIN_FADE`]). Whether one was playing.
    pub fn set_keyed_volume(&mut self, key: u64, volume: f64, fade: Duration) -> bool {
        let fade = fade.max(MIN_FADE);
        let master = self.master_volume;
        if let Some(k) = self.keyed.iter_mut().find(|k| k.key == key && !k.stopped) {
            k.handle
                .set_volume(amplitude_to_db(volume * master), tween(fade));
            return true;
        }
        let now = Instant::now();
        if let Some(o) = self
            .oneshots
            .iter_mut()
            .find(|o| o.key == Some(key) && !o.stolen)
        {
            let from = o.volume_now(now);
            o.ramp = Some(Ramp {
                from,
                to: volume,
                start: now,
                secs: fade.as_secs_f64(),
            });
            return true;
        }
        false
    }

    /// Whether a sound is playing under `key`: started, not stopped, not
    /// yet ended (a remote one by its length, which the worklet doesn't
    /// report).
    pub fn is_keyed_playing(&self, key: u64) -> bool {
        self.playing_keys().any(|k| k == key)
    }

    /// The key of every sound playing now, as
    /// [`is_keyed_playing`](Self::is_keyed_playing) says each one, for a
    /// host to hand its scripts once a frame (`is_sound_playing`, #1000).
    pub fn playing_keys(&self) -> impl Iterator<Item = u64> + '_ {
        let now = Instant::now();
        let on_a_bus = self
            .keyed
            .iter()
            .filter(move |k| !k.stopped && !k.is_finished(now))
            .map(|k| k.key);
        let placed = self
            .oneshots
            .iter()
            .filter(move |o| o.is_sounding(now))
            .filter_map(|o| o.key);
        on_a_bus.chain(placed)
    }

    /// Stop the keyed loops on a bus (for a scene transition): a placed
    /// one goes with [`clear_oneshots`](Self::clear_oneshots), and a keyed
    /// sound that ends by itself plays on, as a one-shot does.
    pub fn stop_keyed_loops(&mut self) {
        let now = Instant::now();
        for k in self.keyed.iter_mut().filter(|k| k.looping && !k.stopped) {
            k.stop(MIN_FADE, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::Link;

    /// A steady tone, long enough to outlast every check.
    fn tone() -> Vec<f32> {
        (0..44_100)
            .map(|i| (i as f32 * 0.0627).sin() * 0.5)
            .collect()
    }

    /// The worklet's mixer, fed what the engine wrote.
    struct Worklet {
        link: Link,
        mixer: flint_audio_worklet::Mixer,
    }

    impl Worklet {
        /// Deliver what the engine wrote, then the peak over the last half
        /// of the next `quanta` 128-frame blocks.
        fn loudness(&mut self, quanta: usize) -> f32 {
            let out = self.link.take();
            for l in &out.loads {
                self.mixer
                    .load_sound(l.id, l.sample_rate, l.channels, &l.samples);
            }
            self.mixer.apply(&out.batch);
            let mut peak = 0.0f32;
            for q in 0..quanta {
                let block = self.mixer.render(128);
                if q >= quanta / 2 {
                    peak = block.iter().fold(peak, |a, s| a.max(s.abs()));
                }
            }
            peak
        }
    }

    fn remote_engine() -> (AudioEngine, Worklet) {
        let mut engine = AudioEngine::silent();
        engine
            .load_sound_from_samples("audio/hum.ogg", 44_100, &[&tone()])
            .unwrap();
        let link = Link::default();
        engine.start_remote(link.clone());
        engine.create_listener(Vec3::new(0.0, 0.0, 0.0)).unwrap();
        let mixer = flint_audio_worklet::Mixer::new(48_000).unwrap();
        (engine, Worklet { link, mixer })
    }

    const LOOP: KeyedPlay = KeyedPlay {
        volume: 1.0,
        pitch: 1.0,
        looping: true,
        bus: Bus::SFX,
    };

    /// #699 through the web's mixer: what a script starts, it can stop,
    /// fade and stop by name, and the worklet goes quiet.
    #[test]
    fn a_keyed_sound_stops_and_fades_in_the_worklet() {
        let (mut engine, mut w) = remote_engine();
        engine.play_keyed(1, "hum", LOOP).unwrap();
        let on = w.loudness(40);
        assert!((on - 0.5).abs() < 0.05, "full volume: {on}");
        assert!(engine.is_keyed_playing(1));

        // Faded to a fifth over 50 ms.
        assert!(engine.set_keyed_volume(1, 0.2, Duration::from_millis(50)));
        let faded = w.loudness(60);
        assert!((faded - 0.1).abs() < 0.02, "a fifth: {faded}");

        assert!(engine.stop_keyed(1, Duration::ZERO));
        assert_eq!(w.loudness(20), 0.0, "stopped");
        assert!(!engine.is_keyed_playing(1));
        assert!(!engine.stop_keyed(1, Duration::ZERO), "once");
        assert!(!engine.stop_keyed(99, Duration::ZERO), "unknown");

        // Two by name, one of them placed; another sound plays on.
        engine.play_keyed(2, "audio/hum.ogg", LOOP).unwrap();
        let ahead = Vec3::new(0.0, 0.0, -0.5);
        engine
            .play_keyed_at(3, "hum", ahead, (1.0, 25.0), 2.0, LOOP)
            .unwrap();
        engine
            .load_sound_from_samples("other", 44_100, &[&tone()])
            .unwrap();
        engine.play_keyed(4, "other", LOOP).unwrap();
        assert!(w.loudness(40) > 0.9, "three at once");
        assert_eq!(engine.stop_keyed_named("hum", Duration::ZERO), 2);
        let left = w.loudness(40);
        assert!((left - 0.5).abs() < 0.05, "only 'other' left: {left}");
        assert!(engine.is_keyed_playing(4) && !engine.is_keyed_playing(3));
        assert_eq!(w.mixer.errors, 0);
    }

    /// A placed keyed sound fades along its ramp, which pan_oneshots
    /// follows each frame, and a placed loop far away still starts.
    #[test]
    fn a_placed_keyed_sound_ramps_and_a_far_loop_still_plays() {
        let (mut engine, mut w) = remote_engine();
        let ahead = Vec3::new(0.0, 0.0, -0.5);
        engine
            .play_keyed_at(7, "hum", ahead, (1.0, 25.0), 2.0, LOOP)
            .unwrap();
        let on = w.loudness(40);
        assert!(on > 0.3, "close by: {on}");
        assert!(engine.set_keyed_volume(7, 0.0, Duration::from_millis(10)));
        std::thread::sleep(Duration::from_millis(20));
        engine.pan_oneshots();
        assert!(w.loudness(40) < 1e-3, "faded out");
        assert!(engine.is_keyed_playing(7), "and still playing");

        let far = Vec3::new(100.0, 0.0, 0.0);
        engine
            .play_keyed_at(8, "hum", far, (1.0, 25.0), 2.0, LOOP)
            .unwrap();
        assert!(engine.is_keyed_playing(8), "a loop out of earshot is kept");
        engine
            .play_keyed_at(9, "hum", far, (1.0, 25.0), 2.0, KeyedPlay::default())
            .unwrap();
        assert!(
            !engine.is_keyed_playing(9),
            "a one-shot out of earshot isn't"
        );

        // A transition stops every loop: placed ones with the one-shots.
        engine.play_keyed(10, "hum", LOOP).unwrap();
        engine.clear_oneshots();
        engine.stop_keyed_loops();
        assert!(!engine.is_keyed_playing(7) && !engine.is_keyed_playing(10));
        assert_eq!(w.loudness(20), 0.0);
        assert_eq!(w.mixer.errors, 0);
    }

    /// A remote one-shot is forgotten once its length has passed; a loop
    /// only once stopped and faded.
    #[test]
    fn keyed_sounds_are_swept_when_they_end() {
        let (mut engine, _w) = remote_engine();
        engine.play_keyed(1, "hum", KeyedPlay::default()).unwrap();
        engine.play_keyed(2, "hum", LOOP).unwrap();
        assert!(engine.keyed[0].ends_at.is_some() && engine.keyed[1].ends_at.is_none());
        engine.keyed[0].ends_at = Some(Instant::now() - Duration::from_secs(1));
        engine.cleanup_finished_oneshots();
        assert_eq!(engine.keyed.len(), 1);
        engine.stop_keyed(2, Duration::ZERO);
        engine.keyed[0].ends_at = Some(Instant::now() - Duration::from_secs(1));
        engine.cleanup_finished_oneshots();
        assert!(engine.keyed.is_empty());
    }

    /// #699 on the desktop: kira itself, on the sound card. Skipped with no
    /// device.
    #[test]
    fn a_keyed_sound_stops_on_the_device() {
        use kira::sound::PlaybackState;
        let mut engine = AudioEngine::new();
        if !engine.is_available() {
            eprintln!("no audio device: keyed stop test skipped");
            return;
        }
        engine
            .load_sound_from_samples("hum", 44_100, &[&tone()])
            .unwrap();
        engine.create_listener(Vec3::new(0.0, 0.0, 0.0)).unwrap();
        let quiet = KeyedPlay {
            volume: 0.05,
            ..LOOP
        };
        engine.play_keyed(1, "hum", quiet).unwrap();
        engine
            .play_keyed_at(2, "hum", Vec3::new(1.0, 0.0, 0.0), (1.0, 25.0), 2.0, quiet)
            .unwrap();
        let state = |e: &AudioEngine, key: u64| {
            e.keyed
                .iter()
                .map(|k| (k.key, &k.handle))
                .chain(e.oneshots.iter().filter_map(|o| Some((o.key?, &o.handle))))
                .find(|(k, _)| *k == key)
                .and_then(|(_, h)| h.local_state())
        };
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(state(&engine, 1), Some(PlaybackState::Playing));
        assert_eq!(state(&engine, 2), Some(PlaybackState::Playing));
        assert_eq!(engine.stop_keyed_named("hum", Duration::ZERO), 2);
        let start = Instant::now();
        let both_stopped = Some(PlaybackState::Stopped);
        while state(&engine, 1) != both_stopped || state(&engine, 2) != both_stopped {
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "kira never stopped them"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        engine.cleanup_finished_oneshots();
        assert!(engine.keyed.is_empty() && engine.oneshots.is_empty());
    }

    /// The presentation's timeline (#1000): every 20 ms over 1.2 s, the
    /// worklet's loudness of a loop and a 0.4 s one-shot beside what
    /// `playing_keys` says of each; the loop is stopped at 0.6 s. Prints CSV.
    #[test]
    #[ignore]
    fn playing_keys_timeline() {
        let (mut engine, mut w) = remote_engine();
        let ding: Vec<f32> = tone().into_iter().take(17_640).collect();
        engine
            .load_sound_from_samples("ding", 44_100, &[&ding])
            .unwrap();
        engine.play_keyed(1, "hum", LOOP).unwrap();
        engine.play_keyed(2, "ding", KeyedPlay::default()).unwrap();
        println!("ms,mixer_peak,hum_playing,ding_playing");
        let start = Instant::now();
        // 20 ms of 48 kHz is 7.5 quanta; 8 renders keep the mixer near the clock
        for step in 0..60 {
            if step == 30 {
                engine.stop_keyed(1, Duration::ZERO);
            }
            let peak = w.loudness(8);
            let keys: Vec<u64> = engine.playing_keys().collect();
            println!(
                "{},{peak:.3},{},{}",
                start.elapsed().as_millis(),
                keys.contains(&1) as u8,
                keys.contains(&2) as u8
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// #1000 on the desktop: a keyed one-shot is among `playing_keys` while
    /// kira plays it and leaves once it has played out; a loop stays until
    /// stopped. Skipped with no device.
    #[test]
    fn a_keyed_one_shot_leaves_the_playing_keys_when_it_ends_on_the_device() {
        let mut engine = AudioEngine::new();
        if !engine.is_available() {
            eprintln!("no audio device: keyed one-shot test skipped");
            return;
        }
        let blip: Vec<f32> = tone().into_iter().take(4_410).collect();
        engine
            .load_sound_from_samples("blip", 44_100, &[&blip])
            .unwrap();
        engine
            .load_sound_from_samples("hum", 44_100, &[&tone()])
            .unwrap();
        engine.create_listener(Vec3::new(0.0, 0.0, 0.0)).unwrap();
        let quiet = KeyedPlay {
            volume: 0.05,
            ..KeyedPlay::default()
        };
        engine.play_keyed(1, "blip", quiet).unwrap();
        engine
            .play_keyed(
                2,
                "hum",
                KeyedPlay {
                    looping: true,
                    ..quiet
                },
            )
            .unwrap();
        let keys = |e: &AudioEngine| {
            let mut k: Vec<u64> = e.playing_keys().collect();
            k.sort();
            k
        };
        assert_eq!(keys(&engine), [1, 2], "both, as soon as they're played");
        let start = Instant::now();
        while engine.is_keyed_playing(1) {
            assert!(
                start.elapsed() < Duration::from_secs(2),
                "a 0.1 s blip never ended"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let took = start.elapsed();
        assert!(took >= Duration::from_millis(50), "it played: {took:?}");
        assert_eq!(keys(&engine), [2], "the loop plays on");
        engine.stop_keyed(2, Duration::ZERO);
        assert!(keys(&engine).is_empty(), "stopped, from the call on");
    }
}
