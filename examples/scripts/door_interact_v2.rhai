// ─── door_interact_v2.rhai ───────────────────────────────
// Enhanced door script: uses animation clips for open/close,
// checks locked state, plays appropriate sounds.
// Requires: door component (locked, open_angle), animator, interactable

let is_open = false;
let open_angle = 90.0;

fn on_init() {
    let me = self_entity();
    let angle = get_field(me, "door", "open_angle");
    if angle != () {
        open_angle = angle;
    }
    log("Door v2 ready: " + entity_name(me));
}

fn on_interact() {
    let me = self_entity();

    // Check if locked
    let locked = get_field(me, "door", "locked");
    if locked == true {
        log("Door is locked: " + entity_name(me));
        play_sound("audio/door_locked.wav");
        return;
    }

    // Toggle state
    is_open = !is_open;

    if is_open {
        play_clip(me, "door_swing");
        play_sound("audio/door_open.ogg");
        fire_event("door_opened");
        log("Opening: " + entity_name(me));
    } else {
        play_clip(me, "door_close");
        play_sound("audio/door_open.ogg");
        fire_event("door_closed");
        log("Closing: " + entity_name(me));
    }
}
