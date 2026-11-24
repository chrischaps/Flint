//! Lists that scroll and the text input (#1229): a 20-item list in a
//! 5-row box, driven by the arrows, the wheel and a finger; a name field
//! typed into. Files come from an in-memory source; input is a
//! [`UiInput`] per frame.

use super::element::Dir;
use super::focus::UiInput;
use super::{UiEvent, UiSystem};
use crate::context::{DrawCommand, DrawItem};
use flint_core::vfs::{self, MemorySource};
use flint_runtime::TextEdit;
use std::path::Path;
use std::sync::Arc;

const SCREEN: (f32, f32) = (1280.0, 720.0);

/// Twenty items of 36 points with 4 between: five rows in a 200-point box.
const LEVELS: &str = r#"
[ui]
style = "ui/a.style.toml"

[elements.levels]
type = "list"
class = "list"
item_class = "row"
scroll = true
items = [
  "Level 1", "Level 2", "Level 3", "Level 4", "Level 5",
  "Level 6", "Level 7", "Level 8", "Level 9", "Level 10",
  "Level 11", "Level 12", "Level 13", "Level 14", "Level 15",
  "Level 16", "Level 17", "Level 18", "Level 19", "Level 20",
]
"#;

const LEVELS_STYLE: &str = r#"
[styles.list]
x = 100
y = 100
width = 220
height = 200

[styles.row]
width = 220
height = 36
margin_bottom = 4
"#;

const ROW: f32 = 40.0;

/// A name field above an OK button.
const NAME: &str = r#"
[ui]
style = "ui/a.style.toml"

[elements.form]
type = "panel"
class = "form"

[elements.name]
type = "text_input"
parent = "form"
class = "field"
placeholder = "Your name"
max_length = 8
autofocus = true

[elements.ok]
type = "button"
parent = "form"
class = "field"
text = "OK"
"#;

const NAME_STYLE: &str = r#"
[styles.form]
x = 100
y = 100
width = 300
height = 200

[styles.field]
width = 240
height = 40
margin_bottom = 10
caret_color = [1.0, 0.8, 0.2, 1.0]
placeholder_color = [0.5, 0.5, 0.5, 1.0]
"#;

fn with_ui(layout: &str, style: &str, f: impl FnOnce(&mut UiSystem)) {
    let mut source = MemorySource::new();
    source.insert("ui/a.ui.toml", layout);
    source.insert("ui/a.style.toml", style);
    vfs::scoped(Arc::new(source), || {
        let mut ui = UiSystem::new();
        ui.load("ui/a.ui.toml", Path::new(""), None)
            .unwrap_or_else(|e| panic!("{e}"));
        f(&mut ui);
    });
}

fn frame(ui: &mut UiSystem, input: UiInput) -> Vec<(String, &'static str)> {
    ui.route(&input, SCREEN);
    ui.take_events()
        .into_iter()
        .map(|UiEvent { element, event, .. }| (element, event))
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

fn typing(edits: Vec<TextEdit>) -> UiInput {
    UiInput {
        text_edits: edits,
        ..Default::default()
    }
}

fn insert(s: &str) -> TextEdit {
    TextEdit::Insert(s.into())
}

fn draws(ui: &mut UiSystem) -> Vec<DrawItem> {
    ui.generate_draw_commands(SCREEN.0, SCREEN.1)
}

/// The texts drawn, each with the clip it carries.
fn texts(ui: &mut UiSystem) -> Vec<(String, Option<[f32; 4]>)> {
    draws(ui)
        .into_iter()
        .filter_map(|i| match i.cmd {
            DrawCommand::Text { text, .. } => Some((text, i.clip)),
            _ => None,
        })
        .collect()
}

fn rect(ui: &mut UiSystem, id: &str) -> (f32, f32, f32, f32) {
    ui.get_rect(id, SCREEN.0, SCREEN.1)
        .unwrap_or_else(|| panic!("{id} has a rect"))
}

/// The focused item is wholly inside the list's box, and drawn.
fn focus_in_view(ui: &mut UiSystem) {
    let focus = ui.focused().expect("something has focus").to_string();
    let (_, y, _, h) = rect(ui, &focus);
    assert!(
        y >= 100.0 - 0.01 && y + h <= 300.0 + 0.01,
        "{focus} at {y}..{} is outside the list's 100..300",
        y + h
    );
    let label = ui.text(&focus).unwrap();
    assert!(
        texts(ui).iter().any(|(t, _)| *t == label),
        "{focus} ({label}) is drawn"
    );
}

#[test]
fn the_arrows_scroll_the_list_and_the_focused_item_is_always_drawn() {
    with_ui(LEVELS, LEVELS_STYLE, |ui| {
        ui.focus(Some("levels.0"));
        idle(ui);
        assert_eq!(ui.scroll_offset("levels"), 0.0);
        for n in 1..20 {
            frame(ui, nav(Dir::Down));
            assert_eq!(ui.focused(), Some(format!("levels.{n}").as_str()));
            focus_in_view(ui);
        }
        // The last row sits at the bottom of the box: 20 rows of 40 less the
        // last's margin is 796, less the 200 in view
        assert!((ui.scroll_offset("levels") - (19.0 * ROW + 36.0 - 200.0)).abs() < 0.01);
        // Down again wraps to the first, and the list goes back to the top
        frame(ui, nav(Dir::Down));
        assert_eq!(ui.focused(), Some("levels.0"));
        focus_in_view(ui);
        assert_eq!(ui.scroll_offset("levels"), 0.0);
        // Up from the first wraps to the last
        frame(ui, nav(Dir::Up));
        assert_eq!(ui.focused(), Some("levels.19"));
        focus_in_view(ui);
    });
}

#[test]
fn only_the_rows_in_view_are_drawn_and_each_is_clipped_to_the_box() {
    with_ui(LEVELS, LEVELS_STYLE, |ui| {
        idle(ui);
        let drawn = texts(ui);
        let labels: Vec<&str> = drawn.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(
            labels,
            ["Level 1", "Level 2", "Level 3", "Level 4", "Level 5"],
            "five rows fit, the sixth starts at the box's end"
        );
        for (_, clip) in &drawn {
            assert_eq!(*clip, Some([100.0, 100.0, 220.0, 200.0]));
        }
        // A row half in view is drawn, clipped
        ui.scroll_by("levels", 20.0);
        let labels: Vec<String> = texts(ui).into_iter().map(|(t, _)| t).collect();
        assert_eq!(labels.first().map(String::as_str), Some("Level 1"));
        assert_eq!(labels.last().map(String::as_str), Some("Level 6"));
    });
}

#[test]
fn the_wheel_scrolls_a_row_a_notch_and_focus_stays_in_view() {
    with_ui(LEVELS, LEVELS_STYLE, |ui| {
        ui.focus(Some("levels.0"));
        idle(ui);
        let over = UiInput {
            pointer: (200.0, 150.0),
            wheel: -3.0,
            ..Default::default()
        };
        let events = frame(ui, over);
        assert!((ui.scroll_offset("levels") - 3.0 * ROW).abs() < 0.01);
        // Level 1 left the view: focus is on the nearest row wholly in it
        assert_eq!(ui.focused(), Some("levels.3"));
        assert!(events.contains(&("levels".to_string(), "change")));
        focus_in_view(ui);
        // Far past the end stops at the end
        let over = UiInput {
            pointer: (200.0, 150.0),
            wheel: -100.0,
            ..Default::default()
        };
        frame(ui, over);
        assert!((ui.scroll_offset("levels") - (19.0 * ROW + 36.0 - 200.0)).abs() < 0.01);
        focus_in_view(ui);
        // Away from the list the wheel does nothing
        let away = UiInput {
            pointer: (900.0, 600.0),
            wheel: 5.0,
            ..Default::default()
        };
        let before = ui.scroll_offset("levels");
        frame(ui, away);
        assert_eq!(ui.scroll_offset("levels"), before);
    });
}

#[test]
fn a_row_clipped_away_cant_be_pointed_at() {
    with_ui(LEVELS, LEVELS_STYLE, |ui| {
        idle(ui);
        // Level 7 would be at 340..376, below the box: hovering there
        // focuses nothing
        frame(
            ui,
            UiInput {
                pointer: (200.0, 350.0),
                pointer_moved: true,
                ..Default::default()
            },
        );
        frame(
            ui,
            UiInput {
                pointer: (201.0, 355.0),
                pointer_moved: true,
                ..Default::default()
            },
        );
        assert_eq!(ui.focused(), None);
        // Inside the box hover still works
        frame(
            ui,
            UiInput {
                pointer: (200.0, 150.0),
                pointer_moved: true,
                ..Default::default()
            },
        );
        assert_eq!(ui.focused(), Some("levels.1"));
    });
}

#[test]
fn a_finger_dragged_up_the_list_scrolls_it_and_its_lift_is_no_tap() {
    with_ui(LEVELS, LEVELS_STYLE, |ui| {
        idle(ui);
        let down = |y: f32| UiInput {
            touches_started: vec![(200.0, y)],
            touches: vec![(200.0, y)],
            ..Default::default()
        };
        let held = |y: f32| UiInput {
            touches: vec![(200.0, y)],
            ..Default::default()
        };
        frame(ui, down(280.0));
        frame(ui, held(276.0));
        assert_eq!(ui.scroll_offset("levels"), 0.0, "inside the slop it waits");
        frame(ui, held(200.0));
        assert!((ui.scroll_offset("levels") - 80.0).abs() < 0.01);
        frame(ui, held(160.0));
        assert!((ui.scroll_offset("levels") - 120.0).abs() < 0.01);
        focus_in_view(ui);
        // The lift reads as a tap where the finger left: it activates nothing
        let lift = UiInput {
            taps: vec![(200.0, 160.0)],
            ..Default::default()
        };
        let events = frame(ui, lift);
        assert!(!events.iter().any(|(_, e)| *e == "activate"), "{events:?}");
        // A plain tap still activates
        let tap = UiInput {
            touches_started: vec![(200.0, 150.0)],
            touches: vec![(200.0, 150.0)],
            ..Default::default()
        };
        frame(ui, tap);
        let events = frame(
            ui,
            UiInput {
                taps: vec![(200.0, 150.0)],
                ..Default::default()
            },
        );
        assert!(events.contains(&("levels".to_string(), "activate")));
    });
}

#[test]
fn an_overflowing_list_draws_its_thumb_as_far_along_as_it_has_scrolled() {
    with_ui(LEVELS, LEVELS_STYLE, |ui| {
        idle(ui);
        let thumb = |ui: &mut UiSystem| {
            draws(ui)
                .into_iter()
                .filter_map(|i| match i.cmd {
                    DrawCommand::RectFilled {
                        x, y, w: 3.0, h, ..
                    } => Some((x, y, h)),
                    _ => None,
                })
                .next_back()
                .expect("a thumb")
        };
        let (x, y, h) = thumb(ui);
        assert_eq!(x, 100.0 + 220.0 - 3.0, "on the right edge");
        assert_eq!(y, 100.0, "at the top");
        // A quarter of the content is in view: 200 of 796
        assert!((h - 200.0 * 200.0 / 796.0).abs() < 0.01);
        ui.scroll_by("levels", 1000.0);
        let (_, y, h) = thumb(ui);
        assert!((y + h - 300.0).abs() < 0.01, "at the bottom");
    });
}

#[test]
fn scroll_belongs_to_lists_alone() {
    let mut source = MemorySource::new();
    source.insert(
        "ui/a.ui.toml",
        "[elements.b]\ntype = \"button\"\nscroll = true\n[elements.t]\ntype = \"text_input\"\nmax_length = -1\n",
    );
    let err = vfs::scoped(Arc::new(source), || {
        UiSystem::new()
            .load("ui/a.ui.toml", Path::new(""), None)
            .unwrap_err()
    });
    assert!(err.contains("`scroll` is not a button's key"), "{err}");
    assert!(err.contains("`max_length` is a whole number"), "{err}");
}

#[test]
fn a_name_field_takes_typing_and_says_change_once_a_frame() {
    with_ui(NAME, NAME_STYLE, |ui| {
        idle(ui);
        assert_eq!(ui.focused(), Some("name"));
        assert!(ui.editing());
        let events = frame(ui, typing(vec![insert("A"), insert("d"), insert("a")]));
        assert_eq!(ui.text("name").as_deref(), Some("Ada"));
        assert_eq!(events, [("name".to_string(), "change")]);
        // Backspace, then the caret back one and a letter in
        frame(ui, typing(vec![TextEdit::Backspace]));
        assert_eq!(ui.text("name").as_deref(), Some("Ad"));
        frame(ui, typing(vec![TextEdit::Left, insert("l")]));
        assert_eq!(ui.text("name").as_deref(), Some("Ald"));
        frame(ui, typing(vec![TextEdit::Home, TextEdit::Delete]));
        assert_eq!(ui.text("name").as_deref(), Some("ld"));
        frame(ui, typing(vec![TextEdit::End, insert("é ñ")]));
        assert_eq!(ui.text("name").as_deref(), Some("ldé ñ"));
        // Past max_length (8) the rest is dropped
        frame(ui, typing(vec![insert("123456")]));
        assert_eq!(ui.text("name").as_deref(), Some("ldé ñ123"));
        // Moving the caret alone changes nothing
        let events = frame(ui, typing(vec![TextEdit::Home]));
        assert!(events.is_empty(), "{events:?}");
        // Enter is done
        let events = frame(ui, typing(vec![TextEdit::Submit]));
        assert_eq!(events, [("name".to_string(), "activate")]);
    });
}

#[test]
fn the_keys_a_field_typed_with_are_its_own() {
    with_ui(NAME, NAME_STYLE, |ui| {
        idle(ui);
        // Space types a space: no ui_accept with it
        let space = UiInput {
            accept: true,
            text_edits: vec![insert(" ")],
            ..Default::default()
        };
        let events = frame(ui, space);
        assert_eq!(events, [("name".to_string(), "change")]);
        // Backspace deletes: no ui_back with it
        let back = UiInput {
            back: true,
            text_edits: vec![TextEdit::Backspace],
            ..Default::default()
        };
        let events = frame(ui, back);
        assert_eq!(events, [("name".to_string(), "change")]);
        // Left and right are the caret's: focus stays
        frame(ui, nav(Dir::Right));
        frame(ui, nav(Dir::Left));
        assert_eq!(ui.focused(), Some("name"));
        // A pad's south button (accept with nothing typed) is done
        let pad = UiInput {
            accept: true,
            ..Default::default()
        };
        assert_eq!(frame(ui, pad), [("name".to_string(), "activate")]);
        // Down leaves the field
        frame(ui, nav(Dir::Down));
        assert_eq!(ui.focused(), Some("ok"));
        assert!(!ui.editing());
        // Typing with the button focused types nothing
        frame(ui, typing(vec![insert("x")]));
        assert_eq!(ui.text("name").as_deref(), Some(""));
    });
}

#[test]
fn the_caret_is_drawn_only_with_focus_and_the_placeholder_only_while_empty() {
    with_ui(NAME, NAME_STYLE, |ui| {
        idle(ui);
        let caret_color = [1.0, 0.8, 0.2, 1.0];
        let carets = |ui: &mut UiSystem| -> Vec<f32> {
            draws(ui)
                .into_iter()
                .filter_map(|i| match i.cmd {
                    DrawCommand::RectFilled { x, color, .. } if color == caret_color => Some(x),
                    _ => None,
                })
                .collect()
        };
        let empty_caret = carets(ui);
        assert_eq!(empty_caret.len(), 1, "focused: one caret");
        assert!(texts(ui).iter().any(|(t, _)| t == "Your name"));
        frame(ui, typing(vec![insert("Bo")]));
        let typed_caret = carets(ui);
        assert!(typed_caret[0] > empty_caret[0], "the caret moved right");
        let shown: Vec<String> = texts(ui).into_iter().map(|(t, _)| t).collect();
        assert!(shown.contains(&"Bo".to_string()) && !shown.contains(&"Your name".to_string()));
        // The text is clipped to the field's inside
        let clip = texts(ui)
            .into_iter()
            .find(|(t, _)| t == "Bo")
            .and_then(|(_, c)| c)
            .expect("the field's text is clipped");
        let (x, y, w, h) = rect(ui, "name");
        assert!(
            clip[0] > x && clip[0] + clip[2] < x + w && clip[1] > y && clip[1] + clip[3] < y + h
        );
        frame(ui, nav(Dir::Down));
        assert!(carets(ui).is_empty(), "no focus, no caret");
    });
}

#[test]
fn a_long_name_slides_left_so_the_caret_stays_in_the_box() {
    with_ui(
        &NAME.replace("max_length = 8", "max_length = 0"),
        NAME_STYLE,
        |ui| {
            idle(ui);
            frame(ui, typing(vec![insert(&"W".repeat(60))]));
            let caret = draws(ui)
                .into_iter()
                .find_map(|i| match i.cmd {
                    DrawCommand::RectFilled {
                        x,
                        w,
                        color: [1.0, 0.8, 0.2, 1.0],
                        ..
                    } => Some(x + w),
                    _ => None,
                })
                .unwrap();
            let (x, _, w, _) = rect(ui, "name");
            assert!(caret <= x + w, "caret {caret} inside {}", x + w);
            assert!(caret > x + w - 20.0, "at the right edge");
        },
    );
}

#[test]
fn a_click_puts_the_caret_at_the_nearest_character() {
    with_ui(NAME, NAME_STYLE, |ui| {
        idle(ui);
        ui.set_text("name", "abcd");
        // The fallback measurer is 0.6 of 16 points a glyph: 9.6 each, from
        // the padding's 8
        let (x, y, _, _) = rect(ui, "name");
        let click = UiInput {
            pointer: (x + 8.0 + 9.6 * 2.0 + 1.0, y + 10.0),
            mouse_pressed: true,
            mouse_down: true,
            ..Default::default()
        };
        frame(ui, click);
        frame(ui, typing(vec![insert("-")]));
        assert_eq!(ui.text("name").as_deref(), Some("ab-cd"));
    });
}

#[test]
fn set_text_puts_the_caret_after_it() {
    with_ui(NAME, NAME_STYLE, |ui| {
        idle(ui);
        ui.set_text("name", "Kit");
        frame(ui, typing(vec![insert("e")]));
        assert_eq!(ui.text("name").as_deref(), Some("Kite"));
    });
}
