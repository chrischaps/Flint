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
    // The first-person package said who the player is at the entry (#1614, #1618)
    let view = app.core.player_view();
    assert!(view.player.is_some() && view.mouse_look);
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
                // Where the controller placed the eye last frame (#1614)
                let eye = app.core.player_view().eye.expect("the player's eye");
                (step, eye.yaw, eye.pitch)
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

/// `ROOM` with its player's controller taken off: a scene nobody walks.
fn without_a_player() -> MemorySource {
    let mut files = game();
    let still = ROOM.replace(
        "[entities.player.character_controller]\nmove_speed = 5.0\n",
        "",
    );
    assert_ne!(still, ROOM);
    files.insert("game/scenes/room.scene.toml", still.as_str());
    files
}

/// `path` loaded and entered with the cursor free, no frame played.
fn entered() -> PlayerApp {
    let path = "game/scenes/room.scene.toml";
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(path, &registry).expect("the scene loads");
    let mut app = PlayerApp::new(world, path.into(), false, None, None);
    app.apply_scene_settings(&file);
    app.core.clock.frame_step = Some(1.0 / 60.0);
    app.enter_scene(SceneEntry::Start)
        .expect("the scene starts");
    app
}

/// #1614's risk: the cursor and the controls line ask who the player is
/// while the scene comes up, so the first-person package writes the view before
/// either, and before the first frame. One frame later the eye is placed
/// and the camera stands in it.
#[test]
fn the_players_view_is_settled_at_the_entry_before_the_first_frame() {
    vfs::scoped(Arc::new(game()), || {
        let mut app = entered();
        let player = app.core.world.get_id("player");
        let view = app.core.player_view();
        assert_eq!(view.player, player);
        assert!(view.mouse_look);
        assert_eq!(view.controls, &flint_first_person::ACTIONS);
        assert_eq!(view.eye, None, "no eye before the first frame");
        assert!(app.is_cursor_captured(), "the entry caught the cursor");
        let said = app.input_layers.said_controls.clone().unwrap_or_default();
        assert!(
            said.contains("move_forward W") && said.contains("mouse look"),
            "{said}"
        );

        frame(&mut app);
        let eye = app.core.player_view().eye.expect("the eye after a frame");
        assert_eq!(app.camera.position, eye.position);
        assert_eq!(app.camera.target, eye.target);
        // At eye height, 85% of the default 1.8 m capsule's 1.5 fallback
        assert!(eye.position.y > 1.0, "{eye:?}");
    });
}

/// A scene with no character controller has no player: the cursor stays
/// free, the controls line has no walk and no mouse look, and the camera
/// stays where the scene put it.
#[test]
fn a_scene_nobody_walks_has_no_player_and_leaves_the_cursor_free() {
    vfs::scoped(Arc::new(without_a_player()), || {
        let mut app = entered();
        assert_eq!(*app.core.player_view(), flint_runtime::PlayerView::NONE);
        assert!(!app.is_cursor_captured());
        let said = app.input_layers.said_controls.clone().unwrap_or_default();
        assert!(!said.contains("mouse look"), "{said}");
        assert!(!said.contains("move_forward"), "{said}");

        let before = app.camera.position;
        frame(&mut app);
        assert_eq!(app.core.player_view().eye, None);
        assert_eq!(app.camera.position, before);
    });
}
