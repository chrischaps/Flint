// Fading props into text (#278): both props are given their ASCII look
// once, at blend 0, so nothing shows; then every frame only the blend
// moves. The barrel dissolves, its cells turning to glyphs in patches
// that spread; the bench cross-fades, every pixel a mix. Ten seconds,
// looping, on game time, so a --frame-step capture shows the same frames
// every run.
//
//   0 s    everything smooth
//   1-4 s  barrel dissolves into green phosphor glyphs
//   3-6 s  bench fades into amber blocks
//   7.5 s  both go back the way they came, smooth again by 9.5 s
//
// A blend rides in the style mask beside the entity's id, so ramping it
// keeps one variant and makes no GPU allocation.

let period = 10.0;

fn smooth(x) {
    let t = if x < 0.0 { 0.0 } else if x > 1.0 { 1.0 } else { x };
    t * t * (3.0 - 2.0 * t)
}

// 0 -> 1 over [rise, rise + len], then 1 -> 0 over [fall, fall + len]
fn ramp(t, rise, fall, len) {
    smooth((t - rise) / len) - smooth((t - fall) / len)
}

fn on_init() {
    let barrel = get_entity("barrel");
    set_entity_style(barrel, "ascii");
    set_style_param(barrel, "blend", 0.0);
    set_style_param(barrel, "transition", "dissolve");
    set_style_param(barrel, "grid", "object");
    set_style_param(barrel, "color", "tint");
    set_style_param(barrel, "tint", [0.35, 1.0, 0.45]);
    set_style_param(barrel, "glow", 0.6);
    set_style_param(barrel, "brightness", 0.25);

    let bench = get_entity("bench");
    set_entity_style(bench, "ascii");
    set_style_param(bench, "blend", 0.0);
    set_style_param(bench, "ramp", "blocks");
    set_style_param(bench, "color", "tint");
    set_style_param(bench, "tint", [1.0, 0.70, 0.30]);
    set_style_param(bench, "levels", 4);
    set_style_param(bench, "brightness", 0.35);
    set_style_param(bench, "cell", 6.0);
}

fn on_update() {
    // A slow drift that keeps both props in frame
    let drift = total_time() * 0.04;
    set_camera_position(-1.8 + 0.6 * drift.sin(), 1.6, 8.6);
    set_camera_target(1.4, 0.4, 4.0);

    let t = total_time() % period;
    set_style_param(get_entity("barrel"), "blend", ramp(t, 1.0, 7.5, 3.0));
    set_style_param(get_entity("bench"), "blend", ramp(t, 3.0, 7.5, 2.5));
}
