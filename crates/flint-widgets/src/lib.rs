//! The kernel's egui (#1583): the widgets every window draws with, a test's
//! headless frame, and the renderer's own controls.
//!
//! - **The widgets**: the helpers below, the curve and gradient editors
//!   ([`curve`]) and the frame-time graph ([`frame_graph`]). `flint frame
//!   open` (its frame and profiler tabs), the particle editor, the viewer's F2
//!   and the debug HUD's panels all draw with them.
//! - **[`run_headless_frame`]**: one egui frame with nothing to paint it,
//!   which every test that runs a frame goes through (#944).
//! - **The renderer's controls**: the Rendering & Effects panel
//!   ([`RenderDebugPanel`], ADR 0053), the style editor ([`StylesEditor`])
//!   and the corner cards for styles that fail ([`paint_style_errors`],
//!   [`paint_style_drops`]). The scene viewer's F4 is this panel, and the
//!   player's F4 is the same one, shown by the debug-hud package.
//!
//! They were the debug-hud package's crate until #1583 (flint-debug-ui,
//! which re-exports them at their old paths), so a game that leaves the
//! package out still linked it into its `flint` and its editors. Now the CLI
//! and the viewer name no package for them.
//!
//! Every helper returns `true` when the value changed, so callers can
//! compose them into a single `dirty` flag. The panels used to carry their
//! own copies of `drag_f32`/`drag_vec3`; they now live here (ADR 0068).

pub mod curve;
pub mod frame_graph;
mod headless_frame;
mod render_panel;
mod style_drops;
mod style_errors;
mod styles_ui;

pub use curve::{CurveEditor, CurveResponse, GradientEditor};
pub use frame_graph::{
    band_legend, band_legend_of, hitch_counters, Band, FrameGraph, GraphFrame, GraphLayout,
    HitchCounters, Placed, BANDS, HITCH_COLOR,
};
pub use headless_frame::run_headless_frame;
pub use render_panel::{RenderDebugPanel, RenderPanelFlags, RENDER_DEBUG_PANEL};
pub use style_drops::paint_style_drops;
pub use style_errors::paint_style_errors;
pub use styles_ui::{StylesEdit, StylesEditor};

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

/// A collapsing section of a panel, open at first if `default_open`.
pub fn section(
    ui: &mut egui::Ui,
    title: &str,
    default_open: bool,
    add: impl FnOnce(&mut egui::Ui),
) {
    egui::CollapsingHeader::new(title)
        .default_open(default_open)
        .show(ui, add);
}

/// A slider that reads `mixed` when what it stands for disagrees (several
/// props, several entities), saying `says` when hovered, as it does unmixed
/// when `says` isn't empty. Returns whether a hand moved it.
pub fn mixed_slider(ui: &mut egui::Ui, slider: egui::Slider<'_>, mixed: bool, says: &str) -> bool {
    let slider = if mixed {
        slider.custom_formatter(|_, _| "mixed".to_string())
    } else {
        slider
    };
    let response = ui.add(slider);
    let changed = response.changed();
    if !says.is_empty() {
        response.on_hover_text(says);
    }
    changed
}

/// Labeled checkbox. Returns true if toggled.
pub fn check(ui: &mut egui::Ui, label: &str, value: &mut bool) -> bool {
    ui.checkbox(value, label).changed()
}
