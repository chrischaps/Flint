// WP6 Screen-Space UI Test Script
// Arrow keys move player, Space jumps, E decreases health, R restores
// Score increments over time

let health = 1.0;
let score = 0;
let score_timer = 0.0;
let me;

fn on_init() {
    me = self_entity();
    camera_follow(me, 0.0, 0.0, 5.0, 1.0, 1.0);
}

fn on_update() {
    let dt = delta_time();

    // Movement
    let vx = 0.0;
    if is_action_pressed("move_right") {
        vx = 5.0;
    }
    if is_action_pressed("move_left") {
        vx = -5.0;
    }

    let vel = get_velocity_2d(me);
    set_velocity_2d(me, vx, vel.vy);

    // Jump
    if is_action_just_pressed("jump") {
        if vel.vy.abs() < 0.5 {
            set_velocity_2d(me, vx, 8.0);
        }
    }

    // E decreases health
    if is_action_just_pressed("interact") {
        health -= 0.1;
        if health < 0.0 { health = 0.0; }

        let fill_id = get_entity("health_fill");
        if fill_id >= 0 {
            ui_set_value(fill_id, health);
        }
    }

    // R restores health (use action_value trick: bind R to an action, or just check move_up)
    // For simplicity, use move_up as a "restore" trigger
    if is_action_just_pressed("move_up") {
        health = 1.0;
        let fill_id = get_entity("health_fill");
        if fill_id >= 0 {
            ui_set_value(fill_id, health);
        }
    }

    // Score increments over time
    score_timer += dt;
    if score_timer >= 0.5 {
        score_timer -= 0.5;
        score += 10;

        let text_id = get_entity("score_text");
        if text_id >= 0 {
            set_text(text_id, "SCORE: " + score);
        }
    }

    // Warmth indicator pulses
    let warmth_id = get_entity("warmth_indicator");
    if warmth_id >= 0 {
        let t = score_timer * 6.28;
        let pulse = 0.6 + 0.4 * t.sin();
        set_field(warmth_id, "sprite", "tint", [1.0, 0.5 * pulse, 0.1, pulse]);
    }
}

fn on_draw_ui() {
    draw_text(10.0, 10.0, "WP6: Screen UI Test", 18.0, 1.0, 1.0, 1.0, 1.0);
    draw_text(10.0, 32.0, "A/D: move | Space: jump | E: damage | W: heal", 14.0, 0.8, 0.8, 0.8, 1.0);
    draw_text(10.0, 50.0, "Health: " + (health * 100.0) + "%", 14.0, 0.7, 0.9, 0.7, 1.0);
}
