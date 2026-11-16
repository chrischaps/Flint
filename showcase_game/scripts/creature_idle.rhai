// ─── creature_idle.rhai ──────────────────────────────────
// Gentle idle behavior for procgen creatures.
// Slight position oscillation for breathing/swaying effect.
// Proximity awareness: speeds up when player is near.
//
// In a scene with a [wooze] table the creature moves on the wooze field's beat
// (#505), holding while the props around it hold, so its bob doesn't undo a
// shallow_wooze it wears. Anywhere else it moves on the wall clock, as it always has.

let base_y = 0.0;
let phase_offset = 0.0;

fn on_init() {
    let me = self_entity();
    let pos = get_position(me);
    base_y = pos.y;
    // Random phase so creatures don't sway in sync
    phase_offset = random() * 6.28;
}

/// The clock the idle moves on where the creature stands: #{ t, dt, amount },
/// the wooze field's beat if the scene has one, else the wall clock in full.
fn idle_clock(pos) {
    if module_enabled("wooze") {
        let beat = wooze_beat(pos.x, pos.y, pos.z);
        if "time" in beat {
            return #{ t: beat.time, dt: beat.delta, amount: beat.amplitude };
        }
    }
    #{ t: total_time(), dt: delta_time(), amount: 1.0 }
}

fn on_update() {
    let me = self_entity();
    let pos = get_position(me);
    let clock = idle_clock(pos);

    // Gentle vertical bob
    let bob = sin(clock.t * 1.5 + phase_offset) * 0.03 * clock.amount;
    set_position(me, pos.x, base_y + bob, pos.z);

    // Slight rotation sway
    let rot = get_rotation(me);
    let sway = sin(clock.t * 0.8 + phase_offset) * 2.0;
    set_rotation(me, rot.x, rot.y + sway * clock.dt, rot.z);
}

fn on_interact() {
    let me = self_entity();
    log("*the creature chititters nervously*");
}
