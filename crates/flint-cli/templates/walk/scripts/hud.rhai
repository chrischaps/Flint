// {{name}}'s HUD: a crosshair, the game's name and a controls panel.
//
// The scene's `hud_controller` entity carries this script. The engine draws
// no UI of its own: on_draw_ui runs every frame and what it draws is the
// HUD. Sizes are in points; (0, 0) is the top left.

let show_help = true;

fn on_update() {
    // `toggle_help` is this game's own action, bound in config/input.toml
    if is_action_just_pressed("toggle_help") {
        show_help = !show_help;
    }
}

fn on_draw_ui() {
    let sw = screen_width();
    let sh = screen_height();

    // Crosshair
    draw_circle(sw / 2.0, sh / 2.0, 2.0, 1.0, 1.0, 1.0, 0.6);

    // Title, top left
    draw_text(24.0, 20.0, "{{name}}", 22.0, 1.0, 0.93, 0.82, 0.95);

    if !show_help {
        draw_text(24.0, sh - 34.0, "H  controls", 14.0, 1.0, 1.0, 1.0, 0.5);
        return;
    }

    // Controls panel, bottom left: a key, then what it does
    let rows = [
        ["WASD", "walk"],
        ["Mouse", "look"],
        ["Space", "jump"],
        ["Shift", "sprint"],
        ["H", "hide this"],
        ["Esc", "quit"],
    ];
    let line_h = 20.0;
    let pad = 14.0;
    let w = 180.0;
    let h = rows.len().to_float() * line_h + pad * 2.0;
    let x = 24.0;
    let y = sh - h - 24.0;
    draw_rect_ex(x, y, w, h, 0.05, 0.05, 0.07, 0.6, 10.0, -1);
    let row_y = y + pad;
    for row in rows {
        draw_text(x + pad, row_y, row[0], 14.0, 1.0, 0.82, 0.55, 0.95);
        draw_text(x + pad + 62.0, row_y, row[1], 14.0, 0.95, 0.93, 0.88, 0.9);
        row_y += line_h;
    }
}
