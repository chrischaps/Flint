// WP5 Scrolling World Test Script
// Tests: camera follow, deadzone, screen shake, chunk loading/unloading

let MOVE_SPEED = 8.0;
let JUMP_FORCE = 12.0;
let me;
let grounded = false;
let chunk_threshold = 12.0;

fn on_init() {
    me = self_entity();
    // Teleport camera to player position initially
    let pos = get_position(me);
    camera_follow_set(pos.x, pos.y + 2.0);
}

fn on_update() {
    let dt = delta_time();

    // --- Movement ---
    let vel = get_velocity_2d(me);
    let vx = 0.0;

    if is_action_pressed("move_left") {
        vx = -MOVE_SPEED;
    }
    if is_action_pressed("move_right") {
        vx = MOVE_SPEED;
    }

    // Jump (only when grounded — check by velocity being near zero)
    let vy = vel.vy;
    if is_action_just_pressed("jump") {
        if vy.abs() < 0.5 {
            vy = JUMP_FORCE;
        }
    }

    set_velocity_2d(me, vx, vy);

    // --- Camera follow with deadzone ---
    // offset_y=2 looks ahead; speed=8 for responsive tracking
    // deadzone 1.5 wide, 2.0 tall — small movements don't jerk camera
    camera_follow(me, 0.0, 2.0, 8.0, 1.5, 2.0);

    // --- Screen shake on interact action ---
    if is_action_just_pressed("interact") {
        camera_shake(0.3, 20.0, 6.0);
    }

    // --- Chunk streaming ---
    let cam = camera_follow_position();
    if cam.y > chunk_threshold && !is_chunk_loaded("upper_platforms") {
        load_chunk("chunks/platforms.chunk.toml", 0.0, 15.0, "upper_platforms");
    }
    if cam.y < chunk_threshold - 5.0 && is_chunk_loaded("upper_platforms") {
        unload_chunk("upper_platforms");
    }
}

fn on_draw_ui() {
    let cam = camera_follow_position();
    draw_text(10.0, 10.0, "WP5: Scrolling Test", 18.0, 1.0, 1.0, 1.0, 1.0);
    draw_text(10.0, 32.0, "A/D: move | Space: jump | E: shake", 14.0, 0.8, 0.8, 0.8, 1.0);
    draw_text(10.0, 50.0, "Camera Y: " + cam.y, 14.0, 0.7, 0.9, 0.7, 1.0);
    if is_chunk_loaded("upper_platforms") {
        draw_text(10.0, 68.0, "Chunk 'upper_platforms': LOADED", 14.0, 0.3, 1.0, 0.3, 1.0);
    } else {
        draw_text(10.0, 68.0, "Chunk: not loaded (climb higher!)", 14.0, 0.6, 0.6, 0.6, 1.0);
    }
}

fn on_collision(other_id) {
    // Track grounded state when colliding with anything below
    grounded = true;
}
