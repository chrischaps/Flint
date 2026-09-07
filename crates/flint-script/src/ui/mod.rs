//! Data-driven UI system: Layout / Style / Logic separation
//!
//! Elements are defined in .ui.toml (structure), styled via .style.toml (visuals),
//! and controlled from Rhai scripts (logic). The existing draw_* API continues to
//! work for procedural elements (minimap, speed lines, etc).
//!
//! Style files may reference named tokens (`"$accent"`) from a local
//! `[tokens]` table or a shared token file — see [`tokens`]. Loaded
//! documents remember their source files and [`UiSystem::poll_reload`]
//! re-parses them in place when any changes, keeping script overrides.

pub mod element;
pub mod layout;
pub mod loader;
pub mod style;
pub mod tokens;

#[cfg(test)]
mod tests;

use crate::context::DrawCommand;
use element::{ElementType, StyleValue, UiElement};
use layout::ResolvedRect;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use style::{ResolvedStyle, StyleClass};
use tokens::TokenSet;

/// Modification fingerprint of one source file: (mtime, length). `None`
/// when the file is missing so that appearing/disappearing counts as a change.
type FileStamp = Option<(SystemTime, u64)>;

fn stamp(path: &Path) -> FileStamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// A loaded UI document (one layout + style pair)
pub struct UiDocument {
    pub elements: Vec<UiElement>,
    pub styles: HashMap<String, StyleClass>,
    /// Tokens the style file resolved against (local `[tokens]` + shared file)
    pub tokens: TokenSet,
    /// Cached layout results (invalidated on screen resize)
    cached_rects: HashMap<String, (ResolvedRect, ResolvedStyle)>,
    cached_screen_w: f32,
    cached_screen_h: f32,
    /// Project root the layout/style/token paths resolve against
    root_dir: PathBuf,
    /// Layout path as passed to `load_ui` (relative to `root_dir`)
    layout_rel: String,
    /// Absolute source files this document was built from
    layout_path: PathBuf,
    style_path: Option<PathBuf>,
    /// Fingerprints of every watched file at last (re)load
    stamps: Vec<(PathBuf, FileStamp)>,
}

/// Script-set state carried across a hot reload for one element id.
#[derive(Debug, Clone)]
struct ElementOverrides {
    visible: bool,
    text_override: Option<String>,
    color_override: Option<[f32; 4]>,
    bg_color_override: Option<[f32; 4]>,
    class_override: Option<String>,
    style_overrides: HashMap<String, StyleValue>,
}

impl ElementOverrides {
    fn capture(e: &UiElement) -> Self {
        Self {
            visible: e.visible,
            text_override: e.text_override.clone(),
            color_override: e.color_override,
            bg_color_override: e.bg_color_override,
            class_override: e.class_override.clone(),
            style_overrides: e.style_overrides.clone(),
        }
    }

    fn apply(self, e: &mut UiElement) {
        e.visible = self.visible;
        e.text_override = self.text_override;
        e.color_override = self.color_override;
        e.bg_color_override = self.bg_color_override;
        e.class_override = self.class_override;
        e.style_overrides = self.style_overrides;
    }
}

/// Everything parsed from disk for one document.
struct ParsedDocument {
    elements: Vec<UiElement>,
    styles: HashMap<String, StyleClass>,
    tokens: TokenSet,
    style_path: Option<PathBuf>,
}

/// Parse layout + style + tokens. The layout is required; a broken style
/// file warns and yields an unstyled document.
fn parse_document(layout_path: &Path, root_dir: &Path) -> Result<ParsedDocument, String> {
    let (elements, style_rel) = loader::load_layout(layout_path)?;

    let mut styles = HashMap::new();
    let mut tokens = TokenSet::default();
    let mut style_path = None;
    if !style_rel.is_empty() {
        let sp = root_dir.join(&style_rel);
        match loader::load_styles(&sp, root_dir) {
            Ok(sheet) => {
                styles = sheet.classes;
                tokens = sheet.tokens;
            }
            Err(e) => tracing::warn!("{}", e),
        }
        style_path = Some(sp);
    }

    Ok(ParsedDocument {
        elements,
        styles,
        tokens,
        style_path,
    })
}

impl UiDocument {
    /// Resolve layout if screen size changed
    fn ensure_layout(&mut self, screen_w: f32, screen_h: f32) {
        if (self.cached_screen_w - screen_w).abs() > 0.5
            || (self.cached_screen_h - screen_h).abs() > 0.5
            || self.cached_rects.is_empty()
        {
            self.cached_rects =
                layout::resolve_layout(&self.elements, &self.styles, screen_w, screen_h);
            self.cached_screen_w = screen_w;
            self.cached_screen_h = screen_h;
        }
    }

    /// Invalidate cached layout (call after element structure changes)
    fn invalidate_cache(&mut self) {
        self.cached_rects.clear();
    }

    /// Files whose change should trigger a reload.
    fn watched_paths(&self) -> Vec<PathBuf> {
        let mut v = vec![self.layout_path.clone()];
        v.extend(self.style_path.iter().cloned());
        v.extend(self.tokens.watched_paths().cloned());
        v
    }

    fn snapshot_stamps(&self) -> Vec<(PathBuf, FileStamp)> {
        self.watched_paths()
            .into_iter()
            .map(|p| {
                let st = stamp(&p);
                (p, st)
            })
            .collect()
    }

    /// True when any watched file's fingerprint differs from the last load.
    fn changed_on_disk(&self) -> bool {
        self.stamps.iter().any(|(p, old)| stamp(p) != *old)
    }

    /// Re-parse this document from disk in place, keeping the handle and
    /// re-applying script overrides for element ids that still exist.
    /// On a parse error the old content stays and the error is returned.
    fn reload(&mut self) -> Result<(), String> {
        let parsed = match parse_document(&self.layout_path, &self.root_dir) {
            Ok(p) => p,
            Err(e) => {
                // Remember the broken state so we do not re-warn every frame.
                self.stamps = self.snapshot_stamps();
                return Err(e);
            }
        };

        let saved: HashMap<String, ElementOverrides> = self
            .elements
            .iter()
            .map(|e| (e.id.clone(), ElementOverrides::capture(e)))
            .collect();

        self.elements = parsed.elements;
        self.styles = parsed.styles;
        self.tokens = parsed.tokens;
        self.style_path = parsed.style_path;

        let mut restored = 0usize;
        for elem in &mut self.elements {
            if let Some(ov) = saved.get(&elem.id) {
                ov.clone().apply(elem);
                restored += 1;
            }
        }

        self.invalidate_cache();
        self.stamps = self.snapshot_stamps();
        tracing::info!(
            "[ui] Hot-reloaded {} ({} elements, {} overrides restored)",
            self.layout_rel,
            self.elements.len(),
            restored
        );
        Ok(())
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

            let (rect, style): (ResolvedRect, ResolvedStyle) = match self.cached_rects.get(&elem.id)
            {
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
                            style::TextAlign::Right if rect.w > 0.0 => (rect.x + rect.w, 2u8),
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
                            font: style.font.clone(),
                            letter_spacing: style.letter_spacing,
                            shadow: style.shadow.map(|(mut sc, dx, dy)| {
                                sc[3] *= opacity;
                                (sc, dx, dy)
                            }),
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
                            rounding: 0.0,
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
                            tint: [
                                style.color[0],
                                style.color[1],
                                style.color[2],
                                style.color[3] * opacity,
                            ],
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
    /// (element id, property) pairs already reported as unknown
    warned_props: HashSet<(String, String)>,
}

impl UiSystem {
    pub fn new() -> Self {
        Self {
            documents: Vec::new(),
            next_handle: 1,
            handle_map: HashMap::new(),
            warned_props: HashSet::new(),
        }
    }

    /// Load a UI document from a layout file path.
    /// The style path is read from the `[ui].style` field in the layout file.
    /// Returns a handle for future operations, or -1 on error.
    pub fn load(&mut self, layout_path: &str, scene_dir: &Path) -> i64 {
        let layout_file = scene_dir.join(layout_path);

        let parsed = match parse_document(&layout_file, scene_dir) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(
                    "load_ui('{}') failed (resolved to {}): {}",
                    layout_path,
                    layout_file.display(),
                    e
                );
                return -1;
            }
        };

        let mut doc = UiDocument {
            elements: parsed.elements,
            styles: parsed.styles,
            tokens: parsed.tokens,
            cached_rects: HashMap::new(),
            cached_screen_w: 0.0,
            cached_screen_h: 0.0,
            root_dir: scene_dir.to_path_buf(),
            layout_rel: layout_path.to_string(),
            layout_path: layout_file,
            style_path: parsed.style_path,
            stamps: Vec::new(),
        };
        doc.stamps = doc.snapshot_stamps();

        let handle = self.next_handle;
        self.next_handle += 1;

        let idx = self.documents.len();
        self.documents.push(doc);
        self.handle_map.insert(handle, idx);

        println!("[ui] Loaded {} (handle {})", layout_path, handle);
        handle
    }

    /// Check every loaded document's source files (`.ui.toml`, `.style.toml`,
    /// shared token file) and re-parse those that changed, in place. Call
    /// once per frame alongside script hot-reload. Returns how many
    /// documents were reloaded.
    pub fn poll_reload(&mut self) -> usize {
        let mut reloaded = 0;
        for doc in &mut self.documents {
            if !doc.changed_on_disk() {
                continue;
            }
            match doc.reload() {
                Ok(()) => reloaded += 1,
                Err(e) => tracing::warn!("[ui] Hot-reload of {} failed: {}", doc.layout_rel, e),
            }
        }
        reloaded
    }

    /// Look up a style token by name (`"accent"`, `"$accent"`,
    /// `"color.accent"`) across all loaded documents, first hit wins.
    pub fn token(&self, name: &str) -> Option<toml::Value> {
        self.documents
            .iter()
            .find_map(|doc| doc.tokens.get(name).cloned())
    }

    /// Point-in-rect test against an element's resolved layout rect.
    pub fn hit(&mut self, element_id: &str, x: f32, y: f32, screen_w: f32, screen_h: f32) -> bool {
        match self.get_rect(element_id, screen_w, screen_h) {
            Some((rx, ry, rw, rh)) => x >= rx && y >= ry && x < rx + rw && y < ry + rh,
            None => false,
        }
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

    /// Override a specific style property. A `"$name"` string resolves
    /// through the owning document's tokens; unknown property names warn
    /// once per (element, property) and are ignored.
    pub fn set_style(&mut self, element_id: &str, prop: &str, val: StyleValue) {
        if !style::is_known_property(prop) {
            let key = (element_id.to_string(), prop.to_string());
            if self.warned_props.insert(key) {
                tracing::warn!(
                    "ui_set_style('{}', '{}'): unknown style property (known: {})",
                    element_id,
                    prop,
                    style::KNOWN_PROPERTIES.join(", ")
                );
            }
            return;
        }
        for doc in &mut self.documents {
            if doc.find_element(element_id).is_none() {
                continue;
            }
            let val = match &val {
                StyleValue::String(s) if TokenSet::is_reference(s) => {
                    match doc.tokens.resolve_style(prop, s) {
                        Some(v) => v,
                        None => return,
                    }
                }
                _ => val,
            };
            if let Some(elem) = doc.find_element_mut(element_id) {
                elem.style_overrides.insert(prop.to_string(), val);
            }
            doc.invalidate_cache();
            return;
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
        self.documents
            .iter()
            .any(|doc| doc.find_element(element_id).is_some())
    }

    /// Get the resolved screen rect for an element
    pub fn get_rect(
        &mut self,
        element_id: &str,
        screen_w: f32,
        screen_h: f32,
    ) -> Option<(f32, f32, f32, f32)> {
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
