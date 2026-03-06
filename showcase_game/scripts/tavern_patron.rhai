// ─── tavern_patron.rhai ──────────────────────────────────
// Tavern patrons with gentle breathing animation.
// Dialog references the ruins and the player's explorations.

let base_speed = 1.0;

fn on_init() {
    let me = self_entity();
    let spd = get_field(me, "animator", "speed");
    if spd != () {
        base_speed = spd;
    }
}

fn on_update() {
    let me = self_entity();
    let t = total_time();
    let phase = base_speed * 7.3;
    let breathe = sin(t * 1.2 + phase) * 0.08;
    set_anim_speed(me, base_speed + breathe);
}

fn on_interact() {
    let me = self_entity();
    play_sound("audio/npc_greeting.wav");

    let r = floor(random() * 4.0);
    if r < 1.0 {
        log("\"The ruins out there... they say the plants have taken over everything.\"");
    } else if r < 2.0 {
        log("\"I heard strange chittering from the crypt last night...\"");
    } else if r < 3.0 {
        log("\"Be careful in those ruins, friend. Things grow down there.\"");
    } else {
        log("*stares into their mug thoughtfully*");
    }
}
