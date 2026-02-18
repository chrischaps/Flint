//! Data-driven UI system: Layout / Style / Logic separation
//!
//! Elements are defined in .ui.toml (structure), styled via .style.toml (visuals),
//! and controlled from Rhai scripts (logic). The existing draw_* API continues to
//! work for procedural elements (minimap, speed lines, etc).

pub mod element;
pub mod layout;
pub mod loader;
pub mod style;

use crate::context::DrawCommand;
use element::{ElementType, UiElement};
use layout::ResolvedRect;
use style::{ResolvedStyle, StyleClass};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A loaded UI document (one layout + style pair)
pub struct UiDocument {
    pub elements: Vec<UiElement>,
    pub styles: HashMap<String, StyleClass>,
    /// Cached layout results (invalidated on screen resize)
    cached_rects: HashMap<String, (ResolvedRect, ResolvedStyle)>,
    cached_screen_w: f32,
    cached_screen_h: f32,
    /// Base directory for resolving relative paths
    #[allow(dead_code)]
    base_dir: PathBuf,
}

impl UiDocument {
    /// Resolve layout if screen size changed
    fn ensure_layout(&mut self, screen_w: f32, screen_h: f32) {
        if (self.cached_screen_w - screen_w).abs() > 0.5
            || (self.cached_screen_h - screen_h).abs() > 0.5
            || self.cached_rects.is_empty()
        {
            self.cached_rects = layout::resolve_layout(&self.elements, &self.styles, screen_w, screen_h);
            self.cached_screen_w = screen_w;
            self.cached_screen_h = screen_h;
        }
    }

    /// Invalidate cached layout (call after element structure changes)
    fn invalidate_cache(&mut self) {
        self.cached_rects.clear();
    }

    /// Find element by ID
    pub fn find_element(&self, id: &str) -> Option<&UiElement> {
        self.elements.iter().find(|e| e.id == id)
    }

    /// Find element by ID (mutable)
    pub fn find_element_mut(&mut self, id: &str) -> Option<&mut UiElement> {
        self.elements.iter_mut().find(|e| e.id == id)
    }

    /// Generate draw commands for all visible elements
    fn generate_commands(&mut self, screen_w: f32, screen_h: f32) -> Vec<DrawCommand> {
        self.ensure_layout(screen_w, screen_h);
        let mut commands = Vec::new();

        for elem in &self.elements {
            if !elem.visible {
                continue;
            }

            // Check parent visibility
            if let Some(ref pid) = elem.parent_id {
                if let Some(parent) = self.elements.iter().find(|e| e.id == *pid) {
                    if !parent.visible {
                        continue;
                    }
                }
            }

            let (rect, style): (ResolvedRect, ResolvedStyle) = match self.cached_rects.get(&elem.id) {
                Some(r) => r.clone(),
                None => continue,
            };

            let opacity = style.opacity;

            match elem.element_type {
                ElementType::Panel => {
                    // Draw background if has bg_color with alpha > 0
                    if style.bg_color[3] > 0.001 {
                        let mut color = style.bg_color;
                        color[3] *= opacity;
                        commands.push(DrawCommand::RectFilled {
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
                ElementType::Text => {
                    let text = elem.effective_text();
                    if !text.is_empty() {
                        let mut color = style.color;
                        color[3] *= opacity;

                        // Pass alignment to renderer for accurate centering with actual font metrics
                        let (text_x, text_align) = match style.text_align {
                            style::TextAlign::Center if rect.w > 0.0 => {
                                (rect.x + rect.w / 2.0, 1u8)
                            }
                            style::TextAlign::Right if rect.w > 0.0 => {
                                (rect.x + rect.w, 2u8)
                            }
                            _ => (rect.x, 0u8),
                        };

                        let stroke = if style.stroke_width > 0.0 {
                            let mut sc = style.stroke_color;
                            sc[3] *= opacity;
                            Some((sc, style.stroke_width))
                        } else {
                            None
                        };
                        commands.push(DrawCommand::Text {
                            x: text_x,
                            y: rect.y,
                            text: text.to_string(),
                            size: style.font_size,
                            color,
                            layer: style.layer,
                            align: text_align,
                            stroke,
                        });
                    }
                }
                ElementType::Rect => {
                    let mut color = style.color;
                    color[3] *= opacity;
                    if style.thickness > 0.0 && style.bg_color[3] < 0.001 {
                        commands.push(DrawCommand::RectOutline {
                            x: rect.x,
                            y: rect.y,
                            w: rect.w,
                            h: rect.h,
                            color,
                            thickness: style.thickness,
                            layer: style.layer,
                        });
                    } else {
                        commands.push(DrawCommand::RectFilled {
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
                    let mut color = style.color;
                    color[3] *= opacity;
                    commands.push(DrawCommand::CircleFilled {
                        x: rect.x + rect.w / 2.0,
                        y: rect.y + rect.h / 2.0,
                        radius: style.radius,
                        color,
                        layer: style.layer,
                    });
                }
                ElementType::Image => {
                    // Image support via sprite draw command
                    if !elem.src.is_empty() {
                        commands.push(DrawCommand::Sprite {
                            x: rect.x,
                            y: rect.y,
                            w: rect.w,
                            h: rect.h,
                            name: elem.src.clone(),
                            uv: [0.0, 0.0, 1.0, 1.0],
                            tint: [style.color[0], style.color[1], style.color[2], style.color[3] * opacity],
                            layer: style.layer,
                        });
                    }
                }
            }
        }

        commands
    }
}

/// The top-level UI system that holds all loaded documents
pub struct UiSystem {
    documents: Vec<UiDocument>,
    next_handle: i64,
    handle_map: HashMap<i64, usize>, // handle → index in documents
}

impl UiSystem {
    pub fn new() -> Self {
        Self {
            documents: Vec::new(),
            next_handle: 1,
            handle_map: HashMap::new(),
        }
    }

    /// Load a UI document from a layout file path.
    /// The style path is read from the [ui].style field in the layout file.
    /// Returns a handle for future operations, or -1 on error.
    pub fn load(&mut self, layout_path: &str, scene_dir: &Path) -> i64 {
        let layout_file = scene_dir.join(layout_path);

        let (elements, style_rel_path) = match loader::load_layout(&layout_file) {
            Ok(result) => result,
            Err(e) => {
                eprintln!("[ui] {}", e);
                return -1;
            }
        };

        let styles = if !style_rel_path.is_empty() {
            let style_file = scene_dir.join(&style_rel_path);
            match loader::load_styles(&style_file) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("[ui] {}", e);
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };

        let base_dir = layout_file.parent().unwrap_or(scene_dir).to_path_buf();

        let doc = UiDocument {
            elements,
            styles,
            cached_rects: HashMap::new(),
            cached_screen_w: 0.0,
            cached_screen_h: 0.0,
            base_dir,
        };

        let handle = self.next_handle;
        self.next_handle += 1;

        let idx = self.documents.len();
        self.documents.push(doc);
        self.handle_map.insert(handle, idx);

        println!("[ui] Loaded {} (handle {})", layout_path, handle);
        handle
    }

    /// Unload a UI document by handle
    pub fn unload(&mut self, handle: i64) {
        if let Some(&idx) = self.handle_map.get(&handle) {
            if idx < self.documents.len() {
                self.documents.remove(idx);
                self.handle_map.remove(&handle);
                // Re-index remaining handles
                let mut new_map = HashMap::new();
                for (&h, &old_idx) in &self.handle_map {
                    if old_idx > idx {
                        new_map.insert(h, old_idx - 1);
                    } else {
                        new_map.insert(h, old_idx);
                    }
                }
                self.handle_map = new_map;
            }
        }
    }

    /// Set text content of an element (searches all documents)
    pub fn set_text(&mut self, element_id: &str, text: &str) {
        for doc in &mut self.documents {
            if let Some(elem) = doc.find_element_mut(element_id) {
                elem.text_override = Some(text.to_string());
                doc.invalidate_cache();
                return;
            }
        }
    }

    /// Show an element
    pub fn show(&mut self, element_id: &str) {
        for doc in &mut self.documents {
            if let Some(elem) = doc.find_element_mut(element_id) {
                elem.visible = true;
                return;
            }
        }
    }

    /// Hide an element
    pub fn hide(&mut self, element_id: &str) {
        for doc in &mut self.documents {
            if let Some(elem) = doc.find_element_mut(element_id) {
                elem.visible = false;
                return;
            }
        }
    }

    /// Set visibility of an element
    pub fn set_visible(&mut self, element_id: &str, visible: bool) {
        for doc in &mut self.documents {
            if let Some(elem) = doc.find_element_mut(element_id) {
                elem.visible = visible;
                return;
            }
        }
    }

    /// Override primary color of an element
    pub fn set_color(&mut self, element_id: &str, r: f32, g: f32, b: f32, a: f32) {
        for doc in &mut self.documents {
            if let Some(elem) = doc.find_element_mut(element_id) {
                elem.color_override = Some([r, g, b, a]);
                doc.invalidate_cache();
                return;
            }
        }
    }

    /// Override background color of an element
    pub fn set_bg_color(&mut self, element_id: &str, r: f32, g: f32, b: f32, a: f32) {
        for doc in &mut self.documents {
            if let Some(elem) = doc.find_element_mut(element_id) {
                elem.bg_color_override = Some([r, g, b, a]);
                doc.invalidate_cache();
                return;
            }
        }
    }

    /// Override a specific style property
    pub fn set_style(&mut self, element_id: &str, prop: &str, val: element::StyleValue) {
        for doc in &mut self.documents {
            if let Some(elem) = doc.find_element_mut(element_id) {
                elem.style_overrides.insert(prop.to_string(), val);
                doc.invalidate_cache();
                return;
            }
        }
    }

    /// Reset all style overrides for an element
    pub fn reset_style(&mut self, element_id: &str) {
        for doc in &mut self.documents {
            if let Some(elem) = doc.find_element_mut(element_id) {
                elem.reset_overrides();
                doc.invalidate_cache();
                return;
            }
        }
    }

    /// Switch an element's style class
    pub fn set_class(&mut self, element_id: &str, class: &str) {
        for doc in &mut self.documents {
            if let Some(elem) = doc.find_element_mut(element_id) {
                elem.class_override = Some(class.to_string());
                doc.invalidate_cache();
                return;
            }
        }
    }

    /// Check if an element exists in any loaded document
    pub fn exists(&self, element_id: &str) -> bool {
        self.documents.iter().any(|doc| doc.find_element(element_id).is_some())
    }

    /// Get the resolved screen rect for an element
    pub fn get_rect(&mut self, element_id: &str, screen_w: f32, screen_h: f32) -> Option<(f32, f32, f32, f32)> {
        for doc in &mut self.documents {
            doc.ensure_layout(screen_w, screen_h);
            if let Some((rect, _)) = doc.cached_rects.get(element_id) {
                return Some((rect.x, rect.y, rect.w, rect.h));
            }
        }
        None
    }

    /// Generate all UI draw commands for all loaded documents
    pub fn generate_draw_commands(&mut self, screen_w: f32, screen_h: f32) -> Vec<DrawCommand> {
        let mut all_commands = Vec::new();
        for doc in &mut self.documents {
            all_commands.extend(doc.generate_commands(screen_w, screen_h));
        }
        all_commands
    }

    /// Clear all loaded documents (for scene transitions)
    pub fn clear(&mut self) {
        self.documents.clear();
        self.handle_map.clear();
        self.next_handle = 1;
    }
}

impl Default for UiSystem {
    fn default() -> Self {
        Self::new()
    }
}
