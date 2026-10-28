//! Who gets an input event, egui or the game (the game egui layer, ADR 0073).
//!
//! While egui can have something interactive on screen (an open debug
//! panel, or any installed `PlayerExtension`, whose `ui()` may draw), every
//! window event goes to egui first. An input event egui wants is consumed:
//! extensions, the player and scripts never see it.
//!
//! - Keyboard and IME: consumed while a text field has keyboard focus
//!   ([`text_field_focused`]), a `TextEdit` being typed in. Any other focus
//!   (a button, a tab, a slot a game's pad ring sits on) leaves the keys to
//!   the game, though egui still sees them, so Enter clicks the focused
//!   button. egui's own `wants_keyboard_input` is true for any focus, so
//!   it isn't used. Unlike egui-winit's own verdict, Tab is not consumed
//!   when no field is focused, so a game can still bind it.
//! - Pointer: egui-winit's verdict. Buttons and the wheel over an egui area
//!   (`wants_pointer_input`), and cursor movement only while egui is using
//!   the pointer (dragging a slider). Clicks on empty space reach the game.
//! - A release follows its press. A key or button whose press egui took
//!   has its release taken too, and one whose press the game saw always
//!   gets its release, so neither side is left holding a key down when
//!   focus moves between them mid-press. Key repeats follow the press the
//!   same way.
//!
//! Synthetic key presses (winit sends them for keys already held when the
//! window gains focus) and lifecycle events are never consumed.
//!
//! `--input-script` keys take the same gate (ADR 0097). They reach egui as
//! the events egui-winit would have made of them ([`scripted_key_events`]).

use std::collections::HashSet;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

/// A key or button that can be held down.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum Held {
    Key(PhysicalKey),
    Mouse(MouseButton),
}

/// An input event, reduced to what routing needs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Routed {
    /// First press of a key (not a repeat) or mouse button.
    Press(Held),
    /// A key repeat while held.
    Repeat(Held),
    Release(Held),
    /// IME composition and commit.
    Ime,
    /// Cursor movement, wheel, touch, pinch.
    Pointer,
    /// Never consumed: lifecycle events, synthetic presses, the rest.
    Other,
}

pub(super) fn classify(event: &WindowEvent) -> Routed {
    match event {
        WindowEvent::KeyboardInput {
            event,
            is_synthetic,
            ..
        } => {
            let key = Held::Key(event.physical_key);
            match event.state {
                ElementState::Pressed if *is_synthetic => Routed::Other,
                ElementState::Pressed if event.repeat => Routed::Repeat(key),
                ElementState::Pressed => Routed::Press(key),
                ElementState::Released => Routed::Release(key),
            }
        }
        WindowEvent::MouseInput { state, button, .. } => {
            let button = Held::Mouse(*button);
            match state {
                ElementState::Pressed => Routed::Press(button),
                ElementState::Released => Routed::Release(button),
            }
        }
        WindowEvent::Ime(_) => Routed::Ime,
        WindowEvent::CursorMoved { .. }
        | WindowEvent::MouseWheel { .. }
        | WindowEvent::Touch(_)
        | WindowEvent::PinchGesture { .. } => Routed::Pointer,
        _ => Routed::Other,
    }
}

/// Whether a text field has the keyboard: egui's focused widget is a
/// `TextEdit`, which keeps a `TextEditState` under its id (ADR 0110's
/// amendment). Unlike `Context::wants_keyboard_input`, a focused button,
/// tab or slot doesn't count, so it doesn't hold the pads or take keys.
pub(super) fn text_field_focused(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.focused())
        .is_some_and(|id| egui::text_edit::TextEditState::load(ctx, id).is_some())
}

/// Remembers which held keys and buttons belong to egui.
#[derive(Default)]
pub(super) struct EguiInputGate {
    held_by_egui: HashSet<Held>,
}

impl EguiInputGate {
    /// Whether egui takes this event. Call after egui-winit has seen it:
    /// `egui_consumed` is its verdict, `wants_keyboard` is
    /// [`text_field_focused`].
    pub(super) fn consumes(
        &mut self,
        routed: Routed,
        egui_consumed: bool,
        wants_keyboard: bool,
    ) -> bool {
        match routed {
            Routed::Press(held) => {
                let take = match held {
                    Held::Key(_) => wants_keyboard,
                    Held::Mouse(_) => egui_consumed,
                };
                if take {
                    self.held_by_egui.insert(held);
                } else {
                    self.held_by_egui.remove(&held);
                }
                take
            }
            Routed::Repeat(held) => self.held_by_egui.contains(&held),
            Routed::Release(held) => self.held_by_egui.remove(&held),
            Routed::Ime => wants_keyboard,
            Routed::Pointer => egui_consumed,
            Routed::Other => false,
        }
    }
}

impl EguiInputGate {
    /// Whether a mouse event skips egui while the cursor is captured
    /// (ADR 0110). The cursor is hidden at the window centre, so a move, a
    /// click or a turn of the wheel there is the game's, whatever egui drew under
    /// it. A release whose press egui took before the capture still goes to
    /// egui, so nothing stays pressed there. Keys and touch are unaffected.
    pub(super) fn skips_captured(&self, event: &WindowEvent) -> bool {
        if matches!(
            event,
            WindowEvent::CursorMoved { .. } | WindowEvent::MouseWheel { .. }
        ) {
            return true;
        }
        match classify(event) {
            Routed::Release(held @ Held::Mouse(_)) => !self.held_by_egui.contains(&held),
            Routed::Press(Held::Mouse(_)) => true,
            _ => false,
        }
    }
}

/// What egui-winit would push for a key from `--input-script` (ADR 0097).
///
/// A scripted key has no winit `KeyEvent` (one can't be built outside
/// winit), so this stands in for egui-winit's `on_keyboard_input` on a US
/// layout: the key, and on a press the text it types, unless Ctrl or the
/// command key is held. No cut, copy or paste: those need the clipboard.
pub(super) fn scripted_key_events(
    key: KeyCode,
    pressed: bool,
    modifiers: egui::Modifiers,
) -> Vec<egui::Event> {
    let mut events = Vec::new();
    if let Some(egui_key) = egui_key(key) {
        events.push(egui::Event::Key {
            key: egui_key,
            physical_key: Some(egui_key),
            pressed,
            repeat: false,
            modifiers,
        });
    }
    let is_cmd = modifiers.ctrl || modifiers.command || modifiers.mac_cmd;
    if pressed && !is_cmd {
        if let Some(c) = typed_char(key, modifiers.shift) {
            events.push(egui::Event::Text(c.to_string()));
        }
    }
    events
}

/// egui's name for a key, by its winit name.
fn egui_key(key: KeyCode) -> Option<egui::Key> {
    let name = format!("{key:?}");
    let name = match key {
        KeyCode::BracketLeft => "[",
        KeyCode::BracketRight => "]",
        KeyCode::NumpadEnter => "Enter",
        KeyCode::NumpadAdd => "+",
        KeyCode::NumpadSubtract => "-",
        KeyCode::NumpadDecimal => ".",
        KeyCode::NumpadDivide => "/",
        _ => name.strip_prefix("Key").unwrap_or(&name),
    };
    egui::Key::from_name(name)
}

/// The character a key types on a US layout.
pub(super) fn typed_char(key: KeyCode, shift: bool) -> Option<char> {
    let pick = |plain, shifted| Some(if shift { shifted } else { plain });
    let name = format!("{key:?}");
    let single = |s: &str| {
        let mut chars = s.chars();
        chars.next().filter(|_| chars.next().is_none())
    };
    if let Some(letter) = name.strip_prefix("Key").and_then(single) {
        return pick(letter.to_ascii_lowercase(), letter);
    }
    if let Some(digit) = name.strip_prefix("Digit").and_then(single) {
        let shifted = ")!@#$%^&*(".chars().nth(digit.to_digit(10)? as usize)?;
        return pick(digit, shifted);
    }
    if let Some(digit) = name.strip_prefix("Numpad").and_then(single) {
        return Some(digit);
    }
    match key {
        KeyCode::Space => Some(' '),
        KeyCode::Minus => pick('-', '_'),
        KeyCode::Equal => pick('=', '+'),
        KeyCode::BracketLeft => pick('[', '{'),
        KeyCode::BracketRight => pick(']', '}'),
        KeyCode::Backslash => pick('\\', '|'),
        KeyCode::Semicolon => pick(';', ':'),
        KeyCode::Quote => pick('\'', '"'),
        KeyCode::Comma => pick(',', '<'),
        KeyCode::Period => pick('.', '>'),
        KeyCode::Slash => pick('/', '?'),
        KeyCode::Backquote => pick('`', '~'),
        KeyCode::NumpadAdd => Some('+'),
        KeyCode::NumpadSubtract => Some('-'),
        KeyCode::NumpadMultiply => Some('*'),
        KeyCode::NumpadDivide => Some('/'),
        KeyCode::NumpadDecimal => Some('.'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(key: KeyCode, modifiers: egui::Modifiers) -> Option<String> {
        scripted_key_events(key, true, modifiers)
            .into_iter()
            .find_map(|e| match e {
                egui::Event::Text(t) => Some(t),
                _ => None,
            })
    }

    #[test]
    fn a_scripted_press_is_the_key_then_its_text() {
        let shift = egui::Modifiers::SHIFT;
        let events = scripted_key_events(KeyCode::KeyA, true, shift);
        assert_eq!(
            events,
            [
                egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: Some(egui::Key::A),
                    pressed: true,
                    repeat: false,
                    modifiers: shift,
                },
                egui::Event::Text("A".into()),
            ]
        );
        // A release types nothing.
        assert_eq!(scripted_key_events(KeyCode::KeyA, false, shift).len(), 1);
    }

    #[test]
    fn scripted_keys_type_as_a_us_keyboard_does() {
        let none = egui::Modifiers::NONE;
        let shift = egui::Modifiers::SHIFT;
        assert_eq!(text(KeyCode::KeyQ, none).as_deref(), Some("q"));
        assert_eq!(text(KeyCode::Digit2, none).as_deref(), Some("2"));
        assert_eq!(text(KeyCode::Digit2, shift).as_deref(), Some("@"));
        assert_eq!(text(KeyCode::Digit0, shift).as_deref(), Some(")"));
        assert_eq!(text(KeyCode::Slash, shift).as_deref(), Some("?"));
        assert_eq!(text(KeyCode::Numpad7, shift).as_deref(), Some("7"));
        assert_eq!(text(KeyCode::Space, none).as_deref(), Some(" "));
        // Keys that type nothing, and Ctrl, which makes a shortcut instead.
        assert_eq!(text(KeyCode::Enter, none), None);
        assert_eq!(text(KeyCode::F2, none), None);
        assert_eq!(text(KeyCode::KeyC, egui::Modifiers::CTRL), None);
    }

    #[test]
    fn scripted_keys_have_egui_names() {
        assert_eq!(egui_key(KeyCode::Enter), Some(egui::Key::Enter));
        assert_eq!(egui_key(KeyCode::NumpadEnter), Some(egui::Key::Enter));
        assert_eq!(egui_key(KeyCode::Escape), Some(egui::Key::Escape));
        assert_eq!(egui_key(KeyCode::KeyZ), Some(egui::Key::Z));
        assert_eq!(egui_key(KeyCode::Digit9), Some(egui::Key::Num9));
        assert_eq!(egui_key(KeyCode::BracketLeft), Some(egui::Key::OpenBracket));
        assert_eq!(egui_key(KeyCode::Backspace), Some(egui::Key::Backspace));
        assert_eq!(egui_key(KeyCode::F12), Some(egui::Key::F12));
        // Modifiers are egui's modifiers, not keys.
        assert_eq!(egui_key(KeyCode::ShiftLeft), None);
    }

    const E: Held = Held::Key(PhysicalKey::Code(KeyCode::KeyE));
    const LEFT: Held = Held::Mouse(MouseButton::Left);

    #[test]
    fn typing_in_a_focused_field_is_consumed() {
        let mut gate = EguiInputGate::default();
        assert!(gate.consumes(Routed::Press(E), false, true));
        assert!(gate.consumes(Routed::Repeat(E), false, true));
        assert!(gate.consumes(Routed::Release(E), false, true));
        assert!(gate.consumes(Routed::Ime, false, true));
    }

    #[test]
    fn keys_reach_the_game_with_nothing_focused() {
        let mut gate = EguiInputGate::default();
        // egui-winit says it consumed Tab; with no focus the game keeps it.
        let tab = Held::Key(PhysicalKey::Code(KeyCode::Tab));
        assert!(!gate.consumes(Routed::Press(tab), true, false));
        assert!(!gate.consumes(Routed::Release(tab), true, false));
        assert!(!gate.consumes(Routed::Ime, true, false));
    }

    #[test]
    fn a_release_follows_its_press() {
        let mut gate = EguiInputGate::default();
        // The game sees the press, then a field takes focus: the release
        // still reaches the game, so the key isn't stuck down there.
        assert!(!gate.consumes(Routed::Press(E), false, false));
        assert!(!gate.consumes(Routed::Repeat(E), false, true));
        assert!(!gate.consumes(Routed::Release(E), false, true));
        // egui takes the press, then loses focus: egui keeps the release.
        assert!(gate.consumes(Routed::Press(E), false, true));
        assert!(gate.consumes(Routed::Release(E), false, false));
    }

    #[test]
    fn clicks_on_egui_are_consumed_and_clicks_elsewhere_are_not() {
        let mut gate = EguiInputGate::default();
        assert!(gate.consumes(Routed::Press(LEFT), true, false));
        assert!(gate.consumes(Routed::Release(LEFT), false, false));
        assert!(!gate.consumes(Routed::Press(LEFT), false, false));
        // Released over a panel, but the press was the game's.
        assert!(!gate.consumes(Routed::Release(LEFT), true, false));
    }

    #[test]
    fn pointer_motion_follows_egui_and_other_events_pass() {
        let mut gate = EguiInputGate::default();
        assert!(gate.consumes(Routed::Pointer, true, false));
        assert!(!gate.consumes(Routed::Pointer, false, true));
        assert!(!gate.consumes(Routed::Other, true, true));
    }

    #[test]
    fn classify_sorts_events() {
        let device_id = winit::event::DeviceId::dummy();
        let click = WindowEvent::MouseInput {
            device_id,
            state: ElementState::Pressed,
            button: MouseButton::Left,
        };
        assert_eq!(classify(&click), Routed::Press(LEFT));
        let moved = WindowEvent::CursorMoved {
            device_id,
            position: winit::dpi::PhysicalPosition::new(1.0, 2.0),
        };
        assert_eq!(classify(&moved), Routed::Pointer);
        assert_eq!(classify(&WindowEvent::RedrawRequested), Routed::Other);
        assert_eq!(
            classify(&WindowEvent::Resized(winit::dpi::PhysicalSize::new(8, 8))),
            Routed::Other
        );
    }
}
