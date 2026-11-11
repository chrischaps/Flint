// {{name}}'s HUD: the game's name and one line of help.
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
    let sh = screen_height();
    draw_text(24.0, 20.0, "{{name}}", 22.0, 1.0, 0.93, 0.82, 0.95);
    if show_help {
        draw_text(24.0, sh - 34.0, "WASD  move    H  hide this    Esc  quit", 14.0, 1.0, 1.0, 1.0, 0.7);
    }
}
