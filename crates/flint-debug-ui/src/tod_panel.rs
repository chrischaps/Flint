//! Time-of-day scrubber panel (F3 family).
//!
//! Controls a game-side `time_of_day` component (convention: fields
//! `time_hours` 0-24, `day_length_sec`, `auto_advance`, `sun_path_tilt_deg`;
//! a game script interpolates keyframes from it). The panel is created only
//! when the component exists, so games without a day/night cycle never see it.
//!
//! While `auto_advance` is on, the game script owns `time_hours`; the player
//! keeps the slider display in sync via [`TimeOfDayDebugPanel::sync_time`].
//! Drag the slider (or uncheck auto) to force a time — the debug slider and
//! natural passage share the game script's single interpolation path, so
//! scrubbing is seamless.

use crate::DebugPanel;
use flint_core::toml_util::toml_f32;
use flint_scene::SceneDocument;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub struct TimeOfDayPanelConfig {
    pub time_hours: f32,
    pub day_length_sec: f32,
    pub auto_advance: bool,
    pub sun_path_tilt_deg: f32,
}

impl TimeOfDayPanelConfig {
    pub fn from_component(value: &toml::Value) -> Self {
        let f = |name: &str, dv: f32| value.get(name).and_then(toml_f32).unwrap_or(dv);
        Self {
            time_hours: f("time_hours", 10.5),
            day_length_sec: f("day_length_sec", 600.0),
            auto_advance: value
                .get("auto_advance")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            sun_path_tilt_deg: f("sun_path_tilt_deg", 28.0),
        }
    }

    pub fn to_fields(&self) -> Vec<(&'static str, toml::Value)> {
        vec![
            ("time_hours", toml::Value::Float(self.time_hours as f64)),
            (
                "day_length_sec",
                toml::Value::Float(self.day_length_sec as f64),
            ),
            ("auto_advance", toml::Value::Boolean(self.auto_advance)),
            (
                "sun_path_tilt_deg",
                toml::Value::Float(self.sun_path_tilt_deg as f64),
            ),
        ]
    }
}

pub struct TimeOfDayDebugPanel {
    config: TimeOfDayPanelConfig,
    original: TimeOfDayPanelConfig,
    scene_path: PathBuf,
    tod_entity_name: String,
    open: bool,
    dirty: bool,
}

impl TimeOfDayDebugPanel {
    pub fn new(
        config: TimeOfDayPanelConfig,
        scene_path: PathBuf,
        tod_entity_name: String,
    ) -> Self {
        Self {
            original: config.clone(),
            config,
            scene_path,
            tod_entity_name,
            open: false,
            dirty: false,
        }
    }

    pub fn config(&self) -> &TimeOfDayPanelConfig {
        &self.config
    }

    pub fn entity_name(&self) -> &str {
        &self.tod_entity_name
    }

    /// Keep the slider tracking the game-advanced time while not being edited.
    pub fn sync_time(&mut self, hours: f32) {
        if !self.dirty {
            self.config.time_hours = hours;
        }
    }

    fn commit_to_file(&self) -> Result<(), String> {
        let mut doc = SceneDocument::from_file(&self.scene_path)?;
        for (field, value) in self.config.to_fields() {
            doc.patch_field(&self.tod_entity_name, "time_of_day", field, &value)?;
        }
        doc.save(&self.scene_path)
    }
}

fn label_for(hours: f32) -> &'static str {
    match hours {
        h if h < 4.5 => "night",
        h if h < 6.5 => "dawn",
        h if h < 10.0 => "morning",
        h if h < 15.0 => "midday",
        h if h < 18.5 => "golden hour",
        h if h < 19.8 => "sunset",
        h if h < 21.0 => "dusk",
        _ => "night",
    }
}

impl DebugPanel for TimeOfDayDebugPanel {
    fn name(&self) -> &str {
        "Time of Day"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;

        let before = self.config.time_hours;
        ui.horizontal(|ui| {
            ui.label("Time");
            ui.add(
                egui::Slider::new(&mut self.config.time_hours, 0.0..=24.0)
                    .fixed_decimals(2)
                    .suffix(" h"),
            );
            ui.label(label_for(self.config.time_hours));
        });
        if (self.config.time_hours - before).abs() > f32::EPSILON {
            changed = true;
        }

        let before_auto = self.config.auto_advance;
        ui.checkbox(&mut self.config.auto_advance, "Advance naturally");
        changed |= self.config.auto_advance != before_auto;

        ui.horizontal(|ui| {
            ui.label("Day length (s)");
            let before = self.config.day_length_sec;
            ui.add(
                egui::DragValue::new(&mut self.config.day_length_sec)
                    .speed(5.0)
                    .range(30.0..=86400.0),
            );
            changed |= (self.config.day_length_sec - before).abs() > f32::EPSILON;
        });

        ui.horizontal(|ui| {
            ui.label("Sun path tilt");
            let before = self.config.sun_path_tilt_deg;
            ui.add(
                egui::DragValue::new(&mut self.config.sun_path_tilt_deg)
                    .speed(0.5)
                    .range(0.0..=75.0)
                    .suffix("°"),
            );
            changed |= (self.config.sun_path_tilt_deg - before).abs() > f32::EPSILON;
        });

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
                    tracing::error!("TimeOfDayDebugPanel: commit_to_file failed: {}", e);
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
