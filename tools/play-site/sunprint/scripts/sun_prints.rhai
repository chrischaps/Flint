// Sun Prints (#430): the print develops in the sun, and E turns it over.
//
// The cyanotype is a user style (styles/cyanotype.*). This script only
// drives two of its settings: `exposure` eases from a faint first print
// up to the scene's 1.3 over the first half minute, the way a sheet left
// in the sun darkens, and `process` flips between a photogram (light
// prints blue, what stood in it stays paper) and a print from a negative.

let negative = false;
let shown_at = 0.0;

fn on_init() {
    set_post_style_param("cyanotype", "exposure", 0.35);
    tween_post_style_param("cyanotype", "exposure", 1.3, 30.0, "ease_out");
}

fn on_update() {
    if is_action_just_pressed("interact") {
        negative = !negative;
        set_post_style_param("cyanotype", "process", if negative { "print" } else { "photogram" });
        shown_at = total_time();
    }
}

fn on_draw_ui() {
    let sw = screen_width();
    let sh = screen_height();
    let t = total_time();

    // The title, laid on the sheet like a printed caption, gone after a while
    let title_alpha = clamp(min(t / 1.5, (11.0 - t) / 2.0), 0.0, 1.0);
    if title_alpha > 0.0 {
        let title = "Sun Prints";
        let tw = measure_text(title, 34.0).width;
        draw_text((sw - tw) / 2.0, sh * 0.16, title, 34.0, 0.97, 0.95, 0.90, title_alpha);
        let line = "after Anna Atkins, 1843 · walk in the light";
        let lw = measure_text(line, 15.0).width;
        draw_text((sw - lw) / 2.0, sh * 0.16 + 46.0, line, 15.0, 0.97, 0.95, 0.90, 0.85 * title_alpha);
    }

    // What E does, and for a moment what it just did: in the margin, below
    // where the coat stops, inked in the print's own blue
    let since = t - shown_at;
    let hint = if shown_at > 0.0 && since < 2.5 {
        if negative { "a print from the negative" } else { "a photogram" }
    } else {
        "E · turn the negative over"
    };
    let hint_alpha = if t < 4.0 { 0.0 } else { min((t - 4.0) / 1.5, 1.0) * 0.9 };
    let hw = measure_text(hint, 14.0).width;
    draw_text((sw - hw) / 2.0, sh - 30.0, hint, 14.0, 0.06, 0.17, 0.38, hint_alpha);
}
