//! Shared egui widgets for debug panels and editors.
//!
//! Every helper returns `true` when the value changed, so callers can
//! compose them into a single `dirty` flag. The panels used to carry their
//! own copies of `drag_f32`/`drag_vec3`; they now live here (ADR 0068).

pub mod curve;

pub use curve::{CurveEditor, CurveResponse, GradientEditor};

/// Single labeled drag value. Returns true if the value changed.
pub fn drag_f32(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(
            egui::DragValue::new(value)
                .speed(speed)
                .range(range)
                .max_decimals(3),
        );
    });
    *value != before
}

/// Single labeled integer drag value. Returns true if the value changed.
pub fn drag_u32(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u32,
    range: std::ops::RangeInclusive<u32>,
) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(value).speed(1.0).range(range));
    });
    *value != before
}

/// Labeled `min`/`max` pair on one row. Keeps `min <= max`. Returns true if
/// either changed.
pub fn drag_range_f32(
    ui: &mut egui::Ui,
    label: &str,
    min: &mut f32,
    max: &mut f32,
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    let before = (*min, *max);
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(
            egui::DragValue::new(min)
                .speed(speed)
                .range(range.clone())
                .max_decimals(3),
        );
        ui.label("-");
        ui.add(
            egui::DragValue::new(max)
                .speed(speed)
                .range(range)
                .max_decimals(3),
        );
    });
    if *min > *max {
        // Whichever the user dragged wins; drag the other along.
        if (*min - before.0).abs() > (*max - before.1).abs() {
            *max = *min;
        } else {
            *min = *max;
        }
    }
    (*min, *max) != before
}

/// Labeled [f32; 3] drag row with R/G/B prefixes (for colors). Returns true if any element changed.
pub fn drag_vec3(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut [f32; 3],
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    drag_vec3_labeled(ui, label, ["R", "G", "B"], value, speed, range)
}

/// Labeled [f32; 3] drag row with X/Y/Z prefixes (for directions). Returns true if any element changed.
pub fn drag_xyz(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut [f32; 3],
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    drag_vec3_labeled(ui, label, ["X", "Y", "Z"], value, speed, range)
}

fn drag_vec3_labeled(
    ui: &mut egui::Ui,
    label: &str,
    names: [&str; 3],
    value: &mut [f32; 3],
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        ui.label(label);
        for (i, n) in names.iter().enumerate() {
            ui.label(*n);
            ui.add(
                egui::DragValue::new(&mut value[i])
                    .speed(speed)
                    .range(range.clone())
                    .max_decimals(3),
            );
        }
    });
    *value != before
}

/// Labeled [f32; 2] drag row (e.g. per-axis size). Returns true if changed.
pub fn drag_vec2(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut [f32; 2],
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        ui.label(label);
        ui.label("W");
        ui.add(
            egui::DragValue::new(&mut value[0])
                .speed(speed)
                .range(range.clone())
                .max_decimals(3),
        );
        ui.label("H");
        ui.add(
            egui::DragValue::new(&mut value[1])
                .speed(speed)
                .range(range)
                .max_decimals(3),
        );
    });
    *value != before
}

/// Labeled RGBA colour button (unmultiplied). Returns true if changed.
pub fn color_rgba(ui: &mut egui::Ui, label: &str, value: &mut [f32; 4]) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        ui.label(label);
        ui.color_edit_button_rgba_unmultiplied(value);
    });
    *value != before
}

/// Labeled combo box over string options. Returns true if the selection changed.
pub fn combo_str(ui: &mut egui::Ui, label: &str, value: &mut String, options: &[&str]) -> bool {
    let before = value.clone();
    ui.horizontal(|ui| {
        if !label.is_empty() {
            ui.label(label);
        }
        // Salt with the auto id so two combos with the same (or empty)
        // label in one panel do not share popup state.
        egui::ComboBox::from_id_salt(ui.next_auto_id().with(label))
            .selected_text(value.as_str())
            .show_ui(ui, |ui| {
                for opt in options {
                    ui.selectable_value(value, (*opt).to_string(), *opt);
                }
            });
    });
    *value != before
}

/// Labeled checkbox. Returns true if toggled.
pub fn check(ui: &mut egui::Ui, label: &str, value: &mut bool) -> bool {
    ui.checkbox(value, label).changed()
}
