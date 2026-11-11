// Forge Fire Flicker — intense fire with wider amplitude than torch
// Same layered sine pattern but ±8 intensity swing for dramatic forge glow

let base_intensity = 0.0;
let base_r = 1.0;
let base_g = 0.5;
let base_b = 0.1;

fn on_init() {
    let me = self_entity();
    base_intensity = get_field(me, "light", "intensity");
    let c = get_field(me, "light", "color");
    if type_of(c) != "()" {
        base_r = c[0];
        base_g = c[1];
        base_b = c[2];
    }
}

fn on_update() {
    let t = total_time();
    let me = self_entity();

    // Aggressive low-frequency pulse + rapid flicker
    let wave1 = sin(t * 2.5) * 0.20;
    let wave2 = sin(t * 5.8 + 0.7) * 0.12;
    let wave3 = sin(t * 11.3 + 2.1) * 0.06;
    let wave4 = sin(t * 17.7 + 4.3) * 0.03;

    let flicker = 1.0 + wave1 + wave2 + wave3 + wave4;
    let intensity = base_intensity * clamp(flicker, 0.55, 1.45);
    set_field(me, "light", "intensity", intensity);

    // Color shifts — hotter orange pulses with deeper red dips
    let heat = sin(t * 1.8) * 0.5 + 0.5;
    let r = clamp(base_r + heat * 0.08, 0.85, 1.0);
    let g = clamp(base_g - (1.0 - heat) * 0.1, 0.3, 0.65);
    let b = clamp(base_b + heat * 0.05, 0.02, 0.2);
    set_field(me, "light", "color", [r, g, b]);
}
