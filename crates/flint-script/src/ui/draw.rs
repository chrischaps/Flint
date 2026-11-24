//! A document's draw commands: each shown element, in element order, by
//! its type, in the style its state picks (#731). An element inside a
//! scrolling list carries the list's clip, and each overflowing list ends
//! with its thumb (#1229).

use super::element::{ElementType, UiElement};
use super::layout::{measure, ResolvedRect, ScrollBox};
use super::style::{self, ResolvedStyle};
use super::text_input;
use super::UiDocument;
use crate::context::{DrawCommand, DrawItem, TextMeasurer};

fn faded(mut color: [f32; 4], opacity: f32) -> [f32; 4] {
    color[3] *= opacity;
    color
}

/// Where an element's commands go, and the clip they carry.
struct Out<'a> {
    items: &'a mut Vec<DrawItem>,
    clip: Option<[f32; 4]>,
}

impl Out<'_> {
    fn push(&mut self, cmd: DrawCommand) {
        self.items.push(DrawItem {
            cmd,
            clip: self.clip,
            over_ui: false,
        });
    }

    /// Push with a clip narrowed to `rect` too.
    fn push_inside(&mut self, cmd: DrawCommand, rect: [f32; 4]) {
        let clip = match self.clip {
            Some(outer) => crate::draw2d::intersect_rect(outer, rect),
            None => rect,
        };
        self.items.push(DrawItem {
            cmd,
            clip: Some(clip),
            over_ui: false,
        });
    }
}

/// The commands for every shown element of `doc`, whose layout (and the
/// state styles in it) is current. `focus` is the element with focus, whose
/// caret a text input draws.
pub(super) fn commands(
    doc: &UiDocument,
    measurer: Option<&TextMeasurer>,
    focus: Option<&str>,
) -> Vec<DrawItem> {
    let mut items = Vec::new();
    for elem in &doc.elements {
        if !doc.is_shown(&elem.id) || !doc.in_view(&elem.id) {
            continue;
        }
        let Some((rect, style)) = doc.cached_rects.get(&elem.id) else {
            continue;
        };
        let mut out = Out {
            items: &mut items,
            clip: doc.cached_clips.get(&elem.id).copied(),
        };
        let focused = focus == Some(elem.id.as_str());
        element(&mut out, elem, *rect, style, measurer, focused);
    }
    // Each overflowing list's thumb, over its items
    for elem in doc.elements.iter().filter(|e| e.scrolls()) {
        let (Some(sb), Some((_, style))) = (
            doc.cached_scrolls.get(&elem.id),
            doc.cached_rects.get(&elem.id),
        ) else {
            continue;
        };
        if !doc.is_shown(&elem.id) {
            continue;
        }
        let mut out = Out {
            items: &mut items,
            clip: doc.cached_clips.get(&elem.id).copied(),
        };
        thumb(&mut out, elem, sb, style);
    }
    items
}

fn element(
    out: &mut Out<'_>,
    elem: &UiElement,
    rect: ResolvedRect,
    style: &ResolvedStyle,
    measurer: Option<&TextMeasurer>,
    focused: bool,
) {
    let opacity = style.opacity;
    match elem.element_type {
        ElementType::Panel | ElementType::List => {
            background(out, rect, style);
            border(out, rect, style);
        }
        ElementType::Button => {
            background(out, rect, style);
            border(out, rect, style);
            let text = elem.effective_text();
            if !text.is_empty() {
                // The label sits in the middle of the box, by its measured
                // height (the font's line, not its caps)
                let (_, th) = measure(measurer, text, style);
                let y = rect.y + (rect.h - th) / 2.0;
                out.push(text_command(text, rect, y, style, style.color));
            }
        }
        ElementType::TextInput => {
            background(out, rect, style);
            border(out, rect, style);
            let place = text_input::place(elem, rect, style, measurer);
            let inner = place.inner.to_clip();
            let text = elem.effective_text();
            let line = ResolvedRect {
                x: place.text_x,
                w: 0.0,
                ..place.inner
            };
            let mut left = style.clone();
            left.text_align = style::TextAlign::Left;
            if !text.is_empty() {
                let cmd = text_command(text, line, place.line_y, &left, style.color);
                out.push_inside(cmd, inner);
            } else if !elem.placeholder.is_empty() {
                let cmd = text_command(
                    &elem.placeholder,
                    line,
                    place.line_y,
                    &left,
                    style.placeholder_color,
                );
                out.push_inside(cmd, inner);
            }
            if focused {
                let caret = DrawCommand::RectFilled {
                    x: place.caret_x,
                    y: place.line_y,
                    w: place.caret_w,
                    h: place.line_h,
                    color: faded(style.caret_color, opacity),
                    rounding: place.caret_w / 2.0,
                    layer: style.layer,
                };
                // Wide enough to hold the caret at the box's very end
                let room = [inner[0], inner[1], inner[2] + place.caret_w, inner[3]];
                out.push_inside(caret, room);
            }
        }
        ElementType::Slider => {
            background(out, rect, style);
            let fraction = elem.fraction();
            if fraction > 0.0 {
                out.push(DrawCommand::RectFilled {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w * fraction,
                    h: rect.h,
                    color: faded(style.color, opacity),
                    rounding: style.rounding,
                    layer: style.layer,
                });
            }
            border(out, rect, style);
            if style.radius > 0.0 {
                out.push(DrawCommand::CircleFilled {
                    x: rect.x + rect.w * fraction,
                    y: rect.y + rect.h / 2.0,
                    radius: style.radius,
                    color: faded(style.knob_color, opacity),
                    layer: style.layer,
                });
            }
        }
        ElementType::Text => {
            let text = elem.effective_text();
            if !text.is_empty() {
                out.push(text_command(text, rect, rect.y, style, style.color));
            }
        }
        ElementType::Rect => {
            let color = faded(style.color, opacity);
            if style.thickness > 0.0 && style.bg_color[3] < 0.001 {
                out.push(DrawCommand::RectOutline {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w,
                    h: rect.h,
                    color,
                    thickness: style.thickness,
                    rounding: 0.0,
                    layer: style.layer,
                });
            } else {
                out.push(DrawCommand::RectFilled {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w,
                    h: rect.h,
                    color,
                    rounding: style.rounding,
                    layer: style.layer,
                });
            }
        }
        ElementType::Circle => {
            out.push(DrawCommand::CircleFilled {
                x: rect.x + rect.w / 2.0,
                y: rect.y + rect.h / 2.0,
                radius: style.radius,
                color: faded(style.color, opacity),
                layer: style.layer,
            });
        }
        ElementType::Image => {
            if !elem.src.is_empty() {
                out.push(DrawCommand::Sprite {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w,
                    h: rect.h,
                    name: elem.src.clone(),
                    uv: style.uv,
                    tint: faded(style.color, opacity),
                    layer: style.layer,
                });
            }
        }
    }
}

/// A scrolling list's thumb along its far edge, when its items run past
/// its box: as long as the share of them in view, as far along as the
/// list has scrolled.
fn thumb(out: &mut Out<'_>, elem: &UiElement, sb: &ScrollBox, style: &ResolvedStyle) {
    let width = style.scrollbar_width;
    let max = sb.max_offset();
    if width <= 0.0 || style.scrollbar_color[3] < 0.001 || max < 0.5 {
        return;
    }
    let view = sb.view_len();
    let len = (view * view / sb.content).clamp(width * 2.0, view);
    let at = (view - len) * (elem.scroll_offset / max).clamp(0.0, 1.0);
    let v = sb.view;
    // Inside the list's border, at its right (or bottom) edge
    let (x, y, w, h) = if sb.horizontal {
        (v.x + at, v.y + v.h - width, len, width)
    } else {
        (v.x + v.w - width, v.y + at, width, len)
    };
    out.push(DrawCommand::RectFilled {
        x,
        y,
        w,
        h,
        color: faded(style.scrollbar_color, style.opacity),
        rounding: width / 2.0,
        layer: style.layer,
    });
}

/// A box's fill, when its `bg_color` shows.
fn background(out: &mut Out<'_>, rect: ResolvedRect, style: &ResolvedStyle) {
    if style.bg_color[3] > 0.001 {
        out.push(DrawCommand::RectFilled {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
            color: faded(style.bg_color, style.opacity),
            rounding: style.rounding,
            layer: style.layer,
        });
    }
}

/// A box's outline, when it has a `border_width`.
fn border(out: &mut Out<'_>, rect: ResolvedRect, style: &ResolvedStyle) {
    if style.border_width > 0.0 && style.border_color[3] > 0.001 {
        out.push(DrawCommand::RectOutline {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
            color: faded(style.border_color, style.opacity),
            thickness: style.border_width,
            rounding: style.rounding,
            layer: style.layer,
        });
    }
}

/// A text's command in `color`, aligned in `rect` across and starting at
/// `y`. The renderer aligns with the font's own metrics, from the anchor
/// given.
fn text_command(
    text: &str,
    rect: ResolvedRect,
    y: f32,
    style: &ResolvedStyle,
    color: [f32; 4],
) -> DrawCommand {
    let opacity = style.opacity;
    let (text_x, align) = match style.text_align {
        style::TextAlign::Center if rect.w > 0.0 => (rect.x + rect.w / 2.0, 1u8),
        style::TextAlign::Right if rect.w > 0.0 => (rect.x + rect.w, 2u8),
        _ => (rect.x, 0u8),
    };
    let stroke = (style.stroke_width > 0.0)
        .then(|| (faded(style.stroke_color, opacity), style.stroke_width));
    DrawCommand::Text {
        x: text_x,
        y,
        text: text.to_string(),
        size: style.font_size,
        color: faded(color, opacity),
        layer: style.layer,
        align,
        stroke,
        font: style.font.clone(),
        letter_spacing: style.letter_spacing,
        shadow: style
            .shadow
            .map(|(sc, dx, dy)| (faded(sc, opacity), dx, dy)),
        glow: (style.glow_radius > 0.0)
            .then(|| (faded(style.glow_color, opacity), style.glow_radius)),
    }
}
