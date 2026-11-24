// {{name}}'s HUD: a crosshair, the game's name, a controls panel and the
// pause menu.
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

// Open the menu or close it. It pauses the game (physics, animation and
// on_update stop) and lets go of the mouse so it can point at the buttons;
// closing it catches the mouse again for looking round.
fn toggle_pause() {
    let paused = menu::toggle_pause("pause_menu");
    set_cursor_captured(!paused);
}

fn on_update() {
    // `toggle_help` is this game's own action, bound in config/input.toml
    if is_action_just_pressed("toggle_help") {
        show_help = !show_help;
    }
}

// What the player chose in the pause menu. on_ui and on_draw_ui run while
// the game is paused; on_update doesn't.
fn on_ui(id, event) {
    let open = menu::is_open("pause_menu");
    if event == "back" && open || id == "resume" && event == "activate" {
        toggle_pause();
    }
    if id == "pause_button" && event == "activate" {
        toggle_pause();
    }
    if id == "restart" && event == "activate" {
        toggle_pause();
        reload_scene();
    }
    if id == "quit" && event == "activate" {
        quit_game();
    }
}

fn on_draw_ui() {
    // `pause` is Escape and a pad's Start, bound in config/input.toml
    if is_action_just_pressed("pause") {
        toggle_pause();
    }
    // The menu has the screen to itself
    if menu::is_open("pause_menu") {
        return;
    }

    let sw = screen_width();
    let sh = screen_height();

    // Crosshair
    draw_circle(sw / 2.0, sh / 2.0, 2.0, 1.0, 1.0, 1.0, 0.79);

    // Title, top left
    draw_text(24.0, 20.0, "{{name}}", 22.0, 1.0, 0.93, 0.82, 0.98);

    if !show_help {
        draw_text(24.0, sh - 34.0, "H  controls", 14.0, 1.0, 1.0, 1.0, 0.73);
        return;
    }

    // Controls panel, bottom left: a key, then what it does
    let rows = [
        ["WASD", "walk"],
        ["Mouse", "look"],
        ["Space", "jump"],
        ["Shift", "sprint"],
        ["H", "hide this"],
        ["Esc", "pause"],
    ];
    let line_h = 20.0;
    let pad = 14.0;
    let w = 180.0;
    let h = rows.len().to_float() * line_h + pad * 2.0;
    let x = 24.0;
    let y = sh - h - 24.0;
    draw_rect(x, y, w, h, #{ color: [0.05, 0.05, 0.07, 0.6], rounding: 10.0, layer: -1 });
    let row_y = y + pad;
    for row in rows {
        draw_text(x + pad, row_y, row[0], 14.0, 1.0, 0.82, 0.55, 0.98);
        draw_text(x + pad + 62.0, row_y, row[1], 14.0, 0.95, 0.93, 0.88, 0.95);
        row_y += line_h;
    }
}
