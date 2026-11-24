//! Typed text, as a frame's stream of edits (#1229).
//!
//! A UI document's `text_input` reads what the player typed this frame:
//! characters (a key's text, or an IME's committed string) and the editing
//! keys (Backspace, Delete, the arrows across, Home, End, Enter), in the
//! order they came, key repeats included. The host turns each window event
//! into edits ([`TextEdit::of_key`], [`TextEdit::typed`]) and hands them to
//! [`crate::InputState::process_text_edit`]; the frame's are read with
//! `text_edits()` and cleared at `end_frame`, like the other "this frame"
//! input.

use winit::keyboard::KeyCode;

/// One thing typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextEdit {
    /// Text to put in at the caret: never a control character.
    Insert(String),
    /// Remove the character before the caret.
    Backspace,
    /// Remove the character after the caret.
    Delete,
    /// Move the caret one character.
    Left,
    Right,
    /// Move the caret to the start or the end.
    Home,
    End,
    /// Enter: the player is done.
    Submit,
}

impl TextEdit {
    /// The edit a pressed key makes, from its physical code and the text the
    /// window says it typed (winit's `KeyEvent::text`; a repeat carries it
    /// too). An editing key is its edit, whatever text comes with it
    /// (Backspace types `\u{8}`, Enter `\r`); any other key is its text,
    /// with control characters left out.
    pub fn of_key(key: KeyCode, text: Option<&str>) -> Option<TextEdit> {
        let edit = match key {
            KeyCode::Backspace => TextEdit::Backspace,
            KeyCode::Delete => TextEdit::Delete,
            KeyCode::ArrowLeft => TextEdit::Left,
            KeyCode::ArrowRight => TextEdit::Right,
            KeyCode::Home => TextEdit::Home,
            KeyCode::End => TextEdit::End,
            KeyCode::Enter | KeyCode::NumpadEnter => TextEdit::Submit,
            _ => return text.and_then(TextEdit::typed),
        };
        Some(edit)
    }

    /// Typed text as an insert, without its control characters; `None` when
    /// nothing printable is left.
    pub fn typed(text: &str) -> Option<TextEdit> {
        let printable: String = text.chars().filter(|c| !c.is_control()).collect();
        (!printable.is_empty()).then_some(TextEdit::Insert(printable))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_editing_key_is_its_edit_whatever_text_it_carries() {
        assert_eq!(
            TextEdit::of_key(KeyCode::Backspace, Some("\u{8}")),
            Some(TextEdit::Backspace)
        );
        assert_eq!(
            TextEdit::of_key(KeyCode::Enter, Some("\r")),
            Some(TextEdit::Submit)
        );
        assert_eq!(
            TextEdit::of_key(KeyCode::ArrowLeft, None),
            Some(TextEdit::Left)
        );
    }

    #[test]
    fn a_letter_is_its_text_and_a_control_character_is_nothing() {
        assert_eq!(
            TextEdit::of_key(KeyCode::KeyA, Some("A")),
            Some(TextEdit::Insert("A".into()))
        );
        assert_eq!(TextEdit::of_key(KeyCode::Tab, Some("\t")), None);
        assert_eq!(TextEdit::of_key(KeyCode::ShiftLeft, None), None);
        assert_eq!(
            TextEdit::typed("é\u{7}"),
            Some(TextEdit::Insert("é".into()))
        );
    }
}
