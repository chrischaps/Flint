// Beat 06, a story in styles (#269). On game time:
//
//   0.0  everything smooth
//   1.2  the barrel comes apart into letters, coarse, settling 24 -> 7 pt
//   3.2  the figure follows in amber blocks, its glow breathing
//   5.2  the crate and its lid are drawn in pencil
//   7.4  all of it given back, smooth by the end of the beat
//
// Each step is applied once, on the frame it starts (style_changed fires
// for each set and clear); the cell size and the glow move every frame.

import "film" as film;

fn step_at(t) {
    if t < 1.2 { 0 } else if t < 3.2 { 1 } else if t < 5.2 { 2 } else if t < 7.4 { 3 } else { 4 }
}

fn on_update() {
    let t = total_time();
    film::dolly([3.3, 1.55, 3.9], [2.6, 1.4, 3.1], [-0.1, 0.75, -0.2], [-0.3, 0.8, -0.4],
        film::ease(t / 9.0));

    let barrel = get_entity("barrel");
    let figure = get_entity("figure");
    let crate_ = get_entity("crate");
    let step = step_at(t);
    if step != get_field(self_entity(), "film", "step") {
        if step == 1 {
            set_entity_style(barrel, "ascii");
            set_style_param(barrel, "grid", "object");
            set_style_param(barrel, "color", "tint");
            set_style_param(barrel, "tint", [0.35, 1.0, 0.45]);
            set_style_param(barrel, "brightness", 0.3);
        } else if step == 2 {
            set_entity_style(figure, "ascii");
            set_style_param(figure, "ramp", "blocks");
            set_style_param(figure, "color", "tint");
            set_style_param(figure, "tint", [1.0, 0.70, 0.30]);
            set_style_param(figure, "levels", 4);
        } else if step == 3 {
            set_entity_style(crate_, "pencil");
            set_style_param(crate_, "direction", "screen");
            set_style_param(crate_, "layers", 4.0);
        } else if step == 4 {
            clear_entity_style(barrel);
            clear_entity_style(figure);
            clear_entity_style(crate_);
        }
        set_field(self_entity(), "film", "step", step);
    }
    if step >= 1 && step <= 3 {
        set_style_param(barrel, "cell", film::lerp(24.0, 7.0, film::ramp(t, 1.2, 1.6)));
    }
    if step >= 2 && step <= 3 {
        set_style_param(figure, "glow", 0.45 + 0.4 * (t * 3.0).sin());
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
