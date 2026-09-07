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
            uv: [0.0, 0.0, 1.0, 1.0],
            font: None,
            letter_spacing: 0.0,
            shadow: None,
            layout: LayoutFlow::Stack,
            margin_bottom: 0.0,
        }
    }
}

/// A named style class parsed from .style.toml
#[derive(Debug, Clone)]
pub struct StyleClass {
    pub name: String,
    pub properties: HashMap<String, StyleValue>,
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
    "layout",
    "margin_bottom",
    "uv",
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
        (k, _) => return is_known_property(k),
    }
    true
}

impl StyleClass {
    /// Resolve this class into a full ResolvedStyle, applying defaults for missing properties
    pub fn resolve(&self) -> ResolvedStyle {
        let mut style = ResolvedStyle::default();
        for (key, val) in &self.properties {
            apply_property(&mut style, key, val);
        }
        style
    }

    /// Apply runtime overrides to a resolved style
    pub fn apply_overrides(style: &mut ResolvedStyle, overrides: &HashMap<String, StyleValue>) {
        for (key, val) in overrides {
            apply_property(style, key, val);
        }
    }
}
