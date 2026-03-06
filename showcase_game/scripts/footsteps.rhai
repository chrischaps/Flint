// ─── footsteps.rhai ──────────────────────────────────────
// Plays footstep sounds at regular intervals when the player
// is moving. Faster interval when sprinting. Randomly picks
// from 7 individual step samples for natural variation.

let step_timer = 0.0;
let step_interval = 0.4;
let sprint_interval = 0.25;
let num_steps = 7;

fn on_update() {
    let dt = delta_time();
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
        let idx = to_int(floor(random() * num_steps)) + 1;
        if idx > num_steps { idx = num_steps; }
        let file = "audio/footstep_stone_" + idx + ".ogg";
        play_sound(file, 0.5);
    }
}
