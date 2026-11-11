// Safe-area HUD (#660). A title screen and HUD drawn by script, kept clear
// of a phone's notch and home bar with safe_area(), and worded and sized for
// a thumb from the first frame with is_touch_device().
//
// The insets are tinted so you can see what they cover; a real game would
// leave them to the world.

fn on_draw_ui() {
    let sw = screen_width();
    let sh = screen_height();
    let s = safe_area();
    let touch = is_touch_device();

    // What the notch and home bar cover.
    let shade = [0.9, 0.35, 0.45, 0.16];
    if s.left > 0.0 { draw_rect(0.0, 0.0, s.left, sh, shade[0], shade[1], shade[2], shade[3]); }
    if s.right > 0.0 { draw_rect(sw - s.right, 0.0, s.right, sh, shade[0], shade[1], shade[2], shade[3]); }
    if s.top > 0.0 { draw_rect(0.0, 0.0, sw, s.top, shade[0], shade[1], shade[2], shade[3]); }
    if s.bottom > 0.0 { draw_rect(0.0, sh - s.bottom, sw, s.bottom, shade[0], shade[1], shade[2], shade[3]); }

    // The corners: score, clock and stage name, inside the insets.
    let pad = 16.0;
    let ink = [0.98, 0.95, 0.88, 1.0];
    let dim = [0.72, 0.7, 0.8, 1.0];
    draw_text_opts(s.left + pad, s.top + pad, "SCORE 004210", 22.0, #{ color: ink, spacing: 2.0 });
    draw_text_opts(s.left + pad, s.top + pad + 28.0, "STAGE 3  ·  AFTERGLOW", 13.0, #{ color: dim, spacing: 2.0 });
    draw_text_opts(sw - s.right - pad, s.top + pad, "01:42", 22.0, #{ color: ink, align: "right", spacing: 2.0 });

    // The menu: rows a thumb can hit on a touch device, tighter elsewhere.
    let row = if touch { 44.0 } else { 30.0 };
    let w = 280.0;
    let x = sw * 0.5 - w * 0.5;
    let y0 = sh * 0.5 - row * 1.5;
    let items = ["CONTINUE", "NEW RUN", "SETTINGS"];
    for i in 0..items.len() {
        let y = y0 + row * i;
        if i == 0 {
            draw_rect(x, y + 3.0, w, row - 6.0, 1.0, 0.55, 0.3, 0.28);
        }
        draw_rect_outline(x, y + 3.0, w, row - 6.0, 1.0, 0.85, 0.7, 0.35, 1.0);
        draw_text_opts(sw * 0.5, y + row * 0.5 - 9.0, items[i], 17.0, #{ color: ink, align: "center", spacing: 3.0 });
    }

    // The prompt, in the words of the device in hand, before any input.
    let prompt = if touch { "TAP TO START" } else { "ENTER  TO START" };
    let device = if touch { "touch-first device" } else { "keyboard & mouse" };
    let py = sh - s.bottom - pad - 48.0;
    draw_text_opts(sw * 0.5, py, prompt, 20.0, #{ color: [1.0, 0.78, 0.55, 1.0], align: "center", spacing: 4.0 });
    draw_text_opts(sw * 0.5, py + 28.0, "is_touch_device() = " + touch + "  ·  " + device + "  ·  last_input_device() = " + last_input_device(),
        12.0, #{ color: dim, align: "center", spacing: 1.0 });
    let insets = "safe_area() = #{ left: " + s.left + ", top: " + s.top + ", right: " + s.right + ", bottom: " + s.bottom + " }";
    draw_text_opts(s.left + pad, s.top + pad + 50.0, insets, 12.0, #{ color: dim, spacing: 1.0 });
}
