// A story told in styles (#269): the clearing starts smooth, then the
// barrel comes apart into letters, coarse at first and settling into a
// fine grid; the bench follows in amber blocks; then both are given back.
// Eight seconds, looping. Game time, so a --frame-step capture shows the
// same beats every run.
//
//   0 s  everything smooth
//   2 s  barrel -> ascii, its cells settle from 22 to 7 points
//   4 s  bench  -> ascii blocks, its glow breathing
//   6 s  both cleared
//
// set_style_param every frame is a uniform write: no GPU allocation.

let period = 8.0;

fn smooth(x) {
    let t = if x < 0.0 { 0.0 } else if x > 1.0 { 1.0 } else { x };
    t * t * (3.0 - 2.0 * t)
}

fn on_update() {
    // A slow drift that keeps both props in frame
    let drift = total_time() * 0.04;
    set_camera_position(-1.8 + 0.6 * drift.sin(), 1.6, 8.6);
    set_camera_target(1.4, 0.4, 4.0);

    let t = total_time() % period;
    let barrel = get_entity("barrel");
    let bench = get_entity("bench");
    let styled = get_field(self_entity(), "story", "beat");

    // The beat changes: set or clear once, on the frame it starts
    let beat = if t < 2.0 { 0 } else if t < 4.0 { 1 } else if t < 6.0 { 2 } else { 3 };
    if beat != styled {
        if beat == 1 {
            set_entity_style(barrel, "ascii");
            set_style_param(barrel, "grid", "object");
            set_style_param(barrel, "brightness", 0.3);
        } else if beat == 2 {
            set_entity_style(bench, "ascii");
            set_style_param(bench, "ramp", "blocks");
            set_style_param(bench, "color", "tint");
            set_style_param(bench, "tint", [1.0, 0.70, 0.30]);
            set_style_param(bench, "levels", 4);
        } else if beat == 0 || beat == 3 {
            clear_entity_style(barrel);
            clear_entity_style(bench);
        }
        set_field(self_entity(), "story", "beat", beat);
    }

    // Every frame while styled: the barrel's cells settle, the bench breathes
    if beat == 1 || beat == 2 {
        let k = smooth((t - 2.0) / 1.5);
        set_style_param(barrel, "cell", 22.0 - 15.0 * k);
    }
    if beat == 2 {
        set_style_param(bench, "glow", 0.4 + 0.4 * (t * 3.0).sin());
    }
}

fn on_event(name, data) {
    if name == "style_changed" {
        let who = entity_name(data.entity);
        if data.style == "" {
            log(`${who} is smooth again`);
        } else {
            log(`${who} is drawn in ${data.style}`);
        }
    }
}
