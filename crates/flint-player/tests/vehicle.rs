//! #1628's first done-when: a headless run drives a vehicle over the same
//! scripted input at 1/30 s and 1/144 s frames, and it ends within a stated
//! tolerance of the same place.
//!
//! The vehicle package steps at the fixed 1/60 s, so the frame rate reaches
//! it only through when each frame's input lands: a frame of 1/30 s runs two
//! steps on one input, one of 1/144 s none or one. A key pressed on a 1/30 s
//! frame's boundary reaches both its steps at 30 Hz but only the second at
//! 144 Hz: one step (16.7 ms) of a turn is a few degrees of heading, metres
//! apart 50 m on. So each change is timed 20 ms before a 1/30 s boundary,
//! inside the step that boundary's frame runs first, where the first 1/144 s
//! frame after it runs that same step. What is left is Rapier and the package.
//!
//! The tolerance is 0.25 m after 5.9 s and 85 m of driving, and 1 degree of
//! heading: ADR 0140 section 8 states 0.1 s a lap for #1629's track, which
//! at these speeds is about 2 m. Measured (2026-11-27): the two runs end in
//! the same place to the bit, so the tolerance is only room for a platform's
//! float. Before the input was timed off the frame boundaries they ended
//! 4.4 m and 3.2 degrees apart, which is the one step a key lands on.
//!
//! It plays through `flint_player::playtest`, the player's own frame loop
//! (`flint playtest`), from outside the crate: the vehicle is the `vehicle`
//! package's crate, which the launcher lists, so the test asks the run's
//! registry for it and passes over a build without it.

use flint_player::{playtest, PlaytestOptions};
use std::path::PathBuf;

/// The input: hold the throttle, a right turn, a left one, then straight.
/// Each time is 20 ms before a 1/30 s frame (0.5, 2.0, 3.0, 4.0, 4.5 s), so
/// at both rates it reaches the step that ends 1/60 s before that frame.
const INPUT: &str = "\
0.48  down KeyW
1.98  down KeyD
2.98  up   KeyD
3.98  down KeyA
4.48  up   KeyA
";

/// FlintKart's action names, the vehicle's defaults: accelerate, brake,
/// steering (an axis on A and D) and drift.
const BINDINGS: &str = r#"
version = 1
game_id = "vehicle-test"

[actions.accelerate]
kind = "button"
[[actions.accelerate.bindings]]
type = "key"
code = "KeyW"

[actions.brake]
kind = "button"
[[actions.brake.bindings]]
type = "key"
code = "KeyS"

[actions.steering]
kind = "axis1d"
[[actions.steering.bindings]]
type = "key"
code = "KeyD"
scale = 1.0
[[actions.steering.bindings]]
type = "key"
code = "KeyA"
scale = -1.0

[actions.drift]
kind = "button"
[[actions.drift.bindings]]
type = "key"
code = "Space"
"#;

/// A flat 1 km slab and the prototype's kart (ADR 0140), player-driven.
const SCENE: &str = r#"
[scene]
name = "Vehicle frame-rate test"
input_config = "input.toml"

[entities.ground.transform]
position = [0.0, -0.5, 0.0]
[entities.ground.rigidbody]
body_type = "static"
[entities.ground.collider]
shape = "box"
size = [1000.0, 1.0, 1000.0]
friction = 0.8

[entities.kart.transform]
position = [0.0, 0.7, 0.0]
[entities.kart.rigidbody]
body_type = "dynamic"
gravity_scale = 2.04
[entities.kart.collider]
shape = "box"
size = [1.2, 0.5, 2.0]
friction = 0.3
[entities.kart.vehicle]
top_speed = 22.0
[entities.kart.vehicle_input]

[entities.wheel_fl]
parent = "kart"
[entities.wheel_fl.transform]
position = [-0.55, 0.0, -0.75]
[entities.wheel_fl.vehicle_wheel]
steer = true

[entities.wheel_fr]
parent = "kart"
[entities.wheel_fr.transform]
position = [0.55, 0.0, -0.75]
[entities.wheel_fr.vehicle_wheel]
steer = true

[entities.wheel_rl]
parent = "kart"
[entities.wheel_rl.transform]
position = [-0.55, 0.0, 0.7]
[entities.wheel_rl.vehicle_wheel]
drive = true
handbrake = true

[entities.wheel_rr]
parent = "kart"
[entities.wheel_rr.transform]
position = [0.55, 0.0, 0.7]
[entities.wheel_rr.vehicle_wheel]
drive = true
handbrake = true
"#;

/// The test's own project: the scene, its bindings, the input script, and
/// the checkout's schemas (which bring every package's, the vehicle's too).
#[allow(clippy::disallowed_methods)] // a test writes its own project
fn project() -> (PathBuf, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("flint_1628_vehicle_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let schemas = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let schemas = schemas.to_string_lossy().replace('\\', "/");
    std::fs::write(
        dir.join("flint.toml"),
        format!("[project]\nschemas = [\"{schemas}\"]\n"),
    )
    .unwrap();
    std::fs::write(dir.join("input.toml"), BINDINGS).unwrap();
    let scene = dir.join("drive.scene.toml");
    std::fs::write(&scene, SCENE).unwrap();
    let input = dir.join("drive.input");
    std::fs::write(&input, INPUT).unwrap();
    (dir, scene, input)
}

/// The run ends at the first frame at or past this: 5.9 s at 1/30, and
/// 5.903 s at 1/144, both just after fixed step 354. Not 6.0 s, where a
/// 1/144 s frame ends on the step's boundary itself and the clock's float
/// sum may leave that step to the next frame.
const END: f64 = 5.897;

/// Where the kart ends, and its heading, after `END` at `frame_step`.
fn drive(
    scene: &std::path::Path,
    input: &std::path::Path,
    frame_step: f64,
) -> ([f64; 3], f64, f64) {
    let mut options = PlaytestOptions::new(scene.to_string_lossy());
    options.input_script = Some(input.to_string_lossy().into_owned());
    options.frame_step = frame_step;
    options.exit_after = Some(END);
    options.dump = vec!["kart".into()];
    let report = playtest(&options);
    assert!(
        report.passed,
        "the run at {frame_step} s passes: {:#?}",
        report.failures
    );
    let kart = &report.entities["kart"];
    let p = &kart["transform"]["position"];
    let at = [0, 1, 2].map(|i| p[i].as_f64().expect("a position"));
    let state = &kart["vehicle_state"];
    let yaw = state["yaw"].as_f64().expect("vehicle_state.yaw");
    let speed = state["speed"].as_f64().expect("vehicle_state.speed");
    (at, yaw, speed)
}

#[test]
fn the_same_input_at_30_and_144_frames_a_second_ends_in_the_same_place() {
    if !flint_player::packages::registry().has("vehicle") {
        eprintln!("skipped: this build's launcher has no vehicle package");
        return;
    }
    let (dir, scene, input) = project();
    let (slow, slow_yaw, slow_speed) = drive(&scene, &input, 1.0 / 30.0);
    let (fast, fast_yaw, fast_speed) = drive(&scene, &input, 1.0 / 144.0);
    let gap = ((slow[0] - fast[0]).powi(2) + (slow[2] - fast[2]).powi(2)).sqrt();
    let travelled = (slow[0].powi(2) + slow[2].powi(2)).sqrt();
    eprintln!(
        "1/30: {slow:?} yaw {slow_yaw:.2} speed {slow_speed:.2}\n\
         1/144: {fast:?} yaw {fast_yaw:.2} speed {fast_speed:.2}\n\
         apart {gap:.4} m after {travelled:.1} m, heading {:.3} deg",
        (slow_yaw - fast_yaw).abs()
    );
    assert!(
        travelled > 40.0,
        "it drove: {travelled:.1} m from the start"
    );
    assert!(
        slow_yaw.abs() > 10.0,
        "the turns changed its heading: {slow_yaw}"
    );
    assert!(
        gap < 0.25,
        "1/30 s and 1/144 s frames end {gap:.4} m apart ({slow:?} against {fast:?})"
    );
    assert!(
        (slow_yaw - fast_yaw).abs() < 1.0,
        "and face the same way: {slow_yaw:.3} against {fast_yaw:.3}"
    );
    let _ = std::fs::remove_dir_all(dir);
}
