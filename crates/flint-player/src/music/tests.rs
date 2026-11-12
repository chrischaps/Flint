//! Done-when 2 of #745: a scene starts a suite's stems with no chart, and a
//! script reads the beat and the bar. Headless, as `flint render --scripts`
//! runs it: the music subsystem keeps the beat on the game clock, so the
//! numbers are exact.

use crate::player_app::headless_game::HeadlessGame;
use flint_ecs::FlintWorld;
use flint_render::{Camera, PostProcessConfig};
use std::path::PathBuf;

const SUITE: &str = r#"
schema_version = 0
[suite]
id = "probe"
title = "Probe"
[audio]
sample_rate = 8000
[[tempo]]
sample = 0
bpm = 120.0
time_signature = [4, 4]
[[sections]]
name = "a"
start_sample = 0
[[sections]]
name = "b"
start_sample = 16000
[buses.pad]
file = "music/pad.wav"
[buses.drums]
file = "music/drums.wav"
"#;

/// Writes the frame's beat, bar and section into the entity's `probe`.
const SCRIPT: &str = r#"
fn on_update() {
    let me = self_entity();
    set_field(me, "probe", "beat", music_beat());
    set_field(me, "probe", "bar", music_bar());
    set_field(me, "probe", "section", music_section());
}
"#;

/// A project of the test's own: the suite, two stems, the script.
fn project(test: &str) -> (PathBuf, String) {
    let dir = std::env::temp_dir().join(format!("flint_745_{test}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("music")).unwrap();
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(dir.join("flint.toml"), "[project]\n").unwrap();
    std::fs::write(dir.join("music/probe.suite.toml"), SUITE).unwrap();
    let tone = vec![0.1f32; 2 * 8000 * 4];
    flint_music::write_wav(&dir.join("music/pad.wav"), &tone, 8000).unwrap();
    flint_music::write_wav(&dir.join("music/drums.wav"), &tone, 8000).unwrap();
    std::fs::write(dir.join("scripts/probe.rhai"), SCRIPT).unwrap();
    let scene = dir.join("probe.scene.toml");
    std::fs::write(&scene, "[scene]\nname = \"probe\"\n").unwrap();
    (dir, scene.to_string_lossy().to_string())
}

fn table(pairs: &[(&str, toml::Value)]) -> toml::Value {
    toml::Value::Table(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

fn probe(game: &HeadlessGame, field: &str) -> toml::Value {
    let world = game.world();
    let id = world.get_id("conductor").unwrap();
    world
        .get_components(id)
        .unwrap()
        .get_field("probe", field)
        .unwrap()
        .clone()
}

#[test]
fn a_script_reads_the_beat_and_bar_of_a_suite_with_no_chart() {
    let (dir, scene) = project("beat");
    let mut world = FlintWorld::new();
    let id = world.spawn("conductor").unwrap();
    world
        .set_component(
            id,
            flint_core::components::MUSIC_SESSION,
            table(&[("manifest", "music/probe.suite.toml".into())]),
        )
        .unwrap();
    world
        .set_component(id, "script", table(&[("source", "probe.rhai".into())]))
        .unwrap();
    world
        .set_component(
            id,
            "probe",
            table(&[
                ("beat", 0.0.into()),
                ("bar", 0.into()),
                ("section", "".into()),
            ]),
        )
        .unwrap();
    let project = flint_core::project::Project::for_scene(std::path::Path::new(&scene));
    let mut game = HeadlessGame::new(world, &scene, project);
    game.start_scripts(true, None, (320.0, 180.0), &PostProcessConfig::default());
    let mut camera = Camera::new();

    // Frame one, 0.25 s: still in the two-beat count-in (1 s at 120 bpm)
    game.step(0.25, &mut camera, None, None);
    let beat = probe(&game, "beat").as_float().unwrap();
    assert!((beat - -1.5).abs() < 1e-9, "count-in beat {beat}");
    assert_eq!(probe(&game, "section").as_str(), Some(""));

    // 0.75 s more ends the count-in; 2.5 s past it is beat 5, in bar 1,
    // section b (from sample 16000, 2 s in)
    game.step(0.75, &mut camera, None, None);
    game.step(2.5, &mut camera, None, None);
    let beat = probe(&game, "beat").as_float().unwrap();
    assert!((beat - 5.0).abs() < 1e-9, "beat {beat}");
    assert_eq!(probe(&game, "bar").as_integer(), Some(1));
    assert_eq!(probe(&game, "section").as_str(), Some("b"));
    let _ = std::fs::remove_dir_all(dir);
}
