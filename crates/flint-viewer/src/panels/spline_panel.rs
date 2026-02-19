//! egui panel for spline control point editing in the track editor.

use crate::spline_editor::SplineEditor;

/// Actions returned by the panel that the app must handle.
pub enum SplinePanelAction {
    Save,
    Undo,
    InsertPoint(usize),
    DeletePoint(usize),
    Resample,
}

/// Draw the spline editor panel. Returns a list of actions to process.
pub fn spline_editor_panel(ui: &mut egui::Ui, editor: &mut SplineEditor) -> Vec<SplinePanelAction> {
    let mut actions = Vec::new();

    ui.heading("Track Editor");
    ui.separator();

    // Track info
    ui.label(format!("Track: {}", editor.name));
    ui.label(format!("Points: {}", editor.control_points.len()));
    ui.label(format!("Length: {:.0}m", editor.track_length()));

    ui.separator();

    // Spline properties
    let mut closed = editor.closed;
    if ui.checkbox(&mut closed, "Closed loop").changed() {
        editor.push_undo();
        editor.closed = closed;
        actions.push(SplinePanelAction::Resample);
        editor.modified = true;
    }

    let mut spacing = editor.spacing;
    ui.horizontal(|ui| {
        ui.label("Spacing:");
        if ui.add(egui::DragValue::new(&mut spacing).speed(0.1).range(0.5..=10.0)).changed() {
            editor.spacing = spacing;
            actions.push(SplinePanelAction::Resample);
        }
    });

    ui.separator();

    // Selected point editing
    if let Some(idx) = editor.selected {
        ui.heading(format!("Point {}", idx));

        let mut changed = false;
        let cp = &mut editor.control_points[idx];

        ui.horizontal(|ui| {
            ui.label("X:");
            changed |= ui
                .add(egui::DragValue::new(&mut cp.position[0]).speed(0.5))
                .changed();
        });
        ui.horizontal(|ui| {
            ui.label("Y:");
            changed |= ui
                .add(egui::DragValue::new(&mut cp.position[1]).speed(0.5))
                .changed();
        });
        ui.horizontal(|ui| {
            ui.label("Z:");
            changed |= ui
                .add(egui::DragValue::new(&mut cp.position[2]).speed(0.5))
                .changed();
        });
        ui.horizontal(|ui| {
            ui.label("Twist:");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut cp.twist)
                        .speed(1.0)
                        .suffix("\u{00b0}"),
                )
                .changed();
        });

        if changed {
            actions.push(SplinePanelAction::Resample);
            editor.modified = true;
        }

        ui.separator();

        ui.horizontal(|ui| {
            if ui.button("Insert After").clicked() {
                actions.push(SplinePanelAction::InsertPoint(idx));
            }
            if ui.button("Delete").clicked() {
                actions.push(SplinePanelAction::DeletePoint(idx));
            }
        });

        // Navigation
        ui.horizontal(|ui| {
            if ui.button("\u{25c0} Prev").clicked() {
                let n = editor.control_points.len();
                editor.selected = Some(if idx == 0 { n - 1 } else { idx - 1 });
            }
            if ui.button("Next \u{25b6}").clicked() {
                let n = editor.control_points.len();
                editor.selected = Some((idx + 1) % n);
            }
        });
    } else {
        ui.label("Click a control point to select it.");
    }

    ui.separator();

    // Actions
    ui.horizontal(|ui| {
        if ui.button("Save (Ctrl+S)").clicked() {
            actions.push(SplinePanelAction::Save);
        }
        if ui.button("Undo (Ctrl+Z)").clicked() {
            actions.push(SplinePanelAction::Undo);
        }
    });

    // Status
    if editor.modified {
        ui.colored_label(egui::Color32::YELLOW, "Unsaved changes");
    }

    ui.separator();

    // Help
    ui.collapsing("Controls", |ui| {
        ui.label("Left-click: select/drag point");
        ui.label("Alt+drag: move vertically");
        ui.label("Middle-drag: orbit camera");
        ui.label("Right-drag: pan camera");
        ui.label("Scroll: zoom");
        ui.label("Tab/Shift+Tab: cycle selection");
        ui.label("I: insert point after selected");
        ui.label("Delete: delete selected point");
        ui.label("Escape: cancel drag");
        ui.label("Ctrl+S: save");
        ui.label("Ctrl+Z: undo");
    });

    actions
}
