// ================================================================
//  Door Script — Doom-style vertical sliding doors
// ================================================================
//
//  State machine: closed → opening → open → closing → closed
//
//  The door slides upward by `open_height` units at `speed` u/s,
//  waits `stay_open_time` seconds, then slides back down.
//  Interacting while closing reverses the door back to opening.
//
//  All state is stored in the `door_state` ECS component — not in
//  module-level variables — so helper functions can access it via
//  get_field/set_field (avoids the Rhai call_fn scope limitation).
// ================================================================

// ─── Helpers ─────────────────────────────────────────

fn to_float(v) {
    if type_of(v) == "f64" { return v; }
    if type_of(v) == "i64" { return v + 0.0; }
    return 0.0;
}

fn find_player() {
    let id = get_entity("player");
    if id < 0 { return -1; }
    if !has_component(id, "character_controller") { return -1; }
    return id;
}

fn distance_to_player(door_id) {
    let player = find_player();
    if player < 0 { return 999.0; }
    let dp = get_position(door_id);
    let pp = get_position(player);
    let dx = pp.x - dp.x;
    let dy = pp.y - dp.y;
    let dz = pp.z - dp.z;
    return sqrt(dx*dx + dy*dy + dz*dz);
}

// ─── Initialization ─────────────────────────────────

fn on_init() {
    // Capture the door's starting Y position as closed_y
    let pos = get_position(this_entity());
    set_field(this_entity(), "door_state", "closed_y", pos.y);
    set_field(this_entity(), "door_state", "move_progress", 0.0);
    set_field(this_entity(), "door_state", "state", "closed");
    set_field(this_entity(), "door_state", "timer", 0.0);
}

// ─── Interaction ────────────────────────────────────

fn on_interact() {
    let me = this_entity();
    let door = get_component(me, "door_state");
    if door == () { return; }

    let state = door.state;
    let locked = door.locked;
    if type_of(locked) == "bool" && locked {
        // Door is locked — play a locked sound, don't open
        play_sound("audio/door_locked.wav", 0.6);
        return;
    }

    if state == "closed" || state == "closing" {
        // Open the door (or reverse if it was closing)
        set_field(me, "door_state", "state", "opening");
        play_sound("audio/door_open.ogg", 0.7);
    }
}

// ─── Frame update — drive the state machine ─────────

fn on_update() {
    let dt = delta_time();
    let me = this_entity();
    let door = get_component(me, "door_state");
    if door == () { return; }

    let state = door.state;
    let speed = to_float(door.speed);
    let open_height = to_float(door.open_height);
    let closed_y = to_float(door.closed_y);
    let progress = to_float(door.move_progress);
    let stay_open_time = to_float(door.stay_open_time);
    let timer = to_float(door.timer);

    // Auto-open: check proximity when closed
    if state == "closed" {
        let auto_open = door.auto_open;
        if type_of(auto_open) == "bool" && auto_open {
            let trigger_range = to_float(door.trigger_range);
            let dist = distance_to_player(me);
            if dist <= trigger_range {
                set_field(me, "door_state", "state", "opening");
                play_sound("audio/door_open.ogg", 0.7);
                return;
            }
        }
    }

    if state == "opening" {
        // Move upward
        let step = speed * dt / open_height;
        progress += step;
        if progress >= 1.0 {
            progress = 1.0;
            set_field(me, "door_state", "state", "open");
            set_field(me, "door_state", "timer", 0.0);
        }
        set_field(me, "door_state", "move_progress", progress);

        let new_y = closed_y + open_height * progress;
        let pos = get_position(me);
        set_position(me, pos.x, new_y, pos.z);

    } else if state == "open" {
        // Count down the stay-open timer
        timer += dt;
        set_field(me, "door_state", "timer", timer);

        if timer >= stay_open_time {
            set_field(me, "door_state", "state", "closing");
        }

    } else if state == "closing" {
        // Move downward
        let step = speed * dt / open_height;
        progress -= step;
        if progress <= 0.0 {
            progress = 0.0;
            set_field(me, "door_state", "state", "closed");
        }
        set_field(me, "door_state", "move_progress", progress);

        let new_y = closed_y + open_height * progress;
        let pos = get_position(me);
        set_position(me, pos.x, new_y, pos.z);
    }
}
