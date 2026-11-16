// ─── beetle_idle.rhai ─────────────────────────────────────
// The showcase creatures' idle (showcase_game/scripts/creature_idle.rhai) for the
// wooze props experiment (#252): a bob and a sway. On the wall clock they fought
// shallow wooze's held beat; on the wooze field's clock (#505) they hold with it.
//
// Kept stateless: the phase comes from where the beetle stands, and its feet are on
// y = 0, as the scene puts them.

fn on_update() {
    let me = self_entity();
    let pos = get_position(me);
    let phase = pos.x * 2.7;

    // The clock it moves on: the wooze field's beat where the scene has a [wooze]
    // table, held while the props around it hold and gliding when they glide, and
    // the wall clock anywhere else.
    let t = total_time();
    let dt = delta_time();
    let amount = 1.0;
    if module_enabled("wooze") {
        let beat = wooze_beat(pos.x, pos.y, pos.z);
        if "time" in beat {
            t = beat.time;
            dt = beat.delta;
            amount = beat.amplitude;
        }
    }

    // Gentle vertical bob, 3 cm at 1.5 rad/s, as much of it as the field shows.
    set_position(me, pos.x, sin(t * 1.5 + phase) * 0.03 * amount, pos.z);

    // Slight rotation sway.
    let rot = get_rotation(me);
    let sway = sin(t * 0.8 + phase) * 2.0;
    set_rotation(me, rot.x, rot.y + sway * dt, rot.z);
}
