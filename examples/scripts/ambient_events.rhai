// ─── ambient_events.rhai ─────────────────────────────────
// Background micro-events that make the tavern feel alive.
// Plays random one-shot sounds at intervals to create atmosphere.
// Attached to a hidden "ambient_controller" entity.

let clink_timer = 0.0;
let clink_interval = 18.0;
let crackle_timer = 0.0;
let crackle_interval = 25.0;

fn on_init() {
    // Randomize initial timers so events don't all start at once
    clink_timer = random_range(5.0, 15.0);
    crackle_timer = random_range(8.0, 20.0);
    log("Ambient event controller active");
}

fn on_update() {
    let dt = delta_time();
    clink_timer -= dt;
    crackle_timer -= dt;

    // Glass clink at bar position
    if clink_timer <= 0.0 {
        clink_interval = random_range(15.0, 25.0);
        clink_timer = clink_interval;
        // Play at bar counter position
        play_sound_at("audio/glasses_clink.ogg", 5.5, 1.0, 0.0, 0.4);
    }

    // Fire crackle variation at fireplace
    if crackle_timer <= 0.0 {
        crackle_interval = random_range(20.0, 35.0);
        crackle_timer = crackle_interval;
        // Play at fireplace position
        play_sound_at("audio/fire_crackle.ogg", -6.5, 1.0, -3.0, 0.6);
    }
}
