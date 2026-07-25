//! Reality panel (F3 family).
//!
//! Controls a game-side `reality` component (convention: a reality script
//! schedules rare "reality tear" render-mode events — 1 = Matrix code,
//! 2 = blood ocean, 3 = drunk, 4 = Tron — publishes `active_mode` / `mix` /
//! `next_in_s`, and honors one-shot `trigger_mode` / `end_now` flags plus a
//! `mix_override` pin). The panel is created only when the component
//! exists, so games without tears never see it.
//!
//! The script owns the published fields; the panel shows them read-only via
//! [`RealityDebugPanel::sync_status`] and never writes them. Trigger
//! buttons queue a one-shot push (see `take_one_shots`) that the script
//! clears after acting on it.

use crate::DebugPanel;
use flint_core::toml_util::toml_f32;
use flint_scene::SceneDocument;
use std::path::PathBuf;

pub const REALITY_MODE_NAMES: [&str; 5] = ["None", "Matrix", "Blood", "Drunk", "Tron"];

#[derive(Debug, Clone, PartialEq)]
pub struct RealityPanelConfig {
    pub min_interval_s: f32,
    pub max_interval_s: f32,
    pub duration_min_s: f32,
    pub duration_max_s: f32,
    pub ease_in_s: f32,
    pub ease_out_s: f32,
    pub w_matrix: f32,
    pub w_blood: f32,
    pub w_drunk: f32,
    pub w_tron: f32,
    pub mask_scale: f32,
    pub mask_style: f32,
    pub mix_override: f32,
}

impl RealityPanelConfig {
    pub fn from_component(value: &toml::Value) -> Self {
        let f = |name: &str, dv: f32| value.get(name).and_then(toml_f32).unwrap_or(dv);
        Self {
            min_interval_s: f("min_interval_s", 480.0),
            max_interval_s: f("max_interval_s", 1200.0),
            duration_min_s: f("duration_min_s", 20.0),
            duration_max_s: f("duration_max_s", 45.0),
            ease_in_s: f("ease_in_s", 2.5),
            ease_out_s: f("ease_out_s", 3.5),
            w_matrix: f("w_matrix", 1.0),
            w_blood: f("w_blood", 1.0),
            w_drunk: f("w_drunk", 1.0),
            w_tron: f("w_tron", 1.0),
            mask_scale: f("mask_scale", 3.0),
            mask_style: f("mask_style", 0.0),
            mix_override: f("mix_override", -1.0),
        }
    }

    pub fn to_fields(&self) -> Vec<(&'static str, toml::Value)> {
        vec![
            (
                "min_interval_s",
                toml::Value::Float(self.min_interval_s as f64),
            ),
            (
                "max_interval_s",
                toml::Value::Float(self.max_interval_s as f64),
            ),
            (
                "duration_min_s",
                toml::Value::Float(self.duration_min_s as f64),
            ),
            (
                "duration_max_s",
                toml::Value::Float(self.duration_max_s as f64),
            ),
            ("ease_in_s", toml::Value::Float(self.ease_in_s as f64)),
            ("ease_out_s", toml::Value::Float(self.ease_out_s as f64)),
            ("w_matrix", toml::Value::Float(self.w_matrix as f64)),
            ("w_blood", toml::Value::Float(self.w_blood as f64)),
            ("w_drunk", toml::Value::Float(self.w_drunk as f64)),
            ("w_tron", toml::Value::Float(self.w_tron as f64)),
            ("mask_scale", toml::Value::Float(self.mask_scale as f64)),
            ("mask_style", toml::Value::Float(self.mask_style as f64)),
            ("mix_override", toml::Value::Float(self.mix_override as f64)),
        ]
    }
}

pub struct RealityDebugPanel {
    config: RealityPanelConfig,
    original: RealityPanelConfig,
    scene_path: PathBuf,
    reality_entity_name: String,
    open: bool,
    dirty: bool,
    // Read-only status published by the reality script.
    active_mode: f32,
    mix: f32,
    next_in_s: f32,
    // One-shot flags queued by buttons, drained by the host each frame.
    pending_trigger: Option<f32>,
    pending_end: bool,
}

impl RealityDebugPanel {
    pub fn new(
        config: RealityPanelConfig,
        scene_path: PathBuf,
        reality_entity_name: String,
    ) -> Self {
        Self {
            original: config.clone(),
            config,
            scene_path,
            reality_entity_name,
            open: false,
            dirty: false,
            active_mode: 0.0,
            mix: 0.0,
            next_in_s: -1.0,
            pending_trigger: None,
            pending_end: false,
        }
    }

    pub fn config(&self) -> &RealityPanelConfig {
        &self.config
    }

    pub fn entity_name(&self) -> &str {
        &self.reality_entity_name
    }

    /// Update the read-only displays from the script-published fields.
    pub fn sync_status(&mut self, active_mode: f32, mix: f32, next_in_s: f32) {
        self.active_mode = active_mode;
        self.mix = mix;
        self.next_in_s = next_in_s;
    }

    /// Drain queued one-shot flags as component field writes.
    pub fn take_one_shots(&mut self) -> Vec<(&'static str, toml::Value)> {
        let mut fields = Vec::new();
        if let Some(mode) = self.pending_trigger.take() {
            fields.push(("trigger_mode", toml::Value::Float(mode as f64)));
        }
        if self.pending_end {
            self.pending_end = false;
            fields.push(("end_now", toml::Value::Boolean(true)));
        }
        fields
    }

    fn commit_to_file(&self) -> Result<(), String> {
        let mut doc = SceneDocument::from_file(&self.scene_path)?;
        for (field, value) in self.config.to_fields() {
            doc.patch_field(&self.reality_entity_name, "reality", field, &value)?;
        }
        doc.save(&self.scene_path)
    }
}

impl DebugPanel for RealityDebugPanel {
    fn name(&self) -> &str {
        "Reality"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;

        // Status line: whether reality is currently torn, and how far.
        let mode_i = (self.active_mode.round() as usize).min(4);
        if mode_i > 0 {
            ui.label(format!(
                "Tear: {} {:.0}%",
                REALITY_MODE_NAMES[mode_i],
                self.mix * 100.0
            ));
        } else if self.next_in_s >= 0.0 {
            ui.label(format!("Whole  ·  next tear in {:.0} s", self.next_in_s));
        } else {
            ui.label("Whole");
        }
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Tear:");
            for (i, name) in REALITY_MODE_NAMES.iter().enumerate().skip(1) {
                if ui.button(*name).clicked() {
                    self.pending_trigger = Some(i as f32);
                }
            }
        });
        if ui.button("End tear now").clicked() {
            self.pending_end = true;
        }

        // Mix pin: freeze the envelope for shader tuning.
        let mut pinned = self.config.mix_override >= 0.0;
        let before_pinned = pinned;
        ui.checkbox(&mut pinned, "Pin mix");
        if pinned != before_pinned {
            self.config.mix_override = if pinned { self.mix.clamp(0.0, 1.0) } else { -1.0 };
            changed = true;
        }
        if pinned {
            let before = self.config.mix_override;
            ui.add(egui::Slider::new(&mut self.config.mix_override, 0.0..=1.0).text("mix"));
            if (self.config.mix_override - before).abs() > f32::EPSILON {
                changed = true;
            }
        }

        ui.separator();
        let drag = |ui: &mut egui::Ui,
                        label: &str,
                        value: &mut f32,
                        speed: f64,
                        range: std::ops::RangeInclusive<f64>|
         -> bool {
            let before = *value;
            ui.horizontal(|ui| {
                ui.label(label);
                ui.add(egui::DragValue::new(value).speed(speed).range(range));
            });
            (*value - before).abs() > f32::EPSILON
        };
        changed |= drag(
            ui,
            "Min interval (s)",
            &mut self.config.min_interval_s,
            5.0,
            10.0..=7200.0,
        );
        changed |= drag(
            ui,
            "Max interval (s)",
            &mut self.config.max_interval_s,
            5.0,
            10.0..=7200.0,
        );
        changed |= drag(
            ui,
            "Duration min (s)",
            &mut self.config.duration_min_s,
            1.0,
            2.0..=600.0,
        );
        changed |= drag(
            ui,
            "Duration max (s)",
            &mut self.config.duration_max_s,
            1.0,
            2.0..=600.0,
        );
        changed |= drag(ui, "Ease in (s)", &mut self.config.ease_in_s, 0.1, 0.1..=30.0);
        changed |= drag(ui, "Ease out (s)", &mut self.config.ease_out_s, 0.1, 0.1..=30.0);
        ui.separator();
        changed |= drag(ui, "Matrix weight", &mut self.config.w_matrix, 0.05, 0.0..=5.0);
        changed |= drag(ui, "Blood weight", &mut self.config.w_blood, 0.05, 0.0..=5.0);
        changed |= drag(ui, "Drunk weight", &mut self.config.w_drunk, 0.05, 0.0..=5.0);
        changed |= drag(ui, "Tron weight", &mut self.config.w_tron, 0.05, 0.0..=5.0);
        ui.separator();
        changed |= drag(ui, "Mask scale", &mut self.config.mask_scale, 0.1, 0.2..=20.0);
        changed |= drag(ui, "Mask style", &mut self.config.mask_style, 1.0, 0.0..=1.0);

        if changed {
            self.dirty = true;
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Reset").clicked() {
                self.config = self.original.clone();
                self.dirty = true;
            }
            if ui.button("Commit to File").clicked() {
                if let Err(e) = self.commit_to_file() {
                    tracing::error!("RealityDebugPanel: commit_to_file failed: {}", e);
                }
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
