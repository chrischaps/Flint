// ─── beetle_idle.rhai ─────────────────────────────────────
// The showcase creatures' idle (showcase_game/scripts/creature_idle.rhai) for the
// wooze props experiment (#252): a bob and a sway on the wall clock, to see whether
// continuous motion fights shallow wooze's held beat.
//
// The same bob and sway, kept stateless: Rhai's module-level `let` isn't visible
// inside callbacks, so the phase comes from where the beetle stands and its feet
// are on y = 0, as the scene puts them.

fn on_update() {
    let me = self_entity();
    let t = total_time();
    let pos = get_position(me);
    let phase = pos.x * 2.7;

    // Gentle vertical bob, 3 cm at 1.5 rad/s.
    set_position(me, pos.x, sin(t * 1.5 + phase) * 0.03, pos.z);

    // Slight rotation sway.
    let rot = get_rotation(me);
    let sway = sin(t * 0.8 + phase) * 2.0;
    set_rotation(me, rot.x, rot.y + sway * delta_time(), rot.z);
}
