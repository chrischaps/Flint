//! UI element types and data structures

use std::collections::HashMap;

/// Element type determines rendering behavior
#[derive(Debug, Clone, PartialEq)]
pub enum ElementType {
    Panel,
    Text,
    Rect,
    Circle,
    Image,
}

impl ElementType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "panel" => Some(Self::Panel),
            "text" => Some(Self::Text),
            "rect" => Some(Self::Rect),
            "circle" => Some(Self::Circle),
            "image" => Some(Self::Image),
            _ => None,
        }
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

/// A single UI element in the hierarchy
#[derive(Debug, Clone)]
pub struct UiElement {
    pub id: String,
    pub element_type: ElementType,
    pub class: String,
    pub anchor: Anchor,
    pub parent_id: Option<String>,
    pub children: Vec<String>,
    pub text: String,
    pub src: String,
    pub visible: bool,

    // Runtime overrides set by scripts
    pub text_override: Option<String>,
    pub color_override: Option<[f32; 4]>,
    pub bg_color_override: Option<[f32; 4]>,
    pub class_override: Option<String>,
    pub style_overrides: HashMap<String, StyleValue>,
}

/// A style property value that can be overridden at runtime
#[derive(Debug, Clone)]
pub enum StyleValue {
    Float(f32),
    Color([f32; 4]),
    String(String),
    Bool(bool),
}

impl UiElement {
    pub fn new(id: String, element_type: ElementType) -> Self {
        Self {
            id,
            element_type,
            class: String::new(),
            anchor: Anchor::TopLeft,
            parent_id: None,
            children: Vec::new(),
            text: String::new(),
            src: String::new(),
            visible: true,
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

    /// Clear all runtime overrides
    pub fn reset_overrides(&mut self) {
        self.text_override = None;
        self.color_override = None;
        self.bg_color_override = None;
        self.class_override = None;
        self.style_overrides.clear();
    }
}
