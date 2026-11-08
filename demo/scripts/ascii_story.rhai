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
// The script only says what happens at each beat. The engine eases the
// settings (tween_style_param, #370): the barrel's cells, and the bench's
// glow, which turns around each time its tween says it's done.

let period = 8.0;

fn on_update() {
    // A slow drift that keeps both props in frame
    let drift = total_time() * 0.04;
    set_camera_position(-1.8 + 0.6 * drift.sin(), 1.6, 8.6);
    set_camera_target(1.4, 0.4, 4.0);

    let t = total_time() % period;
    let barrel = get_entity("barrel");
    let bench = get_entity("bench");
    let styled = get_field(self_entity(), "story", "beat");

    // The beat changes: set, tween or clear once, on the frame it starts
    let beat = if t < 2.0 { 0 } else if t < 4.0 { 1 } else if t < 6.0 { 2 } else { 3 };
    if beat != styled {
        if beat == 1 {
            set_entity_style(barrel, "ascii");
            set_style_param(barrel, "grid", "object");
            set_style_param(barrel, "brightness", 0.3);
            set_style_param(barrel, "cell", 22.0);
            tween_style_param(barrel, "cell", 7.0, 1.5);
        } else if beat == 2 {
            set_entity_style(bench, "ascii");
            set_style_param(bench, "ramp", "blocks");
            set_style_param(bench, "color", "tint");
            set_style_param(bench, "tint", [1.0, 0.70, 0.30]);
            set_style_param(bench, "levels", 4);
            set_style_param(bench, "glow", 0.4);
            tween_style_param(bench, "glow", 0.8, 0.5, "ease_out");
        } else if beat == 0 || beat == 3 {
            // Clearing also stops their tweens
            clear_entity_style(barrel);
            clear_entity_style(bench);
        }
        set_field(self_entity(), "story", "beat", beat);
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
    // The bench breathes: each swell, once done, turns into the next
    // (only while the bench is still styled)
    let me = self_entity();
    if name == "style_tween_done" && data.param == "glow" && get_field(me, "story", "beat") == 2 {
        let up = get_field(me, "story", "inhale");
        let target = if up { 0.8 } else { 0.0 };
        set_field(me, "story", "inhale", !up);
        tween_style_param(data.entity, "glow", target, 1.05, "ease_in_out");
    }
}
