//! #1218: a game's `flint.toml` ducks `music` under `voice`, and a script
//! muffles `sfx` with `set_bus_lowpass`. As `script_bus_tests.rs` does, the
//! player runs the scene with the engine mixing remotely (the browser's
//! worklet link) and what it sent is applied to the worklet's own mixer,
//! so the dip and the muffle are heard. No window, no GPU, no sound card.

use super::scene_entry::SceneEntry;
use super::PlayerApp;
use flint_audio::engine::AudioEngine;
use flint_audio::remote::protocol::{Command, Reader};
use flint_audio::remote::Link;
use flint_audio::Bus;
use flint_audio_worklet::Mixer;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use std::sync::Arc;
use std::time::Duration;

const SCENE: &str = r#"[scene]
name = "Radio play"

[entities.director.script]
source = "director.rhai"
"#;

/// A tone at `hz`, mono, `secs` long, at half scale.
fn tone(hz: f32, secs: f32) -> Vec<f32> {
    (0..(44_100.0 * secs) as usize)
        .map(|i| (i as f32 * hz * std::f32::consts::TAU / 44_100.0).sin() * 0.5)
        .collect()
}

/// The scene with `manifest` and `script`, mixing down a link, and the
/// worklet mixer that hears it.
struct Stage {
    app: PlayerApp,
    link: Link,
    mixer: Mixer,
    sent: Vec<Command>,
}

impl Stage {
    fn run(
        manifest: &str,
        script: &str,
        sounds: &[(&str, Vec<f32>)],
        test: impl FnOnce(&mut Stage),
    ) {
        let mut files = MemorySource::new();
        files.insert("game/flint.toml", manifest);
        files.insert("game/scenes/radio.scene.toml", SCENE);
        files.insert("game/scripts/director.rhai", script);
        vfs::scoped(Arc::new(files), || {
            let path = "game/scenes/radio.scene.toml";
            let registry = flint_schema::SchemaRegistry::new();
            let (world, file) = flint_scene::load_scene(path, &registry).expect("the scene loads");
            let mut app = PlayerApp::new(world, path.into(), false, None, None);
            app.apply_scene_settings(&file);
            app.set_schemas(
                registry,
                SchemaDirs {
                    dirs: Vec::new(),
                    source: SchemaSource::Flags,
                    dropped: Vec::new(),
                    packages: None,
                },
            );
            app.core.clock.frame_step = Some(1.0 / 30.0);
            let mut engine = AudioEngine::silent();
            for (name, samples) in sounds {
                engine
                    .load_sound_from_samples(name, 44_100, &[samples])
                    .unwrap();
            }
            let link = Link::default();
            engine.start_remote(link.clone());
            app.audio.engine = engine;
            app.enter_scene(SceneEntry::Start)
                .expect("the scene starts");
            let mut stage = Stage {
                app,
                link,
                mixer: Mixer::new(48_000).unwrap(),
                sent: Vec::new(),
            };
            stage.deliver();
            test(&mut stage);
            assert_eq!(stage.mixer.errors, 0);
        });
    }

    /// Hand the mixer what the engine sent since the last time.
    fn deliver(&mut self) {
        let out = self.link.take();
        for l in &out.loads {
            self.mixer
                .load_sound(l.id, l.sample_rate, l.channels, &l.samples);
        }
        self.mixer.apply(&out.batch);
        let mut reader = Reader::new(&out.batch);
        self.sent.extend(reader.by_ref());
        assert_eq!(reader.error, None);
    }

    /// `frames` frames of the game, each delivered.
    fn tick(&mut self, frames: usize) {
        for _ in 0..frames {
            self.app.tick();
            self.deliver();
        }
    }

    /// RMS of the left side over `quanta` renders, after `settle` more
    /// (for a tween to finish).
    fn loudness(&mut self, settle: usize, quanta: usize) -> f32 {
        for _ in 0..settle {
            self.mixer.render(128);
        }
        let mut sum = 0.0f32;
        for _ in 0..quanta {
            sum += self.mixer.render(128)[..128]
                .iter()
                .map(|s| s * s)
                .sum::<f32>();
        }
        (sum / (quanta * 128) as f32).sqrt()
    }

    /// The bus volumes sent, by bus, and taken.
    fn bus_volumes(&mut self) -> Vec<(u8, f32, f32)> {
        std::mem::take(&mut self.sent)
            .into_iter()
            .filter_map(|c| match c {
                Command::BusVolume { bus, db, tween } => Some((bus, db, tween)),
                _ => None,
            })
            .collect()
    }
}

const DUCKING: &str = r#"[project]
id = "radio_play"

[audio.buses.music]
duck = { under = "voice", by_db = -12, attack = 0.05, release = 0.2 }

[audio.buses.voice]
"#;

/// The bed from the first frame; a line on voice from the fourth, played
/// nearly silent so that what is heard is the music bus.
const DIRECTOR: &str = r#"
let frame = 0;
fn on_update() {
    if frame == 0 { play_sound("bed", #{ bus: "music", looping: true }); }
    if frame == 3 { play_sound("line", #{ bus: "voice", volume: 0.001 }); }
    frame += 1;
}
"#;

#[test]
fn music_ducks_while_a_voice_line_plays_and_comes_back_after() {
    let sounds = [("bed", tone(440.0, 1.0)), ("line", tone(440.0, 0.25))];
    Stage::run(DUCKING, DIRECTOR, &sounds, |stage| {
        stage.tick(3);
        let full = stage.loudness(8, 40);
        assert!((full - 0.3536).abs() < 0.01, "the bed, undipped: {full}");
        assert!(stage.bus_volumes().is_empty(), "no duck before the line");
        assert!(!stage.app.audio.engine.is_ducked(Bus::MUSIC));

        stage.tick(2); // frame 3 plays the line; the duck follows
        assert!(stage.app.audio.engine.is_ducked(Bus::MUSIC));
        assert_eq!(
            stage.bus_volumes(),
            [(Bus::MUSIC.index() as u8, -12.0, 0.05)],
            "music down twelve decibels over the attack"
        );
        let ducked = stage.loudness(20, 40);
        assert!(
            (ducked / full - 0.2512).abs() < 0.02,
            "ducked: {ducked} vs {full}"
        );

        // The quarter-second line plays out (the worklet doesn't say so:
        // the engine knows its length), and the bed comes back.
        std::thread::sleep(Duration::from_millis(300));
        stage.tick(1);
        assert!(!stage.app.audio.engine.is_ducked(Bus::MUSIC));
        assert_eq!(
            stage.bus_volumes(),
            [(Bus::MUSIC.index() as u8, 0.0, 0.2)],
            "back up over the release"
        );
        let back = stage.loudness(80, 40);
        assert!((back - full).abs() < 0.005, "back: {back} vs {full}");
    });
}

const MUFFLER: &str = r#"
let frame = 0;
let hiss = ();
fn on_update() {
    if frame == 0 { hiss = play_sound("hiss", #{ looping: true }); }
    if frame == 3 { set_bus_lowpass("sfx", 800); }
    if frame == 5 {
        stop_sound(hiss);
        play_sound("hiss", #{ bus: "music", looping: true });
    }
    if frame == 7 { set_bus_lowpass("chorus", 800); }
    frame += 1;
}
"#;

#[test]
fn set_bus_lowpass_on_sfx_muffles_sfx_and_not_music() {
    let manifest = "[project]\nid = \"radio_play\"\n";
    let sounds = [("hiss", tone(3000.0, 1.0))];
    Stage::run(manifest, MUFFLER, &sounds, |stage| {
        stage.tick(3);
        let open = stage.loudness(8, 40);
        assert!((open - 0.3536).abs() < 0.01, "sfx open: {open}");

        stage.tick(2); // frame 3: set_bus_lowpass("sfx", 800)
        let muffled = stage.loudness(8, 40);
        assert!(muffled < open * 0.1, "sfx muffled: {muffled} vs {open}");
        assert_eq!(stage.app.audio.engine.bus_lowpass(Bus::SFX), Some(800.0));

        stage.tick(3); // frame 5: the same sound on music; 7: no such bus
        let music = stage.loudness(8, 40);
        assert!(
            (music - open).abs() < 0.005,
            "music untouched: {music} vs {open}"
        );
        let lowpasses: Vec<(u8, f32)> = stage
            .sent
            .iter()
            .filter_map(|c| match c {
                Command::BusLowpass { bus, hz, .. } => Some((*bus, *hz)),
                _ => None,
            })
            .collect();
        assert_eq!(
            lowpasses,
            [(Bus::SFX.index() as u8, 800.0)],
            "chorus is no bus"
        );
        assert_eq!(
            stage.app.audio.engine.bus_lowpass(Bus::MUSIC),
            Some(20_000.0)
        );
    });
}
