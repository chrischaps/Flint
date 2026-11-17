//! Captures reach the debug keys (H9): a key the input script presses goes
//! through the same handler as a real one, and `CaptureConfig::show_stats`
//! opens the F2 overlay from the first frame. Scripted clicks reach
//! `InputState` the way real ones do (ADR 0082). Headless: no window.

use super::input_script::InputScript;
use super::PlayerApp;
use flint_debug_ui::run_headless_frame;
use flint_ecs::FlintWorld;
use winit::keyboard::KeyCode;

fn headless_app() -> PlayerApp {
    PlayerApp::new(
        FlintWorld::new(),
        "capture_test.scene.toml".into(),
        false,
        None,
        None,
    )
}

fn with_script(src: &str) -> PlayerApp {
    let mut app = headless_app();
    app.capture.input_script = Some(InputScript::parse(src).unwrap());
    app
}

#[test]
fn scripted_f2_steps_the_stats_overlay() {
    use super::StatsView;
    // Frames at t = 0, 0.25, 0.5, 0.75: press, release, press, press.
    let mut app = with_script("0 tap F2\n0.5 tap F2\n0.75 tap F2\n");
    app.core.clock.frame_step = Some(0.25);
    assert_eq!(app.stats.view, StatsView::Off);
    assert!(!app.core.script.timing(), "nothing timed while it's closed");
    app.tick();
    assert_eq!(app.stats.view, StatsView::Full, "the first tap opens it");
    assert!(app.core.script.timing(), "scripts are timed while it shows");
    // The debug key's alone: the game never sees it (#717)
    assert!(!app.input.is_key_down(KeyCode::F2));
    app.input.end_frame();
    app.tick();
    assert_eq!(
        app.stats.view,
        StatsView::Full,
        "the release doesn't step it"
    );
    app.input.end_frame();
    app.tick();
    let second = if cfg!(feature = "debug-hud") {
        StatsView::Graph
    } else {
        StatsView::Off
    };
    assert_eq!(app.stats.view, second, "the second tap: the graph alone");
    app.input.end_frame();
    app.tick();
    if cfg!(feature = "debug-hud") {
        assert_eq!(app.stats.view, StatsView::Off, "the third tap closes it");
        assert!(!app.core.script.timing());
    }
}

/// A debug key with nothing behind it says so where the person pressing it
/// will see it (#367): it used to log at `info`, which the player never prints.
#[cfg(feature = "debug-hud")]
#[test]
fn f3_and_f4_with_nothing_to_toggle_say_so_on_screen() {
    let mut app = with_script("0 tap F3\n0.25 tap F4\n");
    app.core.clock.frame_step = Some(0.25);
    assert!(app.debug_panels.is_empty(), "a scene with no panels");
    assert!(app.hud.key_note.showing().is_none());
    app.tick();
    assert_eq!(
        app.hud.key_note.showing(),
        Some(("F3", "no debug panels in this scene"))
    );
    app.input.end_frame();
    app.tick();
    assert_eq!(
        app.hud.key_note.showing(),
        Some(("F4", "no renderer active — nothing to tune")),
        "the next key's note replaces it"
    );
}

#[test]
fn scripted_game_keys_reach_input_state_as_before() {
    let mut app = with_script("0 down KeyW\n");
    app.tick();
    assert!(app.input.is_key_down(KeyCode::KeyW));
    assert!(!app.stats.view.is_open());
}

/// `app` with an input config layered on, as a game's or a player's file is.
fn with_config(script: &str, config: &str) -> PlayerApp {
    let mut app = with_script(script);
    let config =
        flint_runtime::InputConfig::from_toml_str(&format!("version = 1\n{config}")).unwrap();
    app.input.merge_bindings(config).unwrap();
    app
}

/// Which of `keys` the game's input holds down.
fn pressed(app: &PlayerApp, keys: &[KeyCode]) -> Vec<KeyCode> {
    keys.iter()
        .copied()
        .filter(|&k| app.input.is_key_down(k))
        .collect()
}

/// #717: F5-F9, Backquote and Backslash belong to living props and a charted
/// music session. A scene with neither passes them to the game untouched,
/// where they used to be eaten or answered with a note.
#[test]
fn a_scene_with_no_living_props_or_music_passes_their_keys_to_the_game() {
    let keys = [
        KeyCode::F5,
        KeyCode::F6,
        KeyCode::F7,
        KeyCode::F8,
        KeyCode::F9,
        KeyCode::Backquote,
        KeyCode::Backslash,
    ];
    let mut app = with_script("0 down F5\n0 down F6\n0 down F7\n0 down F8\n0 down F9\n0 down Backquote\n0 down Backslash\n");
    assert!(app.core.subsystems.is_empty(), "no living props, no music");
    app.tick();
    assert_eq!(pressed(&app, &keys), keys, "every one is the game's");
    assert!(app.hud.key_note.showing().is_none(), "and nothing is said");
    let table = app.debug_key_table();
    for e in table.entries.iter().filter(|e| e.owner != "player") {
        assert!(!e.live, "{} is listed but not live", e.action);
    }
}

/// A press the table takes doesn't reach `InputState`, nor does its release.
#[test]
fn a_consumed_debug_key_never_reaches_input_state() {
    let mut app = with_script("0 down F2\n0.5 up F2\n");
    app.core.clock.frame_step = Some(0.25);
    for _ in 0..3 {
        app.tick();
        assert!(!app.input.is_key_down(KeyCode::F2));
        assert!(!app.input.is_key_just_pressed(KeyCode::F2));
        app.input.end_frame();
    }
    assert_eq!(app.stats.view, super::StatsView::Full, "pressed once");
    assert!(app.focus.debug_keys_held.is_empty(), "and let go");
}

/// `[debug_keys]` moves an action to another key (the old one is the
/// game's again) or turns them all off, as a release build would.
#[test]
fn an_input_config_rebinds_or_disables_the_debug_keys() {
    let mut app = with_config("0 down F2\n", "[debug_keys]\nstats = \"F10\"\n");
    app.tick();
    assert_eq!(pressed(&app, &[KeyCode::F2]), [KeyCode::F2], "F2 is free");
    assert!(!app.stats.view.is_open());
    let mut app = with_config("0 down F10\n", "[debug_keys]\nstats = \"F10\"\n");
    app.tick();
    assert!(app.stats.view.is_open(), "F10 opens the stats");
    assert!(pressed(&app, &[KeyCode::F10]).is_empty());

    let mut app = with_config("0 down F2\n0 down F1\n", "[debug_keys]\nenabled = false\n");
    app.tick();
    assert!(!app.stats.view.is_open() && !app.hud.key_table_open);
    assert_eq!(
        pressed(&app, &[KeyCode::F1, KeyCode::F2]),
        [KeyCode::F1, KeyCode::F2],
        "off: every key is the game's"
    );
}

/// A game that binds a debug key gets it, and only the game, as Escape
/// already worked (#639); the table says whose it is.
#[test]
fn a_key_the_game_binds_goes_to_the_game_only() {
    let mut app = with_config(
        "0 down F2\n",
        "[actions.quicksave]\n[[actions.quicksave.bindings]]\ntype = \"key\"\ncode = \"F2\"\n",
    );
    app.tick();
    assert!(!app.stats.view.is_open(), "no stats");
    assert!(app.input.is_key_down(KeyCode::F2), "the save's key");
    assert!(app
        .debug_key_table()
        .text()
        .contains("the game's `quicksave` binds it"));
}

/// F1 shows the card of every debug key, and paints it; F1 again hides it.
#[test]
fn f1_shows_the_table_and_it_paints() {
    let mut app = with_script("0 tap F1\n0.5 tap F1\n");
    app.core.clock.frame_step = Some(0.25);
    app.tick();
    assert!(app.hud.key_table_open);
    let table = app.debug_key_table();
    let out = run_headless_frame(&app.hud.egui_ctx, egui::RawInput::default(), |ui| {
        super::debug_keys::paint_key_table(ui.ctx(), &table)
    });
    assert!(!out.shapes.is_empty(), "the card draws");
    app.input.end_frame();
    app.tick();
    app.input.end_frame();
    app.tick();
    assert!(!app.hud.key_table_open, "and goes");
}

#[test]
fn show_stats_opens_the_overlay_once() {
    // Frames at t = 0, 0.25, 0.5: F2 is tapped on the third.
    let mut app = with_script("0.5 tap F2\n");
    app.core.clock.frame_step = Some(0.25);
    app.capture.show_stats = true;
    app.tick();
    assert_eq!(
        app.stats.view,
        super::StatsView::Full,
        "`--stats`: all of it"
    );
    app.input.end_frame();
    app.tick();
    app.input.end_frame();
    app.tick();
    assert_ne!(app.stats.view, super::StatsView::Full, "F2 still steps it");
}

#[cfg(feature = "debug-hud")]
#[test]
fn stats_graph_opens_the_graph_alone_and_f2_closes_it() {
    let mut app = with_script("0.5 tap F2\n");
    app.core.clock.frame_step = Some(0.25);
    app.capture.show_stats = true;
    app.capture.stats_view = super::StatsView::Graph;
    app.tick();
    assert_eq!(app.stats.view, super::StatsView::Graph);
    app.input.end_frame();
    app.tick();
    app.input.end_frame();
    app.tick();
    assert_eq!(app.stats.view, super::StatsView::Off, "F2 closes it");
}

/// An extension with nothing but defaults, so egui sees input first.
struct Bare;
impl crate::extension::PlayerExtension for Bare {}

/// Run one egui frame on the app's context with a text field focused.
fn focus_a_field(app: &PlayerApp) {
    let _ = run_headless_frame(&app.hud.egui_ctx, egui::RawInput::default(), |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            let mut text = String::new();
            ui.text_edit_singleline(&mut text).request_focus();
        });
    });
}

#[test]
fn a_focused_egui_field_keeps_scripted_keys_from_the_game() {
    // Frames at t = 0, 0.25, 0.5 (ADR 0097).
    let mut app = with_script("0 down KeyW\n0.25 up KeyW\n0.5 tap F2\n");
    app.core.clock.frame_step = Some(0.25);
    app.add_extension(Box::new(Bare));
    focus_a_field(&app);
    assert!(app.hud.egui_ctx.egui_wants_keyboard_input());
    app.tick();
    assert!(!app.input.is_key_down(KeyCode::KeyW), "the field took it");
    app.tick();
    app.tick();
    assert!(
        !app.stats.view.is_open(),
        "the debug keys don't see it either"
    );
}

#[test]
fn a_focused_button_leaves_scripted_keys_to_the_game() {
    let mut app = with_script(
        "0 down KeyW
",
    );
    app.add_extension(Box::new(Bare));
    let _ = run_headless_frame(&app.hud.egui_ctx, egui::RawInput::default(), |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.button("Buy").request_focus();
        });
    });
    assert!(
        app.hud.egui_ctx.egui_wants_keyboard_input(),
        "egui counts any focus"
    );
    app.tick();
    assert!(
        app.input.is_key_down(KeyCode::KeyW),
        "only a field takes keys"
    );
}

#[test]
fn scripted_keys_reach_the_game_when_no_field_has_focus() {
    let mut app = with_script("0 down ShiftLeft\n0 down KeyW\n");
    app.add_extension(Box::new(Bare));
    app.tick();
    assert!(app.input.is_key_down(KeyCode::ShiftLeft));
    assert!(app.input.is_key_down(KeyCode::KeyW));
}

#[test]
fn a_release_follows_a_scripted_press_that_reached_the_game() {
    // The game saw W go down, then a field took focus: the release still
    // reaches the game, so W isn't stuck down there.
    let mut app = with_script("0 down KeyW\n0.25 up KeyW\n");
    app.core.clock.frame_step = Some(0.25);
    app.add_extension(Box::new(Bare));
    app.tick();
    assert!(app.input.is_key_down(KeyCode::KeyW));
    focus_a_field(&app);
    app.tick();
    assert!(!app.input.is_key_down(KeyCode::KeyW));
}

#[test]
fn a_scripted_click_reaches_input_state_a_frame_after_the_pointer() {
    // Frames at t = 0, 0.25, 0.5: move, press, release (ADR 0082).
    let mut app = with_script("0 click 100 50\n");
    app.core.clock.frame_step = Some(0.25);
    app.tick();
    assert_eq!(app.input.mouse_position, (100.0, 50.0));
    assert!(!app.input.is_mouse_button_down(0));
    app.tick();
    assert!(app.input.is_mouse_button_down(0));
    app.tick();
    assert!(!app.input.is_mouse_button_down(0));
}
