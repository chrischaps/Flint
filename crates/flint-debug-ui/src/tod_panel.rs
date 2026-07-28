//! Day / time scrubber panel (F3 family).
//!
//! Controls a game-side `time_of_day` component (convention: fields
//! `time_hours` 0-24, `day_length_sec`, `auto_advance`, `sun_path_tilt_deg`,
//! and optionally a script-owned `day` counter; a game script interpolates
//! keyframes from it). The panel is created only when the component exists,
//! so games without a day/night cycle never see it.
//!
//! While `auto_advance` is on, the game script owns `time_hours`; the player
//! keeps the slider display in sync via [`TimeOfDayDebugPanel::sync_time`].
//! Drag the slider (or uncheck auto) to force a time — the debug slider and
//! natural passage share the game script's single interpolation path, so
//! scrubbing is seamless.
//!
//! The `day` counter is different: the game script owns it outright
//! (midnight wraps increment it; >12h `time_hours` jumps adjust it), so the
//! panel never writes it as part of its persistent config. Day edits are a
//! one-shot override the host drains via [`TimeOfDayDebugPanel::take_day_set`]
//! and the display tracks the live value via
//! [`TimeOfDayDebugPanel::sync_day`]. Scenes without a materialized `day`
//! field (flat-rate scenes) simply hide the day UI.

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
    /// Optional per-day multipliers on `day_length_sec`, indexed by day
    /// number; the last entry repeats for every later day. Games that
    /// compress their opening days declare the same curve their time script
    /// applies, so the panel can report the real length. Empty = flat rate.
    pub day_length_ramp: Vec<f32>,
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
            day_length_ramp: value
                .get("day_length_ramp")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(toml_f32).collect())
                .unwrap_or_default(),
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
    /// Live script-owned day counter, `None` in scenes without a `day` field.
    day: Option<f32>,
    /// Pending one-shot day override; drained by the host each frame.
    pending_day: Option<f32>,
}

impl TimeOfDayDebugPanel {
    pub fn new(config: TimeOfDayPanelConfig, scene_path: PathBuf, tod_entity_name: String) -> Self {
        Self {
            original: config.clone(),
            config,
            scene_path,
            tod_entity_name,
            open: false,
            dirty: false,
            day: None,
            pending_day: None,
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

    /// Track the script-owned day counter (`None` = field not in this scene).
    pub fn sync_day(&mut self, day: Option<f32>) {
        if self.pending_day.is_none() {
            self.day = day;
        }
    }

    /// Drain the one-shot day override, if the user edited the day this frame.
    pub fn take_day_set(&mut self) -> Option<f32> {
        self.pending_day.take()
    }

    fn commit_to_file(&self) -> Result<(), String> {
        let mut doc = SceneDocument::from_file(&self.scene_path)?;
        for (field, value) in self.config.to_fields() {
            doc.patch_field(&self.tod_entity_name, "time_of_day", field, &value)?;
        }
        doc.save(&self.scene_path)
    }
}

fn format_clock(hours: f32) -> String {
    let h = (hours.floor() as i32).rem_euclid(24);
    let m = (hours.fract() * 60.0).floor() as i32;
    format!("{h:02}:{m:02}")
}

/// Day-length multiplier for `day` under a game-declared ramp.
///
/// `ramp` is indexed by day number and its last entry repeats forever, so a
/// game that compresses its opening days declares e.g. `[0.1, 0.2, 0.4, 1.0]`
/// on its `time_of_day` component. An empty ramp means a flat rate.
fn ramp_fraction(day: f32, ramp: &[f32]) -> f32 {
    if ramp.is_empty() {
        return 1.0;
    }
    let idx = (day.round().max(0.0) as usize).min(ramp.len() - 1);
    ramp[idx]
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
        "Day / Time"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;

        // ── Clock line: "Day 3 · 14:32 · midday" ────────────────────────
        let clock = format_clock(self.config.time_hours);
        let phase = label_for(self.config.time_hours);
        let status = match self.day {
            Some(d) => format!("Day {}  ·  {clock}  ·  {phase}", d.round() as i64),
            None => format!("{clock}  ·  {phase}"),
        };
        ui.label(egui::RichText::new(status).strong());
        ui.separator();

        let before = self.config.time_hours;
        ui.horizontal(|ui| {
            ui.label("Time");
            ui.add(
                egui::Slider::new(&mut self.config.time_hours, 0.0..=24.0)
                    .fixed_decimals(2)
                    .suffix(" h"),
            );
        });
        // Presets jump straight to a keyframe hour; a jump across midnight
        // reads as a wrap/scrub to the game script (day +1 / -1 by design).
        ui.horizontal(|ui| {
            for (name, hours) in [
                ("Dawn", 6.0),
                ("Noon", 12.5),
                ("Sunset", 19.0),
                ("Dusk", 20.4),
                ("Night", 0.0),
            ] {
                if ui.small_button(name).clicked() {
                    self.config.time_hours = hours;
                }
            }
        });
        if (self.config.time_hours - before).abs() > f32::EPSILON {
            changed = true;
        }

        let before_auto = self.config.auto_advance;
        ui.checkbox(&mut self.config.auto_advance, "Advance naturally");
        changed |= self.config.auto_advance != before_auto;

        // ── Day counter (script-owned; edits are one-shot overrides) ────
        if let Some(day) = self.day {
            ui.horizontal(|ui| {
                ui.label("Day");
                let mut d = self.pending_day.unwrap_or(day).round() as i32;
                let resp = ui.add(egui::DragValue::new(&mut d).speed(0.05).range(0..=9999));
                if resp.changed() {
                    self.pending_day = Some(d.max(0) as f32);
                }
                if ui.small_button("-1").clicked() && d > 0 {
                    self.pending_day = Some((d - 1) as f32);
                }
                if ui.small_button("+1").clicked() {
                    self.pending_day = Some((d + 1) as f32);
                }
            });
        }

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
        // Effective length under a game-declared ramp (ramped scenes only).
        if let Some(day) = self.day {
            if !self.config.day_length_ramp.is_empty() {
                let frac = ramp_fraction(
                    self.pending_day.unwrap_or(day),
                    &self.config.day_length_ramp,
                );
                let eff = self.config.day_length_sec * frac;
                ui.weak(format!(
                    "effective {eff:.0} s this day (ramp x{frac:.1}) · {:.1} s / game hour",
                    eff / 24.0
                ));
            }
        }

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

#[cfg(test)]
mod ramp_tests {
    use super::{ramp_fraction, TimeOfDayPanelConfig};

    #[test]
    fn empty_ramp_is_flat() {
        assert_eq!(ramp_fraction(0.0, &[]), 1.0);
        assert_eq!(ramp_fraction(99.0, &[]), 1.0);
    }

    #[test]
    fn ramp_indexes_by_day_and_holds_the_last_entry() {
        let ramp = [0.1, 0.2, 0.4, 0.7, 1.0];
        assert_eq!(ramp_fraction(0.0, &ramp), 0.1);
        assert_eq!(ramp_fraction(1.0, &ramp), 0.2);
        assert_eq!(ramp_fraction(3.0, &ramp), 0.7);
        assert_eq!(ramp_fraction(4.0, &ramp), 1.0);
        // Past the end the last entry repeats forever.
        assert_eq!(ramp_fraction(400.0, &ramp), 1.0);
    }

    #[test]
    fn negative_day_clamps_to_the_first_entry() {
        assert_eq!(ramp_fraction(-3.0, &[0.1, 1.0]), 0.1);
    }

    #[test]
    fn absent_ramp_field_parses_as_empty() {
        let v: toml::Value = toml::from_str("time_hours = 6.0").unwrap();
        assert!(TimeOfDayPanelConfig::from_component(&v)
            .day_length_ramp
            .is_empty());
    }

    #[test]
    fn ramp_field_parses_from_the_component() {
        let v: toml::Value = toml::from_str("day_length_ramp = [0.1, 0.5, 1.0]").unwrap();
        assert_eq!(
            TimeOfDayPanelConfig::from_component(&v).day_length_ramp,
            vec![0.1, 0.5, 1.0]
        );
    }
}
