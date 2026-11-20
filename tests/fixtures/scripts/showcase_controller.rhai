// Showcase Controller — the pause menu and the game's states.
// The menu is a UI document (flint-examples/ui/pause.ui.toml, #731): the engine
// moves focus with the keys, a pad, the mouse and touch, and says what the
// player chose in on_ui. Escape opens and closes it (flint-examples/input_showcase.toml).
// Demonstrates: load_ui(), on_ui(), scripts/lib/menu.rhai, register_state(),
// set_post("radial_blur"), set_audio_lowpass(), quit_game().

import "menu" as menu;
load_ui("ui/pause.ui.toml");

fn on_init() {
    register_state("paused", #{ physics: "pause", scripts: "pause", animation: "pause", particles: "pause", audio: "run" });
    // A page has no window to close (#659)
    ui_set_visible("quit", gpu_info().platform != "web");
}

// on_draw_ui and on_ui run while "paused" holds the scripts; on_action doesn't
fn on_draw_ui() {
    if is_action_just_pressed("pause") { menu::toggle_pause("pause_menu"); }
    let paused = menu::is_open("pause_menu");
    set_post("radial_blur", if paused { 0.3 } else { 0.0 });
    set_audio_lowpass(if paused { 800.0 } else { 20000.0 });
}

fn on_ui(id, event) {
    if event == "back" || id == "resume" && event == "activate" { menu::toggle_pause("pause_menu"); }
    if id == "restart" && event == "activate" { menu::toggle_pause("pause_menu"); reload_scene(); }
    if id == "quit" && event == "activate" { quit_game(); }
}
