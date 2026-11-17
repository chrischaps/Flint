//! Layout resolution — converts element tree + styles into screen-space rectangles

use super::element::{Anchor, ElementType, UiElement};
use super::style::{LayoutFlow, ResolvedStyle, States, StyleClass, TextAlign};
use crate::context::TextMeasurer;
use std::collections::HashMap;

/// Screen-space rectangle for a resolved element
#[derive(Debug, Clone, Copy, Default)]
pub struct ResolvedRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl ResolvedRect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }

    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// What a layout pass reads besides the elements and classes: the host's
/// text measurer (the one `measure_text` uses, so a laid-out label is the
/// size it draws at) and each element's state.
pub struct LayoutInputs<'a> {
    pub measurer: Option<&'a TextMeasurer>,
    pub states: &'a dyn Fn(&UiElement) -> States,
}

/// Measure `text` at `style`'s font: the host's measurer, or a glyph-count
/// estimate before a host installs one (a test, a tool with no fonts).
pub fn measure(measurer: Option<&TextMeasurer>, text: &str, style: &ResolvedStyle) -> (f32, f32) {
    match measurer {
        Some(m) => m(
            text,
            style.font_size,
            style.font.as_deref(),
            style.letter_spacing,
        ),
        None => {
            let chars = text.chars().count() as f32;
            (
                chars * style.font_size * 0.6 + style.letter_spacing * (chars - 1.0).max(0.0),
                style.font_size,
            )
        }
    }
}

/// An element's style: its type's defaults, its class, the class's tables
/// for the states it is in, then what scripts set on it.
pub fn element_style(
    elem: &UiElement,
    styles: &HashMap<String, StyleClass>,
    states: States,
) -> ResolvedStyle {
    let mut style = ResolvedStyle::for_type(elem.element_type);
    if let Some(class) = styles.get(elem.effective_class()) {
        class.apply_to(&mut style, states);
    }
    StyleClass::apply_overrides(&mut style, &elem.style_overrides);
    if let Some(color) = elem.color_override {
        style.color = color;
    }
    if let Some(bg_color) = elem.bg_color_override {
        style.bg_color = bg_color;
    }
    style
}

/// The style a stack advances by for an element of an older type: its
/// class alone, as it always was, so a layout that positions children
/// with runtime `y` overrides keeps its places.
fn class_style(elem: &UiElement, styles: &HashMap<String, StyleClass>) -> ResolvedStyle {
    styles
        .get(elem.effective_class())
        .map(|c| c.resolve())
        .unwrap_or_default()
}

/// Whether an element takes the room its laid-out rectangle has in a stack
/// (the controls, which size themselves to their label), rather than its
/// class's `height` (every older type, as before #731).
fn sizes_itself(elem: &UiElement) -> bool {
    matches!(
        elem.element_type,
        ElementType::Button | ElementType::Slider | ElementType::List
    )
}

/// The size an element is with no parent percentage: its style's, or for
/// a `width` or `height` of 0 what its content needs. A text is its
/// label's size; a button its label's plus its padding (or half the font
/// size round it, with none); a slider 160 by its font size.
fn own_size(
    elem: &UiElement,
    style: &ResolvedStyle,
    measurer: Option<&TextMeasurer>,
) -> (f32, f32) {
    let (mut w, mut h) = (style.width, style.height);
    match elem.element_type {
        ElementType::Text | ElementType::Button if w <= 0.0 || h <= 0.0 => {
            let text = elem.effective_text();
            let (tw, th) = measure(measurer, if text.is_empty() { " " } else { text }, style);
            let (pad_x, pad_y) = if elem.element_type == ElementType::Button {
                let p = style.padding;
                if p == [0.0; 4] {
                    (style.font_size, style.font_size * 0.5 * 2.0)
                } else {
                    (p[0] + p[2], p[1] + p[3])
                }
            } else {
                (0.0, 0.0)
            };
            if w <= 0.0 {
                w = tw + pad_x;
            }
            if h <= 0.0 {
                h = th + pad_y;
            }
        }
        ElementType::Slider => {
            if w <= 0.0 {
                w = 160.0;
            }
            if h <= 0.0 {
                h = style.font_size;
            }
        }
        _ => {}
    }
    (w, h)
}

/// Resolve all element positions given screen dimensions
pub fn resolve_layout(
    elements: &[UiElement],
    styles: &HashMap<String, StyleClass>,
    screen_w: f32,
    screen_h: f32,
    inputs: &LayoutInputs<'_>,
) -> HashMap<String, (ResolvedRect, ResolvedStyle)> {
    let mut results: HashMap<String, (ResolvedRect, ResolvedStyle)> = HashMap::new();
    let elem_map: HashMap<&str, &UiElement> = elements.iter().map(|e| (e.id.as_str(), e)).collect();
    let pass = Pass {
        elem_map: &elem_map,
        styles,
        screen_w,
        screen_h,
        inputs,
    };
    for root in elements.iter().filter(|e| e.parent_id.is_none()) {
        pass.resolve(root, None, &mut results);
    }
    results
}

struct Pass<'a> {
    elem_map: &'a HashMap<&'a str, &'a UiElement>,
    styles: &'a HashMap<String, StyleClass>,
    screen_w: f32,
    screen_h: f32,
    inputs: &'a LayoutInputs<'a>,
}

impl Pass<'_> {
    /// How far a stack (or row) moves on past `child`.
    fn advance(&self, child: &UiElement, rect: ResolvedRect, horizontal: bool) -> f32 {
        if sizes_itself(child) {
            let style = element_style(child, self.styles, (self.inputs.states)(child));
            if horizontal {
                style.x + rect.w + style.margin_bottom
            } else {
                style.y + rect.h + style.margin_bottom
            }
        } else {
            let s = class_style(child, self.styles);
            if horizontal {
                s.x + s.width + s.margin_bottom
            } else {
                s.y + s.height + s.margin_bottom
            }
        }
    }

    fn resolve(
        &self,
        elem: &UiElement,
        parent_rect: Option<&ResolvedRect>,
        results: &mut HashMap<String, (ResolvedRect, ResolvedStyle)>,
    ) -> ResolvedRect {
        let style = element_style(elem, self.styles, (self.inputs.states)(elem));
        let (mut w, mut h) = own_size(elem, &style, self.inputs.measurer);
        if let Some(pct) = style.width_pct {
            let parent_w = parent_rect.map(|r| r.w).unwrap_or(self.screen_w);
            w = parent_w * pct / 100.0;
        }
        if let Some(pct) = style.height_pct {
            let parent_h = parent_rect.map(|r| r.h).unwrap_or(self.screen_h);
            h = parent_h * pct / 100.0;
        }

        // A root is anchored on the screen; an anchored child in its
        // parent's content box (which is what `parent_rect` is for one)
        let frame = match parent_rect {
            Some(parent) if elem.anchored => Some(*parent),
            Some(_) => None,
            None => Some(ResolvedRect {
                x: 0.0,
                y: 0.0,
                w: self.screen_w,
                h: self.screen_h,
            }),
        };
        let place = |w: f32, h: f32, frame: ResolvedRect| {
            let (ax, ay) = elem.anchor.screen_origin();
            let offset_x = match elem.anchor {
                Anchor::TopRight | Anchor::CenterRight | Anchor::BottomRight => -w,
                Anchor::TopCenter | Anchor::Center | Anchor::BottomCenter => -w / 2.0,
                _ => 0.0,
            };
            let offset_y = match elem.anchor {
                Anchor::BottomLeft | Anchor::BottomCenter | Anchor::BottomRight => -h,
                Anchor::CenterLeft | Anchor::Center | Anchor::CenterRight => -h / 2.0,
                _ => 0.0,
            };
            (
                frame.x + frame.w * ax + offset_x + style.x,
                frame.y + frame.h * ay + offset_y + style.y,
            )
        };
        let (x, y) = match (frame, parent_rect) {
            (Some(frame), _) => place(w, h, frame),
            // Child element in its parent's flow: offset from its content area
            (None, Some(parent)) => (
                parent.x + style.padding[0] + style.x,
                parent.y + style.padding[1] + style.y,
            ),
            (None, None) => unreachable!("a root always has the screen as its frame"),
        };
        let mut rect = ResolvedRect { x, y, w, h };

        let children: Vec<&UiElement> = elem
            .children
            .iter()
            .filter_map(|id| self.elem_map.get(id.as_str()).copied())
            .collect();

        // Auto-height: the children's extent
        if style.height_auto && !children.is_empty() {
            let mut child_extent = 0.0_f32;
            let mut child_offset = style.padding[1];
            for child in children.iter().filter(|c| !c.anchored) {
                let bottom = if sizes_itself(child) {
                    let cs = element_style(child, self.styles, (self.inputs.states)(child));
                    let (_, ch) = own_size(child, &cs, self.inputs.measurer);
                    child_offset + cs.y + ch + cs.margin_bottom
                } else {
                    let cs = class_style(child, self.styles);
                    child_offset + cs.y + cs.height + cs.margin_bottom
                };
                child_extent = child_extent.max(bottom);
                if style.layout == LayoutFlow::Stack {
                    child_offset = bottom;
                }
            }
            rect.h = child_extent + style.padding[3];
            // Anchored at its middle or bottom, it moves with its height
            if let Some(frame) = frame {
                rect.y = place(rect.w, rect.h, frame).1;
            }
        }

        results.insert(elem.id.clone(), (rect, style.clone()));

        // Children with stacking flow
        let content_x = rect.x + style.padding[0];
        let content_y = rect.y + style.padding[1];
        let content_w = rect.w - style.padding[0] - style.padding[2];
        let content_h = rect.h - style.padding[1] - style.padding[3];
        let horizontal = style.layout == LayoutFlow::Horizontal;
        let mut flow_offset = 0.0_f32;
        for child in children {
            if child.anchored {
                // Out of the flow: placed in the content box, takes no room
                let content = ResolvedRect {
                    x: content_x,
                    y: content_y,
                    w: content_w,
                    h: content_h,
                };
                self.resolve(child, Some(&content), results);
                continue;
            }
            let flow_rect = if horizontal {
                ResolvedRect {
                    x: content_x + flow_offset,
                    y: content_y,
                    w: content_w - flow_offset,
                    h: content_h,
                }
            } else {
                ResolvedRect {
                    x: content_x,
                    y: content_y + flow_offset,
                    w: content_w,
                    h: content_h - flow_offset,
                }
            };
            let child_rect = self.resolve(child, Some(&flow_rect), results);
            flow_offset += self.advance(child, child_rect, horizontal);
        }
        rect
    }
}

/// Adjust text x position based on alignment within available width
pub fn align_text_x(x: f32, text_width: f32, container_width: f32, align: TextAlign) -> f32 {
    match align {
        TextAlign::Left => x,
        TextAlign::Center => x + (container_width - text_width) / 2.0,
        TextAlign::Right => x + container_width - text_width,
    }
}
