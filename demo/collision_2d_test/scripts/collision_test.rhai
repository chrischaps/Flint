// Player controller for 2D collision test
// Uses set_velocity_2d for movement, logs collision callbacks

let move_speed = 5.0;

fn on_update() {
    let me = self_entity();
    let dt = delta_time();

    // WASD movement via velocity
    let vx = 0.0;
    let vy = 0.0;

    if is_action_pressed("move_left") {
        vx -= move_speed;
    }
    if is_action_pressed("move_right") {
        vx += move_speed;
    }
    if is_action_just_pressed("jump") {
        vy = 8.0;
    }

    // Only set horizontal velocity; let gravity handle vertical
    let vel = get_velocity_2d(me);
    if type_of(vel) == "map" {
        // Preserve vertical velocity unless jumping
        let cur_vy = vel.vy;
        if vy > 0.0 {
            cur_vy = vy;
        }
        set_velocity_2d(me, vx, cur_vy);
    }

    // Test overlap_rect around the player
    let pos = get_position(me);
    let overlaps = overlap_rect(pos.x - 2.0, pos.y - 2.0, 4.0, 4.0);
    // (could log overlap count if needed)
}

fn on_collision(other_id) {
    let name = entity_name(other_id);
    log("Player collided with: " + name);
}

fn on_collision_exit(other_id) {
    let name = entity_name(other_id);
    log("Player stopped colliding with: " + name);
}
