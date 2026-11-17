//! A document's draw commands: each shown element, in element order, by
//! its type, in the style its state picks (#731).

use super::element::{ElementType, UiElement};
use super::layout::{measure, ResolvedRect};
use super::style::{self, ResolvedStyle};
use super::UiDocument;
use crate::context::{DrawCommand, TextMeasurer};

fn faded(mut color: [f32; 4], opacity: f32) -> [f32; 4] {
    color[3] *= opacity;
    color
}

/// The commands for every shown element of `doc`, whose layout (and the
/// state styles in it) is current.
pub(super) fn commands(doc: &UiDocument, measurer: Option<&TextMeasurer>) -> Vec<DrawCommand> {
    let mut out = Vec::new();
    for elem in &doc.elements {
        if !doc.is_shown(&elem.id) {
            continue;
        }
        let Some((rect, style)) = doc.cached_rects.get(&elem.id) else {
            continue;
        };
        element(&mut out, elem, *rect, style, measurer);
    }
    out
}

fn element(
    out: &mut Vec<DrawCommand>,
    elem: &UiElement,
    rect: ResolvedRect,
    style: &ResolvedStyle,
    measurer: Option<&TextMeasurer>,
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
                out.push(text_command(text, rect, y, style));
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
                out.push(text_command(text, rect, rect.y, style));
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

/// A box's fill, when its `bg_color` shows.
fn background(out: &mut Vec<DrawCommand>, rect: ResolvedRect, style: &ResolvedStyle) {
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
fn border(out: &mut Vec<DrawCommand>, rect: ResolvedRect, style: &ResolvedStyle) {
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

/// A text's command, aligned in `rect` across and starting at `y`. The
/// renderer aligns with the font's own metrics, from the anchor given.
fn text_command(text: &str, rect: ResolvedRect, y: f32, style: &ResolvedStyle) -> DrawCommand {
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
        color: faded(style.color, opacity),
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
