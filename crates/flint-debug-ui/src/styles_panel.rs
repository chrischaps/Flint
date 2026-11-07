//! Styles panel (F3, #268): the scene's stylized look among the other
//! scene panels. The same [`StylesEditor`] the F4 Rendering & Effects menu
//! draws, plus the scene's styled entities: each entity with a
//! `style_mask` component, the style it asks for, and the hue the mask
//! view (`--debug-mode style`) paints that style in.
//!
//! The host creates it when the scene uses a style (a
//! `[post_process.styles]` table or a `style_mask` entity), mirrors the
//! renderer's config into it while it is clean, and while it is dirty
//! copies its `pp.styles` (and nothing else) back into the renderer's
//! config, as the F4 panel does for the whole config (ADR 0053).

use crate::styles_ui::{StylesEdit, StylesEditor};
use crate::DebugPanel;
use flint_render::stylize::StyleSettings;
use flint_render::PostProcessConfig;

pub const STYLES_DEBUG_PANEL: &str = "Styles";

/// One styled entity, as the host found it.
#[derive(Debug, Clone, PartialEq)]
pub struct StyledEntityRow {
    pub name: String,
    /// The style its `style_mask` names.
    pub style: String,
    /// The component's `enabled`.
    pub enabled: bool,
    /// The mask view's hue for the style's id this frame, when it has one.
    pub color: Option<[f32; 3]>,
}

pub struct StylesDebugPanel {
    /// The renderer's config, mirrored; only `styles` is written back.
    pub pp: PostProcessConfig,
    authored: Option<StyleSettings>,
    entities: Vec<StyledEntityRow>,
    editor: StylesEditor,
    edit: StylesEdit,
    open: bool,
    dirty: bool,
}

impl StylesDebugPanel {
    /// `authored`: the scene's own `[post_process.styles]`, for the
    /// editor's Authored button and its markers.
    pub fn new(authored: Option<StyleSettings>) -> Self {
        Self {
            pp: PostProcessConfig::default(),
            authored,
            entities: Vec::new(),
            editor: StylesEditor::default(),
            edit: StylesEdit::default(),
            open: false,
            dirty: false,
        }
    }

    /// Mirror the renderer's config and the styled entities. No-op on the
    /// config while there are unapplied edits.
    pub fn refresh(&mut self, pp: &PostProcessConfig, entities: Vec<StyledEntityRow>) {
        if !self.dirty {
            self.pp = pp.clone();
        }
        self.entities = entities;
    }

    /// The scene's folder, where the editor finds the scene's own palettes
    /// and glyph sets to offer (#322).
    pub fn with_scene_dir(mut self, scene_dir: Option<std::path::PathBuf>) -> Self {
        self.editor.set_scene_dir(scene_dir);
        self
    }

    /// What the last edit asks of the host.
    pub fn take_edit(&mut self) -> StylesEdit {
        std::mem::take(&mut self.edit)
    }

    /// The styled entities shown, as last refreshed.
    pub fn entities(&self) -> &[StyledEntityRow] {
        &self.entities
    }
}

impl DebugPanel for StylesDebugPanel {
    fn name(&self) -> &str {
        STYLES_DEBUG_PANEL
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let edit = self.editor.ui(ui, &mut self.pp, self.authored.as_ref());
        if edit.changed {
            self.edit.changed = true;
            self.edit.needs_resources |= edit.needs_resources;
            self.dirty = true;
        }
        ui.separator();
        egui::CollapsingHeader::new(format!("Styled entities ({})", self.entities.len()))
            .default_open(!self.entities.is_empty())
            .show(ui, |ui| {
                if self.entities.is_empty() {
                    ui.weak("No entity has a style_mask: styles apply to the whole frame");
                }
                egui::Grid::new("styled entities")
                    .striped(true)
                    .min_col_width(0.0)
                    .show(ui, |ui| {
                        for row in &self.entities {
                            match row.color {
                                Some([r, g, b]) => {
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(10.0, 10.0),
                                        egui::Sense::hover(),
                                    );
                                    // The mask view paints these display values
                                    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                                    ui.painter().rect_filled(
                                        rect,
                                        2.0,
                                        egui::Color32::from_rgb(byte(r), byte(g), byte(b)),
                                    );
                                }
                                None => {
                                    ui.label("");
                                }
                            }
                            ui.label(&row.name);
                            let style = if row.enabled {
                                egui::RichText::new(&row.style)
                            } else {
                                egui::RichText::new(format!("{} (off)", row.style)).weak()
                            };
                            ui.label(style);
                            ui.end_row();
                        }
                    });
                if self.entities.iter().any(|r| r.color.is_some()) {
                    ui.weak(
                        "Swatches: the mask view's hue for each style (Debug mode -> Style Mask)",
                    );
                }
            });
    }

    fn is_open(&self) -> bool {
        self.open
    }
    fn toggle(&mut self) {
        self.open = !self.open;
    }
    fn is_dirty(&self) -> bool {
        self.dirty
    }
    fn clear_dirty(&mut self) {
        self.dirty = false;
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
