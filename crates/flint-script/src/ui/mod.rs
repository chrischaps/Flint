//! Data-driven UI system: Layout / Style / Logic separation
//!
//! Elements are defined in .ui.toml (structure), styled via .style.toml (visuals),
//! and controlled from Rhai scripts (logic). The existing draw_* API continues to
//! work for procedural elements (minimap, speed lines, etc).
//!
//! It is the one structured UI (#731): buttons, lists and sliders take
//! focus, which the arrows, a pad's d-pad and stick, the mouse and touch all
//! move ([`focus`]), and what happens to them reaches the script that loaded
//! the document as `on_ui(element_id, event)`.
//!
//! Style files may reference named tokens (`"$accent"`) from a local
//! `[tokens]` table or a shared token file — see [`tokens`]. Loaded
//! documents remember their source files and [`UiSystem::poll_reload`]
//! re-parses them in place when any changes, keeping script overrides.

mod draw;
pub mod element;
pub mod focus;
pub mod layout;
pub mod loader;
pub mod scroll;
pub mod style;
pub mod text_input;
pub mod tokens;

#[cfg(test)]
mod focus_tests;
#[cfg(test)]
mod scroll_tests;
#[cfg(test)]
mod tests;

use crate::context::{DrawItem, TextMeasurer};
use element::{ElementType, StyleValue, UiElement};
use flint_core::EntityId;
use layout::{LayoutInputs, ResolvedRect};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use style::{ResolvedStyle, States, StyleClass};
use tokens::TokenSet;

/// Modification fingerprint of one source file: (mtime, length). `None`
/// when the file is missing so that appearing/disappearing counts as a change.
/// Hot reload watches the disk, through `flint_core::file_watch`, which in the
/// player never waits on it (#633).
type FileStamp = Option<flint_core::file_watch::Stamp>;

fn stamp(path: &Path) -> FileStamp {
    flint_core::file_watch::stamp(path)
}

/// Which element has focus and which a pointer holds down: what an
/// element's state is drawn from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Interaction {
    pub focus: Option<String>,
    pub pressed: Option<String>,
}

impl Interaction {
    pub fn states(&self, e: &UiElement) -> States {
        States {
            focus: self.focus.as_deref() == Some(e.id.as_str()),
            pressed: self.pressed.as_deref() == Some(e.id.as_str()),
            disabled: !e.enabled,
        }
    }
}

/// A loaded UI document (one layout + style pair)
pub struct UiDocument {
    pub elements: Vec<UiElement>,
    pub styles: HashMap<String, StyleClass>,
    /// Tokens the style file resolved against (local `[tokens]` + shared file)
    pub tokens: TokenSet,
    /// The entity whose script loaded it: who hears its `on_ui` events.
    pub owner: Option<EntityId>,
    /// Cached layout results, dropped when anything they depend on changes
    cached_rects: HashMap<String, (ResolvedRect, ResolvedStyle)>,
    /// The clip of each element inside a scrolling list, and each scrolling
    /// list's box, from the same layout (#1229)
    cached_clips: HashMap<String, [f32; 4]>,
    cached_scrolls: HashMap<String, layout::ScrollBox>,
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
    enabled: bool,
    value: f32,
    caret: usize,
    scroll_offset: f32,
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
            enabled: e.enabled,
            value: e.value,
            caret: e.caret,
            scroll_offset: e.scroll_offset,
            text_override: e.text_override.clone(),
            color_override: e.color_override,
            bg_color_override: e.bg_color_override,
            class_override: e.class_override.clone(),
            style_overrides: e.style_overrides.clone(),
        }
    }

    fn apply(self, e: &mut UiElement) {
        e.visible = self.visible;
        e.enabled = self.enabled;
        e.value = self.value;
        e.caret = self.caret;
        e.scroll_offset = self.scroll_offset;
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

/// Parse layout + style + tokens, and check that every class an element
/// names is one the style file has. Every problem is a line naming the file
/// and the element or class.
fn parse_document(layout_path: &Path, root_dir: &Path) -> Result<ParsedDocument, Vec<String>> {
    let layout = loader::load_layout(layout_path)?;

    let mut styles = HashMap::new();
    let mut tokens = TokenSet::default();
    let mut style_path = None;
    if !layout.style.is_empty() {
        let sp = root_dir.join(&layout.style);
        let sheet = loader::load_styles(&sp, root_dir)?;
        styles = sheet.classes;
        tokens = sheet.tokens;
        style_path = Some(sp);
    }

    let shown = layout_path.display();
    let mut problems = Vec::new();
    for e in &layout.elements {
        let names = [(&e.class, "class"), (&e.item_class, "item_class")];
        for (class, key) in names {
            if class.is_empty() || styles.contains_key(class.as_str()) {
                continue;
            }
            let id = if e.made_by_list {
                e.parent_id.as_deref().unwrap_or(&e.id)
            } else {
                &e.id
            };
            let why = if layout.style.is_empty() {
                "the layout names no style file ([ui] style)".to_string()
            } else {
                format!("{} has no [styles.{class}]", layout.style)
            };
            problems.push(format!("{shown}: element `{id}`: {key} `{class}`: {why}"));
        }
    }
    problems.dedup();
    if !problems.is_empty() {
        return Err(problems);
    }

    Ok(ParsedDocument {
        elements: layout.elements,
        styles,
        tokens,
        style_path,
    })
}

/// Every problem a layout (and the style file it names) has, without
/// loading it: what `flint check` reports for each `.ui.toml` (#731).
/// `root_dir` is where its paths resolve, the project root.
pub fn check_document(layout_path: &Path, root_dir: &Path) -> Vec<String> {
    match parse_document(layout_path, root_dir) {
        Ok(_) => Vec::new(),
        Err(problems) => problems,
    }
}

/// [`check_document`] with each problem split into the file it is in (the
/// layout or its style file) and what is wrong there.
pub fn check_document_at(layout_path: &Path, root_dir: &Path) -> Vec<(PathBuf, String)> {
    check_document(layout_path, root_dir)
        .into_iter()
        .map(|problem| match problem.split_once(": ") {
            Some((file, what)) => (PathBuf::from(file), what.to_string()),
            None => (layout_path.to_path_buf(), problem),
        })
        .collect()
}

impl UiDocument {
    /// Resolve layout if it was dropped or the screen size changed
    fn ensure_layout(
        &mut self,
        screen_w: f32,
        screen_h: f32,
        interaction: &Interaction,
        measurer: Option<&TextMeasurer>,
    ) {
        if (self.cached_screen_w - screen_w).abs() > 0.5
            || (self.cached_screen_h - screen_h).abs() > 0.5
            || self.cached_rects.is_empty()
        {
            let states = |e: &UiElement| interaction.states(e);
            let inputs = LayoutInputs {
                measurer,
                states: &states,
            };
            let laid = layout::lay_out(&self.elements, &self.styles, screen_w, screen_h, &inputs);
            self.cached_rects = laid.rects;
            self.cached_clips = laid.clips;
            self.cached_scrolls = laid.scrolls;
            self.cached_screen_w = screen_w;
            self.cached_screen_h = screen_h;
        }
    }

    /// Invalidate cached layout (call after element structure changes)
    fn invalidate_cache(&mut self) {
        self.cached_rects.clear();
        self.cached_clips.clear();
        self.cached_scrolls.clear();
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
                return Err(e.join("\n"));
            }
        };

        let saved: HashMap<String, ElementOverrides> = self
            .elements
            .iter()
            .map(|e| (e.id.clone(), ElementOverrides::capture(e)))
            .collect();
        let items: HashMap<String, Vec<String>> = self
            .elements
            .iter()
            .filter(|e| e.element_type == ElementType::List)
            .map(|e| (e.id.clone(), e.items.clone()))
            .collect();

        self.elements = parsed.elements;
        self.styles = parsed.styles;
        self.tokens = parsed.tokens;
        self.style_path = parsed.style_path;

        // Items a script set stay set
        for (list, items) in items {
            if self
                .find_element(&list)
                .is_some_and(|l| l.element_type == ElementType::List && l.items != items)
            {
                self.replace_items(&list, items);
            }
        }

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

    /// Whether an element and every element above it are visible.
    pub fn is_shown(&self, id: &str) -> bool {
        let mut at = self.find_element(id);
        for _ in 0..=self.elements.len() {
            match at {
                Some(e) if e.visible => match &e.parent_id {
                    Some(p) => at = self.find_element(p),
                    None => return true,
                },
                _ => return false,
            }
        }
        false
    }

    /// Replace a list's items: its made buttons go, new ones are made after
    /// it, and its value stays inside the new count.
    fn replace_items(&mut self, list_id: &str, items: Vec<String>) {
        self.elements
            .retain(|e| !(e.made_by_list && e.parent_id.as_deref() == Some(list_id)));
        let Some(at) = self.elements.iter().position(|e| e.id == list_id) else {
            return;
        };
        self.elements[at].items = items;
        let count = self.elements[at].items.len();
        if count > 0 {
            self.elements[at].value = self.elements[at].value.clamp(0.0, (count - 1) as f32);
        } else {
            self.elements[at].value = 0.0;
        }
        let made = loader::list_items(&self.elements[at]);
        let tail = self.elements.split_off(at + 1);
        self.elements.extend(made);
        self.elements.extend(tail);
        loader::link(&mut self.elements);
        self.invalidate_cache();
    }
}

/// What happened to an element, for its document's owner's `on_ui`.
#[derive(Debug, Clone, PartialEq)]
pub struct UiEvent {
    /// The entity whose script loaded the document.
    pub owner: Option<EntityId>,
    /// The element: a list's own id for its made items.
    pub element: String,
    /// `focus`, `blur`, `activate`, `change` or `back`.
    pub event: &'static str,
}

/// The top-level UI system that holds all loaded documents
pub struct UiSystem {
    documents: Vec<UiDocument>,
    next_handle: i64,
    handle_map: HashMap<i64, usize>, // handle → index in documents
    /// (element id, property) pairs already reported as unknown
    warned_props: HashSet<(String, String)>,
    /// Focus and pointer press, across every document
    interaction: Interaction,
    /// What happened since the engine last took them ([`Self::take_events`])
    events: Vec<UiEvent>,
    /// The host's text measurer, which layout sizes labels with
    measurer: Option<TextMeasurer>,
    /// The pointer and the pad between frames (repeat, drags, releases)
    pointer: focus::PointerState,
    /// Focus or a press moved since layout last ran: the next draw or rect
    /// lays out again, in the new states' styles
    restyle: bool,
    /// Focus moved since the scrolling lists last settled: the next layout
    /// brings the focused element into view (#1229)
    reveal: bool,
}

impl UiSystem {
    pub fn new() -> Self {
        Self {
            documents: Vec::new(),
            next_handle: 1,
            handle_map: HashMap::new(),
            warned_props: HashSet::new(),
            interaction: Interaction::default(),
            events: Vec::new(),
            measurer: None,
            pointer: focus::PointerState::default(),
            restyle: false,
            reveal: false,
        }
    }

    /// Drop every layout when a state changed since it ran.
    fn restyle_if_needed(&mut self) {
        if std::mem::take(&mut self.restyle) {
            self.invalidate_all();
        }
    }

    /// The text measurer layout sizes labels with: the host's, the one
    /// `measure_text` uses.
    pub fn set_measurer(&mut self, measurer: Option<TextMeasurer>) {
        self.measurer = measurer;
        self.invalidate_all();
    }

    fn invalidate_all(&mut self) {
        for doc in &mut self.documents {
            doc.invalidate_cache();
        }
    }

    /// Load a UI document from a layout file path, for `owner`'s script.
    /// The style path is read from the `[ui].style` field in the layout file.
    /// Returns a handle for future operations, or every problem the layout
    /// and its style file have, each naming the file and the element.
    pub fn load(
        &mut self,
        layout_path: &str,
        scene_dir: &Path,
        owner: Option<EntityId>,
    ) -> Result<i64, String> {
        let layout_file = scene_dir.join(layout_path);

        let parsed = parse_document(&layout_file, scene_dir).map_err(|problems| {
            format!(
                "load_ui(\"{layout_path}\"): {}",
                if problems.len() == 1 {
                    problems[0].clone()
                } else {
                    format!("{} problems:\n  {}", problems.len(), problems.join("\n  "))
                }
            )
        })?;

        let mut doc = UiDocument {
            elements: parsed.elements,
            styles: parsed.styles,
            tokens: parsed.tokens,
            owner,
            cached_rects: HashMap::new(),
            cached_clips: HashMap::new(),
            cached_scrolls: HashMap::new(),
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

        tracing::info!(target: "script", "UI loaded: {} (handle {})", layout_path, handle);
        Ok(handle)
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
                let doc = self.documents.remove(idx);
                for held in [&mut self.interaction.focus, &mut self.interaction.pressed] {
                    if held
                        .as_deref()
                        .is_some_and(|id| doc.find_element(id).is_some())
                    {
                        *held = None;
                    }
                }
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

    /// The document an element is in, and the element.
    fn with_element(&mut self, element_id: &str, f: impl FnOnce(&mut UiElement)) -> bool {
        for doc in &mut self.documents {
            if let Some(elem) = doc.find_element_mut(element_id) {
                f(elem);
                doc.invalidate_cache();
                return true;
            }
        }
        false
    }

    /// Set text content of an element (searches all documents); a text
    /// input's caret goes after it.
    pub fn set_text(&mut self, element_id: &str, text: &str) {
        self.with_element(element_id, |e| {
            e.text_override = Some(text.to_string());
            if e.element_type == ElementType::TextInput {
                e.caret = text.chars().count();
            }
        });
    }

    /// Show an element
    pub fn show(&mut self, element_id: &str) {
        self.set_visible(element_id, true);
    }

    /// Hide an element
    pub fn hide(&mut self, element_id: &str) {
        self.set_visible(element_id, false);
    }

    /// Set visibility of an element
    pub fn set_visible(&mut self, element_id: &str, visible: bool) {
        self.with_element(element_id, |e| e.visible = visible);
    }

    /// Let focus reach an element or not; a disabled one draws its class's
    /// `disabled` state.
    pub fn set_enabled(&mut self, element_id: &str, enabled: bool) {
        self.with_element(element_id, |e| e.enabled = enabled);
    }

    /// Override primary color of an element
    pub fn set_color(&mut self, element_id: &str, r: f32, g: f32, b: f32, a: f32) {
        self.with_element(element_id, |e| e.color_override = Some([r, g, b, a]));
    }

    /// Override background color of an element
    pub fn set_bg_color(&mut self, element_id: &str, r: f32, g: f32, b: f32, a: f32) {
        self.with_element(element_id, |e| e.bg_color_override = Some([r, g, b, a]));
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
        self.with_element(element_id, |e| e.reset_overrides());
    }

    /// Switch an element's style class
    pub fn set_class(&mut self, element_id: &str, class: &str) {
        self.with_element(element_id, |e| e.class_override = Some(class.to_string()));
    }

    /// Whether an element is drawn: it and every element above it visible.
    pub fn is_shown(&self, element_id: &str) -> bool {
        self.documents.iter().any(|doc| doc.is_shown(element_id))
    }

    /// The text an element shows (a script's, else its layout's).
    pub fn text(&self, element_id: &str) -> Option<String> {
        self.documents
            .iter()
            .find_map(|doc| doc.find_element(element_id))
            .map(|e| e.effective_text().to_string())
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
        self.restyle_if_needed();
        self.lay_out_settled((screen_w, screen_h));
        self.documents
            .iter()
            .find_map(|doc| doc.cached_rects.get(element_id))
            .map(|(rect, _)| (rect.x, rect.y, rect.w, rect.h))
    }

    /// Lay out every document, then settle its scrolling lists (#1229).
    fn lay_out_settled(&mut self, screen: (f32, f32)) {
        let measurer = self.measurer.as_ref();
        for doc in &mut self.documents {
            doc.ensure_layout(screen.0, screen.1, &self.interaction, measurer);
        }
        self.settle_scrolls(screen);
    }

    /// Generate all UI draw commands for all loaded documents, each with
    /// the clip of the scrolling lists it is inside (#1229).
    pub fn generate_draw_commands(&mut self, screen_w: f32, screen_h: f32) -> Vec<DrawItem> {
        self.restyle_if_needed();
        self.lay_out_settled((screen_w, screen_h));
        let measurer = self.measurer.as_ref();
        let focus = self.interaction.focus.as_deref();
        let mut all_commands = Vec::new();
        for doc in &self.documents {
            all_commands.extend(draw::commands(doc, measurer, focus));
        }
        all_commands
    }

    /// What happened to elements since the last take, oldest first.
    pub fn take_events(&mut self) -> Vec<UiEvent> {
        std::mem::take(&mut self.events)
    }

    /// Clear all loaded documents (for scene transitions)
    pub fn clear(&mut self) {
        self.documents.clear();
        self.handle_map.clear();
        self.next_handle = 1;
        self.interaction = Interaction::default();
        self.events.clear();
        self.pointer = focus::PointerState::default();
        self.restyle = false;
        self.reveal = false;
    }
}

impl Default for UiSystem {
    fn default() -> Self {
        Self::new()
    }
}
