//! UI element types and data structures

use std::collections::HashMap;

/// Element type determines rendering behavior
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementType {
    Panel,
    Text,
    Rect,
    Circle,
    Image,
    /// A labelled box a player focuses and activates (#731): keyboard,
    /// gamepad, mouse and touch all reach it.
    Button,
    /// A container whose focusable children are one list (#731): along its
    /// `layout` the arrows step through them and wrap, and its value is the
    /// index of the one focused. `items` makes the children for it.
    List,
    /// A value between `min` and `max` (#731): left and right step it, a
    /// pointer drags it.
    Slider,
}

impl ElementType {
    /// Every `type` a `.ui.toml` may name, in the order the errors list them.
    pub const NAMES: &'static [&'static str] = &[
        "panel", "text", "rect", "circle", "image", "button", "list", "slider",
    ];

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "panel" => Some(Self::Panel),
            "text" => Some(Self::Text),
            "rect" => Some(Self::Rect),
            "circle" => Some(Self::Circle),
            "image" => Some(Self::Image),
            "button" => Some(Self::Button),
            "list" => Some(Self::List),
            "slider" => Some(Self::Slider),
            _ => None,
        }
    }

    /// Whether an element of this type takes focus unless it says
    /// `focusable = false`.
    pub fn focusable_by_default(self) -> bool {
        matches!(self, Self::Button | Self::Slider)
    }
}

/// Anchor point on screen for root elements
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Anchor {
    #[default]
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Anchor {
    /// Every `anchor` a `.ui.toml` may name.
    pub const NAMES: &'static [&'static str] = &[
        "top-left",
        "top-center",
        "top-right",
        "center-left",
        "center",
        "center-right",
        "bottom-left",
        "bottom-center",
        "bottom-right",
    ];

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "top-left" => Some(Self::TopLeft),
            "top-center" => Some(Self::TopCenter),
            "top-right" => Some(Self::TopRight),
            "center-left" => Some(Self::CenterLeft),
            "center" => Some(Self::Center),
            "center-right" => Some(Self::CenterRight),
            "bottom-left" => Some(Self::BottomLeft),
            "bottom-center" => Some(Self::BottomCenter),
            "bottom-right" => Some(Self::BottomRight),
            _ => None,
        }
    }

    /// Returns the (x_fraction, y_fraction) of screen space this anchor maps to
    pub fn screen_origin(self) -> (f32, f32) {
        match self {
            Self::TopLeft => (0.0, 0.0),
            Self::TopCenter => (0.5, 0.0),
            Self::TopRight => (1.0, 0.0),
            Self::CenterLeft => (0.0, 0.5),
            Self::Center => (0.5, 0.5),
            Self::CenterRight => (1.0, 0.5),
            Self::BottomLeft => (0.0, 1.0),
            Self::BottomCenter => (0.5, 1.0),
            Self::BottomRight => (1.0, 1.0),
        }
    }
}

/// A direction focus moves in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    pub const ALL: [Dir; 4] = [Dir::Up, Dir::Down, Dir::Left, Dir::Right];

    /// The unit step on screen (y grows down).
    pub fn vector(self) -> (f32, f32) {
        match self {
            Dir::Up => (0.0, -1.0),
            Dir::Down => (0.0, 1.0),
            Dir::Left => (-1.0, 0.0),
            Dir::Right => (1.0, 0.0),
        }
    }

    /// The `.ui.toml` key that names where this direction goes from an
    /// element, overriding the layout: `nav_up`, `nav_down`, ...
    pub fn key(self) -> &'static str {
        match self {
            Dir::Up => "nav_up",
            Dir::Down => "nav_down",
            Dir::Left => "nav_left",
            Dir::Right => "nav_right",
        }
    }

    pub fn is_horizontal(self) -> bool {
        matches!(self, Dir::Left | Dir::Right)
    }
}

/// A single UI element in the hierarchy
#[derive(Debug, Clone)]
pub struct UiElement {
    pub id: String,
    pub element_type: ElementType,
    pub class: String,
    pub anchor: Anchor,
    /// The layout gave it an `anchor`: a child then sits at that point of
    /// its parent's content box, out of its stack (#731), as a root does on
    /// the screen.
    pub anchored: bool,
    pub parent_id: Option<String>,
    pub children: Vec<String>,
    pub text: String,
    pub src: String,
    pub visible: bool,

    // Interaction (#731)
    /// Whether focus can land here (buttons and sliders unless they say not).
    pub focusable: bool,
    /// A disabled element is drawn with its class's `disabled` state and
    /// focus skips it.
    pub enabled: bool,
    /// Takes focus when it is shown and nothing has focus.
    pub autofocus: bool,
    /// `nav_up` and the rest: where a direction goes from here, by id.
    pub nav: HashMap<Dir, String>,
    /// A slider's value, or a list's selected index.
    pub value: f32,
    pub min: f32,
    pub max: f32,
    /// A slider's step for left and right; 0 is a tenth of its range.
    pub step: f32,
    /// A list's items, each a button made for it (`<list>.<index>`).
    pub items: Vec<String>,
    /// The class each made item takes.
    pub item_class: String,
    /// Whether a list's arrows wrap from its last item to its first.
    pub wrap: bool,
    /// Made by a list for one of its `items`, not written in the file.
    pub made_by_list: bool,

    // Runtime overrides set by scripts
    pub text_override: Option<String>,
    pub color_override: Option<[f32; 4]>,
    pub bg_color_override: Option<[f32; 4]>,
    pub class_override: Option<String>,
    pub style_overrides: HashMap<String, StyleValue>,
}

/// A style property value that can be overridden at runtime
#[derive(Debug, Clone, PartialEq)]
pub enum StyleValue {
    Float(f32),
    Color([f32; 4]),
    String(String),
    Bool(bool),
    /// Drop shadow `[dx, dy, r, g, b, a]`: offset plus colour
    Shadow(f32, f32, [f32; 4]),
}

impl StyleValue {
    /// Interpret a numeric array the way `.style.toml` and
    /// `ui_set_style_array` do: 3 values → opaque `Color`, 4 values →
    /// `Color` (also used for `padding` and `uv`), 6 values → `Shadow`
    /// `[dx, dy, r, g, b, a]`.
    pub fn from_numbers(values: &[f32]) -> Option<Self> {
        match values {
            [r, g, b] => Some(StyleValue::Color([*r, *g, *b, 1.0])),
            [r, g, b, a] => Some(StyleValue::Color([*r, *g, *b, *a])),
            [dx, dy, r, g, b, a] => Some(StyleValue::Shadow(*dx, *dy, [*r, *g, *b, *a])),
            _ => None,
        }
    }
}

impl UiElement {
    pub fn new(id: String, element_type: ElementType) -> Self {
        Self {
            id,
            element_type,
            class: String::new(),
            anchor: Anchor::TopLeft,
            anchored: false,
            parent_id: None,
            children: Vec::new(),
            text: String::new(),
            src: String::new(),
            visible: true,
            focusable: element_type.focusable_by_default(),
            enabled: true,
            autofocus: false,
            nav: HashMap::new(),
            value: 0.0,
            min: 0.0,
            max: 1.0,
            step: 0.0,
            items: Vec::new(),
            item_class: String::new(),
            wrap: true,
            made_by_list: false,
            text_override: None,
            color_override: None,
            bg_color_override: None,
            class_override: None,
            style_overrides: HashMap::new(),
        }
    }

    /// Get the effective text content (override or default)
    pub fn effective_text(&self) -> &str {
        self.text_override.as_deref().unwrap_or(&self.text)
    }

    /// Get the effective class name (override or default)
    pub fn effective_class(&self) -> &str {
        self.class_override.as_deref().unwrap_or(&self.class)
    }

    /// A slider's value as a fraction of its range, 0..1.
    pub fn fraction(&self) -> f32 {
        let span = self.max - self.min;
        if span.abs() < f32::EPSILON {
            0.0
        } else {
            ((self.value - self.min) / span).clamp(0.0, 1.0)
        }
    }

    /// A slider's step: its own, or a tenth of its range.
    pub fn effective_step(&self) -> f32 {
        if self.step > 0.0 {
            self.step
        } else {
            (self.max - self.min) / 10.0
        }
    }

    /// Clear all runtime overrides
    pub fn reset_overrides(&mut self) {
        self.text_override = None;
        self.color_override = None;
        self.bg_color_override = None;
        self.class_override = None;
        self.style_overrides.clear();
    }
}
