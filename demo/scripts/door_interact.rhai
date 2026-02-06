// ─── door_interact.rhai ──────────────────────────────────
// Toggles a door between open and closed when the player
// presses Interact (E) within range. Plays a sound on toggle.

let is_open = false;
let swing_time = 0.0;
let swing_duration = 0.8;
let target_angle = 0.0;
let open_angle = 90.0;

fn on_init() {
    let me = self_entity();
    // Read the door's configured open angle if present
    let angle = get_field(me, "door", "open_angle");
    if angle != () {
        open_angle = angle;
    }
    log("Door script ready: " + entity_name(me));
}

fn on_interact() {
    let me = self_entity();

    // Check if the door is locked
    let locked = get_field(me, "door", "locked");
    if locked == true {
        log("Door is locked!");
        return;
    }

    // Toggle state
    is_open = !is_open;
    swing_time = 0.0;

    if is_open {
        target_angle = open_angle;
        log("Opening door: " + entity_name(me));
    } else {
        target_angle = 0.0;
        log("Closing door: " + entity_name(me));
    }

    // Play door sound
    play_sound("audio/door_open.ogg");
}

fn on_update(dt) {
    // Animate the door swing
    if swing_time < swing_duration {
        swing_time += dt;
        let t = clamp(swing_time / swing_duration, 0.0, 1.0);
        // Smooth ease-out curve
        let eased = 1.0 - (1.0 - t) * (1.0 - t);

        let me = self_entity();
        let rot = get_rotation(me);
        let current_y = if is_open {
            lerp(0.0, target_angle, eased)
        } else {
            lerp(open_angle, 0.0, eased)
        };
        set_rotation(me, rot.x, current_y, rot.z);
    }
}
