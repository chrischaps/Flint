// ─── npc_bartender.rhai ──────────────────────────────────
// The bartender: alert and attentive. Speeds up idle animation
// when player approaches, turns to face them, greets on interact.

let base_speed = 1.0;
let greeted = false;

fn on_init() {
    let me = self_entity();
    let spd = get_field(me, "animator", "speed");
    if spd != () {
        base_speed = spd;
    }
    log("Bartender ready: " + entity_name(me));
}

fn on_update(dt) {
    let me = self_entity();
    let player = get_entity("player");
    if player < 0 { return; }

    let dist = distance(me, player);

    // Within 6 units: become alert (speed up idle)
    if dist < 6.0 {
        let alertness = clamp(1.0 - (dist - 3.0) / 3.0, 0.0, 1.0);
        let speed = lerp(base_speed, base_speed * 1.4, alertness);
        set_anim_speed(me, speed);

        // Within 3 units: face the player
        if dist < 3.0 {
            let my_pos = get_position(me);
            let pl_pos = get_position(player);
            let dx = pl_pos.x - my_pos.x;
            let dz = pl_pos.z - my_pos.z;
            // Facing angle in degrees
            let angle = atan2(dx, dz) * 57.2958;
            let rot = get_rotation(me);
            let target_y = angle;
            let current_y = rot.y;
            let new_y = lerp(current_y, target_y, dt * 3.0);
            set_rotation(me, rot.x, new_y, rot.z);
        }
    } else {
        set_anim_speed(me, base_speed);
    }
}

fn on_interact() {
    let me = self_entity();
    play_sound("audio/npc_greeting.wav");

    if !greeted {
        log("\"Welcome to the Rusty Flint! What'll it be?\"");
        greeted = true;
    } else {
        let r = floor(random() * 3.0);
        if r < 1.0 {
            log("\"Another round? Coming right up.\"");
        } else if r < 2.0 {
            log("\"Watch yourself around that stranger in the corner...\"");
        } else {
            log("\"Best ale this side of the Ashlands, friend.\"");
        }
    }

    // Brief excitement animation
    set_anim_speed(me, base_speed * 1.6);
}
