//! The scene's diagnostics in the viewer (ADR 0115 decision 1, #795): the
//! list `flint validate` prints, in the status panel, refreshed on every
//! load and hot reload. A row that names an entity selects it on click.
//!
//! It is the same pass as validate's ([`flint_scene::diagnose_all`]), so
//! `[project] strict` holds here as it does there, and what the project's
//! `[project.diagnostics] allow` names is left out and counted. A reload
//! that fails (the file stopped being TOML) keeps the last world on screen,
//! and this list says why.

use flint_core::diagnostic::{self, Diagnostic, Severity};
use flint_core::project::Project;
use flint_schema::SchemaRegistry;
use std::path::Path;

const ERROR: egui::Color32 = egui::Color32::from_rgb(236, 104, 92);
const WARNING: egui::Color32 = egui::Color32::from_rgb(230, 180, 72);
const CLEAN: egui::Color32 = egui::Color32::from_rgb(110, 200, 120);

/// What the panel shows: the scene's diagnostics, errors first, and how
/// many the project allowed.
#[derive(Debug, Default)]
pub struct DiagnosticsPanel {
    list: Vec<Diagnostic>,
    allowed: usize,
    /// The file the list is about, so a diagnostic about another one (a
    /// prefab, a schema file) can say so.
    scene: Option<std::path::PathBuf>,
}

impl DiagnosticsPanel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Diagnose the scene at `scene` again, as `flint validate` would.
    pub fn refresh(&mut self, scene: &Path, registry: &SchemaRegistry) {
        let project = Project::for_scene(scene);
        let list = flint_scene::diagnose_all(scene, registry, &project);
        self.set(scene, list, &project);
    }

    /// Show `list` for `scene`, less what `project` allows.
    fn set(&mut self, scene: &Path, mut list: Vec<Diagnostic>, project: &Project) {
        self.allowed = diagnostic::drop_allowed(&mut list, &project.allow);
        diagnostic::sort_errors_first(&mut list);
        self.list = list;
        self.scene = Some(scene.to_path_buf());
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.list
    }

    pub fn allowed(&self) -> usize {
        self.allowed
    }

    /// `2 errors, 1 warning (1 allowed by flint.toml)`, or `no problems`.
    pub fn counts(&self) -> String {
        format!(
            "{}{}",
            diagnostic::counts_line(&self.list),
            diagnostic::allowed_note(self.allowed)
        )
    }

    /// The one-line count for the status bar, in the colour of the worst.
    pub fn summary_ui(&self, ui: &mut egui::Ui) {
        let colour = if diagnostic::has_errors(&self.list) {
            ERROR
        } else if self.list.is_empty() {
            CLEAN
        } else {
            WARNING
        };
        ui.colored_label(colour, format!("Scene: {}", self.counts()))
            .on_hover_text("What `flint validate` says of this scene; refreshed on every reload");
    }

    /// Every diagnostic, a row each: severity, code, where, what, and the
    /// hint beneath. Returns the entity of the row clicked, if any.
    pub fn ui(&self, ui: &mut egui::Ui, max_height: f32) -> Option<String> {
        if self.list.is_empty() {
            return None;
        }
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .id_salt("scene_diagnostics")
            .max_height(max_height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for d in &self.list {
                    if let Some(entity) = self.row(ui, d) {
                        clicked = Some(entity);
                    }
                }
            });
        clicked
    }

    fn row(&self, ui: &mut egui::Ui, d: &Diagnostic) -> Option<String> {
        let mut clicked = None;
        let colour = match d.severity {
            Severity::Error => ERROR,
            Severity::Warning => WARNING,
        };
        // The severity, code, file and place are each kept whole on a line;
        // only the message wraps
        let whole =
            |ui: &mut egui::Ui, text: egui::RichText| ui.add(egui::Label::new(text).extend());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            whole(
                ui,
                egui::RichText::new(format!("{:<7}", d.severity.as_str()))
                    .color(colour)
                    .strong()
                    .monospace(),
            );
            whole(ui, egui::RichText::new(d.code).monospace().weak());
            if let Some(file) = self.elsewhere(d) {
                whole(ui, egui::RichText::new(file).monospace().weak());
            }
            let place = d.location();
            match &d.entity {
                Some(entity) => {
                    let link = egui::RichText::new(&place)
                        .monospace()
                        .underline()
                        .color(ui.visuals().hyperlink_color);
                    let response = ui
                        .add(egui::Label::new(link).extend().sense(egui::Sense::click()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(format!("select '{entity}'"));
                    if response.clicked() {
                        clicked = Some(entity.clone());
                    }
                }
                None if !place.is_empty() => {
                    whole(ui, egui::RichText::new(&place).monospace());
                }
                None => {}
            }
            // A parse error's message points at a column: keep it monospace
            // so the caret lands under the place
            if d.message.contains('\n') {
                ui.label(egui::RichText::new(&d.message).monospace());
            } else {
                ui.label(&d.message);
            }
        });
        if let Some(hint) = &d.hint {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(18.0);
                ui.label(
                    egui::RichText::new(format!("hint: {hint}"))
                        .italics()
                        .weak(),
                );
            });
        }
        clicked
    }

    /// The file a diagnostic is about, when it is not the scene.
    fn elsewhere(&self, d: &Diagnostic) -> Option<String> {
        let file = d.file.as_deref()?;
        let same = self
            .scene
            .as_deref()
            .is_some_and(|scene| flint_core::project::same_path(file, scene));
        (!same).then(|| flint_core::project::shown(file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::diagnostic::{codes, Allow};

    fn project_allowing(entries: &[&str]) -> Project {
        let mut project = Project::for_scene(Path::new("nowhere-795/a.scene.toml"));
        project.allow = entries.iter().map(|e| Allow::parse(e).unwrap()).collect();
        project
    }

    #[test]
    fn errors_come_first_and_the_allowed_are_counted() {
        let list = vec![
            Diagnostic::warning(codes::UNKNOWN_FIELD, "no field `wetness`")
                .entity("road")
                .component("material")
                .field("wetness"),
            Diagnostic::warning(codes::UNKNOWN_FIELD, "no field `intensty`")
                .entity("sun")
                .component("light")
                .field("intensty"),
            Diagnostic::error(codes::UNKNOWN_COMPONENT, "no component named `point_light`")
                .entity("lamp")
                .component("point_light"),
        ];
        let mut panel = DiagnosticsPanel::new();
        let scene = Path::new("a.scene.toml");
        panel.set(
            scene,
            list,
            &project_allowing(&["unknown-field:material.wetness"]),
        );
        let codes_shown: Vec<&str> = panel.diagnostics().iter().map(|d| d.code).collect();
        assert_eq!(
            codes_shown,
            [codes::UNKNOWN_COMPONENT, codes::UNKNOWN_FIELD]
        );
        assert_eq!(panel.allowed(), 1);
        assert_eq!(
            panel.counts(),
            "1 error, 1 warning (1 allowed by flint.toml)"
        );

        panel.set(scene, Vec::new(), &project_allowing(&[]));
        assert_eq!(panel.counts(), "no problems");
    }

    #[test]
    fn a_diagnostic_about_another_file_names_it() {
        let mut panel = DiagnosticsPanel::new();
        panel.set(
            Path::new("scenes/a.scene.toml"),
            Vec::new(),
            &project_allowing(&[]),
        );
        let here = Diagnostic::warning(codes::UNKNOWN_FIELD, "m").file("scenes/a.scene.toml");
        let there =
            Diagnostic::warning(codes::OLD_TYPE_NAME, "m").file("schemas/components/x.toml");
        assert_eq!(panel.elsewhere(&here), None);
        assert_eq!(
            panel.elsewhere(&there).as_deref(),
            Some("schemas/components/x.toml")
        );
    }

    /// The panel draws every row, hints and all, in a frame of its own.
    #[test]
    fn the_panel_draws_its_rows() {
        let mut panel = DiagnosticsPanel::new();
        panel.set(
            Path::new("a.scene.toml"),
            vec![
                Diagnostic::error(codes::UNKNOWN_COMPONENT, "no component named `point_light`")
                    .entity("lamp")
                    .component("point_light")
                    .hint("a light is a `light` component with type = \"point\""),
                Diagnostic::warning(codes::UNKNOWN_TABLE, "no scene table named `lights`")
                    .table("lights"),
            ],
            &project_allowing(&[]),
        );
        let ctx = egui::Context::default();
        let mut clicked = Some(String::from("not drawn"));
        // No GPU here: the font atlas this frame makes is dropped (#944)
        #[allow(clippy::disallowed_methods)]
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            panel.summary_ui(ui);
            clicked = panel.ui(ui, 200.0);
        });
        output.textures_delta.clear();
        assert_eq!(clicked, None, "nothing was clicked");
        assert!(!output.shapes.is_empty(), "the rows were drawn");
    }
}
