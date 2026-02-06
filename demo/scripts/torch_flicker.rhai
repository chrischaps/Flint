// ─── torch_flicker.rhai ──────────────────────────────────
// Creates an organic, living light flicker effect using
// layered sine waves at different frequencies and amplitudes.
// Attached to point-light entities like the tavern fireplace.

let base_intensity = 0.0;

fn on_init() {
    let me = self_entity();
    // Read the initial light intensity as our baseline
    let intensity = get_field(me, "light", "intensity");
    if intensity != () {
        base_intensity = intensity;
    } else {
        base_intensity = 20.0;
    }
    log("Torch flicker active: " + entity_name(me));
}

fn on_update(dt) {
    let t = total_time();
    let me = self_entity();

    // Layer multiple sine waves for organic movement
    // Primary flicker: slow, large amplitude
    let wave1 = sin(t * 3.7) * 0.15;
    // Secondary: faster, smaller
    let wave2 = sin(t * 7.3 + 1.5) * 0.08;
    // Tertiary: rapid shimmer
    let wave3 = sin(t * 13.1 + 3.0) * 0.04;
    // Occasional dip (simulates wind gust)
    let gust = sin(t * 0.8) * sin(t * 0.8) * 0.1;

    let flicker = 1.0 + wave1 + wave2 + wave3 - gust;
    let intensity = base_intensity * clamp(flicker, 0.5, 1.4);

    set_field(me, "light", "intensity", intensity);

    // Subtly shift the fire color temperature
    let r = clamp(1.0 + sin(t * 2.1) * 0.05, 0.9, 1.0);
    let g = clamp(0.6 + sin(t * 3.3) * 0.08, 0.45, 0.75);
    let b = clamp(0.15 + sin(t * 5.7) * 0.05, 0.05, 0.25);

    set_field(me, "light", "color", [r, g, b]);
}
