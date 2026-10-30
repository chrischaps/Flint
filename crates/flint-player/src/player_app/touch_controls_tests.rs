//! The on-screen touch controls in the player (#133): scripted fingers take
//! a real one's path, the controls hold theirs before extensions (and egui)
//! can, and a key hides them. Headless: no window, scale 1.

use super::input_script::InputScript;
use super::PlayerApp;
use flint_ecs::FlintWorld;
use flint_runtime::InputConfig;
use winit::event::WindowEvent;

fn headless_app(script: &str) -> PlayerApp {
    let mut app = PlayerApp::new(
        FlintWorld::new(),
        "touch_test.scene.toml".into(),
        false,
        None,
        None,
    );
    app.capture.input_script = Some(InputScript::parse(script).unwrap());
    app.clock.frame_step = Some(0.25);
    app.input.set_screen_size(1000.0, 500.0);
    app.input
        .load_bindings(
            InputConfig::from_toml_str(
                r#"
version = 1
[actions.move_forward]
bindings = [{ type = "key", code = "KeyW" }]
[actions.jump]
bindings = [{ type = "key", code = "Space" }]

[touch_controls]
[[touch_controls.sticks]]
up = "move_forward"
radius = 50
[[touch_controls.buttons]]
action = "jump"
anchor = "bottom_right"
offset = [80, 80]
radius = 30
"#,
            )
            .unwrap(),
        )
        .unwrap();
    app
}

fn frame(app: &mut PlayerApp) {
    app.tick();
    app.input.end_frame();
}

/// An extension that takes every finger it's offered.
struct Greedy;
impl crate::extension::PlayerExtension for Greedy {
    fn on_window_event(&mut self, event: &WindowEvent) -> bool {
        matches!(event, WindowEvent::Touch(_))
    }
}

#[test]
fn scripted_thumbs_walk_and_jump_past_a_greedy_extension() {
    let mut app = headless_app(
        "0 touch 1 150 400\n0 touch 2 920 420\n0.25 drag 1 150 320\n\
         0.75 lift 2\n0.75 touch 3 600 100\n",
    );
    app.add_extension(Box::new(Greedy));
    frame(&mut app); // t = 0: both thumbs land
    assert!(app.input.touch_controls_visible());
    assert!(app.input.is_action_pressed("jump"), "the button kept its finger");
    frame(&mut app); // 0.25: the stick pushes up
    assert!(app.input.is_action_pressed("move_forward"));
    frame(&mut app); // 0.5
    frame(&mut app); // 0.75: jump lifts; a finger off the controls...
    assert!(!app.input.is_action_pressed("jump"));
    assert!(app.input.is_action_pressed("move_forward"));
    // ...is the extension's: the game never sees it.
    assert_eq!(app.input.touch_count(), 0);
}

#[test]
fn a_key_hides_the_controls_and_a_finger_brings_them_back() {
    let mut app = headless_app("0 touch 1 150 400\n0.25 tap KeyW\n0.75 touch 2 600 100\n");
    frame(&mut app);
    assert!(app.input.touch_controls_visible());
    frame(&mut app); // 0.25: W
    assert!(!app.input.touch_controls_visible());
    frame(&mut app);
    frame(&mut app); // 0.75
    assert!(app.input.touch_controls_visible());
}
