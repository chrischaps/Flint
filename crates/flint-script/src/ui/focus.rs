//! Focus, navigation and input routing for the data-driven UI (#731).
//!
//! One element across every loaded document has focus. It moves:
//!
//! - by a direction (`ui_up`, `ui_down`, `ui_left`, `ui_right`: the arrows,
//!   a pad's d-pad and left stick), to the nearest shown, enabled, focusable
//!   element that way in the same document, unless the element names where
//!   that direction goes (`nav_up = "id"`); inside a list, along it, from
//!   one item to the next, wrapping at the ends; on a slider, left and right
//!   step its value instead;
//! - to the element under the mouse when it moves, or under a touch;
//! - to an `autofocus` element when one is shown and nothing has focus;
//! - by `ui_focus(id)` from a script.
//!
//! A direction with nothing focused does nothing: a game's movement keys
//! never wake a menu the player can't see (a menu takes focus with
//! `autofocus` or `ui_focus`).
//!
//! `ui_accept` (Enter, Space, a pad's south button) activates the focused
//! button, a click or tap on one activates it, and `ui_back` (Backspace, a
//! pad's east button) is a `back` on the focused element. Each reaches the
//! document's owner as `on_ui(element_id, event)`, through
//! [`UiSystem::take_events`].
//!
//! A focused `text_input` takes the frame's typed text first, and the keys
//! it typed with are its own ([`super::text_input`]); a scrolling list keeps
//! the focused item in view and takes the wheel and a dragged finger
//! ([`super::scroll`]).

use flint_runtime::TextEdit;

use super::element::{Dir, ElementType, UiElement};
use super::layout::ResolvedRect;
use super::style::LayoutFlow;
use super::{UiDocument, UiEvent, UiSystem};
use crate::context::InputSnapshot;

/// The actions the UI reads (built into every game's input config,
/// `flint_runtime::InputConfig::built_in_defaults`).
pub const UI_UP: &str = "ui_up";
pub const UI_DOWN: &str = "ui_down";
pub const UI_LEFT: &str = "ui_left";
pub const UI_RIGHT: &str = "ui_right";
pub const UI_ACCEPT: &str = "ui_accept";
pub const UI_BACK: &str = "ui_back";

/// A held direction repeats after this long, then every [`REPEAT_EVERY`].
pub const REPEAT_AFTER: f64 = 0.4;
pub const REPEAT_EVERY: f64 = 0.12;

/// What the UI is told about one frame's input, in logical points.
#[derive(Debug, Clone, Default)]
pub struct UiInput {
    /// A direction pressed this frame, or repeating while held.
    pub nav: Option<Dir>,
    pub accept: bool,
    pub back: bool,
    pub pointer: (f32, f32),
    /// The mouse moved since the last frame.
    pub pointer_moved: bool,
    /// The left button went down this frame; is down now.
    pub mouse_pressed: bool,
    pub mouse_down: bool,
    /// Touches that began this frame, and every touch down now.
    pub touches_started: Vec<(f32, f32)>,
    pub touches: Vec<(f32, f32)>,
    /// Taps finished this frame.
    pub taps: Vec<(f32, f32)>,
    /// Wheel notches this frame, up positive (#1229).
    pub wheel: f32,
    /// What was typed this frame, in order (#1229).
    pub text_edits: Vec<TextEdit>,
}

/// What routing keeps between frames.
#[derive(Debug, Clone, Default)]
pub struct PointerState {
    /// The direction held, and the time it next repeats at.
    held: Option<(Dir, f64)>,
    /// The clock, advanced by each frame's delta.
    time: f64,
    last_pointer: Option<(f32, f32)>,
    /// A slider a pointer is dragging.
    dragging: Option<String>,
    /// Whether the press being held is a touch's (else the mouse's).
    touch_press: bool,
    /// A finger on a scrolling list (#1229).
    pub(super) touch_scroll: Option<super::scroll::TouchScroll>,
}

impl UiInput {
    /// This frame's input from the scripts' snapshot: `pointer` is the mouse
    /// in points, `screen` the size touches' 0..1 positions scale to, `dt`
    /// the frame's seconds (for a held direction's repeat).
    pub fn from_snapshot(
        snap: &InputSnapshot,
        pointer: (f32, f32),
        screen: (f32, f32),
        dt: f64,
        state: &mut PointerState,
    ) -> Self {
        state.time += dt.max(0.0);
        let dirs = [
            (Dir::Up, UI_UP),
            (Dir::Down, UI_DOWN),
            (Dir::Left, UI_LEFT),
            (Dir::Right, UI_RIGHT),
        ];
        let mut nav = dirs
            .iter()
            .find(|(_, a)| snap.actions_just_pressed.contains(*a))
            .map(|(d, _)| *d);
        if let Some(d) = nav {
            state.held = Some((d, state.time + REPEAT_AFTER));
        } else if let Some((d, at)) = state.held {
            let action = dirs.iter().find(|(x, _)| *x == d).map(|(_, a)| *a);
            if action.is_some_and(|a| snap.actions_pressed.contains(a)) {
                if state.time >= at {
                    nav = Some(d);
                    state.held = Some((d, state.time + REPEAT_EVERY));
                }
            } else {
                state.held = None;
            }
        }
        let moved = state
            .last_pointer
            .is_some_and(|p| (p.0 - pointer.0).abs() > 0.5 || (p.1 - pointer.1).abs() > 0.5);
        state.last_pointer = Some(pointer);
        let to_points = |(x, y): (f64, f64)| (x as f32 * screen.0, y as f32 * screen.1);
        Self {
            nav,
            accept: snap.actions_just_pressed.contains(UI_ACCEPT),
            back: snap.actions_just_pressed.contains(UI_BACK),
            pointer,
            pointer_moved: moved,
            mouse_pressed: snap.mouse_buttons_just_pressed.contains(&0),
            mouse_down: snap.mouse_buttons_down.contains(&0),
            touches_started: snap
                .touch_just_started
                .iter()
                .map(|&(_, x, y)| to_points((x, y)))
                .collect(),
            touches: snap
                .touches
                .iter()
                .map(|&(_, x, y)| to_points((x, y)))
                .collect(),
            taps: snap.touch_taps.iter().map(|&p| to_points(p)).collect(),
            wheel: snap.mouse_wheel.1 as f32,
            text_edits: snap.text_edits.clone(),
        }
    }
}

/// Where an element is in the documents: (document, element) indices.
pub(super) type At = (usize, usize);

impl UiDocument {
    /// Whether focus may land on an element now.
    fn takes_focus(&self, e: &UiElement) -> bool {
        e.focusable && e.enabled && self.is_shown(&e.id)
    }
}

impl UiSystem {
    pub(super) fn locate(&self, id: &str) -> Option<At> {
        self.documents
            .iter()
            .enumerate()
            .find_map(|(d, doc)| doc.elements.iter().position(|e| e.id == id).map(|i| (d, i)))
    }

    pub(super) fn element(&self, (d, i): At) -> &UiElement {
        &self.documents[d].elements[i]
    }

    fn rect(&self, (d, i): At) -> Option<ResolvedRect> {
        let doc = &self.documents[d];
        doc.cached_rects.get(&doc.elements[i].id).map(|(r, _)| *r)
    }

    pub(super) fn push(&mut self, (d, _): At, element: String, event: &'static str) {
        let owner = self.documents[d].owner;
        self.events.push(UiEvent {
            owner,
            element,
            event,
        });
    }

    /// The id an element's events carry: a list's own for its made items.
    fn event_id(&self, at: At) -> String {
        let e = self.element(at);
        match (&e.parent_id, e.made_by_list) {
            (Some(list), true) => list.clone(),
            _ => e.id.clone(),
        }
    }

    fn lay_out(&mut self, screen: (f32, f32)) {
        let measurer = self.measurer.as_ref();
        for doc in &mut self.documents {
            doc.ensure_layout(screen.0, screen.1, &self.interaction, measurer);
        }
    }

    /// The element focused now, if any.
    pub fn focused(&self) -> Option<&str> {
        self.interaction.focus.as_deref()
    }

    /// Move focus to `id` (`None` or an id no document has: nowhere), with
    /// `blur` and `focus` events, and a list's `change` when it lands on
    /// another of the list's items.
    pub fn focus(&mut self, id: Option<&str>) {
        let target = id.and_then(|id| self.locate(id));
        let new = target.map(|at| self.element(at).id.clone());
        if new == self.interaction.focus {
            return;
        }
        if let Some(old) = self.interaction.focus.take() {
            if let Some(at) = self.locate(&old) {
                let ev = self.event_id(at);
                self.push(at, ev, "blur");
            }
        }
        self.interaction.focus = new;
        if let Some(at) = target {
            let ev = self.event_id(at);
            self.push(at, ev, "focus");
            self.select_in_list(at);
            if self.element(at).element_type == ElementType::TextInput {
                self.caret_to_end(at);
            }
            // A scrolling list it is in brings it into view (#1229)
            self.reveal = true;
        }
        self.restyle = true;
    }

    /// Whether a text input has focus: the player is typing, so a game can
    /// leave its own keys alone (#1229).
    pub fn editing(&self) -> bool {
        self.interaction
            .focus
            .as_deref()
            .and_then(|f| self.locate(f))
            .is_some_and(|at| self.element(at).element_type == ElementType::TextInput)
    }

    /// A list's value follows focus onto one of its items.
    fn select_in_list(&mut self, at: At) {
        let (d, _) = at;
        let Some(list_id) = self.element(at).parent_id.clone() else {
            return;
        };
        let Some(list_at) = self.locate(&list_id) else {
            return;
        };
        if self.element(list_at).element_type != ElementType::List {
            return;
        }
        let items = self.list_items(list_at);
        if let Some(index) = items.iter().position(|&x| x == at) {
            let list = &mut self.documents[d].elements[list_at.1];
            if list.value != index as f32 {
                list.value = index as f32;
                self.push(list_at, list_id, "change");
            }
        }
    }

    /// A list's children focus can land on, in order.
    fn list_items(&self, list: At) -> Vec<At> {
        let (d, _) = list;
        let doc = &self.documents[d];
        self.element(list)
            .children
            .iter()
            .filter_map(|c| doc.elements.iter().position(|e| &e.id == c))
            .filter(|&i| doc.takes_focus(&doc.elements[i]))
            .map(|i| (d, i))
            .collect()
    }

    /// Set a slider's value (clamped, snapped to its step when it has
    /// one) or a list's selection (focusing that item when the list holds
    /// focus). A `change` when it moved.
    pub fn set_value(&mut self, id: &str, value: f32) {
        let Some(at) = self.locate(id) else {
            return;
        };
        match self.element(at).element_type {
            ElementType::Slider => self.slide(at, value),
            ElementType::List => {
                let items = self.list_items(at);
                let index = (value.round().max(0.0) as usize).min(items.len().saturating_sub(1));
                let focus_inside = self
                    .interaction
                    .focus
                    .as_deref()
                    .and_then(|f| self.locate(f))
                    .is_some_and(|f| items.contains(&f));
                match items.get(index) {
                    Some(&item) if focus_inside => {
                        let item_id = self.element(item).id.clone();
                        self.focus(Some(&item_id));
                    }
                    _ => {
                        let (d, i) = at;
                        let list = &mut self.documents[d].elements[i];
                        if list.value != index as f32 {
                            list.value = index as f32;
                            self.push(at, id.to_string(), "change");
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn slide(&mut self, at: At, value: f32) {
        let (d, i) = at;
        let e = &mut self.documents[d].elements[i];
        let mut v = value.clamp(e.min, e.max);
        if e.step > 0.0 {
            v = (e.min + ((v - e.min) / e.step).round() * e.step).clamp(e.min, e.max);
        }
        if (v - e.value).abs() > f32::EPSILON {
            e.value = v;
            let id = e.id.clone();
            self.push(at, id, "change");
        }
    }

    /// A slider's value or a list's selected index; 0 for anything else.
    pub fn value(&self, id: &str) -> f32 {
        self.locate(id)
            .map(|at| self.element(at).value)
            .unwrap_or(0.0)
    }

    /// Give a list new items, each a button of its `item_class`.
    pub fn set_items(&mut self, id: &str, items: Vec<String>) {
        let Some((d, i)) = self.locate(id) else {
            return;
        };
        if self.documents[d].elements[i].element_type != ElementType::List {
            return;
        }
        // Focus on one of the items going goes with it
        if let Some(f) = self.interaction.focus.clone() {
            if self.locate(&f).is_some_and(|at| {
                let e = self.element(at);
                e.made_by_list && e.parent_id.as_deref() == Some(id)
            }) {
                self.interaction.focus = None;
            }
        }
        self.documents[d].replace_items(id, items);
    }

    /// The focusable element on top under a point: the highest layer, then
    /// the last drawn.
    fn under(&self, (x, y): (f32, f32)) -> Option<At> {
        let mut best: Option<(i32, At)> = None;
        for (d, doc) in self.documents.iter().enumerate() {
            for (i, e) in doc.elements.iter().enumerate() {
                // Clipped away by a scrolling list, it can't be pointed at
                if !doc.takes_focus(e) || !doc.reaches(&e.id, x, y) {
                    continue;
                }
                let Some((rect, style)) = doc.cached_rects.get(&e.id) else {
                    continue;
                };
                if rect.contains(x, y) && best.is_none_or(|(layer, _)| style.layer >= layer) {
                    best = Some((style.layer, (d, i)));
                }
            }
        }
        best.map(|(_, at)| at)
    }

    /// Where `dir` goes from `from`: the element it names, along its list,
    /// or the nearest that way in its document.
    fn next(&self, from: At, dir: Dir) -> Option<At> {
        let (d, _) = from;
        let doc = &self.documents[d];
        let e = self.element(from);
        if let Some(target) = e.nav.get(&dir) {
            return doc
                .elements
                .iter()
                .position(|x| &x.id == target)
                .filter(|&i| doc.takes_focus(&doc.elements[i]))
                .map(|i| (d, i));
        }
        if let Some(step) = self.along_list(from, dir) {
            return Some(step);
        }
        let here = self.rect(from)?.center();
        let (vx, vy) = dir.vector();
        let mut best: Option<(f32, At)> = None;
        for (i, x) in doc.elements.iter().enumerate() {
            if (d, i) == from || !doc.takes_focus(x) || !doc.in_view(&x.id) {
                continue;
            }
            let Some(rect) = self.rect((d, i)) else {
                continue;
            };
            let (cx, cy) = rect.center();
            let (dx, dy) = (cx - here.0, cy - here.1);
            let along = dx * vx + dy * vy;
            if along <= 1.0 {
                continue;
            }
            let across = (dx * vy - dy * vx).abs();
            let score = along + 2.0 * across;
            if best.is_none_or(|(s, _)| score < s) {
                best = Some((score, (d, i)));
            }
        }
        best.map(|(_, at)| at)
    }

    /// The next item of the list `from` is in, when `dir` runs along it.
    fn along_list(&self, from: At, dir: Dir) -> Option<At> {
        let parent = self.element(from).parent_id.as_ref()?;
        let list = self.locate(parent)?;
        let l = self.element(list);
        if l.element_type != ElementType::List {
            return None;
        }
        let horizontal = self.documents[list.0]
            .cached_rects
            .get(&l.id)
            .is_some_and(|(_, s)| s.layout == LayoutFlow::Horizontal);
        if dir.is_horizontal() != horizontal {
            return None;
        }
        let items = self.list_items(list);
        let at = items.iter().position(|&x| x == from)?;
        let forward = matches!(dir, Dir::Down | Dir::Right);
        let n = items.len();
        let next = match (forward, at) {
            (true, a) if a + 1 < n => a + 1,
            (false, a) if a > 0 => a - 1,
            (true, _) if l.wrap => 0,
            (false, _) if l.wrap => n - 1,
            _ => return None,
        };
        Some(items[next])
    }

    fn activate(&mut self, at: At, done: &mut Vec<At>) {
        if done.contains(&at) {
            return;
        }
        done.push(at);
        let e = self.element(at);
        if e.element_type != ElementType::Button {
            return;
        }
        let id = self.event_id(at);
        self.select_in_list(at);
        self.push(at, id, "activate");
    }

    fn drag_to(&mut self, at: At, x: f32) {
        if let Some(rect) = self.rect(at) {
            let e = self.element(at);
            let fraction = if rect.w > 0.0 {
                ((x - rect.x) / rect.w).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let value = e.min + fraction * (e.max - e.min);
            self.slide(at, value);
        }
    }

    /// [`Self::route`] on the scripts' input snapshot: `pointer` is the mouse
    /// in points, `dt` the frame's seconds.
    pub fn route_snapshot(
        &mut self,
        snap: &InputSnapshot,
        pointer: (f32, f32),
        screen: (f32, f32),
        dt: f64,
    ) {
        if self.documents.is_empty() {
            return;
        }
        let input = UiInput::from_snapshot(snap, pointer, screen, dt, &mut self.pointer);
        self.route(&input, screen);
    }

    /// One frame of input: focus moves, controls change, events queue.
    /// `screen` is the size layout runs at, in points.
    pub fn route(&mut self, input: &UiInput, screen: (f32, f32)) {
        // Layout follows the host's measurer, which can come ready late.
        // Routing hit-tests and steers by these rects; a state that changes
        // during it restyles at the next draw (`restyle`).
        self.invalidate_all();
        self.restyle = false;
        self.lay_out(screen);
        self.settle_scrolls(screen);

        // Focus on what can't hold it any more lets go
        if let Some(f) = self.interaction.focus.clone() {
            let keeps = self.locate(&f).is_some_and(|(d, i)| {
                self.documents[d].takes_focus(&self.documents[d].elements[i])
            });
            if !keeps {
                self.focus(None);
            }
        }
        // An autofocus element shown with nothing focused takes it
        if self.interaction.focus.is_none() {
            let auto = self.documents.iter().find_map(|doc| {
                doc.elements
                    .iter()
                    .find(|e| e.autofocus && doc.takes_focus(e))
                    .map(|e| e.id.clone())
            });
            if let Some(id) = auto {
                self.focus(Some(&id));
            }
        }

        let mut done = Vec::new();
        let pointer_state_dragging = self.pointer.dragging.clone();

        // The mouse: hover focuses, a press holds, a release over the held
        // button activates it, a held slider follows
        if input.pointer_moved {
            if let Some(at) = self.under(input.pointer) {
                let id = self.element(at).id.clone();
                self.focus(Some(&id));
            }
        }
        if input.mouse_pressed {
            if let Some(at) = self.under(input.pointer) {
                let id = self.element(at).id.clone();
                self.focus(Some(&id));
                self.press(Some(id.clone()), false);
                match self.element(at).element_type {
                    ElementType::Slider => {
                        self.pointer.dragging = Some(id);
                        self.drag_to(at, input.pointer.0);
                    }
                    ElementType::TextInput => self.caret_to(at, input.pointer.0),
                    _ => {}
                }
            }
        } else if !self.pointer.touch_press && self.interaction.pressed.is_some() {
            if input.mouse_down {
                if let Some(at) = pointer_state_dragging
                    .as_deref()
                    .and_then(|d| self.locate(d))
                {
                    self.drag_to(at, input.pointer.0);
                }
            } else {
                let held = self.interaction.pressed.clone();
                if let (Some(at), Some(held)) = (self.under(input.pointer), held) {
                    if self.element(at).id == held {
                        self.activate(at, &mut done);
                    }
                }
                self.press(None, false);
            }
        }
        // The wheel scrolls the list under the pointer (#1229)
        self.wheel(input.pointer, input.wheel);

        // Touch: a touch down presses (and drags a slider), a tap activates,
        // a finger dragged along a scrolling list scrolls it
        if let Some(&p) = input.touches_started.first() {
            if let Some(at) = self.under(p) {
                let id = self.element(at).id.clone();
                self.focus(Some(&id));
                self.press(Some(id.clone()), true);
                match self.element(at).element_type {
                    ElementType::Slider => self.pointer.dragging = Some(id),
                    ElementType::TextInput => self.caret_to(at, p.0),
                    _ => {}
                }
            }
        }
        let scrolled = self.touch_scroll(
            input.touches_started.first().copied(),
            input.touches.first().copied(),
        );
        if self.pointer.touch_press {
            if let (Some(at), Some(&p)) = (
                self.pointer
                    .dragging
                    .as_deref()
                    .and_then(|d| self.locate(d)),
                input.touches.first(),
            ) {
                self.drag_to(at, p.0);
            }
        }
        for &p in input.taps.iter().filter(|_| !scrolled) {
            if let Some(at) = self.under(p) {
                let id = self.element(at).id.clone();
                self.focus(Some(&id));
                self.activate(at, &mut done);
            }
        }
        if self.pointer.touch_press && input.touches.is_empty() {
            self.press(None, true);
        }

        // A focused text input takes what was typed; the keys it typed with
        // are its own this frame (#1229)
        let typed = match self
            .interaction
            .focus
            .as_deref()
            .and_then(|f| self.locate(f))
        {
            Some(at) if self.element(at).element_type == ElementType::TextInput => {
                self.type_into(at, &input.text_edits)
            }
            _ => false,
        };

        // Keys and pads
        if let Some(dir) = input.nav {
            if let Some(at) = self
                .interaction
                .focus
                .as_deref()
                .and_then(|f| self.locate(f))
            {
                let e = self.element(at);
                if e.element_type == ElementType::TextInput && dir.is_horizontal() {
                    // The caret's keys, not focus's
                } else if e.element_type == ElementType::Slider && dir.is_horizontal() {
                    let step = e.effective_step();
                    let value = e.value + if dir == Dir::Right { step } else { -step };
                    self.slide(at, value);
                } else if let Some(next) = self.next(at, dir) {
                    let id = self.element(next).id.clone();
                    self.focus(Some(&id));
                }
            }
        }
        if let Some(at) = self
            .interaction
            .focus
            .as_deref()
            .and_then(|f| self.locate(f))
        {
            if input.accept && !typed {
                if self.element(at).element_type == ElementType::TextInput {
                    let id = self.event_id(at);
                    self.push(at, id, "activate");
                } else {
                    self.activate(at, &mut done);
                }
            }
            if input.back && !typed {
                let id = self.event_id(at);
                self.push(at, id, "back");
            }
        }
    }

    /// Hold an element down (or let go), redrawing in its `pressed` state.
    pub(super) fn press(&mut self, id: Option<String>, touch: bool) {
        if id.is_none() {
            self.pointer.dragging = None;
        }
        self.pointer.touch_press = touch && id.is_some();
        if self.interaction.pressed != id {
            self.interaction.pressed = id;
            self.restyle = true;
        }
    }
}
