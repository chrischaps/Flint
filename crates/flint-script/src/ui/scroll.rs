//! Lists that scroll (#1229).
//!
//! A `list` with `scroll = true` clips its children to its content box
//! (each one's clip goes on its draw items, `DrawItem::clip`, which the HUD
//! painter honours) and moves them back by its `scroll_offset`, which stays
//! between 0 and the overflow. It moves:
//!
//! - so the focused item is in view, whenever focus moves ([`UiSystem::focus`]
//!   asks for it, and the next layout settles it);
//! - by a mouse wheel over it, a row a notch; focus then moves to the
//!   nearest item still wholly in view, so the focused item is always drawn;
//! - by a finger dragged along it, which lets go of the press it began.
//!
//! What is clipped away can't be hovered, clicked or tapped.

use super::element::ElementType;
use super::layout::{clip_contains, ResolvedRect, ScrollBox};
use super::{UiDocument, UiSystem};

/// A finger moves this far along a list before it scrolls it, in points.
pub const DRAG_SLOP: f32 = 8.0;

/// A finger on a scrolling list: where it began and the offset then.
#[derive(Debug, Clone)]
pub struct TouchScroll {
    pub list: String,
    pub start: (f32, f32),
    pub start_offset: f32,
    /// It has moved past [`DRAG_SLOP`] and scrolls the list.
    pub dragging: bool,
}

impl UiDocument {
    /// After a layout: hold each scrolling list's offset inside its range,
    /// and bring `reveal` (the focused element) into view in each list it is
    /// inside. Whether any offset moved, so the layout must run again.
    pub(super) fn settle_scroll(&mut self, reveal: Option<&str>) -> bool {
        let mut moved = false;
        // The lists `reveal` is inside, innermost first
        let mut inside = Vec::new();
        if let Some(id) = reveal.filter(|id| self.find_element(id).is_some()) {
            let mut at = self.find_element(id).and_then(|e| e.parent_id.clone());
            for _ in 0..self.elements.len() {
                let Some(p) = at else { break };
                at = self.find_element(&p).and_then(|e| e.parent_id.clone());
                inside.push(p);
            }
        }
        let target = reveal
            .and_then(|id| self.cached_rects.get(id))
            .map(|(r, _)| *r);
        let lists: Vec<(String, ScrollBox)> = self
            .cached_scrolls
            .iter()
            .map(|(id, sb)| (id.clone(), *sb))
            .collect();
        for (id, sb) in lists {
            let Some(list) = self.find_element_mut(&id) else {
                continue;
            };
            let mut offset = list.scroll_offset;
            if let (Some(r), true) = (target, inside.contains(&id)) {
                offset += reveal_delta(&sb, &r);
            }
            let offset = offset.clamp(0.0, sb.max_offset());
            if (offset - list.scroll_offset).abs() > 0.01 {
                list.scroll_offset = offset;
                moved = true;
            }
        }
        moved
    }

    /// Whether an element can be seen and pointed at here: inside the clip
    /// of the scrolling lists it is in.
    pub(super) fn reaches(&self, id: &str, x: f32, y: f32) -> bool {
        self.cached_clips
            .get(id)
            .is_none_or(|&clip| clip_contains(clip, x, y))
    }

    /// Whether any of an element is inside the scrolling lists it is in.
    pub(super) fn in_view(&self, id: &str) -> bool {
        let Some(&clip) = self.cached_clips.get(id) else {
            return true;
        };
        let Some((r, _)) = self.cached_rects.get(id) else {
            return false;
        };
        r.x < clip[0] + clip[2]
            && r.x + r.w > clip[0]
            && r.y < clip[1] + clip[3]
            && r.y + r.h > clip[1]
    }

    /// The scrolling list, shown, whose view a point is in: the smallest,
    /// when lists nest.
    pub(super) fn scroll_list_at(&self, x: f32, y: f32) -> Option<String> {
        self.cached_scrolls
            .iter()
            .filter(|(id, sb)| {
                sb.view.contains(x, y) && self.is_shown(id) && self.reaches(id, x, y)
            })
            .min_by(|a, b| {
                let area = |sb: &ScrollBox| sb.view.w * sb.view.h;
                area(a.1).total_cmp(&area(b.1))
            })
            .map(|(id, _)| id.clone())
    }

    /// The step a wheel notch scrolls a list: from one item to the next,
    /// else the first item's length, else 40 points.
    fn row_step(&self, list: &str, sb: &ScrollBox) -> f32 {
        let rects: Vec<ResolvedRect> = self
            .find_element(list)
            .map(|l| {
                l.children
                    .iter()
                    .filter(|c| self.is_shown(c))
                    .filter_map(|c| self.cached_rects.get(c).map(|(r, _)| *r))
                    .take(2)
                    .collect()
            })
            .unwrap_or_default();
        match rects.as_slice() {
            [a, b, ..] => (sb.span(b).0 - sb.span(a).0).abs().max(1.0),
            [a] => (sb.span(a).1 - sb.span(a).0).max(1.0),
            [] => 40.0,
        }
    }
}

/// How far a list moves so `r` is wholly in its view: its start to the
/// view's start when above or longer than the view, its end to the view's
/// end when below.
fn reveal_delta(sb: &ScrollBox, r: &ResolvedRect) -> f32 {
    let (start, end) = sb.span(r);
    let view = sb.view_len();
    if start < 0.0 || end - start > view {
        start
    } else if end > view {
        end - view
    } else {
        0.0
    }
}

impl UiSystem {
    /// Scroll a list `by` points (clamped to its range), with focus kept on
    /// an item in view when it is inside the list.
    pub(super) fn scroll_by(&mut self, list: &str, by: f32) {
        self.scroll_to(list, None, by);
    }

    /// Set a list's offset (`to`), or move it (`by`).
    fn scroll_to(&mut self, list: &str, to: Option<f32>, by: f32) {
        let Some(d) = self
            .documents
            .iter()
            .position(|doc| doc.cached_scrolls.contains_key(list))
        else {
            return;
        };
        let doc = &self.documents[d];
        let sb = doc.cached_scrolls[list];
        let Some(l) = doc.find_element(list) else {
            return;
        };
        let old = l.scroll_offset;
        let new = to.unwrap_or(old + by).clamp(0.0, sb.max_offset());
        let delta = new - old;
        if delta.abs() < 0.01 {
            return;
        }

        // Focus on an item that would leave the view moves to the nearest
        // item wholly in it, judged by where the items will be
        let items: Vec<(String, f32, f32)> = l
            .children
            .iter()
            .filter(|c| {
                doc.find_element(c)
                    .is_some_and(|e| e.focusable && e.enabled && doc.is_shown(c))
            })
            .filter_map(|c| {
                let (r, _) = doc.cached_rects.get(c)?;
                let (s, e) = sb.span(r);
                Some((c.clone(), s - delta, e - delta))
            })
            .collect();
        let view = sb.view_len();
        let focus = self.interaction.focus.clone();
        let refocus = focus.as_deref().and_then(|f| {
            let (_, s, e) = items.iter().find(|(id, ..)| id == f)?;
            if *s >= -0.5 && *e <= view + 0.5 {
                return None;
            }
            let at = items.iter().position(|(id, ..)| id == f)?;
            items
                .iter()
                .enumerate()
                .filter(|(_, (_, s, e))| *s >= -0.5 && *e <= view + 0.5)
                .min_by_key(|(i, _)| i.abs_diff(at))
                .map(|(_, (id, ..))| id.clone())
        });

        let doc = &mut self.documents[d];
        if let Some(l) = doc.find_element_mut(list) {
            l.scroll_offset = new;
        }
        doc.invalidate_cache();
        if let Some(id) = refocus {
            self.focus(Some(&id));
        }
    }

    /// A wheel's notches over the pointer: the scrolling list under it moves
    /// a row a notch (up is toward its start).
    pub(super) fn wheel(&mut self, pointer: (f32, f32), notches: f32) {
        if notches.abs() < f32::EPSILON {
            return;
        }
        let Some((d, list)) = self
            .documents
            .iter()
            .enumerate()
            .find_map(|(d, doc)| doc.scroll_list_at(pointer.0, pointer.1).map(|l| (d, l)))
        else {
            return;
        };
        let doc = &self.documents[d];
        let step = doc.row_step(&list, &doc.cached_scrolls[&list]);
        self.scroll_by(&list, -notches * step);
    }

    /// A finger on a scrolling list: begun, followed, let go. Whether it
    /// scrolls (so the press it began is let go, and its tap ignored).
    pub(super) fn touch_scroll(
        &mut self,
        started: Option<(f32, f32)>,
        now: Option<(f32, f32)>,
    ) -> bool {
        if let Some(p) = started {
            self.pointer.touch_scroll = self.documents.iter().find_map(|doc| {
                let list = doc.scroll_list_at(p.0, p.1)?;
                let offset = doc.find_element(&list)?.scroll_offset;
                Some(TouchScroll {
                    list,
                    start: p,
                    start_offset: offset,
                    dragging: false,
                })
            });
        }
        let Some(mut ts) = self.pointer.touch_scroll.take() else {
            return false;
        };
        let Some(p) = now else {
            // Let go: a drag's tap is no tap
            return ts.dragging;
        };
        let horizontal = self
            .documents
            .iter()
            .find_map(|doc| doc.cached_scrolls.get(&ts.list))
            .is_some_and(|sb| sb.horizontal);
        let moved = if horizontal {
            ts.start.0 - p.0
        } else {
            ts.start.1 - p.1
        };
        if !ts.dragging && moved.abs() > DRAG_SLOP {
            ts.dragging = true;
            self.press(None, true);
        }
        let dragging = ts.dragging;
        if dragging {
            let (list, to) = (ts.list.clone(), ts.start_offset + moved);
            self.pointer.touch_scroll = Some(ts);
            self.scroll_to(&list, Some(to), 0.0);
        } else {
            self.pointer.touch_scroll = Some(ts);
        }
        dragging
    }

    /// Settle every document's scrolling lists after a layout, bringing the
    /// focused element into view when focus moved since the last.
    pub(super) fn settle_scrolls(&mut self, screen: (f32, f32)) {
        let reveal = std::mem::take(&mut self.reveal)
            .then(|| self.interaction.focus.clone())
            .flatten();
        let measurer = self.measurer.as_ref();
        for doc in &mut self.documents {
            if doc.cached_scrolls.is_empty() {
                continue;
            }
            // Twice at most: a nested list moves after its outer one
            for _ in 0..2 {
                if !doc.settle_scroll(reveal.as_deref()) {
                    break;
                }
                doc.invalidate_cache();
                doc.ensure_layout(screen.0, screen.1, &self.interaction, measurer);
            }
        }
    }

    /// How far a list has scrolled, in points; 0 for anything else.
    pub fn scroll_offset(&self, id: &str) -> f32 {
        self.documents
            .iter()
            .find_map(|doc| doc.find_element(id))
            .filter(|e| e.element_type == ElementType::List)
            .map_or(0.0, |e| e.scroll_offset)
    }
}
