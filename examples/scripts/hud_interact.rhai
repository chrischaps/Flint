// ================================================================
//  Interaction Prompt HUD — Tavern Demo
// ================================================================
//
//  Lightweight UI script that shows "[E] verb target" when near
//  an interactable entity. No health/ammo display (tavern has none).
//
//  Attach to a dedicated entity with `script.source = "hud_interact.rhai"`.
// ================================================================

let prompt_alpha = 0.0;

fn on_update() {
    let dt = delta_time();
    let nearest = find_nearest_interactable();
    if type_of(nearest) != "()" {
        prompt_alpha = min(prompt_alpha + 5.0 * dt, 1.0);
    } else {
        prompt_alpha = max(prompt_alpha - 5.0 * dt, 0.0);
    }
}

fn on_draw_ui() {
    let sw = screen_width();
    let sh = screen_height();

    // Crosshair dot
    let cx = sw / 2.0;
    let cy = sh / 2.0;
    draw_circle(cx, cy, 2.0, 1.0, 1.0, 1.0, 0.55);
    draw_circle_outline(cx, cy, 3.5, 0.0, 0.0, 0.0, 0.31, 0.8);

    // Interaction prompt
    if prompt_alpha < 0.01 { return; }

    let nearest = find_nearest_interactable();
    let prompt = "[E] Interact";
    if type_of(nearest) != "()" {
        let verb = "Use";
        let itype = nearest.interaction_type;
        if itype == "talk" { verb = "Talk to"; }
        else if itype == "examine" { verb = "Examine"; }
        prompt = "[E] " + verb + " " + nearest.prompt_text;
    }

    let text_size = 16.0;
    let measurement = measure_text(prompt, text_size);
    let tw = measurement.width;
    let pad_x = 20.0;
    let pad_y = 10.0;

    let box_w = tw + pad_x * 2.0;
    let box_h = text_size + pad_y * 2.0;
    let box_x = (sw - box_w) / 2.0;
    let box_y = sh - 80.0;

    // Background pill
    draw_rect_ex(box_x, box_y, box_w, box_h, 0.04, 0.04, 0.04, 0.71 * prompt_alpha, 12.0, -1);

    // Prompt text
    let tx = box_x + pad_x;
    let ty = box_y + pad_y;
    draw_text_ex(tx, ty, prompt, text_size, 0.94, 0.92, 0.86, prompt_alpha, 1);
}
