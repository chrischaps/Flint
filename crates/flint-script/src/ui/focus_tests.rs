//! The structured UI (#731): a strict loader, file order, and buttons,
//! lists and sliders driven by keys, pads, the mouse and touch. Files come
//! from an in-memory source; input is a [`UiInput`] per frame.

use super::element::Dir;
use super::focus::UiInput;
use super::{UiEvent, UiSystem};
use crate::context::DrawCommand;
use flint_core::vfs::{self, MemorySource};
use flint_core::EntityId;
use std::path::Path;
use std::sync::Arc;

const SCREEN: (f32, f32) = (1280.0, 720.0);

const PAUSE: &str = r#"
[ui]
style = "ui/pause.style.toml"

[elements.menu]
type = "panel"
anchor = "center"
class = "menu"

[elements.resume]
type = "button"
parent = "menu"
class = "button"
text = "Resume"
autofocus = true

[elements.restart]
type = "button"
parent = "menu"
class = "button"
text = "Restart"

[elements.quit]
type = "button"
parent = "menu"
class = "button"
text = "Quit"
"#;

const PAUSE_STYLE: &str = r#"
[styles.menu]
width = 240
height = 200

[styles.button]
width = 200
height = 40
margin_bottom = 10
bg_color = [0.2, 0.2, 0.2, 1.0]

[styles.button.focus]
bg_color = [1.0, 0.5, 0.0, 1.0]
"#;

/// Run `f` with `files` as the file system and a UI system that loaded
/// `ui/a.ui.toml` for entity 7.
fn with_ui(files: &[(&str, &str)], f: impl FnOnce(&mut UiSystem)) {
    let mut source = MemorySource::new();
    for (path, text) in files {
        source.insert(path, *text);
    }
    vfs::scoped(Arc::new(source), || {
        let mut ui = UiSystem::new();
        ui.load("ui/a.ui.toml", Path::new(""), Some(owner()))
            .unwrap_or_else(|e| panic!("{e}"));
        f(&mut ui);
    });
}

fn owner() -> EntityId {
    EntityId::from_raw(7)
}

fn load_error(layout: &str, style: &str) -> String {
    let mut source = MemorySource::new();
    source.insert("ui/a.ui.toml", layout);
    source.insert("ui/a.style.toml", style);
    vfs::scoped(Arc::new(source), || {
        UiSystem::new()
            .load("ui/a.ui.toml", Path::new(""), None)
            .unwrap_err()
    })
}

fn pause(f: impl FnOnce(&mut UiSystem)) {
    with_ui(
        &[
            ("ui/a.ui.toml", PAUSE),
            ("ui/pause.style.toml", PAUSE_STYLE),
        ],
        f,
    );
}

fn frame(ui: &mut UiSystem, input: UiInput) -> Vec<(String, &'static str)> {
    ui.route(&input, SCREEN);
    ui.take_events()
        .into_iter()
        .map(
            |UiEvent {
                 element,
                 event,
                 owner: o,
             }| {
                assert_eq!(o, Some(owner()), "events go to the script that loaded it");
                (element, event)
            },
        )
        .collect()
}

fn idle(ui: &mut UiSystem) -> Vec<(String, &'static str)> {
    frame(ui, UiInput::default())
}

fn nav(dir: Dir) -> UiInput {
    UiInput {
        nav: Some(dir),
        ..Default::default()
    }
}

fn accept() -> UiInput {
    UiInput {
        accept: true,
        ..Default::default()
    }
}

fn center(ui: &mut UiSystem, id: &str) -> (f32, f32) {
    let (x, y, w, h) = ui.get_rect(id, SCREEN.0, SCREEN.1).unwrap();
    (x + w / 2.0, y + h / 2.0)
}

fn ev(id: &str, event: &'static str) -> (String, &'static str) {
    (id.to_string(), event)
}

#[test]
fn a_bad_type_is_an_error_naming_the_file_and_the_element() {
    let err = load_error("[elements.title]\ntype = \"buton\"\n", "");
    assert!(err.contains("ui/a.ui.toml"), "{err}");
    assert!(err.contains("element `title`"), "{err}");
    assert!(err.contains("unknown type `buton`"), "{err}");
    assert!(err.contains("button"), "names the types it takes: {err}");
}

#[test]
fn every_mistake_in_a_layout_is_named_at_once() {
    let err = load_error(
        "[ui]\nstyle = \"ui/a.style.toml\"\n[elements.a]\nclass = \"x\"\n",
        "[styles.x]\ncolour = [1, 0, 0]\nfont_size = \"big\"\n[styles.x.focus]\nbg = 1\n",
    );
    // Each of the style file's mistakes, with its class (and state)
    assert!(
        err.contains("class `x`: unknown style property `colour`"),
        "{err}"
    );
    assert!(err.contains("`font_size` takes a number"), "{err}");
    assert!(
        err.contains("class `x`, focus: unknown style property `bg`"),
        "{err}"
    );

    let err = load_error(
        r#"
[elements.a]
anchor = "middle"
parnet = "b"
[elements.b]
parent = "nowhere"
[elements.c]
type = "text"
min = 1
"#,
        "",
    );
    for want in [
        "element `a`: unknown anchor `middle`",
        "element `a`: unknown key `parnet`",
        "element `b`: parent `nowhere` is no element",
        "element `c`: `min` is not a text's key",
    ] {
        assert!(err.contains(want), "missing {want:?} in:\n{err}");
    }
    assert!(err.contains("4 problems"), "{err}");
}

#[test]
fn a_class_the_style_file_lacks_is_an_error() {
    let err = load_error(
        "[ui]\nstyle = \"ui/a.style.toml\"\n[elements.a]\nclass = \"titel\"\n",
        "[styles.title]\nfont_size = 20\n",
    );
    assert!(
        err.contains("element `a`: class `titel`: ui/a.style.toml has no [styles.titel]"),
        "{err}"
    );
}

#[test]
fn siblings_stack_in_the_order_the_file_writes_them() {
    let layout = r#"
[elements.list]
type = "panel"
[elements.zebra]
parent = "list"
class = "row"
[elements.apple]
parent = "list"
class = "row"
[elements.mango]
parent = "list"
class = "row"
"#;
    let style = "[styles.row]\nheight = 10\n";
    let layout = format!("[ui]\nstyle = \"ui/a.style.toml\"\n{layout}");
    with_ui(
        &[("ui/a.ui.toml", &layout), ("ui/a.style.toml", style)],
        |ui| {
            let y = |ui: &mut UiSystem, id| ui.get_rect(id, 100.0, 100.0).unwrap().1;
            assert_eq!(y(ui, "zebra"), 0.0);
            assert_eq!(y(ui, "apple"), 10.0);
            assert_eq!(y(ui, "mango"), 20.0);
        },
    );
}

#[test]
fn a_pause_menu_works_from_the_keys_and_a_pad() {
    pause(|ui| {
        // Shown with nothing focused: the autofocus button takes it
        assert_eq!(idle(ui), vec![ev("resume", "focus")]);
        assert_eq!(
            frame(ui, nav(Dir::Down)),
            vec![ev("resume", "blur"), ev("restart", "focus")]
        );
        assert_eq!(
            frame(ui, nav(Dir::Down)),
            vec![ev("restart", "blur"), ev("quit", "focus")]
        );
        // Nothing below the last: focus stays
        assert!(frame(ui, nav(Dir::Down)).is_empty());
        frame(ui, nav(Dir::Up));
        assert_eq!(frame(ui, accept()), vec![ev("restart", "activate")]);
        let back = UiInput {
            back: true,
            ..Default::default()
        };
        assert_eq!(frame(ui, back), vec![ev("restart", "back")]);
    });
}

#[test]
fn hiding_the_menu_lets_focus_go_and_arrows_then_do_nothing() {
    pause(|ui| {
        idle(ui);
        ui.hide("menu");
        assert_eq!(idle(ui), vec![ev("resume", "blur")]);
        assert!(frame(ui, nav(Dir::Down)).is_empty());
        assert!(frame(ui, accept()).is_empty());
        assert_eq!(ui.focused(), None);
        // Nothing of a hidden parent's draws, its grandchildren included
        assert!(ui.generate_draw_commands(SCREEN.0, SCREEN.1).is_empty());
        ui.show("menu");
        assert_eq!(idle(ui), vec![ev("resume", "focus")]);
    });
}

#[test]
fn the_mouse_hovers_to_focus_and_clicks_to_activate() {
    pause(|ui| {
        idle(ui);
        let at = center(ui, "quit");
        frame(
            ui,
            UiInput {
                pointer: (0.0, 0.0),
                ..Default::default()
            },
        );
        let hover = frame(
            ui,
            UiInput {
                pointer: at,
                pointer_moved: true,
                ..Default::default()
            },
        );
        assert_eq!(hover, vec![ev("resume", "blur"), ev("quit", "focus")]);
        let down = UiInput {
            pointer: at,
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        };
        assert!(
            frame(ui, down).is_empty(),
            "a press alone activates nothing"
        );
        let up = UiInput {
            pointer: at,
            ..Default::default()
        };
        assert_eq!(frame(ui, up), vec![ev("quit", "activate")]);
    });
}

#[test]
fn a_release_off_the_button_activates_nothing() {
    pause(|ui| {
        idle(ui);
        let at = center(ui, "quit");
        frame(
            ui,
            UiInput {
                pointer: at,
                mouse_pressed: true,
                mouse_down: true,
                ..Default::default()
            },
        );
        let away = frame(
            ui,
            UiInput {
                pointer: (5.0, 5.0),
                ..Default::default()
            },
        );
        assert!(!away.contains(&ev("quit", "activate")), "{away:?}");
    });
}

#[test]
fn a_tap_activates_the_button_under_it() {
    pause(|ui| {
        idle(ui);
        let at = center(ui, "restart");
        let tap = UiInput {
            touches_started: vec![at],
            taps: vec![at],
            ..Default::default()
        };
        assert_eq!(
            frame(ui, tap),
            vec![
                ev("resume", "blur"),
                ev("restart", "focus"),
                ev("restart", "activate")
            ]
        );
    });
}

#[test]
fn focus_draws_its_state() {
    pause(|ui| {
        idle(ui);
        let fills = |ui: &mut UiSystem| -> Vec<[f32; 4]> {
            ui.generate_draw_commands(SCREEN.0, SCREEN.1)
                .into_iter()
                .filter_map(|c| match c {
                    DrawCommand::RectFilled { color, .. } => Some(color),
                    _ => None,
                })
                .collect()
        };
        let orange = [1.0, 0.5, 0.0, 1.0];
        let grey = [0.2, 0.2, 0.2, 1.0];
        assert_eq!(fills(ui), vec![orange, grey, grey]);
        frame(ui, nav(Dir::Down));
        assert_eq!(fills(ui), vec![grey, orange, grey]);
        // The label is drawn centred in the box
        let label = ui
            .generate_draw_commands(SCREEN.0, SCREEN.1)
            .into_iter()
            .find_map(|c| match c {
                DrawCommand::Text { text, align, .. } if text == "Restart" => Some(align),
                _ => None,
            });
        assert_eq!(label, Some(1));
    });
}

const SETTINGS: &str = r#"
[elements.difficulty]
type = "list"
items = ["Easy", "Normal", "Hard"]
value = 1

[elements.volume]
type = "slider"
min = 0
max = 10
step = 1
value = 5
"#;

#[test]
fn a_list_steps_through_its_items_and_wraps() {
    with_ui(&[("ui/a.ui.toml", SETTINGS)], |ui| {
        assert_eq!(ui.value("difficulty"), 1.0);
        ui.focus(Some("difficulty.1"));
        ui.take_events();
        // An item's events carry the list's id; its value follows focus
        assert_eq!(
            frame(ui, nav(Dir::Down)),
            vec![
                ev("difficulty", "blur"),
                ev("difficulty", "focus"),
                ev("difficulty", "change")
            ]
        );
        assert_eq!(ui.value("difficulty"), 2.0);
        frame(ui, nav(Dir::Down));
        assert_eq!(ui.value("difficulty"), 0.0, "wraps to the first");
        assert_eq!(frame(ui, accept()), vec![ev("difficulty", "activate")]);
        ui.set_items("difficulty", vec!["Story".into(), "Hard".into()]);
        assert!(ui.exists("difficulty.1") && !ui.exists("difficulty.2"));
    });
}

#[test]
fn a_slider_steps_with_left_and_right_and_follows_a_drag() {
    with_ui(&[("ui/a.ui.toml", SETTINGS)], |ui| {
        ui.focus(Some("volume"));
        ui.take_events();
        assert_eq!(frame(ui, nav(Dir::Right)), vec![ev("volume", "change")]);
        assert_eq!(ui.value("volume"), 6.0);
        frame(ui, nav(Dir::Left));
        frame(ui, nav(Dir::Left));
        assert_eq!(ui.value("volume"), 4.0);
        let (x, y, w, h) = ui.get_rect("volume", SCREEN.0, SCREEN.1).unwrap();
        let at = (x + w * 0.9, y + h / 2.0);
        frame(
            ui,
            UiInput {
                pointer: at,
                mouse_pressed: true,
                mouse_down: true,
                ..Default::default()
            },
        );
        assert_eq!(ui.value("volume"), 9.0);
        ui.set_value("volume", 42.0);
        assert_eq!(ui.value("volume"), 10.0, "clamped to max");
    });
}

#[test]
fn a_held_direction_repeats() {
    use super::focus::{PointerState, REPEAT_AFTER, REPEAT_EVERY};
    use crate::context::InputSnapshot;
    let mut state = PointerState::default();
    let mut snap = InputSnapshot::default();
    snap.actions_pressed.insert("ui_down".into());
    snap.actions_just_pressed.insert("ui_down".into());
    let mut step = |snap: &InputSnapshot, dt| {
        UiInput::from_snapshot(snap, (0.0, 0.0), SCREEN, dt, &mut state).nav
    };
    assert_eq!(step(&snap, 0.016), Some(Dir::Down));
    snap.actions_just_pressed.clear();
    assert_eq!(step(&snap, REPEAT_AFTER / 2.0), None);
    assert_eq!(step(&snap, REPEAT_AFTER), Some(Dir::Down));
    assert_eq!(step(&snap, REPEAT_EVERY / 2.0), None);
    assert_eq!(step(&snap, REPEAT_EVERY), Some(Dir::Down));
    snap.actions_pressed.clear();
    assert_eq!(step(&snap, 1.0), None);
}
