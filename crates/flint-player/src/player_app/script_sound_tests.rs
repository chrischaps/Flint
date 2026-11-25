//! #699: a script's `stop_sound` reaches the audio engine. The player runs
//! a scene whose script starts a loop, fades it and stops it by its handle,
//! then stops two more by name, with the engine mixing remotely (the
//! browser's worklet link, its outbox read here): every call the script
//! made is a command the mixer gets. No window, no GPU, no sound card.
//! #1000: and `is_sound_playing` says which of them still play.

use super::scene_entry::SceneEntry;
use super::PlayerApp;
use flint_audio::engine::AudioEngine;
use flint_audio::remote::protocol::{Command, Reader};
use flint_audio::remote::Link;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use std::sync::Arc;

/// Frame 0 starts a loop and two one-shots of another sound; frame 2 fades
/// the loop; frame 4 stops it by its handle; frame 6 stops the others by
/// name.
const SCRIPT: &str = r#"
let frame = 0;
let hum = 0;
fn on_update() {
    if frame == 0 {
        hum = play_sound("hum", #{ looping: true, volume: 0.5 });
        play_sound("chime");
        play_sound_at("chime", 1.0, 0.0, 0.0, #{ looping: true });
    }
    if frame == 2 { set_sound_volume(hum, 0.1, 0.5); }
    if frame == 4 { stop_sound(hum); }
    if frame == 6 { stop_sound("chime", 0.25); }
    frame += 1;
}
"#;

const SCENE: &str = r#"[scene]
name = "Sounds"

[entities.speaker.script]
source = "speaker.rhai"
"#;

/// What the engine wrote to the mixer since the last call, as commands.
fn sent(link: &Link) -> Vec<Command> {
    let out = link.take();
    let mut reader = Reader::new(&out.batch);
    let commands: Vec<Command> = reader.by_ref().collect();
    assert_eq!(reader.error, None, "the batch reads whole");
    commands
}

/// A player on `script`'s scene, mixing for the worklet's link in place of
/// the sound card, each of `sounds` a tone that many seconds long; `run`
/// gets it once the scene has started, with what the start sent taken.
fn with_sounds(script: &str, sounds: &[(&str, f32)], run: impl FnOnce(&mut PlayerApp, &Link)) {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert("game/scenes/sounds.scene.toml", SCENE);
    files.insert("game/scripts/speaker.rhai", script);
    vfs::scoped(Arc::new(files), || {
        let path = "game/scenes/sounds.scene.toml";
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
        for (name, seconds) in sounds {
            let tone: Vec<f32> = (0..(44_100.0 * seconds) as usize)
                .map(|i| (i as f32 * 0.06).sin() * 0.5)
                .collect();
            engine
                .load_sound_from_samples(name, 44_100, &[&tone])
                .unwrap();
        }
        let link = Link::default();
        engine.start_remote(link.clone());
        app.audio.engine = engine;
        app.enter_scene(SceneEntry::Start)
            .expect("the scene starts");
        sent(&link);
        run(&mut app, &link);
    });
}

#[test]
fn a_scripts_stop_sound_stops_the_sound_it_started() {
    // A second long, so the chime is still playing when it's stopped.
    with_sounds(SCRIPT, &[("hum", 1.0), ("chime", 1.0)], |app, link| {
        let mut frames = Vec::new();
        for _ in 0..8 {
            app.tick();
            frames.push(sent(link));
        }
        let plays: Vec<(u32, bool)> = frames
            .iter()
            .flatten()
            .filter_map(|c| match c {
                Command::Play { voice, looping, .. } => Some((*voice, *looping)),
                _ => None,
            })
            .collect();
        assert_eq!(plays.len(), 3, "a loop and two chimes: {plays:?}");
        let (hum, chime, placed) = (plays[0].0, plays[1].0, plays[2].0);
        assert_eq!(
            plays.iter().map(|p| p.1).collect::<Vec<_>>(),
            [true, false, true]
        );
        let at = |frame: usize, want: &dyn Fn(&Command) -> bool| frames[frame].iter().any(want);
        assert!(
            frames.iter().position(|f| f.iter().any(
                |c| matches!(c, Command::VoiceVolume { voice, tween, .. } if *voice == hum && (*tween - 0.5).abs() < 1e-6)
            )).is_some(),
            "set_sound_volume fades the loop"
        );
        let stopped_at = |voice: u32| {
            frames.iter().position(|f| {
                f.iter()
                    .any(|c| matches!(c, Command::VoiceStop { voice: v, .. } if *v == voice))
            })
        };
        let hum_stop = stopped_at(hum).expect("stop_sound(handle) stops the loop");
        assert!(
            !at(
                hum_stop,
                &|c| matches!(c, Command::VoiceStop { voice, .. } if *voice == chime)
            ),
            "and nothing else"
        );
        let chime_stop = stopped_at(chime).expect("stop_sound(name) stops the chime");
        assert_eq!(
            stopped_at(placed),
            Some(chime_stop),
            "and the placed one by the same name, in the same frame"
        );
        assert!(hum_stop < chime_stop);
        assert!(
            frames[chime_stop].iter().any(
                |c| matches!(c, Command::VoiceStop { voice, tween } if *voice == chime && (*tween - 0.25).abs() < 1e-6)
            ),
            "with the fade asked for"
        );
    });
}

/// #1000: each frame the script asks `is_sound_playing` of a loop it stops
/// by its handle on frame 2, a half-second one-shot left to end, a sound
/// that names no file, a one-shot placed out of earshot and a handle
/// nobody was given, and logs the answers under the frame's number.
const ASKING: &str = r#"
let frame = 0;
let hum = 0;
let ding = 0;
let nope = 0;
let far = 0;
fn on_update() {
    if frame == 0 {
        hum = play_sound("hum", #{ looping: true });
        ding = play_sound("ding");
        nope = play_sound("no_such_sound");
        far = play_sound_at("ding", 500.0, 0.0, 0.0, 1.0);
    }
    if frame == 2 { stop_sound(hum); }
    test_log(`f${frame}`, [
        is_sound_playing(hum),
        is_sound_playing(ding),
        is_sound_playing(nope),
        is_sound_playing(far),
        is_sound_playing(987654321),
    ]);
    frame += 1;
}
"#;

#[test]
fn a_script_hears_its_sound_play_until_it_ends_or_is_stopped() {
    with_sounds(ASKING, &[("hum", 1.0), ("ding", 0.5)], |app, _link| {
        for _ in 0..4 {
            app.tick();
        }
        // Past the ding's half second, on the wall clock the engine
        // reckons a remote one-shot's end by
        std::thread::sleep(std::time::Duration::from_millis(700));
        app.tick();
        let logged = app.core.script.test_record().logged;
        let frame = |n: usize| -> Vec<bool> {
            logged[&format!("f{n}")]
                .as_array()
                .expect("a list")
                .iter()
                .map(|v| v.as_bool().expect("a bool"))
                .collect()
        };
        // [hum, ding, nope, far, unknown]
        assert_eq!(
            frame(0),
            [false; 5],
            "the frame play_sound is called, nothing plays yet"
        );
        assert_eq!(
            frame(1),
            [true, true, false, false, false],
            "the frame after: the loop and the ding; never the sound with no file, the one out of earshot or a made-up handle"
        );
        assert!(
            frame(2)[0],
            "stop_sound is a command: the frame it's called still says playing"
        );
        assert!(!frame(3)[0], "and the frame after, stopped");
        assert_eq!(
            frame(4),
            [false; 5],
            "the ding has played out, and nothing else came back"
        );
    });
}
