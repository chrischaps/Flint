// ─── zone_ambience.rhai ─────────────────────────────────
// Per-zone ambient sound events. Plays random one-shot sounds
// at intervals based on the current scene.

let timer_a = 0.0;
let timer_b = 0.0;

fn on_init() {
    timer_a = random_range(5.0, 15.0);
    timer_b = random_range(8.0, 20.0);
}

fn on_update() {
    let dt = delta_time();
    timer_a -= dt;
    timer_b -= dt;

    if timer_a <= 0.0 {
        timer_a = random_range(15.0, 25.0);
        // Glass clink / ambient hit
        play_sound_at("audio/glasses_clink.ogg", 5.5, 1.0, 0.0, 0.4);
    }

    if timer_b <= 0.0 {
        timer_b = random_range(20.0, 35.0);
        // Fire crackle
        play_sound_at("audio/fire_crackle.ogg", -6.5, 1.0, -3.0, 0.6);
    }
}
