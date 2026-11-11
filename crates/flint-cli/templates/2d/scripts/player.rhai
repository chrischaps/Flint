// Steers the player sprite with the engine's built-in move actions (WASD).
// The field is 2D, so "forward" is up the screen.

let speed = 6.0;
// Half the field the camera shows, less half the sprite
let limit_x = 10.0;
let limit_y = 5.5;

fn on_update() {
    let me = self_entity();
    let dt = delta_time();
    let dx = 0.0;
    let dy = 0.0;
    if is_action_pressed("move_left") { dx -= 1.0; }
    if is_action_pressed("move_right") { dx += 1.0; }
    if is_action_pressed("move_forward") { dy += 1.0; }
    if is_action_pressed("move_backward") { dy -= 1.0; }
    if dx == 0.0 && dy == 0.0 { return; }

    let pos = get_position(me);
    let x = clamp(pos.x + dx * speed * dt, -limit_x, limit_x);
    let y = clamp(pos.y + dy * speed * dt, -limit_y, limit_y);
    set_position(me, x, y, pos.z);
}
