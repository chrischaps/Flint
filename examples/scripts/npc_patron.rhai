// ─── npc_patron.rhai ─────────────────────────────────────
// Tavern patrons: gentle idle speed variation for organic
// breathing feel. Occasional one-liners on interact.

let base_speed = 1.0;

fn on_init() {
    log("npc_patron on_init");
    let me = self_entity();
    let spd = get_field(me, "animator", "speed");
    if spd != () {
        base_speed = spd;
    }
}

fn on_update() {

    let me = self_entity();
    let t = total_time();

    // Gentle sinusoidal speed variation — simulates breathing rhythm
    // Each patron gets a slightly different phase from their base_speed
    let phase = base_speed * 7.3;
    let breathe = sin(t * 1.2 + phase) * 0.08;
    set_anim_speed(me, base_speed + breathe);
}

fn on_interact() {
    log("npc_patron on_interact");
    let me = self_entity();
    play_sound("audio/npc_greeting.wav");

    let r = floor(random() * 4.0);
    if r < 1.0 {
        log("*raises mug*");
    } else if r < 2.0 {
        log("*nods slowly*");
    } else if r < 3.0 {
        log("\"Fine evening, isn't it?\"");
    } else {
        log("\"Pull up a chair, stranger.\"");
    }
}
