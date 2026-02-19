//! Style definition and resolution

use super::element::StyleValue;
use std::collections::HashMap;

/// Layout flow direction
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LayoutFlow {
    #[default]
    Stack,      // Vertical stacking (default)
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

impl StyleClass {
    /// Resolve this class into a full ResolvedStyle, applying defaults for missing properties
    pub fn resolve(&self) -> ResolvedStyle {
        let mut style = ResolvedStyle::default();

        for (key, val) in &self.properties {
            match key.as_str() {
                "x" => if let StyleValue::Float(v) = val { style.x = *v; },
                "y" => if let StyleValue::Float(v) = val { style.y = *v; },
                "width" => if let StyleValue::Float(v) = val { style.width = *v; },
                "height" => if let StyleValue::Float(v) = val { style.height = *v; },
                "width_pct" => if let StyleValue::Float(v) = val { style.width_pct = Some(*v); },
                "height_pct" => if let StyleValue::Float(v) = val { style.height_pct = Some(*v); },
                "height_auto" => if let StyleValue::Bool(v) = val { style.height_auto = *v; },
                "font_size" => if let StyleValue::Float(v) = val { style.font_size = *v; },
                "rounding" => if let StyleValue::Float(v) = val { style.rounding = *v; },
                "opacity" => if let StyleValue::Float(v) = val { style.opacity = *v; },
                "thickness" => if let StyleValue::Float(v) = val { style.thickness = *v; },
                "radius" => if let StyleValue::Float(v) = val { style.radius = *v; },
                "layer" => if let StyleValue::Float(v) = val { style.layer = *v as i32; },
                "margin_bottom" => if let StyleValue::Float(v) = val { style.margin_bottom = *v; },
                "color" => if let StyleValue::Color(c) = val { style.color = *c; },
                "bg_color" => if let StyleValue::Color(c) = val { style.bg_color = *c; },
                "stroke_color" => if let StyleValue::Color(c) = val { style.stroke_color = *c; },
                "stroke_width" => if let StyleValue::Float(v) = val { style.stroke_width = *v; },
                "text_align" => if let StyleValue::String(s) = val {
                    style.text_align = match s.as_str() {
                        "center" => TextAlign::Center,
                        "right" => TextAlign::Right,
                        _ => TextAlign::Left,
                    };
                },
                "layout" => if let StyleValue::String(s) = val {
                    style.layout = match s.as_str() {
                        "horizontal" => LayoutFlow::Horizontal,
                        _ => LayoutFlow::Stack,
                    };
                },
                "padding" => if let StyleValue::Color(p) = val {
                    // Reuse Color([f32;4]) for 4-value padding
                    style.padding = *p;
                },
                _ => {}
            }
        }

        style
    }

    /// Apply runtime overrides to a resolved style
    pub fn apply_overrides(style: &mut ResolvedStyle, overrides: &HashMap<String, StyleValue>) {
        for (key, val) in overrides {
            match key.as_str() {
                "x" => if let StyleValue::Float(v) = val { style.x = *v; },
                "y" => if let StyleValue::Float(v) = val { style.y = *v; },
                "width" => if let StyleValue::Float(v) = val { style.width = *v; },
                "height" => if let StyleValue::Float(v) = val { style.height = *v; },
                "font_size" => if let StyleValue::Float(v) = val { style.font_size = *v; },
                "rounding" => if let StyleValue::Float(v) = val { style.rounding = *v; },
                "opacity" => if let StyleValue::Float(v) = val { style.opacity = *v; },
                "layer" => if let StyleValue::Float(v) = val { style.layer = *v as i32; },
                "color" => if let StyleValue::Color(c) = val { style.color = *c; },
                "bg_color" => if let StyleValue::Color(c) = val { style.bg_color = *c; },
                _ => {}
            }
        }
    }
}
