// WP3a touch input validation script
//
// Left-half touch / A key  → move left
// Right-half touch / D key → move right
// Tap / Space key           → jump (impulse)
// Active touch points drawn as circles

let move_speed = 6.0;
let jump_impulse = 8.0;
let gravity = -16.0;
let vel_y = 0.0;
let ground_y = -3.0;

fn on_init() {
    log_info("Touch demo initialized — tap / click to test");
}

fn on_update() {
    let me = self_entity();
    let dt = delta_time();
    let pos = get_position(me);
    let x = pos.x;
    let y = pos.y;

    // Movement via unified action system (keyboard + touch zones)
    if is_action_pressed("move_left") {
        x -= move_speed * dt;
    }
    if is_action_pressed("move_right") {
        x += move_speed * dt;
    }

    // Jump on tap or space
    let taps = tap_count();
    if taps > 0 || is_action_just_pressed("jump") {
        if y <= ground_y + 0.1 {
            vel_y = jump_impulse;
        }
    }

    // Gravity
    vel_y += gravity * dt;
    y += vel_y * dt;
    if y < ground_y {
        y = ground_y;
        vel_y = 0.0;
    }

    // Clamp to screen
    if x < -7.0 { x = -7.0; }
    if x > 7.0 { x = 7.0; }

    set_position(me, x, y, pos.z);
}

fn on_draw_ui() {
    let sw = screen_width();
    let sh = screen_height();

    // Draw a subtle dividing line at screen center
    draw_line_ex(sw / 2.0, 0.0, sw / 2.0, sh, 0.3, 0.3, 0.4, 0.3, 1.0, -1);

    // Label the zones
    draw_text_ex(10.0, 20.0, "LEFT", 18.0, 0.5, 0.5, 0.6, 0.5, -1);
    draw_text_ex(sw / 2.0 + 10.0, 20.0, "RIGHT", 18.0, 0.5, 0.5, 0.6, 0.5, -1);

    // Draw circles at all active touch points
    let count = touch_count();
    let i = 0;
    while i < count {
        let tx = touch_x(i);
        let ty = touch_y(i);
        if tx >= 0.0 {
            // Convert normalized coords to screen coords
            let sx = tx * sw;
            let sy = ty * sh;
            draw_circle_ex(sx, sy, 20.0, 1.0, 0.4, 0.2, 0.6, 1);
        }
        i += 1;
    }

    // Draw tap indicators
    let tc = tap_count();
    let j = 0;
    while j < tc {
        let tx = tap_x(j) * sw;
        let ty = tap_y(j) * sh;
        draw_circle_ex(tx, ty, 30.0, 0.2, 1.0, 0.4, 0.8, 2);
        j += 1;
    }

    // Status line
    let status = "Touches: " + count + "  Taps: " + tc;
    draw_text_ex(10.0, sh - 30.0, status, 16.0, 0.7, 0.7, 0.7, 1.0, 0);
}
