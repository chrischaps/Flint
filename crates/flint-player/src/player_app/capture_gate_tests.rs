//! The capture and focus gates (ADR 0110): a captured cursor reads as the
//! window centre and its clicks skip egui; pads and raw motion stay out of
//! the game while a text field has focus; real pads are released while the
//! window is unfocused; `look` feeds raw motion. Headless: no window.

use super::egui_input::EguiInputGate;
use super::input_script::InputScript;
use super::PlayerApp;
use flint_ecs::FlintWorld;
use flint_runtime::InputConfig;
use std::sync::{Arc, Mutex};
use winit::event::{DeviceId, ElementState, MouseButton, WindowEvent};

fn headless_app(script: &str) -> PlayerApp {
    let mut app = PlayerApp::new(
        FlintWorld::new(),
        "capture_test.scene.toml".into(),
        false,
        None,
        None,
    );
    app.capture.input_script = Some(InputScript::parse(script).unwrap());
    app.clock.frame_step = Some(0.25);
    app.input.set_screen_size(1280.0, 720.0);
    app.input
        .load_bindings(
            InputConfig::from_toml_str(
                r#"
version = 1
[actions.walk]
kind = "axis1d"
[[actions.walk.bindings]]
type = "gamepad_axis"
axis = "LeftStickY"
gamepad = "any"
"#,
            )
            .unwrap(),
        )
        .unwrap();
    app
}

/// One frame, as the player runs it: tick, then clear the frame's input.
fn frame(app: &mut PlayerApp) {
    app.tick();
    app.input.end_frame();
}

/// An extension with nothing but defaults, so egui sees input first.
struct Bare;
impl crate::extension::PlayerExtension for Bare {}

/// Run one egui frame on the app's context with a text field focused.
fn focus_a_field(app: &PlayerApp) {
    let _ = app.hud.egui_ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut text = String::new();
            ui.text_edit_singleline(&mut text).request_focus();
        });
    });
}

/// Records what the game reads each frame, from `pre_update`, after the
/// frame's input has been routed and before it's cleared.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<((f64, f64), bool)>>>);
impl crate::extension::PlayerExtension for Seen {
    fn pre_update(&mut self, ctx: &mut crate::extension::ExtensionCtx) {
        let seen = (ctx.input.raw_mouse_delta(), ctx.is_cursor_captured());
        self.0.lock().unwrap().push(seen);
    }
}
impl Seen {
    fn last(&self) -> ((f64, f64), bool) {
        *self.0.lock().unwrap().last().unwrap()
    }
}

fn mouse(state: ElementState) -> WindowEvent {
    WindowEvent::MouseInput {
        device_id: DeviceId::dummy(),
        state,
        button: MouseButton::Left,
    }
}

#[test]
fn a_captured_click_skips_egui() {
    let mut gate = EguiInputGate::default();
    assert!(gate.skips_captured(&mouse(ElementState::Pressed)));
    assert!(gate.skips_captured(&mouse(ElementState::Released)));
    let moved = WindowEvent::CursorMoved {
        device_id: DeviceId::dummy(),
        position: winit::dpi::PhysicalPosition::new(4.0, 4.0),
    };
    assert!(gate.skips_captured(&moved));
    // A press egui took before the capture keeps its release, so the
    // button isn't left down in egui.
    let left = super::egui_input::Held::Mouse(MouseButton::Left);
    assert!(gate.consumes(super::egui_input::Routed::Press(left), true, false));
    assert!(!gate.skips_captured(&mouse(ElementState::Released)));
    // Keys aren't the capture's business.
    assert!(!gate.skips_captured(&WindowEvent::RedrawRequested));
}

#[test]
fn a_captured_cursor_reads_the_centre_and_still_clicks() {
    // Frames at t = 0, 0.25, 0.5, 0.75: move, then a click.
    let mut app = headless_app("0 mouse 100 50\n0.5 click 900 600\n");
    frame(&mut app);
    assert_eq!(app.input.mouse_position, (100.0, 50.0));
    app.capture_cursor();
    assert!(app.is_cursor_captured());
    assert_eq!(app.input.mouse_position, (640.0, 360.0));
    frame(&mut app);
    assert!(app.script.cursor_captured(), "scripts read it too");
    // The click's move reaches no one; its button reaches the game.
    app.tick();
    assert_eq!(app.input.mouse_position, (640.0, 360.0));
    app.input.end_frame();
    app.tick();
    assert!(app.input.is_mouse_button_down(0));
    app.input.end_frame();
    app.release_cursor();
    assert!(!app.is_cursor_captured());
}

#[test]
fn look_feeds_raw_motion_only_while_captured() {
    let mut app = headless_app("0 look 40 -10\n0.25 look 7 0\n");
    let seen = Seen::default();
    app.add_extension(Box::new(seen.clone()));
    frame(&mut app);
    assert_eq!(seen.last(), ((0.0, 0.0), false), "not captured");
    app.capture_cursor();
    frame(&mut app);
    // What `mouse_delta_x/y` read, and extensions see the capture.
    assert_eq!(seen.last(), ((7.0, 0.0), true));
}

#[test]
fn look_is_held_while_a_field_has_focus() {
    let mut app = headless_app("0 look 40 -10\n");
    let seen = Seen::default();
    app.add_extension(Box::new(seen.clone()));
    app.capture_cursor();
    focus_a_field(&app);
    frame(&mut app);
    assert_eq!(seen.last(), ((0.0, 0.0), true));
}

#[test]
fn pad_axes_read_zero_while_a_field_has_focus() {
    let mut app = headless_app("0 axis LeftStickY 0.8\n");
    app.add_extension(Box::new(Bare));
    focus_a_field(&app);
    app.tick();
    assert_eq!(app.input.action_value("walk"), 0.0);
    app.input.end_frame();
    // The field lets go: the stick reads where it stands.
    let _ = app.hud.egui_ctx.run(egui::RawInput::default(), |ctx| {
        ctx.memory_mut(|m| m.stop_text_input());
    });
    assert!(!app.hud.egui_ctx.wants_keyboard_input());
    app.tick();
    assert!(app.input.action_value("walk") > 0.7);
}

/// Run one egui frame on the app's context with a button focused.
fn focus_a_button(app: &PlayerApp) {
    let _ = app.hud.egui_ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.button("Buy").request_focus();
        });
    });
}

#[test]
fn a_focused_button_leaves_the_pads_to_the_game() {
    // egui counts any focus as wanting the keyboard; the gate counts only
    // a text field (ADR 0110's amendment), so a pad ring's focused button
    // between frames holds nothing.
    let mut app = headless_app(
        "0 axis LeftStickY 0.8
",
    );
    app.add_extension(Box::new(Bare));
    focus_a_button(&app);
    assert!(app.hud.egui_ctx.wants_keyboard_input());
    assert!(!app.text_has_focus());
    app.tick();
    assert!(app.input.action_value("walk") > 0.7);
}

#[test]
fn pad_axes_read_zero_while_the_window_is_unfocused() {
    let mut app = headless_app("");
    // A real pad on slot 0 (gilrs can't be faked headless; its events
    // record the slot as this does).
    app.focus.real_pad_slots.insert(0);
    app.input.process_gamepad_axis(0, "LeftStickY", 0.8);
    assert!(app.input.action_value("walk") > 0.7);
    app.window_focus_changed(false);
    assert_eq!(app.input.action_value("walk"), 0.0);
    assert!(!app.focus.window_focused);
    app.window_focus_changed(true);
    assert!(app.focus.window_focused);
}

#[test]
fn a_capture_hears_its_script_alone() {
    let device_id = DeviceId::dummy();
    let wheel = WindowEvent::MouseWheel {
        device_id,
        delta: winit::event::MouseScrollDelta::LineDelta(0.0, 3.0),
        phase: winit::event::TouchPhase::Moved,
    };
    let click = WindowEvent::MouseInput {
        device_id,
        state: ElementState::Pressed,
        button: MouseButton::Left,
    };
    let moved = WindowEvent::CursorMoved {
        device_id,
        position: winit::dpi::PhysicalPosition::new(10.0, 20.0),
    };
    let resized = WindowEvent::Resized(winit::dpi::PhysicalSize::new(800, 600));
    // An input script, a screenshot or an exit time makes a capture
    let mut app = headless_app(
        "1.0 wheel 2
",
    );
    assert!(app.capture.is_capture());
    for real in [&wheel, &click, &moved] {
        assert!(app.capture_drops(real), "{real:?}");
    }
    assert!(
        !app.capture_drops(&resized),
        "lifecycle events still arrive"
    );
    assert!(!app.capture_drops(&WindowEvent::Focused(true)));
    app.capture.input_script = None;
    app.capture.exit_after = Some(4.0);
    assert!(app.capture_drops(&wheel));
    app.capture.exit_after = None;
    app.capture.path = Some("shot.png".into());
    assert!(app.capture_drops(&wheel));
    // A run that's no capture hears the real mouse
    app.capture.path = None;
    assert!(!app.capture.is_capture());
    assert!(!app.capture_drops(&wheel));
}

#[test]
fn a_captures_script_still_reaches_the_game() {
    // The script's keys take the path a real keyboard's does, past the
    // capture's gate: the frame they play on, the game sees them.
    let mut app = headless_app(
        "0 down KeyW
",
    );
    app.add_extension(Box::new(Bare));
    app.tick();
    assert!(app.input.is_key_down(winit::keyboard::KeyCode::KeyW));
}

#[test]
fn the_script_pad_plays_on_while_the_window_is_unfocused() {
    // Captures run unfocused: the input script's pad isn't a real one.
    let mut app = headless_app("0 axis LeftStickY 0.8\n");
    app.window_focus_changed(false);
    app.tick();
    assert!(app.input.action_value("walk") > 0.7);
}

#[test]
fn escape_is_the_games_when_it_binds_it() {
    use super::events::Escape;
    // Unbound: let go of the cursor, or else close the window.
    let captured = Escape::decide(true, false);
    assert!(captured.release_cursor && !captured.exit && !captured.to_game);
    let free = Escape::decide(false, false);
    assert!(!free.release_cursor && free.exit && !free.to_game);
    // Bound (#639): the game hears it and the window stays open; a
    // captured cursor still lets go, so its pause menu can be clicked.
    let free = Escape::decide(false, true);
    assert!(!free.exit && free.to_game && !free.release_cursor);
    let captured = Escape::decide(true, true);
    assert!(!captured.exit && captured.to_game && captured.release_cursor);
}

#[test]
fn a_shot_that_never_reached_the_disk_fails_the_run() {
    use super::{CaptureConfig, CAPTURE_RETRIES};
    // No screenshot asked for: nothing can be missing.
    assert_eq!(
        CaptureConfig::new(None, 0.0, None, Some(1.0)).missing(),
        None
    );

    // Asked for, and the run ended first.
    let mut capture = CaptureConfig::new(Some("x.png".into()), 3.0, None, Some(2.0));
    capture.last_frame_time = Some(2.0);
    let why = capture.missing().expect("missing");
    assert!(
        why.contains("x.png") && why.contains("before --screenshot-at"),
        "{why}"
    );

    // A failed read-back is retried, and `--exit-after` waits for it.
    let mut capture = CaptureConfig::new(Some("x.png".into()), 1.0, None, Some(1.2));
    assert!(!capture.shot_pending(0.5), "not due yet");
    assert!(capture.shot_pending(1.2));
    capture.taken = true;
    capture.note_failure("read-back of x.png failed: lost".into());
    assert!(!capture.taken, "tried again next frame");
    assert!(capture.shot_pending(1.3), "the exit waits");
    capture.taken = true;
    capture.written = 1;
    assert!(!capture.shot_pending(1.3));
    assert_eq!(capture.missing(), None, "a retry that wrote it passes");

    // Retries run out: the exit stops waiting and the reason is reported.
    let mut capture = CaptureConfig::new(Some("x.png".into()), 0.0, None, Some(1.0));
    for _ in 0..=CAPTURE_RETRIES {
        capture.taken = true;
        capture.note_failure("read-back of x.png failed: lost".into());
    }
    assert!(capture.taken, "given up");
    assert!(!capture.shot_pending(2.0));
    let why = capture.missing().expect("missing");
    assert!(
        why.contains("never written") && why.contains("read-back"),
        "{why}"
    );

    // A sequence that lost a frame fails too: a capture needs them all.
    let mut capture = CaptureConfig::new(Some("f.png".into()), 0.0, Some(0.1), None);
    capture.written = 9;
    capture.note_failure("failed to save f_0004.png: disk full".into());
    let why = capture.missing().expect("missing");
    assert!(why.contains("lost 1 of 10"), "{why}");
}

#[test]
fn a_player_that_failed_to_start_reports_it() {
    let mut app = headless_app("");
    assert_eq!(app.run_failure(), None);
    app.outcome.init_error = Some("Failed to get adapter".into());
    let why = app.run_failure().expect("failure");
    assert!(why.contains("failed to start"), "{why}");
}
