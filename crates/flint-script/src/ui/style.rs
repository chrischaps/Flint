//! Style definition and resolution

use super::element::StyleValue;
use std::collections::HashMap;

/// Layout flow direction
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LayoutFlow {
    #[default]
    Stack, // Vertical stacking (default)
    Horizontal, // Horizontal flow
}

/// Text alignment
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Resolved style for a single element — all visual properties with defaults applied
#[derive(Debug, Clone)]
pub struct ResolvedStyle {
    // Position (offset from anchor/parent)
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub width_pct: Option<f32>,
    pub height_pct: Option<f32>,
    pub height_auto: bool,

    // Visual
    pub color: [f32; 4],
    pub bg_color: [f32; 4],
    pub font_size: f32,
    pub text_align: TextAlign,
    pub rounding: f32,
    pub opacity: f32,
    pub thickness: f32,
    pub radius: f32,
    pub layer: i32,
    pub padding: [f32; 4], // L, T, R, B
    pub stroke_color: [f32; 4],
    pub stroke_width: f32,
    /// Glow under the text and its stroke (#626): colour, and reach in
    /// points (0 = none).
    pub glow_color: [f32; 4],
    pub glow_radius: f32,
    /// Image sub-rectangle `[u0, v0, u1, v1]` in 0..1 texture space
    /// (sprite-sheet cells, cropped atlases). Images only.
    pub uv: [f32; 4],
    /// Font family (file stem or `fonts.toml` alias); None = default font
    pub font: Option<String>,
    /// Extra glyph spacing in logical points
    pub letter_spacing: f32,
    /// Drop shadow: (colour, dx, dy)
    pub shadow: Option<([f32; 4], f32, f32)>,

    // Layout
    pub layout: LayoutFlow,
    pub margin_bottom: f32,

    // Controls (#731)
    /// An outline round a panel, button, list or slider: colour and width
    /// in points (0 = none).
    pub border_color: [f32; 4],
    pub border_width: f32,
    /// A slider's knob; its size is `radius` (0 = no knob).
    pub knob_color: [f32; 4],

    // Scrolling lists and text inputs (#1229)
    /// A scrolling list's thumb, drawn when its items overflow: colour and
    /// width in points (0 = none).
    pub scrollbar_color: [f32; 4],
    pub scrollbar_width: f32,
    /// A text input's placeholder text, and its caret.
    pub placeholder_color: [f32; 4],
    pub caret_color: [f32; 4],
}

impl Default for ResolvedStyle {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
            width_pct: None,
            height_pct: None,
            height_auto: false,
            color: [1.0, 1.0, 1.0, 1.0],
            bg_color: [0.0, 0.0, 0.0, 0.0],
            font_size: 16.0,
            text_align: TextAlign::Left,
            rounding: 0.0,
            opacity: 1.0,
            thickness: 1.0,
            radius: 0.0,
            layer: 0,
            padding: [0.0; 4],
            stroke_color: [0.0, 0.0, 0.0, 1.0],
            stroke_width: 0.0,
            glow_color: [1.0, 1.0, 1.0, 0.73],
            glow_radius: 0.0,
            uv: [0.0, 0.0, 1.0, 1.0],
            font: None,
            letter_spacing: 0.0,
            shadow: None,
            layout: LayoutFlow::Stack,
            margin_bottom: 0.0,
            border_color: [1.0, 1.0, 1.0, 1.0],
            border_width: 0.0,
            knob_color: [1.0, 1.0, 1.0, 1.0],
            scrollbar_color: [1.0, 1.0, 1.0, 0.35],
            scrollbar_width: 3.0,
            placeholder_color: [1.0, 1.0, 1.0, 0.4],
            caret_color: [1.0, 1.0, 1.0, 1.0],
        }
    }
}

impl ResolvedStyle {
    /// The defaults an element of `element_type` starts from: a button's
    /// label is centred.
    pub fn for_type(element_type: super::element::ElementType) -> Self {
        let mut style = Self::default();
        if element_type == super::element::ElementType::Button {
            style.text_align = TextAlign::Center;
        }
        style
    }
}

/// A state an element can be drawn in (#731). A class's sub-table of the
/// state's name (`[styles.button.focus]`) goes over the class while the
/// element is in it: `focus`, then `pressed`, then `disabled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiState {
    Focus,
    Pressed,
    Disabled,
}

impl UiState {
    pub const ALL: [UiState; 3] = [UiState::Focus, UiState::Pressed, UiState::Disabled];

    pub fn name(self) -> &'static str {
        match self {
            UiState::Focus => "focus",
            UiState::Pressed => "pressed",
            UiState::Disabled => "disabled",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// Which states an element is in this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct States {
    pub focus: bool,
    pub pressed: bool,
    pub disabled: bool,
}

/// A named style class parsed from .style.toml
#[derive(Debug, Clone)]
pub struct StyleClass {
    pub name: String,
    pub properties: HashMap<String, StyleValue>,
    /// What goes over `properties` in each state (`[styles.<name>.focus]`).
    pub states: HashMap<UiState, HashMap<String, StyleValue>>,
}

/// Every property name the style parser and `ui_set_style` understand.
pub const KNOWN_PROPERTIES: &[&str] = &[
    "x",
    "y",
    "width",
    "height",
    "width_pct",
    "height_pct",
    "height_auto",
    "color",
    "bg_color",
    "font_size",
    "font",
    "letter_spacing",
    "shadow",
    "text_align",
    "rounding",
    "opacity",
    "thickness",
    "radius",
    "layer",
    "padding",
    "stroke_color",
    "stroke_width",
    "glow_color",
    "glow_radius",
    "layout",
    "margin_bottom",
    "uv",
    "border_color",
    "border_width",
    "knob_color",
    "scrollbar_color",
    "scrollbar_width",
    "placeholder_color",
    "caret_color",
];

/// True when `prop` is a property name the style system understands.
pub fn is_known_property(prop: &str) -> bool {
    KNOWN_PROPERTIES.contains(&prop)
}

/// Apply one named property to a resolved style. Returns `false` when the
/// property name is unknown (a wrongly-typed value for a known name is
/// ignored but still returns `true`).
///
/// This is the single place property names are interpreted: class
/// resolution from `.style.toml` and runtime `ui_set_style` overrides both
/// go through it, so anything the parser accepts a script can set too.
pub fn apply_property(style: &mut ResolvedStyle, key: &str, val: &StyleValue) -> bool {
    use StyleValue::*;
    match (key, val) {
        ("x", Float(v)) => style.x = *v,
        ("y", Float(v)) => style.y = *v,
        ("width", Float(v)) => style.width = *v,
        ("height", Float(v)) => style.height = *v,
        ("width_pct", Float(v)) => style.width_pct = Some(*v),
        ("height_pct", Float(v)) => style.height_pct = Some(*v),
        ("height_auto", Bool(v)) => style.height_auto = *v,
        ("font_size", Float(v)) => style.font_size = *v,
        ("rounding", Float(v)) => style.rounding = *v,
        ("opacity", Float(v)) => style.opacity = *v,
        ("thickness", Float(v)) => style.thickness = *v,
        ("radius", Float(v)) => style.radius = *v,
        ("layer", Float(v)) => style.layer = *v as i32,
        ("margin_bottom", Float(v)) => style.margin_bottom = *v,
        ("color", Color(c)) => style.color = *c,
        ("bg_color", Color(c)) => style.bg_color = *c,
        ("stroke_color", Color(c)) => style.stroke_color = *c,
        ("stroke_width", Float(v)) => style.stroke_width = *v,
        ("glow_color", Color(c)) => style.glow_color = *c,
        ("glow_radius", Float(v)) => style.glow_radius = *v,
        ("letter_spacing", Float(v)) => style.letter_spacing = *v,
        ("shadow", Shadow(dx, dy, c)) => style.shadow = Some((*c, *dx, *dy)),
        // Reuse Color([f32; 4]) for 4-value padding and image uv rects
        ("padding", Color(p)) => style.padding = *p,
        ("uv", Color(u)) => style.uv = *u,
        ("text_align", String(s)) => {
            style.text_align = match s.as_str() {
                "center" => TextAlign::Center,
                "right" => TextAlign::Right,
                _ => TextAlign::Left,
            }
        }
        ("layout", String(s)) => {
            style.layout = match s.as_str() {
                "horizontal" => LayoutFlow::Horizontal,
                _ => LayoutFlow::Stack,
            }
        }
        ("font", String(s)) => style.font = if s.is_empty() { None } else { Some(s.clone()) },
        ("border_color", Color(c)) => style.border_color = *c,
        ("border_width", Float(v)) => style.border_width = *v,
        ("knob_color", Color(c)) => style.knob_color = *c,
        ("scrollbar_color", Color(c)) => style.scrollbar_color = *c,
        ("scrollbar_width", Float(v)) => style.scrollbar_width = *v,
        ("placeholder_color", Color(c)) => style.placeholder_color = *c,
        ("caret_color", Color(c)) => style.caret_color = *c,
        (k, _) => return is_known_property(k),
    }
    true
}

/// What's wrong with `key = val` in a style class, if anything: an unknown
/// property (naming the known ones), or a value of the wrong shape for it.
pub fn property_problem(key: &str, val: &StyleValue) -> Option<String> {
    if !is_known_property(key) {
        return Some(format!(
            "unknown style property `{key}` (known: {})",
            KNOWN_PROPERTIES.join(", ")
        ));
    }
    let shape = shape_of(key);
    let fits = match (shape, val) {
        (Shape::Number, StyleValue::Float(_))
        | (Shape::Bool, StyleValue::Bool(_))
        | (Shape::Color | Shape::Four, StyleValue::Color(_))
        | (Shape::Shadow, StyleValue::Shadow(..))
        | (Shape::Text, StyleValue::String(_)) => true,
        (Shape::OneOf(choices), StyleValue::String(s)) => choices.contains(&s.as_str()),
        _ => false,
    };
    (!fits).then(|| format!("`{key}` takes {}, not {}", shape.describe(), describe(val)))
}

/// What a property's value is.
#[derive(Clone, Copy)]
enum Shape {
    Number,
    Bool,
    Color,
    Four,
    Shadow,
    Text,
    OneOf(&'static [&'static str]),
}

impl Shape {
    fn describe(self) -> String {
        match self {
            Shape::Number => "a number".into(),
            Shape::Bool => "true or false".into(),
            Shape::Color => "a colour, [r, g, b] or [r, g, b, a]".into(),
            Shape::Four => "4 numbers".into(),
            Shape::Shadow => "[dx, dy, r, g, b, a]".into(),
            Shape::Text => "a string".into(),
            Shape::OneOf(choices) => format!("one of {}", choices.join(", ")),
        }
    }
}

fn shape_of(key: &str) -> Shape {
    match key {
        "height_auto" => Shape::Bool,
        "color" | "bg_color" | "stroke_color" | "glow_color" | "border_color" | "knob_color"
        | "scrollbar_color" | "placeholder_color" | "caret_color" => Shape::Color,
        "padding" | "uv" => Shape::Four,
        "shadow" => Shape::Shadow,
        "font" => Shape::Text,
        "text_align" => Shape::OneOf(&["left", "center", "right"]),
        "layout" => Shape::OneOf(&["stack", "vertical", "horizontal"]),
        _ => Shape::Number,
    }
}

fn describe(val: &StyleValue) -> String {
    match val {
        StyleValue::Float(v) => format!("{v}"),
        StyleValue::Color(c) => format!("{c:?}"),
        StyleValue::String(s) => format!("\"{s}\""),
        StyleValue::Bool(b) => format!("{b}"),
        StyleValue::Shadow(..) => "6 numbers".into(),
    }
}

impl StyleClass {
    /// Resolve this class into a full ResolvedStyle, applying defaults for missing properties
    pub fn resolve(&self) -> ResolvedStyle {
        let mut style = ResolvedStyle::default();
        self.apply_to(&mut style, States::default());
        style
    }

    /// Lay this class over `style`, then the table of each state `states`
    /// has on: focus, then pressed, then disabled.
    pub fn apply_to(&self, style: &mut ResolvedStyle, states: States) {
        for (key, val) in &self.properties {
            apply_property(style, key, val);
        }
        for (state, on) in [
            (UiState::Focus, states.focus),
            (UiState::Pressed, states.pressed),
            (UiState::Disabled, states.disabled),
        ] {
            if let (true, Some(props)) = (on, self.states.get(&state)) {
                for (key, val) in props {
                    apply_property(style, key, val);
                }
            }
        }
    }

    /// Apply runtime overrides to a resolved style
    pub fn apply_overrides(style: &mut ResolvedStyle, overrides: &HashMap<String, StyleValue>) {
        for (key, val) in overrides {
            apply_property(style, key, val);
        }
    }
}
