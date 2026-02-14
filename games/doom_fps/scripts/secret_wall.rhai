// ================================================================
//  Secret Wall Script — Doom-style push-wall hidden areas
// ================================================================
//
//  State machine: closed → opening → open
//
//  The wall slides horizontally by (slide_dx, slide_dz) at `speed`
//  units/sec. Once fully open it stays open permanently — authentic
//  Doom behavior. A subtle shade difference is the only visual hint.
//
//  All state stored in `secret` ECS component (not module-level vars)
//  to avoid the Rhai call_fn scope limitation.
// ================================================================

// ─── Helpers ─────────────────────────────────────────

fn to_float(v) {
    if type_of(v) == "f64" { return v; }
    if type_of(v) == "i64" { return v + 0.0; }
    return 0.0;
}

// ─── Initialization ─────────────────────────────────

fn on_init() {
    let me = this_entity();
    let pos = get_position(me);
    set_field(me, "secret", "closed_x", pos.x);
    set_field(me, "secret", "closed_z", pos.z);
    set_field(me, "secret", "move_progress", 0.0);
    set_field(me, "secret", "state", "closed");
}

// ─── Interaction ────────────────────────────────────

fn on_interact() {
    let me = this_entity();
    let secret = get_component(me, "secret");
    if secret == () { return; }

    let state = secret.state;
    if state == "closed" {
        set_field(me, "secret", "state", "opening");
        set_field(me, "secret", "discovered", true);
        play_sound("audio/secret_found.ogg", 0.8);
    }
}

// ─── Frame update — drive the state machine ─────────

fn on_update() {
    let dt = delta_time();
    let me = this_entity();
    let secret = get_component(me, "secret");
    if secret == () { return; }

    let state = secret.state;
    if state != "opening" { return; }

    let speed = to_float(secret.speed);
    let slide_dx = to_float(secret.slide_dx);
    let slide_dz = to_float(secret.slide_dz);
    let closed_x = to_float(secret.closed_x);
    let closed_z = to_float(secret.closed_z);
    let progress = to_float(secret.move_progress);

    // Total distance to travel
    let total_dist = sqrt(slide_dx * slide_dx + slide_dz * slide_dz);
    if total_dist < 0.001 { return; }

    // Advance progress
    progress += speed * dt / total_dist;
    if progress >= 1.0 {
        progress = 1.0;
        set_field(me, "secret", "state", "open");
    }
    set_field(me, "secret", "move_progress", progress);

    // Update position
    let new_x = closed_x + slide_dx * progress;
    let new_z = closed_z + slide_dz * progress;
    let pos = get_position(me);
    set_position(me, new_x, pos.y, new_z);
}
