//! The `input` module: actions, the mouse, gamepads, touch, the safe area
//! and cursor capture, read from the frame's input snapshot (ADR 0116).

use super::builder::kind::{Bool, Int, Map, Num, Str};
use super::builder::ScriptApi;

pub(crate) fn register(api: &mut ScriptApi) {
    let mut m = api.module("input");

    // ── Actions ──
    m.func("is_action_pressed", "Whether the action is held.")
        .arg("action", Str)
        .returns(Bool, "false for an action nothing binds")
        .run(|call, (action,)| {
            call.ctx_ref()
                .input
                .actions_pressed
                .contains(action.as_str())
        });

    m.func(
        "is_action_just_pressed",
        "Whether the action was pressed this frame.",
    )
    .arg("action", Str)
    .returns(Bool, "")
    .run(|call, (action,)| {
        call.ctx_ref()
            .input
            .actions_just_pressed
            .contains(action.as_str())
    });

    m.func(
        "is_action_just_released",
        "Whether the action was released this frame.",
    )
    .arg("action", Str)
    .returns(Bool, "")
    .run(|call, (action,)| {
        call.ctx_ref()
            .input
            .actions_just_released
            .contains(action.as_str())
    });

    m.func("action_value", "The analog value of an axis action.")
        .arg("action", Str)
        .returns(Num, "0.0 when nothing binds it")
        .run(|call, (action,)| {
            call.ctx_ref()
                .input
                .action_values
                .get(action.as_str())
                .copied()
                .unwrap_or(0.0)
        });

    // ── Devices ──
    m.func(
        "any_input_just_pressed",
        "Whether any key, mouse or pad button was pressed, or a finger landed, this frame; bound or not.",
    )
    .returns(Bool, "")
    .run(|call, ()| call.ctx_ref().input.any_just_pressed);

    m.func(
        "last_input_device",
        "The device of the most recent deliberate input (a press, a stick past the noise floor, a touch).",
    )
    .returns(Str, "\"keyboard\", \"mouse\", \"gamepad\" or \"touch\"; \"keyboard\" before any input")
    .run(|call, ()| {
        let device = &call.ctx_ref().input.last_device;
        if device.is_empty() {
            "keyboard".to_string()
        } else {
            device.clone()
        }
    });

    m.func(
        "gamepad_connected",
        "Whether at least one gamepad is attached.",
    )
    .returns(Bool, "")
    .run(|call, ()| call.ctx_ref().input.gamepad_connected);

    // ── Touch ──
    m.func(
        "touch_controls_visible",
        "Whether the engine is drawing the input config's on-screen sticks and buttons now.",
    )
    .returns(Bool, "")
    .run(|call, ()| call.ctx_ref().input.touch_controls_visible);

    m.func(
        "is_touch_device",
        "Whether this is a touch-first device (a pointer: coarse page, Android), from the first frame.",
    )
    .returns(Bool, "")
    .run(|call, ()| call.ctx_ref().input.touch_device);

    m.func(
        "safe_area",
        "The safe-area insets in points (a notch, a rounded corner, a home bar).",
    )
    .returns(
        Map,
        "#{left, top, right, bottom}; zeros where there are none",
    )
    .run(|call, ()| {
        let [left, top, right, bottom] = call.ctx_ref().input.safe_area;
        let mut map = rhai::Map::new();
        map.insert("left".into(), left.into());
        map.insert("top".into(), top.into());
        map.insert("right".into(), right.into());
        map.insert("bottom".into(), bottom.into());
        map
    });

    m.func(
        "set_touch_controls",
        "Put the on-screen controls away (false) or bring them back (true), from the end of this frame.",
    )
    .arg("enabled", Bool)
    .run(|call, (enabled,)| {
        call.ctx().requests.touch_controls = Some(enabled);
    });

    // ── Mouse ──
    m.func("mouse_delta_x", "Horizontal mouse movement this frame.")
        .returns(Num, "")
        .run(|call, ()| call.ctx_ref().input.mouse_delta.0);

    m.func("mouse_delta_y", "Vertical mouse movement this frame.")
        .returns(Num, "")
        .run(|call, ()| call.ctx_ref().input.mouse_delta.1);

    m.func(
        "mouse_wheel_x",
        "Horizontal wheel lines this frame, right positive; zero over egui.",
    )
    .returns(Num, "")
    .run(|call, ()| call.ctx_ref().input.mouse_wheel.0);

    m.func(
        "mouse_wheel_y",
        "Vertical wheel lines (notches) this frame, up positive; zero over egui.",
    )
    .returns(Num, "")
    .run(|call, ()| call.ctx_ref().input.mouse_wheel.1);

    m.func(
        "mouse_x",
        "The cursor's x in logical points (the screen_width() and draw_* space).",
    )
    .returns(Num, "")
    .run(|call, ()| call.ctx_ref().mouse_pos_logical.0 as f64);

    m.func(
        "mouse_y",
        "The cursor's y in logical points (the screen_height() and draw_* space).",
    )
    .returns(Num, "")
    .run(|call, ()| call.ctx_ref().mouse_pos_logical.1 as f64);

    m.func(
        "is_mouse_pressed",
        "Whether a mouse button is held: 0 left, 1 right, 2 middle.",
    )
    .arg("button", Int)
    .returns(Bool, "")
    .run(|call, (button,)| call.ctx_ref().input.mouse_buttons_down.contains(&button));

    m.func(
        "is_mouse_just_pressed",
        "Whether a mouse button went down this frame: 0 left, 1 right, 2 middle.",
    )
    .arg("button", Int)
    .returns(Bool, "")
    .run(|call, (button,)| {
        call.ctx_ref()
            .input
            .mouse_buttons_just_pressed
            .contains(&button)
    });

    // ── Cursor ──
    m.func(
        "set_cursor_captured",
        "Capture (hide and lock) or release the mouse cursor, at the end of this frame.",
    )
    .arg("captured", Bool)
    .run(|call, (captured,)| {
        call.ctx().requests.cursor_captured = Some(captured);
    });

    m.func(
        "is_cursor_captured",
        "Whether the cursor is captured now; a request shows from the next frame.",
    )
    .returns(Bool, "")
    .run(|call, ()| call.ctx_ref().cursor_captured);
}
