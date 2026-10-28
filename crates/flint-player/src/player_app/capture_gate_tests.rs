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
    let _ = app.egui_ctx.run(egui::RawInput::default(), |ctx| {
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
    let _ = app.egui_ctx.run(egui::RawInput::default(), |ctx| {
        ctx.memory_mut(|m| m.stop_text_input());
    });
    assert!(!app.egui_ctx.wants_keyboard_input());
    app.tick();
    assert!(app.input.action_value("walk") > 0.7);
}

/// Run one egui frame on the app's context with a button focused.
fn focus_a_button(app: &PlayerApp) {
    let _ = app.egui_ctx.run(egui::RawInput::default(), |ctx| {
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
    let mut app = headless_app("0 axis LeftStickY 0.8
");
    app.add_extension(Box::new(Bare));
    focus_a_button(&app);
    assert!(app.egui_ctx.wants_keyboard_input());
    assert!(!app.text_has_focus());
    app.tick();
    assert!(app.input.action_value("walk") > 0.7);
}

#[test]
fn pad_axes_read_zero_while_the_window_is_unfocused() {
    let mut app = headless_app("");
    // A real pad on slot 0 (gilrs can't be faked headless; its events
    // record the slot as this does).
    app.real_pad_slots.insert(0);
    app.input.process_gamepad_axis(0, "LeftStickY", 0.8);
    assert!(app.input.action_value("walk") > 0.7);
    app.window_focus_changed(false);
    assert_eq!(app.input.action_value("walk"), 0.0);
    assert!(!app.window_focused);
    app.window_focus_changed(true);
    assert!(app.window_focused);
}

#[test]
fn the_script_pad_plays_on_while_the_window_is_unfocused() {
    // Captures run unfocused: the input script's pad isn't a real one.
    let mut app = headless_app("0 axis LeftStickY 0.8\n");
    app.window_focus_changed(false);
    app.tick();
    assert!(app.input.action_value("walk") > 0.7);
}
