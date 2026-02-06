// ─── footsteps.rhai ──────────────────────────────────────
// Plays footstep sounds at regular intervals when the player
// is moving. Faster interval when sprinting.

let step_timer = 0.0;
let step_interval = 0.4;
let sprint_interval = 0.25;

fn on_update(dt) {
    let me = self_entity();

    // Check if any movement key is pressed
    let moving = is_action_pressed("move_forward")
              || is_action_pressed("move_backward")
              || is_action_pressed("move_left")
              || is_action_pressed("move_right");

    if !moving {
        step_timer = 0.0;
        return;
    }

    let sprinting = is_action_pressed("sprint");
    let interval = if sprinting { sprint_interval } else { step_interval };

    step_timer += dt;
    if step_timer >= interval {
        step_timer -= interval;
        let pos = get_position(me);
        play_sound_at("audio/footstep_stone.wav", pos.x, pos.y, pos.z, 0.5);
    }
}
