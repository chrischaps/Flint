// menu.rhai: the engine's menu helpers (#731), for a .ui.toml whose
// buttons, lists and sliders the engine already drives from the keys,
// a pad, the mouse and touch. `import "menu" as menu;` in any game's
// script: the search finds this file in the engine (scripts/lib/).
//
// A menu is a panel of a loaded UI document. Give the button that should
// have focus when it opens `autofocus = true`; the script hears what the
// player does as on_ui(element_id, event):
//
//     import "menu" as menu;
//     load_ui("ui/pause.ui.toml");
//
//     fn on_draw_ui() {
//         if is_action_just_pressed("pause") { menu::toggle_pause("pause_menu"); }
//     }
//
//     fn on_ui(id, event) {
//         if event == "back" || id == "resume" && event == "activate" {
//             menu::toggle_pause("pause_menu");
//         }
//         if id == "quit" && event == "activate" { quit_game(); }
//     }
//
// on_draw_ui and on_ui run while the game is paused; on_update and
// on_action don't.

/// Show a menu panel. Focus goes to its `autofocus` element, or to `first`
/// when that is the id of one of its buttons ("" leaves it to autofocus).
fn open(panel, first) {
    ui_show(panel);
    if first != "" {
        ui_focus(first);
    }
}

/// Hide a menu panel; focus inside it lets go.
fn close(panel) {
    ui_hide(panel);
}

/// Whether a menu panel (and everything above it) is shown.
fn is_open(panel) {
    ui_is_shown(panel)
}

/// Open the menu if it is closed and close it if it is open; returns
/// whether it is open now.
fn toggle(panel) {
    if ui_is_shown(panel) {
        ui_hide(panel);
        false
    } else {
        ui_show(panel);
        true
    }
}

/// A pause menu: open it and pause the game (the "paused" state stops
/// on_update, physics and animation), or close it and play on. Returns
/// whether the game is paused now.
fn toggle_pause(panel) {
    if ui_is_shown(panel) {
        ui_hide(panel);
        if get_current_state() == "paused" {
            pop_state();
        }
        false
    } else {
        ui_show(panel);
        push_state("paused");
        true
    }
}

/// The label a list item shows, from the list's id and its value: for
/// on_ui(list, "activate") on a list made from `items`.
fn chosen(list, items) {
    let i = ui_get_value(list).to_int();
    if i >= 0 && i < items.len() { items[i] } else { "" }
}
