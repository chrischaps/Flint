//! A bus ducking under another, and a low-pass on every bus (#1218).
//!
//! A duck needs no level metering: the engine knows what it started on
//! each bus (keyed sounds, placed one-shots, the unkeyed one-shots in
//! `tails`), and [`AudioSync`](crate::sync::AudioSync) knows its
//! `audio_source`s. Once a frame [`update_ducks`](AudioEngine::update_ducks)
//! asks which buses have something sounding (a sound on a child bus counts
//! for its parents), and a bus whose `under` bus does tweens down by its
//! duck over `attack`, and back over `release` once nothing does. The dip
//! is a bus volume, so it reaches either mixer through
//! `MixerBackend::set_bus_volume` like any other, multiplied with the
//! volume the game asked for.

use super::{AudioEngine, Bus};
use crate::bus::clamp_lowpass;
use flint_core::{FlintError, Result};
use web_time::Instant;

impl AudioEngine {
    /// A bus's own low-pass cutoff, Hz: 20000 is open, lower muffles what
    /// plays on it and on the buses under it, and nothing else. It holds
    /// until changed, and a mixer started later opens at it.
    pub fn set_bus_lowpass(&mut self, bus: Bus, hz: f64) {
        let Some(slot) = self.bus_lowpass.get_mut(bus.index()) else {
            tracing::warn!("[flint-audio] set_bus_lowpass: no bus {}", bus.index());
            return;
        };
        let hz = clamp_lowpass(hz);
        if *slot == hz {
            return;
        }
        *slot = hz;
        self.mixer.set_bus_lowpass(bus, hz as f32, 0.016);
    }

    /// [`set_bus_lowpass`](Self::set_bus_lowpass) by name (a script's
    /// `set_bus_lowpass("sfx", 800)`). A name that is no bus changes
    /// nothing and is reported once, naming the buses there are.
    pub fn set_bus_lowpass_named(&mut self, name: &str, hz: f64) -> Result<()> {
        let Some(bus) = self.buses.get(name) else {
            let names: Vec<&str> = self.buses.names().collect();
            let message = format!("no bus '{name}' (the buses are {})", names.join(", "));
            if self.unknown_buses.insert(name.to_string()) {
                tracing::warn!("[flint-audio] set_bus_lowpass: {message}");
            }
            return Err(FlintError::AudioError(message));
        };
        self.set_bus_lowpass(bus, hz);
        Ok(())
    }

    /// A bus's low-pass cutoff as last asked for, Hz.
    pub fn bus_lowpass(&self, bus: Bus) -> Option<f64> {
        self.bus_lowpass.get(bus.index()).copied()
    }

    /// Whether a bus is ducked now: its duck's `under` bus has something
    /// sounding, as of the last [`update_ducks`](Self::update_ducks).
    pub fn is_ducked(&self, bus: Bus) -> bool {
        self.ducked.get(bus.index()).copied().unwrap_or(false)
    }

    /// Each bus, by number: whether something is sounding on it or on a
    /// bus under it. `also` are the buses of sounds the engine doesn't
    /// hold, playing now (an `audio_source`'s).
    pub fn sounding_buses(&self, also: impl IntoIterator<Item = Bus>) -> Vec<bool> {
        let now = Instant::now();
        let held = self
            .keyed
            .iter()
            .filter(|k| k.is_sounding(now))
            .map(|k| k.bus)
            .chain(
                self.oneshots
                    .iter()
                    .filter(|o| o.is_sounding(now))
                    .map(|o| o.bus),
            )
            .chain(
                self.tails
                    .iter()
                    .filter(|&&(_, end)| now < end)
                    .map(|&(bus, _)| bus),
            );
        let mut sounding = vec![false; self.buses.len()];
        for bus in also.into_iter().chain(held) {
            for up in self.buses.with_parents(bus) {
                sounding[up.index()] = true;
            }
        }
        sounding
    }

    /// Once a frame: duck each bus whose `under` bus has something
    /// sounding, and bring back each whose hasn't, over the duck's attack
    /// or release. `also` as for [`sounding_buses`](Self::sounding_buses).
    pub fn update_ducks(&mut self, also: impl IntoIterator<Item = Bus>) {
        let sounding = self.sounding_buses(also);
        let turns: Vec<(Bus, bool, f64)> = self
            .buses
            .iter()
            .filter_map(|(bus, decl)| {
                let duck = decl.duck?;
                let down = sounding[duck.under.index()];
                (down != self.ducked[bus.index()]).then_some((
                    bus,
                    down,
                    if down { duck.attack } else { duck.release },
                ))
            })
            .collect();
        for (bus, down, tween) in turns {
            self.ducked[bus.index()] = down;
            self.send_bus_volume(bus, tween);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::KeyedPlay;
    use super::*;
    use crate::bus::Buses;
    use crate::remote::protocol::{Command, Reader};
    use crate::remote::Link;
    use std::time::Duration;

    fn tone() -> Vec<f32> {
        (0..44_100)
            .map(|i| (i as f32 * 0.0627).sin() * 0.5)
            .collect()
    }

    fn engine(manifest: &str) -> (AudioEngine, Link) {
        let mut engine = AudioEngine::silent();
        engine
            .load_sound_from_samples("line", 44_100, &[&tone()])
            .unwrap();
        // A tenth of a second.
        engine
            .load_sound_from_samples("blip", 44_100, &[&tone()[..4410]])
            .unwrap();
        engine.set_buses(&Buses::from_manifest(&manifest.parse().unwrap(), "flint.toml").unwrap());
        let link = Link::default();
        engine.start_remote(link.clone());
        link.take();
        (engine, link)
    }

    /// The bus volumes sent since the last look.
    fn volumes(link: &Link) -> Vec<(u8, f32, f32)> {
        let out = link.take();
        Reader::new(&out.batch)
            .filter_map(|c| match c {
                Command::BusVolume { bus, db, tween } => Some((bus, db, tween)),
                _ => None,
            })
            .collect()
    }

    const DUCKING: &str = r#"
        [audio.buses.music]
        volume = 0.5
        duck = { under = "voice", by_db = -12, attack = 0.08, release = 0.6 }
        [audio.buses.voice]
        [audio.buses.narrator]
        parent = "voice"
    "#;

    /// #1217 under #1218: the player's own volume is one more factor. A
    /// duck under a player's 0.4 is heard at game x player x duck, a slider
    /// moved while ducked keeps the duck, and the duck lets go to game x
    /// player, never to the game's alone.
    #[test]
    fn a_duck_rides_on_the_players_own_volume() {
        let (mut e, link) = engine(DUCKING);
        let voice = e.bus("voice").unwrap();
        e.set_user_bus_volume("music", 0.4).unwrap();
        let (_, db, _) = volumes(&link)[0];
        assert!((db - 20.0 * 0.2f32.log10()).abs() < 1e-3, "0.5 x 0.4");

        let play = KeyedPlay {
            bus: voice,
            looping: true,
            ..Default::default()
        };
        e.play_keyed(1, "line", play).unwrap();
        e.update_ducks([]);
        let (_, db, _) = volumes(&link)[0];
        assert!((db - (20.0 * 0.2f32.log10() - 12.0)).abs() < 1e-3, "{db}");

        // The slider moves while the line plays: still ducked.
        e.set_user_bus_volume("music", 0.8).unwrap();
        let (_, db, _) = volumes(&link)[0];
        assert!((db - (20.0 * 0.4f32.log10() - 12.0)).abs() < 1e-3, "{db}");

        e.stop_keyed(1, Duration::ZERO);
        e.update_ducks([]);
        let (_, db, _) = volumes(&link)[0];
        assert!((db - 20.0 * 0.4f32.log10()).abs() < 1e-3, "{db}");
        assert_eq!(e.user_bus_volume("music"), 0.8);
    }

    #[test]
    fn music_ducks_while_a_keyed_line_plays_and_comes_back_after() {
        let (mut e, link) = engine(DUCKING);
        let voice = e.bus("voice").unwrap();
        e.update_ducks([]);
        assert!(volumes(&link).is_empty(), "nothing plays: no duck");

        let play = KeyedPlay {
            bus: voice,
            looping: true,
            ..Default::default()
        };
        e.play_keyed(1, "line", play).unwrap();
        e.update_ducks([]);
        assert!(e.is_ducked(Bus::MUSIC));
        // 0.5, twelve decibels down, over the attack.
        let down = 20.0 * 0.5f32.log10() - 12.0;
        let sent = volumes(&link);
        assert_eq!(sent.len(), 1, "{sent:?}");
        let (bus, db, tween) = sent[0];
        assert_eq!(bus, Bus::MUSIC.code());
        assert!((db - down).abs() < 1e-3 && (tween - 0.08).abs() < 1e-6);
        e.update_ducks([]);
        assert!(volumes(&link).is_empty(), "sent once, not every frame");

        // The game turns music down meanwhile: the duck stays on top.
        e.set_bus_volume(Bus::MUSIC, 0.25);
        let (_, db, _) = volumes(&link)[0];
        assert!((db - (20.0 * 0.25f32.log10() - 12.0)).abs() < 1e-3);

        e.stop_keyed(1, Duration::ZERO);
        e.update_ducks([]);
        assert!(!e.is_ducked(Bus::MUSIC));
        let (_, db, tween) = volumes(&link)[0];
        assert!((db - 20.0 * 0.25f32.log10()).abs() < 1e-3 && (tween - 0.6).abs() < 1e-6);
    }

    #[test]
    fn a_sound_on_a_bus_under_voice_and_an_unkeyed_one_shot_duck_it_too() {
        let (mut e, link) = engine(DUCKING);
        let narrator = e.bus("narrator").unwrap();
        // A one-shot nobody keeps a key for (an extension's), a tenth of a
        // second, on the bus under voice.
        e.play_non_spatial("blip", 1.0, 1.0, false, narrator)
            .unwrap();
        e.update_ducks([]);
        assert!(e.is_ducked(Bus::MUSIC), "a child's sound is its parent's");
        std::thread::sleep(Duration::from_millis(130));
        e.cleanup_finished_oneshots();
        e.update_ducks([]);
        assert!(!e.is_ducked(Bus::MUSIC), "back once it has played out");
        assert_eq!(volumes(&link).len(), 2);

        // What the host says is sounding (an audio_source's bus) counts.
        let voice = e.bus("voice").unwrap();
        e.update_ducks([voice]);
        assert!(e.is_ducked(Bus::MUSIC));
        // Sfx sounding is nothing to music.
        e.update_ducks([Bus::SFX]);
        assert!(!e.is_ducked(Bus::MUSIC));
    }

    // ── Both mixers, heard (#1218's "on both mixers") ──

    use super::super::backend::Unhosted;
    use super::super::local::{Hosts, Local};
    use kira::backend::{Backend, Renderer};
    use std::sync::{Arc, Mutex};

    /// A kira backend with no device that hands the renderer back, so the
    /// main thread's mixer ([`Local`]) can be rendered by hand.
    struct Capture(Arc<Mutex<Option<Renderer>>>);

    impl Backend for Capture {
        type Settings = Arc<Mutex<Option<Renderer>>>;
        type Error = &'static str;
        fn setup(slot: Self::Settings, _: usize) -> std::result::Result<(Self, u32), Self::Error> {
            Ok((Self(slot), 48_000))
        }
        fn start(&mut self, renderer: Renderer) -> std::result::Result<(), Self::Error> {
            *self.0.lock().unwrap() = Some(renderer);
            Ok(())
        }
    }

    impl Hosts for Capture {
        fn lend(
            _: &mut kira::AudioManager<Self>,
        ) -> std::result::Result<&mut dyn crate::clocked::ClockedMixer, Unhosted> {
            Err(Unhosted::Silent)
        }
    }

    /// Where an engine's mix is heard: kira here, or the browser's worklet
    /// mixer fed what the engine wrote down its link.
    enum Ear {
        Here(Box<Renderer>),
        Worklet(Link, Box<flint_audio_worklet::Mixer>),
    }

    impl Ear {
        /// RMS of the left side over the last half of `quanta` 128-frame
        /// renders, after handing the mixer what was sent.
        fn loudness(&mut self, quanta: usize) -> f32 {
            let mut buf = vec![0.0f32; 256];
            let (mut sum, mut n) = (0.0f32, 0usize);
            for q in 0..quanta {
                let left: Vec<f32> = match self {
                    Ear::Here(r) => {
                        r.on_start_processing();
                        r.process(&mut buf, 2);
                        buf.chunks(2).map(|f| f[0]).collect()
                    }
                    Ear::Worklet(link, mixer) => {
                        let out = link.take();
                        for l in &out.loads {
                            mixer.load_sound(l.id, l.sample_rate, l.channels, &l.samples);
                        }
                        mixer.apply(&out.batch);
                        mixer.render(128)[..128].to_vec()
                    }
                };
                if q >= quanta / 2 {
                    sum += left.iter().map(|s| s * s).sum::<f32>();
                    n += left.len();
                }
            }
            (sum / n as f32).sqrt()
        }
    }

    /// A 3 kHz tone: an 800 Hz low-pass takes most of it.
    fn bright() -> Vec<f32> {
        (0..44_100)
            .map(|i| (i as f32 * 3000.0 * std::f32::consts::TAU / 44_100.0).sin() * 0.5)
            .collect()
    }

    /// An engine with `manifest`'s buses and the two tones, mixing on each
    /// mixer in turn.
    fn both_mixers(manifest: &str) -> [(AudioEngine, Ear, &'static str); 2] {
        let make = || {
            let mut e = AudioEngine::silent();
            e.load_sound_from_samples("bright", 44_100, &[&bright()])
                .unwrap();
            e.load_sound_from_samples("line", 44_100, &[&tone()])
                .unwrap();
            e.set_buses(&Buses::from_manifest(&manifest.parse().unwrap(), "flint.toml").unwrap());
            e
        };
        let mut here = make();
        let slot = Arc::new(Mutex::new(None));
        let settings = kira::AudioManagerSettings::<Capture> {
            backend_settings: slot.clone(),
            ..Default::default()
        };
        here.install(Box::new(
            Local::<Capture>::open(settings, 20_000.0).unwrap(),
        ));
        let renderer = slot.lock().unwrap().take().unwrap();

        let mut there = make();
        let link = Link::default();
        there.start_remote(link.clone());
        let mixer = Box::new(flint_audio_worklet::Mixer::new(48_000).unwrap());
        [
            (here, Ear::Here(Box::new(renderer)), "main thread"),
            (there, Ear::Worklet(link, mixer), "worklet"),
        ]
    }

    fn looping(bus: Bus, volume: f64) -> KeyedPlay {
        KeyedPlay {
            volume,
            looping: true,
            bus,
            ..Default::default()
        }
    }

    /// `bus` alone, a loop of `sound`: how loud it is.
    fn alone(e: &mut AudioEngine, ear: &mut Ear, key: u64, sound: &str, bus: Bus) -> f32 {
        e.play_keyed(key, sound, looping(bus, 1.0)).unwrap();
        ear.loudness(8);
        let l = ear.loudness(40);
        e.stop_keyed(key, Duration::ZERO);
        ear.loudness(8);
        l
    }

    #[test]
    fn set_bus_lowpass_on_sfx_muffles_sfx_and_not_music_on_both_mixers() {
        for (mut e, mut ear, mixer) in both_mixers("") {
            let sfx = alone(&mut e, &mut ear, 1, "bright", Bus::SFX);
            let music = alone(&mut e, &mut ear, 2, "bright", Bus::MUSIC);
            assert!(
                (sfx - 0.3536).abs() < 0.01,
                "{mixer}: open, the tone: {sfx}"
            );
            assert!((music - sfx).abs() < 0.005, "{mixer}: {music}");

            e.set_bus_lowpass_named("sfx", 800.0).unwrap();
            let muffled = alone(&mut e, &mut ear, 3, "bright", Bus::SFX);
            let still = alone(&mut e, &mut ear, 4, "bright", Bus::MUSIC);
            assert!(
                muffled < sfx * 0.1,
                "{mixer}: sfx muffled: {muffled} vs {sfx}"
            );
            assert!(
                (still - music).abs() < 0.005,
                "{mixer}: music untouched: {still}"
            );

            e.set_bus_lowpass_named("sfx", 20_000.0).unwrap();
            let open = alone(&mut e, &mut ear, 5, "bright", Bus::SFX);
            assert!((open - sfx).abs() < 0.005, "{mixer}: open again: {open}");
        }
    }

    #[test]
    fn music_ducks_under_a_voice_line_and_comes_back_on_both_mixers() {
        for (mut e, mut ear, mixer) in both_mixers(DUCKING) {
            let voice = e.bus("voice").unwrap();
            e.play_keyed(1, "line", looping(Bus::MUSIC, 1.0)).unwrap();
            e.update_ducks([]);
            ear.loudness(8);
            // Music at 0.5, the tone 0.5: RMS 0.177.
            let full = ear.loudness(40);
            assert!((full - 0.1768).abs() < 0.01, "{mixer}: {full}");

            // A line on voice, heard so quietly it adds nothing.
            e.play_keyed(2, "line", looping(voice, 0.0)).unwrap();
            e.update_ducks([]);
            ear.loudness(40); // the 80 ms attack
            let ducked = ear.loudness(40);
            assert!(
                (ducked / full - 0.2512).abs() < 0.02,
                "{mixer}: twelve decibels down: {ducked} vs {full}"
            );

            e.stop_keyed(2, Duration::ZERO);
            e.update_ducks([]);
            ear.loudness(240); // the 0.6 s release
            let back = ear.loudness(40);
            assert!(
                (back - full).abs() < 0.005,
                "{mixer}: back: {back} vs {full}"
            );
        }
    }

    #[test]
    fn a_bus_lowpass_is_sent_clamped_and_kept_for_a_mixer_started_later() {
        let mut e = AudioEngine::silent();
        e.set_buses(
            &Buses::from_manifest(
                &"[audio.buses.radio]\nlowpass = 2500\n".parse().unwrap(),
                "flint.toml",
            )
            .unwrap(),
        );
        e.set_bus_lowpass(Bus::SFX, 800.0);
        e.set_bus_lowpass(Bus::MUSIC, 1e9);
        assert_eq!(e.bus_lowpass(Bus::SFX), Some(800.0));
        assert_eq!(e.bus_lowpass(Bus::MUSIC), Some(20_000.0), "clamped open");
        assert!(e.set_bus_lowpass_named("nope", 100.0).is_err());

        let link = Link::default();
        e.start_remote(link.clone());
        let sent: Vec<Command> = Reader::new(&link.take().batch).collect();
        assert!(sent.contains(&Command::BusLowpass {
            bus: Bus::SFX.code(),
            hz: 800.0,
            tween: 0.0,
        }));
        assert!(sent.contains(&Command::BusNew {
            bus: 2,
            parent: crate::remote::protocol::BUS_MAIN,
            db: 0.0,
            lowpass: 2500.0,
        }));
        assert!(
            !sent
                .iter()
                .any(|c| matches!(c, Command::BusLowpass { bus: 1, .. })),
            "music is open: nothing to send"
        );
    }
}
