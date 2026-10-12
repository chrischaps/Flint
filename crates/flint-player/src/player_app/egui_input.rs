//! Who gets an input event, egui or the game (the game egui layer, H3).
//!
//! While egui can have something interactive on screen (an open debug
//! panel, or any installed `PlayerExtension`, whose `ui()` may draw), every
//! window event goes to egui first. An input event egui wants is consumed:
//! extensions, the player and scripts never see it.
//!
//! - Keyboard and IME: consumed while an egui widget has keyboard focus
//!   (`wants_keyboard_input`), such as a `TextEdit` being typed in. Unlike
//!   egui-winit's own verdict, Tab is not consumed when nothing is focused,
//!   so a game can still bind it.
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

use std::collections::HashSet;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::keyboard::PhysicalKey;

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

/// Remembers which held keys and buttons belong to egui.
#[derive(Default)]
pub(super) struct EguiInputGate {
    held_by_egui: HashSet<Held>,
}

impl EguiInputGate {
    /// Whether egui takes this event. Call after egui-winit has seen it:
    /// `egui_consumed` is its verdict, `wants_keyboard` is
    /// `Context::wants_keyboard_input()`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::KeyCode;

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
