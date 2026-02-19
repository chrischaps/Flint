//! Layout resolution — converts element tree + styles into screen-space rectangles

use super::element::{Anchor, UiElement};
use super::style::{LayoutFlow, ResolvedStyle, StyleClass, TextAlign};
use std::collections::HashMap;

/// Screen-space rectangle for a resolved element
#[derive(Debug, Clone, Copy, Default)]
pub struct ResolvedRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Resolve all element positions given screen dimensions
pub fn resolve_layout(
    elements: &[UiElement],
    styles: &HashMap<String, StyleClass>,
    screen_w: f32,
    screen_h: f32,
) -> HashMap<String, (ResolvedRect, ResolvedStyle)> {
    let mut results: HashMap<String, (ResolvedRect, ResolvedStyle)> = HashMap::new();

    // Build element lookup
    let elem_map: HashMap<&str, &UiElement> = elements.iter()
        .map(|e| (e.id.as_str(), e))
        .collect();

    // Process root elements first (those without a parent), then children
    let roots: Vec<&UiElement> = elements.iter()
        .filter(|e| e.parent_id.is_none())
        .collect();

    for root in &roots {
        resolve_element(root, &elem_map, styles, screen_w, screen_h, None, &mut results);
    }

    results
}

fn resolve_element(
    elem: &UiElement,
    elem_map: &HashMap<&str, &UiElement>,
    styles: &HashMap<String, StyleClass>,
    screen_w: f32,
    screen_h: f32,
    parent_rect: Option<&ResolvedRect>,
    results: &mut HashMap<String, (ResolvedRect, ResolvedStyle)>,
) {
    // Resolve style from class
    let class_name = elem.effective_class();
    let mut style = styles.get(class_name)
        .map(|c| c.resolve())
        .unwrap_or_default();

    // Apply runtime style overrides
    StyleClass::apply_overrides(&mut style, &elem.style_overrides);

    // Apply color overrides
    if let Some(color) = elem.color_override {
        style.color = color;
    }
    if let Some(bg_color) = elem.bg_color_override {
        style.bg_color = bg_color;
    }

    // Resolve dimensions
    let mut w = style.width;
    let mut h = style.height;

    if let Some(pct) = style.width_pct {
        let parent_w = parent_rect.map(|r| r.w).unwrap_or(screen_w);
        w = parent_w * pct / 100.0;
    }
    if let Some(pct) = style.height_pct {
        let parent_h = parent_rect.map(|r| r.h).unwrap_or(screen_h);
        h = parent_h * pct / 100.0;
    }

    // Resolve position
    let (x, y) = if let Some(parent) = parent_rect {
        // Child element: offset from parent's content area
        let content_x = parent.x + style.padding[0]; // left padding
        let content_y = parent.y + style.padding[1]; // top padding
        (content_x + style.x, content_y + style.y)
    } else {
        // Root element: anchor determines screen origin
        let (ax, ay) = elem.anchor.screen_origin();
        let origin_x = screen_w * ax;
        let origin_y = screen_h * ay;

        // Adjust for anchor alignment
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

        (origin_x + offset_x + style.x, origin_y + offset_y + style.y)
    };

    let mut rect = ResolvedRect { x, y, w, h };

    // Auto-height: first resolve children, then compute height from children extent
    if style.height_auto && !elem.children.is_empty() {
        // First pass: resolve children to figure out total height
        let mut child_extent = 0.0_f32;
        let mut child_offset = style.padding[1]; // top padding

        for child_id in &elem.children {
            if let Some(child_elem) = elem_map.get(child_id.as_str()) {
                let child_class = child_elem.effective_class();
                let child_style = styles.get(child_class)
                    .map(|c| c.resolve())
                    .unwrap_or_default();

                let child_h = child_style.height;
                let child_bottom = child_offset + child_style.y + child_h + child_style.margin_bottom;
                if child_bottom > child_extent {
                    child_extent = child_bottom;
                }
                if style.layout == LayoutFlow::Stack {
                    child_offset = child_bottom;
                }
            }
        }

        rect.h = child_extent + style.padding[3]; // bottom padding
    }

    results.insert(elem.id.clone(), (rect, style.clone()));

    // Resolve children with stacking flow
    let content_x = rect.x + style.padding[0];
    let content_y = rect.y + style.padding[1];
    let content_w = rect.w - style.padding[0] - style.padding[2];
    let content_h = rect.h - style.padding[1] - style.padding[3];

    let mut flow_offset = 0.0_f32;

    for child_id in &elem.children {
        if let Some(child_elem) = elem_map.get(child_id.as_str()) {
            let child_class = child_elem.effective_class();
            let child_style_resolved = styles.get(child_class)
                .map(|c| c.resolve())
                .unwrap_or_default();

            // Create a parent rect that accounts for flow offset
            let flow_rect = match style.layout {
                LayoutFlow::Stack => ResolvedRect {
                    x: content_x,
                    y: content_y + flow_offset,
                    w: content_w,
                    h: content_h - flow_offset,
                },
                LayoutFlow::Horizontal => ResolvedRect {
                    x: content_x + flow_offset,
                    y: content_y,
                    w: content_w - flow_offset,
                    h: content_h,
                },
            };

            resolve_element(child_elem, elem_map, styles, screen_w, screen_h, Some(&flow_rect), results);

            // Advance flow offset
            match style.layout {
                LayoutFlow::Stack => {
                    flow_offset += child_style_resolved.y + child_style_resolved.height + child_style_resolved.margin_bottom;
                }
                LayoutFlow::Horizontal => {
                    flow_offset += child_style_resolved.x + child_style_resolved.width + child_style_resolved.margin_bottom;
                }
            }
        }
    }
}

/// Compute approximate text width (matching engine's measure_text convention)
pub fn approximate_text_width(text: &str, font_size: f32) -> f32 {
    text.len() as f32 * font_size * 0.6
}

/// Adjust text x position based on alignment within available width
pub fn align_text_x(x: f32, text_width: f32, container_width: f32, align: TextAlign) -> f32 {
    match align {
        TextAlign::Left => x,
        TextAlign::Center => x + (container_width - text_width) / 2.0,
        TextAlign::Right => x + container_width - text_width,
    }
}
