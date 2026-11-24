//! Text inputs (#1229).
//!
//! A `text_input` takes the frame's typed edits while it has focus
//! ([`flint_runtime::TextEdit`]: characters, Backspace, Delete, the arrows
//! across, Home, End, Enter). Its text is the element's text (`ui_set_text`
//! sets it, `ui_get_text` reads it), its caret a character index. Each frame
//! that changes the text sends one `change`; Enter, or a pad's `ui_accept`,
//! sends `activate`: the player is done with it. Keys the field typed with
//! are its own that frame: Space types a space rather than `ui_accept`,
//! Backspace deletes rather than `ui_back`, and left and right move the
//! caret rather than focus (up and down still leave the field).
//!
//! A click puts the caret at the character nearest it. The text slides left
//! when the caret would pass the box's right edge, and is clipped to it.

use super::element::UiElement;
use super::focus::At;
use super::layout::{control_padding, measure, ResolvedRect};
use super::style::ResolvedStyle;
use super::UiSystem;
use crate::context::TextMeasurer;
use flint_runtime::TextEdit;

/// Where a text input's text and caret go in its box.
#[derive(Debug, Clone, Copy)]
pub struct TextPlace {
    /// The box inside its padding, which the text is clipped to.
    pub inner: ResolvedRect,
    /// Where the text starts (left of `inner` once it has slid).
    pub text_x: f32,
    /// The top of the line, which sits in the middle of the box.
    pub line_y: f32,
    pub line_h: f32,
    /// The caret's left edge, and its width.
    pub caret_x: f32,
    pub caret_w: f32,
}

/// The text before character `n`.
fn prefix(text: &str, n: usize) -> &str {
    match text.char_indices().nth(n) {
        Some((at, _)) => &text[..at],
        None => text,
    }
}

/// Lay out a text input's line in `rect`.
pub fn place(
    elem: &UiElement,
    rect: ResolvedRect,
    style: &ResolvedStyle,
    measurer: Option<&TextMeasurer>,
) -> TextPlace {
    let p = control_padding(style);
    let inner = ResolvedRect {
        x: rect.x + p[0],
        y: rect.y + p[1],
        w: (rect.w - p[0] - p[2]).max(0.0),
        h: (rect.h - p[1] - p[3]).max(0.0),
    };
    let text = elem.effective_text();
    let (_, line_h) = measure(measurer, " ", style);
    let caret = elem.caret.min(text.chars().count());
    let (before, _) = measure(measurer, prefix(text, caret), style);
    let caret_w = (style.font_size / 12.0).max(1.5);
    // Slide left so the caret stays inside the box
    let slide = (before + caret_w - inner.w).max(0.0);
    let text_x = inner.x - slide;
    TextPlace {
        inner,
        text_x,
        line_y: rect.y + (rect.h - line_h) / 2.0,
        line_h,
        caret_x: text_x + before,
        caret_w,
    }
}

impl UiSystem {
    /// Apply a frame's typed edits to the text input at `at`: a `change`
    /// when its text moved, an `activate` on Enter. Whether there were any.
    pub(super) fn type_into(&mut self, at: At, edits: &[TextEdit]) -> bool {
        if edits.is_empty() {
            return false;
        }
        let (d, i) = at;
        let e = &mut self.documents[d].elements[i];
        let before: Vec<char> = e.effective_text().chars().collect();
        let mut text = before.clone();
        let mut caret = e.caret.min(text.len());
        let mut submit = false;
        for edit in edits {
            match edit {
                TextEdit::Insert(s) => {
                    for c in s.chars() {
                        if e.max_length > 0 && text.len() >= e.max_length {
                            break;
                        }
                        text.insert(caret, c);
                        caret += 1;
                    }
                }
                TextEdit::Backspace if caret > 0 => {
                    caret -= 1;
                    text.remove(caret);
                }
                TextEdit::Delete if caret < text.len() => {
                    text.remove(caret);
                }
                TextEdit::Left => caret = caret.saturating_sub(1),
                TextEdit::Right => caret = (caret + 1).min(text.len()),
                TextEdit::Home => caret = 0,
                TextEdit::End => caret = text.len(),
                TextEdit::Submit => submit = true,
                TextEdit::Backspace | TextEdit::Delete => {}
            }
        }
        e.caret = caret;
        let changed = text != before;
        if changed {
            e.text_override = Some(text.into_iter().collect());
        }
        let id = e.id.clone();
        if changed {
            self.push(at, id.clone(), "change");
        }
        if submit {
            self.push(at, id, "activate");
        }
        true
    }

    /// Put a text input's caret at the character nearest `x`.
    pub(super) fn caret_to(&mut self, at: At, x: f32) {
        let (d, i) = at;
        let doc = &self.documents[d];
        let elem = &doc.elements[i];
        let Some((rect, style)) = doc.cached_rects.get(&elem.id) else {
            return;
        };
        let measurer = self.measurer.as_ref();
        let place = place(elem, *rect, style, measurer);
        let text = elem.effective_text();
        let count = text.chars().count();
        let mut best = (f32::MAX, count);
        for n in 0..=count {
            let (w, _) = measure(measurer, prefix(text, n), style);
            let off = (place.text_x + w - x).abs();
            if off < best.0 {
                best = (off, n);
            }
        }
        self.documents[d].elements[i].caret = best.1;
    }

    /// Put a text input's caret after its text.
    pub(super) fn caret_to_end(&mut self, at: At) {
        let (d, i) = at;
        let e = &mut self.documents[d].elements[i];
        e.caret = e.effective_text().chars().count();
    }
}

#[cfg(test)]
mod tests {
    use super::prefix;

    #[test]
    fn a_prefix_counts_characters_not_bytes() {
        assert_eq!(prefix("héllo", 2), "hé");
        assert_eq!(prefix("héllo", 9), "héllo");
        assert_eq!(prefix("", 0), "");
    }
}
