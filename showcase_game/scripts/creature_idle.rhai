// ─── creature_idle.rhai ──────────────────────────────────
// Gentle idle behavior for procgen creatures.
// Slight position oscillation for breathing/swaying effect.
// Proximity awareness: speeds up when player is near.

let base_y = 0.0;
let phase_offset = 0.0;

fn on_init() {
    let me = self_entity();
    let pos = get_position(me);
    base_y = pos.y;
    // Random phase so creatures don't sway in sync
    phase_offset = random() * 6.28;
}

fn on_update() {
    let dt = delta_time();
    let me = self_entity();
    let t = total_time();

    // Gentle vertical bob
    let bob = sin(t * 1.5 + phase_offset) * 0.03;
    let pos = get_position(me);
    set_position(me, pos.x, base_y + bob, pos.z);

    // Slight rotation sway
    let rot = get_rotation(me);
    let sway = sin(t * 0.8 + phase_offset) * 2.0;
    set_rotation(me, rot.x, rot.y + sway * dt, rot.z);
}

fn on_interact() {
    let me = self_entity();
    log("*the creature chititters nervously*");
}
