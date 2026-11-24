// {{name}}'s HUD: the game's name, one line of help and the pause menu.
//
// The scene's `hud_controller` entity carries this script. The engine draws
// no UI of its own: on_draw_ui runs every frame and what it draws is the
// HUD. Sizes are in points; (0, 0) is the top left.
//
// The pause menu is a UI document, ui/pause.ui.toml. The engine moves focus
// between its buttons from the keys, a pad, the mouse and touch, and this
// script hears what was chosen in on_ui. `menu` is scripts/lib/menu.rhai.

import "menu" as menu;
load_ui("ui/pause.ui.toml");

let show_help = true;

fn on_init() {
    // A page has no window to close
    ui_set_visible("quit", gpu_info().platform != "web");
    // A finger has no Escape: a pause button in the corner instead
    ui_set_visible("pause_button", is_touch_device());
}

fn on_update() {
    // `toggle_help` is this game's own action, bound in config/input.toml
    if is_action_just_pressed("toggle_help") {
        show_help = !show_help;
    }
}

// What the player chose in the pause menu. Opening it pauses the game
// (physics, animation, on_update and so scripts/player.rhai stop); on_ui
// and on_draw_ui still run.
fn on_ui(id, event) {
    let open = menu::is_open("pause_menu");
    if event == "back" && open || id == "resume" && event == "activate" {
        menu::toggle_pause("pause_menu");
    }
    if id == "pause_button" && event == "activate" {
        menu::toggle_pause("pause_menu");
    }
    if id == "restart" && event == "activate" {
        menu::toggle_pause("pause_menu");
        reload_scene();
    }
    if id == "quit" && event == "activate" {
        quit_game();
    }
}

fn on_draw_ui() {
    // `pause` is Escape and a pad's Start, bound in config/input.toml
    if is_action_just_pressed("pause") {
        menu::toggle_pause("pause_menu");
    }
    // The menu has the screen to itself
    if menu::is_open("pause_menu") {
        return;
    }

    let sh = screen_height();
    draw_text(24.0, 20.0, "{{name}}", 22.0, 1.0, 0.93, 0.82, 0.98);
    if show_help {
        draw_text(24.0, sh - 34.0, "WASD  move    H  hide this    Esc  pause", 14.0, 1.0, 1.0, 1.0, 0.85);
    }
}
