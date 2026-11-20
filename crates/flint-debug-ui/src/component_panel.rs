//! A component's F3 panel, drawn from its schema (#744).
//!
//! What a game used to patch into the engine (a status line, a doorbell,
//! a few dials for a script-driven controller) is written beside the
//! component's fields instead: `panel = "status" | "tune" | "trigger"`
//! (`flint_schema::panel` has the keys). The player makes one
//! [`ComponentPanel`] for each entity that carries such a component.
//!
//! The panel is a live view of the component: each frame [`sync`] writes
//! what the widgets changed into the world and reads the component back, so
//! a field the script moves (the time of day while it advances) is what the
//! slider shows, and a trigger is a single write the script clears. Reset
//! writes back the tune fields the scene loaded with; Commit to File writes
//! them into the scene file.
//!
//! [`sync`]: crate::DebugPanel::sync

use crate::{DebugPanel, PanelCtx};
use flint_ecs::FlintWorld;
use flint_schema::panel::number_of;
use flint_schema::{PanelRole, PanelRow, PanelSpec, PanelValue};
use std::path::PathBuf;

pub struct ComponentPanel {
    spec: PanelSpec,
    component: String,
    entity: String,
    title: String,
    open: bool,
    /// The component as the last sync read it (empty while closed).
    live: toml::Table,
    /// The tune fields that Commit writes, as the scene loaded them: what
    /// Reset puts back.
    original: Vec<(String, toml::Value)>,
    /// Field writes the widgets made, for the next sync.
    writes: Vec<(String, toml::Value)>,
    /// Where Commit to File writes; `None` when the scene is not a file on
    /// the disk (a bundle, the web).
    scene_file: Option<PathBuf>,
    /// What the last commit did, and whether it worked.
    note: Option<(String, bool)>,
}

impl ComponentPanel {
    /// The panel of `component` on `entity`, whose table is `loaded` as
    /// the scene loaded it. `title` is the panel's heading and egui id, so
    /// it is unique among the frame's panels (the player adds the entity's
    /// name when several carry the component).
    pub fn new(
        spec: PanelSpec,
        component: &str,
        entity: &str,
        title: String,
        loaded: Option<&toml::Value>,
        scene_file: Option<PathBuf>,
    ) -> Self {
        let original = spec
            .rows_with(PanelRole::Tune)
            .filter(|row| row.commit)
            .filter_map(|row| {
                let value = loaded?.get(&row.field)?.clone();
                Some((row.field.clone(), value))
            })
            .collect();
        Self {
            spec,
            component: component.to_string(),
            entity: entity.to_string(),
            title,
            open: false,
            live: toml::Table::new(),
            original,
            writes: Vec::new(),
            scene_file,
            note: None,
        }
    }

    pub fn component(&self) -> &str {
        &self.component
    }

    pub fn entity_name(&self) -> &str {
        &self.entity
    }

    /// The part of [`sync`](DebugPanel::sync) that needs only the world:
    /// write what the widgets changed, then read the component back while
    /// the panel is open.
    pub fn sync_world(&mut self, world: &mut FlintWorld) {
        let Some(id) = world.get_id(&self.entity) else {
            self.writes.clear();
            return;
        };
        if !self.writes.is_empty() {
            if let Some(mut comps) = world.get_components_mut(id) {
                for (field, value) in self.writes.drain(..) {
                    comps.set_field(&self.component, &field, value);
                }
            }
        }
        if self.open {
            self.live = world
                .get_components(id)
                .and_then(|c| c.get(&self.component))
                .and_then(|v| v.as_table())
                .cloned()
                .unwrap_or_default();
        }
    }

    /// The status line as it reads now: each status part, `  ·  ` between.
    pub fn status_line(&self) -> String {
        self.spec
            .rows
            .iter()
            .filter(|row| row.on_status_line())
            .filter_map(|row| row.status_text(self.live.get(&row.field)))
            .collect::<Vec<_>>()
            .join("  ·  ")
    }

    /// What a trigger's button does: queue its `press` (or, for a trigger
    /// with labels, the index `pick`) for the next sync. False when `field`
    /// is no trigger of the panel.
    pub fn press(&mut self, field: &str, pick: Option<usize>) -> bool {
        let Some(row) = self
            .spec
            .rows_with(PanelRole::Trigger)
            .find(|r| r.field == field)
            .cloned()
        else {
            return false;
        };
        let value = match pick {
            Some(i) => number_value(&row, i as f64),
            None => row.press.clone(),
        };
        self.write(field, value);
        true
    }

    fn write(&mut self, field: &str, value: toml::Value) {
        self.live.insert(field.to_string(), value.clone());
        self.writes.retain(|(f, _)| f != field);
        self.writes.push((field.to_string(), value));
    }

    fn number(&self, row: &PanelRow) -> f64 {
        self.live.get(&row.field).and_then(number_of).unwrap_or(0.0)
    }

    fn commit_to_file(&self) -> Result<PathBuf, String> {
        let Some(path) = &self.scene_file else {
            return Err("the scene is not a file on the disk".into());
        };
        let mut doc = flint_scene::SceneDocument::from_file(path)?;
        for row in self.spec.rows_with(PanelRole::Tune).filter(|r| r.commit) {
            if let Some(value) = self.live.get(&row.field) {
                doc.patch_field(&self.entity, &self.component, &row.field, value)?;
            }
        }
        doc.save(path)?;
        Ok(path.clone())
    }

    fn triggers_ui(&mut self, ui: &mut egui::Ui) {
        let rows: Vec<PanelRow> = self.spec.rows_with(PanelRole::Trigger).cloned().collect();
        if rows.is_empty() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for row in &rows {
                if row.labels.is_empty() {
                    if hover(ui.button(&row.label), row).clicked() {
                        self.press(&row.field, None);
                    }
                    continue;
                }
                ui.label(format!("{}:", row.label));
                for (i, name) in row.labels.iter().enumerate() {
                    if !name.is_empty() && hover(ui.button(name), row).clicked() {
                        self.press(&row.field, Some(i));
                    }
                }
            }
        });
        ui.separator();
    }

    fn tune_ui(&mut self, ui: &mut egui::Ui, row: &PanelRow) {
        match &row.value {
            PanelValue::Bool => {
                let mut on = self
                    .live
                    .get(&row.field)
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if hover(ui.checkbox(&mut on, &row.label), row).changed() {
                    self.write(&row.field, toml::Value::Boolean(on));
                }
            }
            PanelValue::Choice(values) => {
                let current = self
                    .live
                    .get(&row.field)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let mut picked = current.clone();
                ui.horizontal(|ui| {
                    ui.label(&row.label);
                    egui::ComboBox::from_id_salt((&self.title, &row.field))
                        .selected_text(&current)
                        .show_ui(ui, |ui| {
                            for value in values {
                                ui.selectable_value(&mut picked, value.clone(), value);
                            }
                        });
                });
                if picked != current {
                    self.write(&row.field, toml::Value::String(picked));
                }
            }
            PanelValue::Text => {
                let mut text = self
                    .live
                    .get(&row.field)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let changed = ui
                    .horizontal(|ui| {
                        ui.label(&row.label);
                        hover(ui.text_edit_singleline(&mut text), row).changed()
                    })
                    .inner;
                if changed {
                    self.write(&row.field, toml::Value::String(text));
                }
            }
            PanelValue::Int | PanelValue::Float => self.number_ui(ui, row),
            PanelValue::Other => {}
        }
    }

    fn number_ui(&mut self, ui: &mut egui::Ui, row: &PanelRow) {
        let n = self.number(row);
        let mut next = n;
        ui.horizontal(|ui| {
            let set = match row.unset {
                Some(unset) => {
                    let was = (n - unset).abs() > 1e-9;
                    let mut on = was;
                    hover(ui.checkbox(&mut on, &row.label), row);
                    if on && !was {
                        next = self.starting_value(row);
                    } else if !on && was {
                        next = unset;
                    }
                    on && was
                }
                None => {
                    hover(ui.label(&row.label), row);
                    true
                }
            };
            if set {
                number_editor(ui, row, &self.title, &mut next);
            }
        });
        if !row.presets.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.add_space(12.0);
                for (name, value) in &row.presets {
                    if ui.small_button(name).clicked() {
                        next = *value;
                    }
                }
            });
        }
        if (next - n).abs() > f64::EPSILON {
            self.write(&row.field, number_value(row, next));
        }
    }

    /// Where a tune row with `unset` starts when it is checked: its `from`
    /// field's live value (rounded to a label), else its minimum.
    fn starting_value(&self, row: &PanelRow) -> f64 {
        let from = row
            .from
            .as_ref()
            .and_then(|f| self.live.get(f))
            .and_then(number_of);
        let mut start = from.unwrap_or(row.min.unwrap_or(0.0));
        if !row.labels.is_empty() || row.value == PanelValue::Int {
            start = start.round();
        }
        clamp(row, start)
    }
}

fn clamp(row: &PanelRow, n: f64) -> f64 {
    let n = row.min.map_or(n, |min| n.max(min));
    row.max.map_or(n, |max| n.min(max))
}

/// A row's number as its field's TOML type.
fn number_value(row: &PanelRow, n: f64) -> toml::Value {
    match row.value {
        PanelValue::Int => toml::Value::Integer(n.round() as i64),
        _ => toml::Value::Float(n),
    }
}

fn hover(response: egui::Response, row: &PanelRow) -> egui::Response {
    match &row.description {
        Some(text) => response.on_hover_text(text),
        None => response,
    }
}

/// The editor of a number: a choice of its labels, a slider, or a drag
/// value, within its range.
fn number_editor(ui: &mut egui::Ui, row: &PanelRow, title: &str, n: &mut f64) {
    if !row.labels.is_empty() {
        let mut i = n.round().max(0.0) as usize;
        let shown = row.labels.get(i).cloned().unwrap_or_else(|| format!("{n}"));
        egui::ComboBox::from_id_salt((title, &row.field))
            .selected_text(shown)
            .show_ui(ui, |ui| {
                for (k, name) in row.labels.iter().enumerate() {
                    if !name.is_empty() {
                        ui.selectable_value(&mut i, k, name);
                    }
                }
            });
        *n = i as f64;
        return;
    }
    let integer = row.value == PanelValue::Int;
    if row.slider {
        if let (Some(min), Some(max)) = (row.min, row.max) {
            let mut slider = egui::Slider::new(n, min..=max);
            if integer {
                slider = slider.integer();
            }
            ui.add(slider);
            return;
        }
    }
    let speed = row.step.unwrap_or(match (integer, row.min, row.max) {
        (true, _, _) => 0.1,
        (false, Some(min), Some(max)) => ((max - min) / 200.0).max(1e-4),
        _ => (n.abs() * 0.01).max(0.01),
    });
    let mut drag = egui::DragValue::new(n).speed(speed);
    if let (Some(min), Some(max)) = (row.min, row.max) {
        drag = drag.range(min..=max);
    } else if let Some(min) = row.min {
        drag = drag.range(min..=f64::INFINITY);
    } else if let Some(max) = row.max {
        drag = drag.range(f64::NEG_INFINITY..=max);
    }
    if integer {
        drag = drag.fixed_decimals(0);
    }
    ui.add(drag);
}

impl DebugPanel for ComponentPanel {
    fn name(&self) -> &str {
        &self.title
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let status = self.status_line();
        if !status.is_empty() {
            ui.label(egui::RichText::new(status).strong());
            ui.separator();
        }
        self.triggers_ui(ui);

        let tunes: Vec<PanelRow> = self.spec.rows_with(PanelRole::Tune).cloned().collect();
        if tunes.is_empty() {
            return;
        }
        for row in &tunes {
            self.tune_ui(ui, row);
        }
        if self.original.is_empty() && self.scene_file.is_none() {
            return;
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Reset").clicked() {
                for (field, value) in self.original.clone() {
                    self.write(&field, value);
                }
            }
            match &self.scene_file {
                Some(_) => {
                    if ui.button("Commit to File").clicked() {
                        self.note = Some(match self.commit_to_file() {
                            Ok(path) => {
                                (format!("wrote {}", flint_core::project::shown(&path)), true)
                            }
                            Err(e) => {
                                tracing::error!("{}: commit to file failed: {e}", self.title);
                                (e, false)
                            }
                        });
                    }
                }
                None => {
                    ui.weak("(the scene is not on the disk)");
                }
            }
        });
        if let Some((text, ok)) = &self.note {
            if *ok {
                ui.weak(text);
            } else {
                ui.colored_label(ui.visuals().error_fg_color, text);
            }
        }
    }

    fn sync(&mut self, ctx: &mut PanelCtx) {
        self.sync_world(ctx.world);
    }

    /// A row a field, and one each for the status line, the trigger row
    /// and the Reset / Commit row.
    fn weight(&self) -> u32 {
        let tunes = self.spec.rows_with(PanelRole::Tune).count() as u32;
        let status = self.spec.rows.iter().any(|r| r.on_status_line()) as u32;
        let triggers = self.spec.rows_with(PanelRole::Trigger).next().is_some() as u32;
        1 + status + triggers + tunes + (tunes > 0) as u32
    }

    fn is_open(&self) -> bool {
        self.open
    }
    fn toggle(&mut self) {
        self.open = !self.open;
    }
    fn is_dirty(&self) -> bool {
        !self.writes.is_empty()
    }
    fn clear_dirty(&mut self) {}
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run_headless_frame;

    const FOG: &str = r#"
[component.fog_bank.panel]
title = "Fog Bank"

[component.fog_bank.fields]
phase = { type = "f32", default = 0.0, panel = { role = "status", labels = ["Idle", "Rolling in", "Fogbound", "Lifting"] } }
fog = { type = "f32", default = 0.0, panel = { role = "status", format = "fog {:.2}" } }
next_in_s = { type = "f32", default = -1.0, panel = { role = "status", format = "next in {:.0}s", unset = -1.0 } }
trigger_fog = { type = "f32", default = -1.0, panel = { role = "trigger", label = "Fog now" } }
end_now = { type = "bool", default = false, panel = { role = "trigger", label = "End now" } }
hold_hours = { type = "f32", default = 4.0, panel = { role = "tune", min = 0.0, max = 24.0 } }
"#;

    fn world_and_panel() -> (FlintWorld, ComponentPanel) {
        let mut registry = flint_schema::SchemaRegistry::new();
        registry.load_component_string(FOG).unwrap();
        let spec = registry
            .get_component("fog_bank")
            .unwrap()
            .panel
            .clone()
            .unwrap();
        let mut world = FlintWorld::new();
        let id = world.spawn("fog_controller").unwrap();
        let table: toml::Value = toml::from_str(
            "phase = 2.0\nfog = 0.954\nnext_in_s = -1.0\ntrigger_fog = -1.0\nend_now = false\nhold_hours = 4.0\n",
        )
        .unwrap();
        world.set_component(id, "fog_bank", table.clone()).unwrap();
        let panel = ComponentPanel::new(
            spec,
            "fog_bank",
            "fog_controller",
            "Fog Bank".into(),
            Some(&table),
            None,
        );
        (world, panel)
    }

    fn field(world: &FlintWorld, name: &str) -> toml::Value {
        let id = world.get_id("fog_controller").unwrap();
        world.get_components(id).unwrap().get("fog_bank").unwrap()[name].clone()
    }

    #[test]
    fn the_status_line_reads_the_live_component() {
        let (mut world, mut panel) = world_and_panel();
        panel.toggle();
        panel.sync_world(&mut world);
        assert_eq!(panel.status_line(), "Fogbound  ·  fog 0.95");
        assert_eq!(panel.weight(), 1 + 1 + 1 + 1 + 1);
    }

    #[test]
    fn a_trigger_is_one_write_into_the_world() {
        let (mut world, mut panel) = world_and_panel();
        panel.toggle();
        panel.sync_world(&mut world);
        // The panel draws every kind of row from the live component
        let ctx = egui::Context::default();
        run_headless_frame(&ctx, egui::RawInput::default(), |ui| panel.ui(ui));
        assert!(!panel.is_dirty(), "drawing writes nothing");
        // What the End now button does when pressed
        assert!(panel.press("end_now", None));
        assert!(!panel.press("fog", None), "a status row is no trigger");
        assert!(panel.is_dirty());
        panel.sync_world(&mut world);
        assert_eq!(field(&world, "end_now"), toml::Value::Boolean(true));
        assert!(!panel.is_dirty(), "written once");
    }

    #[test]
    fn reset_writes_back_what_the_scene_loaded() {
        let (mut world, mut panel) = world_and_panel();
        panel.toggle();
        panel.write("hold_hours", toml::Value::Float(9.0));
        panel.sync_world(&mut world);
        assert_eq!(field(&world, "hold_hours"), toml::Value::Float(9.0));
        for (f, v) in panel.original.clone() {
            panel.write(&f, v);
        }
        panel.sync_world(&mut world);
        assert_eq!(field(&world, "hold_hours"), toml::Value::Float(4.0));
    }
}
