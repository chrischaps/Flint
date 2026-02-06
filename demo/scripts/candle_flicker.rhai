// ─── candle_flicker.rhai ─────────────────────────────────
// Gentle candle glow: lower intensity, warmer color, tighter range
// than the torch. Different frequency offsets create variety.

let base_intensity = 0.0;

fn on_init() {
    let me = self_entity();
    let intensity = get_field(me, "light", "intensity");
    if intensity != () {
        base_intensity = intensity;
    } else {
        base_intensity = 5.0;
    }
    log("Candle flicker active: " + entity_name(me));
}

fn on_update(dt) {
    let t = total_time();
    let me = self_entity();

    // Softer, more intimate flicker than the torch
    let wave1 = sin(t * 4.1 + 2.0) * 0.12;
    let wave2 = sin(t * 8.7 + 0.5) * 0.06;
    let wave3 = sin(t * 15.3 + 4.2) * 0.03;
    // Gentle draft effect
    let draft = sin(t * 1.1 + 1.0) * sin(t * 0.7) * 0.08;

    let flicker = 1.0 + wave1 + wave2 + wave3 - draft;
    let intensity = base_intensity * clamp(flicker, 0.6, 1.3);

    set_field(me, "light", "intensity", intensity);

    // Warm candle color — amber-gold, subtly shifting
    let r = clamp(1.0 + sin(t * 2.5 + 1.0) * 0.03, 0.95, 1.0);
    let g = clamp(0.75 + sin(t * 3.9 + 2.0) * 0.06, 0.65, 0.85);
    let b = clamp(0.25 + sin(t * 6.1 + 3.0) * 0.05, 0.15, 0.35);

    set_field(me, "light", "color", [r, g, b]);
}
