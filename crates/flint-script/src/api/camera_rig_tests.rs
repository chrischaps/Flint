//! The camera rig through Rhai (#1158): a script flies a path with a bank
//! and a shake in a few lines, the same frames come out twice, and the
//! shot list says when a shot begins.

use crate::context::{CameraOverrides, InputSnapshot, ScriptCommand};
use crate::engine::test_support::*;
use crate::engine::ScriptEngine;
use flint_ecs::FlintWorld;

/// The issue's script: a path with a bank, a kick at 1 s that dies away.
/// Nine lines, and no maths of its own.
const FLIGHT: &str = r#"
const PATH = [[0.0, 3.0, 12.0], [8.0, 4.0, 4.0], [0.0, 5.0, -6.0], [-8.0, 3.0, 2.0]];
let kicked = false;
fn on_update() {
    let t = total_time();
    set_camera_on_path(global::PATH, t / 4.0, #{ bank: 6.0 });
    if t >= 1.0 && !kicked { shake_view(0.25, 14.0, 3.0); kicked = true; }
}
"#;

/// Run a script `frames` frames at 60 fps and keep each frame's camera.
fn fly(src: &str, frames: usize) -> Vec<CameraOverrides> {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("director").unwrap();
    probe_component(&mut world, id);
    let ast = engine.compile(src).unwrap();
    engine.add_script(id, ast, "flight.rhai".into());
    let dt = 1.0 / 60.0;
    let mut out = Vec::new();
    for f in 0..frames {
        engine.provide_context(InputSnapshot::default(), dt, f as f64 * dt);
        engine.call_updates(&mut world);
        let camera = engine
            .drain_commands()
            .into_iter()
            .find_map(|c| match c {
                ScriptCommand::SetCamera { camera } => Some(camera),
                _ => None,
            })
            .unwrap_or_default();
        out.push(camera);
    }
    assert!(
        engine.errors().is_empty(),
        "{:?}",
        engine.errors().summary()
    );
    out
}

#[test]
fn a_script_flies_a_banked_path_with_a_shake_in_a_few_lines_and_twice_alike() {
    let lines = FLIGHT.trim().lines().count();
    assert!(lines < 10, "{lines} lines");
    let a = fly(FLIGHT, 240);
    let b = fly(FLIGHT, 240);
    assert_eq!(a, b, "the same frames twice");
    // Every frame set the camera and leaned into the turn
    assert!(a.iter().all(|c| c.position.is_some() && c.target.is_some()));
    assert!(a.iter().any(|c| c.roll.unwrap_or(0.0).abs() > 0.1));
    // The shake: without it the path alone is smooth, so measure the
    // frame-to-frame wobble before and after the kick
    let calm = fly(&FLIGHT.replace("shake_view(0.25, 14.0, 3.0);", ""), 240);
    let off = |i: usize| {
        let (p, q) = (a[i].position.unwrap(), calm[i].position.unwrap());
        ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
    };
    assert_eq!(off(59), 0.0, "nothing before the kick");
    let early = (61..90).map(off).fold(0.0, f32::max);
    let late = (200..240).map(off).fold(0.0, f32::max);
    assert!(early > 0.05, "shaken: {early}");
    assert!(late < early * 0.1, "and dying away: {late} after {early}");
}

#[test]
fn the_camera_looks_at_a_point_an_entity_or_a_second_path() {
    let point = fly(
        "fn on_update() { set_camera_on_path([[0, 2, 10], [10, 2, 0]], 0.5, #{ look_at: [1, 1, 1] }); }",
        1,
    );
    assert_eq!(point[0].target, Some([1.0, 1.0, 1.0]));
    assert_eq!(point[0].roll, None, "no bank asked for, no roll set");

    let along = fly(
        "fn on_update() { set_camera_on_path([[0, 0, 0], [0, 0, -10]], 0.5); }",
        1,
    );
    let (p, t) = (along[0].position.unwrap(), along[0].target.unwrap());
    assert!((p[2] + 5.0).abs() < 1e-3, "{p:?}");
    assert!(
        (t[2] - (p[2] - 1.0)).abs() < 1e-3,
        "looking along it: {t:?}"
    );

    let entity = fly(
        r#"fn on_update() {
            let lamp = spawn_entity("lamp");
            set_position(lamp, 4.0, 5.0, 6.0);
            set_camera_on_path([[0, 0, 0], [0, 0, -10]], 0.2, #{ look_at: lamp });
        }"#,
        1,
    );
    assert_eq!(entity[0].target, Some([4.0, 5.0, 6.0]));

    let second = fly(
        "fn on_update() { set_camera_on_path([[0, 0, 0], [0, 0, -10]], 1.0, #{ look_at: [[5, 0, 0], [5, 0, -10]] }); }",
        1,
    );
    let t = second[0].target.unwrap();
    assert!(
        (t[0] - 5.0).abs() < 1e-4 && (t[2] + 10.0).abs() < 1e-3,
        "{t:?}"
    );
}

#[test]
fn a_shot_list_fires_each_shots_first_frame_once() {
    // Count the first frames each shot sees, calling shot() twice a frame
    let src = r#"
let firsts = [0.0, 0.0, 0.0];
fn on_update() {
    let s = shot(total_time(), [0.0, 0.5, 1.0, 1.5]);
    let again = shot(total_time(), [0.0, 0.5, 1.0, 1.5]);
    if s.first && again.first && s.index >= 0 { firsts[s.index] += 1.0; }
    set_field(self_entity(), "probe", "a", firsts[0]);
    set_field(self_entity(), "probe", "b", firsts[1]);
    set_field(self_entity(), "probe", "c", firsts[2]);
    set_field(self_entity(), "probe", "u", s.u);
}
"#;
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("director").unwrap();
    probe_component(&mut world, id);
    engine.add_script(id, engine.compile(src).unwrap(), "shots.rhai".into());
    let dt = 1.0 / 60.0;
    for f in 0..80 {
        engine.provide_context(InputSnapshot::default(), dt, f as f64 * dt);
        engine.call_updates(&mut world);
    }
    assert!(
        engine.errors().is_empty(),
        "{:?}",
        engine.errors().summary()
    );
    for k in ["a", "b", "c"] {
        assert_eq!(probe_float(&world, id, k), 1.0, "shot {k} began once");
    }
    // 79 frames in: past the last start, a third of the way through
    let u = probe_float(&world, id, "u");
    assert!((u - (79.0 / 60.0 - 1.0) / 0.5).abs() < 1e-9, "{u}");
}

#[test]
fn a_hand_held_camera_moves_a_script_camera_and_noise_is_seeded() {
    let still = fly(
        "fn on_update() { set_camera_position(0.0, 2.0, 5.0); set_camera_target(0.0, 2.0, 0.0); }",
        30,
    );
    let held = fly(
        "set_hand_held(1.0, #{ steps: 1.8 });
         fn on_update() { set_camera_position(0.0, 2.0, 5.0); set_camera_target(0.0, 2.0, 0.0); }",
        30,
    );
    assert!(still.iter().all(|c| c.position == Some([0.0, 2.0, 5.0])));
    assert!(held[1..]
        .iter()
        .any(|c| c.position != Some([0.0, 2.0, 5.0])));
    // Seeded: the same in Rhai as in Rust, and another seed differs
    let engine = ScriptEngine::new();
    let n: f64 = engine.rhai_engine().eval("noise(2.37, 4)").unwrap();
    assert_eq!(n, crate::rig::noise(2.37, 4));
    let m: f64 = engine.rhai_engine().eval("noise(2.37)").unwrap();
    assert_ne!(n, m);
}

#[test]
fn a_path_that_isnt_one_is_a_script_error_at_its_line() {
    let mut engine = ScriptEngine::new();
    let mut world = FlintWorld::new();
    let id = world.spawn("director").unwrap();
    let src = "fn on_update() {\n  camera_path([[0, 0, 0]], 0.5);\n}";
    engine.add_script(id, engine.compile(src).unwrap(), "bad.rhai".into());
    engine.provide_context(InputSnapshot::default(), 0.016, 0.0);
    engine.call_updates(&mut world);
    let summary = engine.errors().summary().join("\n");
    assert!(summary.contains("at least 2 points"), "{summary}");
}
