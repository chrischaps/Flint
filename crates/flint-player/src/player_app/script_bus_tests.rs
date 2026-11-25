//! #730: a game declares a third bus in its `flint.toml` and a script turns
//! it down by name. The player runs the scene with the engine mixing
//! remotely (the browser's worklet link, read here) and then applies what
//! it sent to the worklet's own mixer, so the bus is heard to go quiet. No
//! window, no GPU, no sound card.

use super::scene_entry::SceneEntry;
use super::PlayerApp;
use flint_audio::engine::AudioEngine;
use flint_audio::remote::protocol::{self, Command, Reader};
use flint_audio::remote::Link;
use flint_audio::Bus;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use std::sync::Arc;

const MANIFEST: &str = r#"[project]
id = "radio_play"

[audio.buses.voice]
volume = 0.8

[audio.buses.ui]
parent = "sfx"
"#;

/// Frame 0 starts a line on the voice bus; frame 3 turns the bus down.
const SCRIPT: &str = r#"
let frame = 0;
fn on_update() {
    if frame == 0 { play_sound("line", #{ bus: "voice", looping: true }); }
    if frame == 3 { set_bus_volume("voice", 0.25); }
    if frame == 4 { set_bus_volume("chorus", 0.5); }
    frame += 1;
}
"#;

const SCENE: &str = r#"[scene]
name = "Radio play"

[entities.narrator.script]
source = "narrator.rhai"
"#;

#[test]
fn a_bus_declared_in_flint_toml_is_made_played_on_and_turned_down_by_name() {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", MANIFEST);
    files.insert("game/scenes/radio.scene.toml", SCENE);
    files.insert("game/scripts/narrator.rhai", SCRIPT);
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

        let tone: Vec<f32> = (0..44_100)
            .map(|i| (i as f32 * 0.0627).sin() * 0.5)
            .collect();
        let mut engine = AudioEngine::silent();
        engine
            .load_sound_from_samples("line", 44_100, &[&tone])
            .unwrap();
        let link = Link::default();
        engine.start_remote(link.clone());
        app.audio.engine = engine;
        app.enter_scene(SceneEntry::Start)
            .expect("the scene starts");

        // The manifest's buses, numbered after the built-in two.
        let voice = app.audio.engine.bus("voice").expect("voice is a bus");
        let ui = app.audio.engine.bus("ui").expect("ui is a bus");
        assert_eq!(
            (ui.index(), voice.index()),
            (2, 3),
            "by name, parents first"
        );
        let names: Vec<&str> = app.audio.engine.buses().names().collect();
        assert_eq!(names, ["sfx", "music", "ui", "voice"]);

        let mut mixer = flint_audio_worklet::Mixer::new(48_000).unwrap();
        let mut sent = Vec::new();
        let mut deliver = |mixer: &mut flint_audio_worklet::Mixer| {
            let out = link.take();
            for l in &out.loads {
                mixer.load_sound(l.id, l.sample_rate, l.channels, &l.samples);
            }
            mixer.apply(&out.batch);
            let mut reader = Reader::new(&out.batch);
            sent.extend(reader.by_ref());
            assert_eq!(reader.error, None);
        };
        let loudness = |mixer: &mut flint_audio_worklet::Mixer| {
            let mut peak = 0.0f32;
            for _ in 0..40 {
                let out = mixer.render(128);
                peak = out.iter().fold(peak, |a, s| a.max(s.abs()));
            }
            peak
        };
        deliver(&mut mixer);

        for _ in 0..3 {
            app.tick();
        }
        deliver(&mut mixer);
        loudness(&mut mixer); // past the bus's tween
        let before = loudness(&mut mixer);
        app.tick(); // frame 3: set_bus_volume("voice", 0.25)
        app.tick(); // frame 4: a bus that isn't one changes nothing
        deliver(&mut mixer);
        loudness(&mut mixer);
        let after = loudness(&mut mixer);

        assert!(
            sent.contains(&Command::BusNew {
                bus: 3,
                parent: protocol::BUS_MAIN,
                db: (20.0 * 0.8f32.log10()),
                lowpass: 20_000.0,
            }),
            "voice is made under the main track at its volume: {sent:?}"
        );
        assert!(sent.contains(&Command::BusNew {
            bus: 2,
            parent: protocol::BUS_SFX,
            db: 0.0,
            lowpass: 20_000.0,
        }));
        assert!(
            sent.iter().any(|c| matches!(
                c,
                Command::Play {
                    target: protocol::Target::Bus(3),
                    ..
                }
            )),
            "the line plays on the voice bus"
        );
        let bus_volumes: Vec<u8> = sent
            .iter()
            .filter_map(|c| match c {
                Command::BusVolume { bus, .. } => Some(*bus),
                _ => None,
            })
            .collect();
        assert_eq!(bus_volumes, [3], "one change, to voice; chorus is no bus");
        assert_eq!(mixer.errors, 0);
        // 0.8 of a 0.5 tone, then a quarter of the tone.
        assert!((before - 0.4).abs() < 0.03, "before: {before}");
        assert!((after - 0.125).abs() < 0.02, "after: {after}");
        assert_eq!(app.audio.engine.bus_volume(voice), Some(0.25));
        assert_eq!(app.audio.engine.bus_volume(Bus::SFX), Some(1.0));
    });
}
