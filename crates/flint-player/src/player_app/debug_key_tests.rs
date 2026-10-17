//! Captures reach the debug keys (H9): a key the input script presses goes
//! through the same handler as a real one, and `CaptureConfig::show_stats`
//! opens the F2 overlay from the first frame. Scripted clicks reach
//! `InputState` the way real ones do (ADR 0082). Headless: no window.

use super::input_script::InputScript;
use super::PlayerApp;
use flint_ecs::FlintWorld;
use winit::keyboard::KeyCode;

fn headless_app() -> PlayerApp {
    PlayerApp::new(
        FlintWorld::new(),
        "capture_test.scene.toml".into(),
        false,
        None,
        None,
    )
}

fn with_script(src: &str) -> PlayerApp {
    let mut app = headless_app();
    app.capture.input_script = Some(InputScript::parse(src).unwrap());
    app
}

#[test]
fn scripted_f2_toggles_the_stats_overlay() {
    // Frames at t = 0, 0.25, 0.5: press, release, press.
    let mut app = with_script("0 tap F2\n0.5 tap F2\n");
    app.clock.frame_step = Some(0.25);
    assert!(!app.show_stats);
    app.tick();
    assert!(app.show_stats, "the first tap opens it");
    // Still pressed this frame, as a real key would be.
    assert!(app.input.is_key_down(KeyCode::F2));
    app.input.end_frame();
    app.tick();
    assert!(app.show_stats, "the release doesn't toggle it");
    app.input.end_frame();
    app.tick();
    assert!(!app.show_stats, "the second tap closes it");
}

#[test]
fn scripted_game_keys_reach_input_state_as_before() {
    let mut app = with_script("0 down KeyW\n");
    app.tick();
    assert!(app.input.is_key_down(KeyCode::KeyW));
    assert!(!app.show_stats);
}

#[test]
fn show_stats_opens_the_overlay_once() {
    // Frames at t = 0, 0.25, 0.5: F2 is tapped on the third.
    let mut app = with_script("0.5 tap F2\n");
    app.clock.frame_step = Some(0.25);
    app.capture.show_stats = true;
    app.tick();
    assert!(app.show_stats);
    app.input.end_frame();
    app.tick();
    app.input.end_frame();
    app.tick();
    assert!(!app.show_stats, "F2 still closes it");
}

#[test]
fn a_scripted_click_reaches_input_state_a_frame_after_the_pointer() {
    // Frames at t = 0, 0.25, 0.5: move, press, release (ADR 0082).
    let mut app = with_script("0 click 100 50\n");
    app.clock.frame_step = Some(0.25);
    app.tick();
    assert_eq!(app.input.mouse_position, (100.0, 50.0));
    assert!(!app.input.is_mouse_button_down(0));
    app.tick();
    assert!(app.input.is_mouse_button_down(0));
    app.tick();
    assert!(!app.input.is_mouse_button_down(0));
}
