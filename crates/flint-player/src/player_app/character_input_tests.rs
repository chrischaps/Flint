//! #728: the character takes mouse look and the jump press once a frame,
//! not once a fixed step. A frame of two steps (30 Hz) turned twice as far,
//! and a frame of none (most frames at 144 Hz) lost its motion and its
//! press. Played through the player's own frame, on a stepped clock, with
//! an input script.
//!
//! The project is a `MemorySource`; no window, no GPU.

use super::input_script::InputScript;
use super::scene_entry::SceneEntry;
use super::PlayerApp;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use std::sync::Arc;

const ROOM: &str = r#"[scene]
name = "Room"

[entities.floor.transform]
position = [0.0, -0.5, 0.0]
[entities.floor.rigidbody]
body_type = "static"
[entities.floor.collider]
shape = "box"
size = [100.0, 1.0, 100.0]

[entities.player.transform]
position = [0.0, 1.0, 0.0]
[entities.player.rigidbody]
body_type = "kinematic"
[entities.player.collider]
shape = "capsule"
size = [0.6, 1.8, 0.6]
[entities.player.character_controller]
move_speed = 5.0
"#;

fn game() -> MemorySource {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert("game/scenes/room.scene.toml", ROOM);
    files
}

/// The room, started on a clock stepped `step` seconds a frame, playing
/// `script` with the cursor captured (so `look` reaches the game).
fn started(step: f64, script: &str) -> PlayerApp {
    let path = "game/scenes/room.scene.toml";
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
    app.core.clock.frame_step = Some(step);
    app.capture.input_script = Some(InputScript::parse(script).unwrap());
    app.focus.cursor_captured = true;
    app.enter_scene(SceneEntry::Start)
        .expect("the scene starts");
    assert!(app.core.physics.has_player_entity());
    app
}

/// One frame, as the player runs it: tick, then clear the frame's input.
fn frame(app: &mut PlayerApp) {
    app.tick();
    app.input.end_frame();
}

/// Frames until `seconds` of game time have passed.
fn frames_for(step: f64, seconds: f64) -> usize {
    (seconds / step).round() as usize
}

#[test]
fn a_turn_is_the_same_from_24_to_240_frames_a_second() {
    // 450 units of motion over a second, then still
    let script = "0.2 look 450 -60 1.0\n";
    let rates = [24.0, 30.0, 60.0, 90.0, 120.0, 144.0, 240.0];
    let turned: Vec<(f64, f32, f32)> = rates
        .into_iter()
        .map(|hz| 1.0 / hz)
        .map(|step| {
            vfs::scoped(Arc::new(game()), || {
                let mut app = started(step, script);
                for _ in 0..frames_for(step, 2.0) {
                    frame(&mut app);
                }
                (
                    step,
                    app.core.physics.camera_yaw(),
                    app.core.physics.camera_pitch(),
                )
            })
        })
        .collect();

    // 450 units at 0.172 degrees each is 77.4 degrees to the right
    let want = -(450.0f32 * 0.172).to_radians();
    for &(step, yaw, pitch) in &turned {
        eprintln!(
            "turn at {:>3.0} Hz: yaw {:>7.2} pitch {:>6.2} degrees",
            1.0 / step,
            -yaw.to_degrees(),
            pitch.to_degrees()
        );
    }
    for &(step, yaw, pitch) in &turned {
        assert!(
            (yaw - want).abs() < 1e-3,
            "at 1/{:.0} s the turn was {:.2} degrees, not {:.2}",
            1.0 / step,
            yaw.to_degrees(),
            want.to_degrees()
        );
        assert!(
            (pitch - (60.0f32 * 0.172).to_radians()).abs() < 1e-3,
            "at 1/{:.0} s pitch {:.2}",
            1.0 / step,
            pitch.to_degrees()
        );
    }
}

#[test]
fn every_jump_tapped_at_144_frames_a_second_jumps() {
    let step = 1.0 / 144.0;
    // A press is "just pressed" on one frame only; at 144 Hz about half of
    // the frames run no fixed step, and each tap lands on a different
    // phase of the step
    let taps = [1.0, 2.507, 4.013, 5.52, 7.031];
    let script: String = taps.iter().map(|t| format!("{t} tap Space\n")).collect();
    let jumps = vfs::scoped(Arc::new(game()), || {
        let mut app = started(step, &script);
        let id = app.core.world.get_id("player").unwrap();
        let mut floor = None;
        let mut airborne = false;
        let mut jumps = 0;
        for _ in 0..frames_for(step, 8.5) {
            frame(&mut app);
            let y = app.core.world.get_transform(id).unwrap().position.y;
            if app.core.clock.total_time < 0.9 {
                floor = Some(y);
                continue;
            }
            let rise = y - floor.unwrap();
            if !airborne && rise > 0.5 {
                airborne = true;
                jumps += 1;
            } else if airborne && rise < 0.05 {
                airborne = false;
            }
        }
        jumps
    });
    eprintln!("jumps at 144 Hz: {jumps} of {} taps", taps.len());
    assert_eq!(jumps, taps.len(), "each tap jumped");
}

/// #1228: a click catches the cursor for mouse look, but not while the
/// game is paused. A pause menu's state stops physics, and its clicks are
/// the menu's: caught, the pointer would vanish from over its buttons.
#[test]
fn a_click_catches_the_cursor_only_while_the_game_runs() {
    use winit::event::{ElementState, MouseButton};
    vfs::scoped(Arc::new(game()), || {
        let mut app = started(1.0 / 60.0, "");
        frame(&mut app);
        let press = |app: &mut PlayerApp| {
            app.handle_mouse_input(ElementState::Pressed, MouseButton::Left);
        };

        // Paused, as menu::toggle_pause leaves it: the click is the menu's
        app.release_cursor();
        assert!(app.core.state_machine.push_state("paused"));
        press(&mut app);
        assert!(
            !app.is_cursor_captured(),
            "a paused click caught the cursor"
        );
        // ...and the game, the menu's focus with it, still hears it
        assert!(app.input.is_mouse_button_just_pressed(0));
        app.handle_mouse_input(ElementState::Released, MouseButton::Left);
        app.input.end_frame();

        // Playing again: the first click catches it for looking round
        app.core.state_machine.pop_state();
        press(&mut app);
        assert!(app.is_cursor_captured());
    });
}
