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
    app.core.clock.frame_step = Some(0.25);
    assert!(!app.stats.show);
    app.tick();
    assert!(app.stats.show, "the first tap opens it");
    // Still pressed this frame, as a real key would be.
    assert!(app.input.is_key_down(KeyCode::F2));
    app.input.end_frame();
    app.tick();
    assert!(app.stats.show, "the release doesn't toggle it");
    app.input.end_frame();
    app.tick();
    assert!(!app.stats.show, "the second tap closes it");
}

/// A debug key with nothing behind it says so where the person pressing it
/// will see it (#367): it used to log at `info`, which the player never prints.
#[cfg(feature = "debug-hud")]
#[test]
fn f3_and_f4_with_nothing_to_toggle_say_so_on_screen() {
    let mut app = with_script("0 tap F3\n0.25 tap F4\n");
    app.core.clock.frame_step = Some(0.25);
    assert!(app.debug_panels.is_empty(), "a scene with no panels");
    assert!(app.hud.key_note.showing().is_none());
    app.tick();
    assert_eq!(
        app.hud.key_note.showing(),
        Some(("F3", "no debug panels in this scene"))
    );
    app.input.end_frame();
    app.tick();
    assert_eq!(
        app.hud.key_note.showing(),
        Some(("F4", "no renderer active — nothing to tune")),
        "the next key's note replaces it"
    );
}

#[test]
fn scripted_game_keys_reach_input_state_as_before() {
    let mut app = with_script("0 down KeyW\n");
    app.tick();
    assert!(app.input.is_key_down(KeyCode::KeyW));
    assert!(!app.stats.show);
}

#[test]
fn show_stats_opens_the_overlay_once() {
    // Frames at t = 0, 0.25, 0.5: F2 is tapped on the third.
    let mut app = with_script("0.5 tap F2\n");
    app.core.clock.frame_step = Some(0.25);
    app.capture.show_stats = true;
    app.tick();
    assert!(app.stats.show);
    app.input.end_frame();
    app.tick();
    app.input.end_frame();
    app.tick();
    assert!(!app.stats.show, "F2 still closes it");
}

/// An extension with nothing but defaults, so egui sees input first.
struct Bare;
impl crate::extension::PlayerExtension for Bare {}

/// Run one egui frame on the app's context with a text field focused.
fn focus_a_field(app: &PlayerApp) {
    let _ = app.hud.egui_ctx.run_ui(egui::RawInput::default(), |ui| {
        let ctx = &ui.ctx().clone();
        egui::CentralPanel::default().show(ui, |ui| {
            let mut text = String::new();
            ui.text_edit_singleline(&mut text).request_focus();
        });
    });
}

#[test]
fn a_focused_egui_field_keeps_scripted_keys_from_the_game() {
    // Frames at t = 0, 0.25, 0.5 (ADR 0097).
    let mut app = with_script("0 down KeyW\n0.25 up KeyW\n0.5 tap F2\n");
    app.core.clock.frame_step = Some(0.25);
    app.add_extension(Box::new(Bare));
    focus_a_field(&app);
    assert!(app.hud.egui_ctx.egui_wants_keyboard_input());
    app.tick();
    assert!(!app.input.is_key_down(KeyCode::KeyW), "the field took it");
    app.tick();
    app.tick();
    assert!(!app.stats.show, "the debug keys don't see it either");
}

#[test]
fn a_focused_button_leaves_scripted_keys_to_the_game() {
    let mut app = with_script(
        "0 down KeyW
",
    );
    app.add_extension(Box::new(Bare));
    let _ = app.hud.egui_ctx.run_ui(egui::RawInput::default(), |ui| {
        let ctx = &ui.ctx().clone();
        egui::CentralPanel::default().show(ui, |ui| {
            ui.button("Buy").request_focus();
        });
    });
    assert!(
        app.hud.egui_ctx.egui_wants_keyboard_input(),
        "egui counts any focus"
    );
    app.tick();
    assert!(
        app.input.is_key_down(KeyCode::KeyW),
        "only a field takes keys"
    );
}

#[test]
fn scripted_keys_reach_the_game_when_no_field_has_focus() {
    let mut app = with_script("0 down ShiftLeft\n0 down KeyW\n");
    app.add_extension(Box::new(Bare));
    app.tick();
    assert!(app.input.is_key_down(KeyCode::ShiftLeft));
    assert!(app.input.is_key_down(KeyCode::KeyW));
}

#[test]
fn a_release_follows_a_scripted_press_that_reached_the_game() {
    // The game saw W go down, then a field took focus: the release still
    // reaches the game, so W isn't stuck down there.
    let mut app = with_script("0 down KeyW\n0.25 up KeyW\n");
    app.core.clock.frame_step = Some(0.25);
    app.add_extension(Box::new(Bare));
    app.tick();
    assert!(app.input.is_key_down(KeyCode::KeyW));
    focus_a_field(&app);
    app.tick();
    assert!(!app.input.is_key_down(KeyCode::KeyW));
}

#[test]
fn a_scripted_click_reaches_input_state_a_frame_after_the_pointer() {
    // Frames at t = 0, 0.25, 0.5: move, press, release (ADR 0082).
    let mut app = with_script("0 click 100 50\n");
    app.core.clock.frame_step = Some(0.25);
    app.tick();
    assert_eq!(app.input.mouse_position, (100.0, 50.0));
    assert!(!app.input.is_mouse_button_down(0));
    app.tick();
    assert!(app.input.is_mouse_button_down(0));
    app.tick();
    assert!(!app.input.is_mouse_button_down(0));
}
