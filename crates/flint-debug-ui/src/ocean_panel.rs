//! Live-tuning panel for the `ocean` component (F3 family).
//!
//! Edits are pushed by the player into the world's `ocean` component, so the
//! renderer (spectrum regen on param change) and the script API
//! (`ocean_height`) both pick them up on the next frame — one source of
//! truth, three consumers.

use crate::DebugPanel;
use flint_core::ocean::OceanParams;
use flint_core::toml_util::{toml_color, toml_f32};
use flint_scene::SceneDocument;
use std::path::PathBuf;

fn drag_f32(
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

fn drag_i64(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut i64,
    range: std::ops::RangeInclusive<i64>,
) -> bool {
    let before = *value;
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(value).speed(0.1).range(range));
    });
    *value != before
}

fn color_rgba(ui: &mut egui::Ui, label: &str, value: &mut [f32; 4]) -> bool {
    let mut rgb = [value[0], value[1], value[2]];
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        if ui.color_edit_button_rgb(&mut rgb).changed() {
            changed = true;
        }
    });
    if changed {
        value[0] = rgb[0];
        value[1] = rgb[1];
        value[2] = rgb[2];
    }
    changed
}

/// Flat mirror of every tunable `ocean` component field.
#[derive(Debug, Clone, PartialEq)]
pub struct OceanPanelConfig {
    // Simulation
    pub seed: i64,
    pub num_waves: i64,
    pub wavelength_min: f32,
    pub wavelength_max: f32,
    pub amplitude: f32,
    pub choppiness: f32,
    pub direction_deg: f32,
    pub spread_deg: f32,
    pub speed_scale: f32,
    // Style
    pub deep_color: [f32; 4],
    pub shallow_color: [f32; 4],
    pub foam_color: [f32; 4],
    pub sss_color: [f32; 4],
    pub foam_threshold: f32,
    pub foam_noise_scale: f32,
    pub ramp_steps: f32,
    pub specular_strength: f32,
    // Refraction
    pub turbidity: f32,
    pub refraction_strength: f32,
    // Grid
    pub grid_scale: f32,
    pub fade_start: f32,
    pub fade_end: f32,
}

impl OceanPanelConfig {
    /// Parse from the `ocean` component TOML (missing fields keep defaults).
    pub fn from_component(value: &toml::Value) -> Self {
        let p = OceanParams::from_component(value);
        let f = |name: &str, dv: f32| value.get(name).and_then(toml_f32).unwrap_or(dv);
        let c = |name: &str, dv: [f32; 4]| value.get(name).and_then(toml_color).unwrap_or(dv);
        Self {
            seed: p.seed,
            num_waves: p.num_waves as i64,
            wavelength_min: p.wavelength_min,
            wavelength_max: p.wavelength_max,
            amplitude: p.amplitude,
            choppiness: p.choppiness,
            direction_deg: p.direction_deg,
            spread_deg: p.spread_deg,
            speed_scale: p.speed_scale,
            deep_color: c("deep_color", [0.055, 0.22, 0.53, 1.0]),
            shallow_color: c("shallow_color", [0.16, 0.51, 0.83, 1.0]),
            foam_color: c("foam_color", [0.96, 0.99, 1.0, 1.0]),
            sss_color: c("sss_color", [0.10, 0.75, 0.75, 1.0]),
            foam_threshold: f("foam_threshold", 0.62),
            foam_noise_scale: f("foam_noise_scale", 0.35),
            ramp_steps: f("ramp_steps", 3.0),
            specular_strength: f("specular_strength", 1.0),
            turbidity: f("turbidity", 0.8),
            refraction_strength: f("refraction_strength", 0.6),
            grid_scale: f("grid_scale", 60.0),
            fade_start: f("fade_start", 90.0),
            fade_end: f("fade_end", 170.0),
        }
    }

    /// All fields as (name, toml value) pairs — the apply/commit contract.
    pub fn to_fields(&self) -> Vec<(&'static str, toml::Value)> {
        let f = |v: f32| toml::Value::Float(v as f64);
        let i = toml::Value::Integer;
        let c = |arr: [f32; 4]| {
            toml::Value::Array(arr.iter().map(|&v| toml::Value::Float(v as f64)).collect())
        };
        vec![
            ("seed", i(self.seed)),
            ("num_waves", i(self.num_waves)),
            ("wavelength_min", f(self.wavelength_min)),
            ("wavelength_max", f(self.wavelength_max)),
            ("amplitude", f(self.amplitude)),
            ("choppiness", f(self.choppiness)),
            ("direction_deg", f(self.direction_deg)),
            ("spread_deg", f(self.spread_deg)),
            ("speed_scale", f(self.speed_scale)),
            ("deep_color", c(self.deep_color)),
            ("shallow_color", c(self.shallow_color)),
            ("foam_color", c(self.foam_color)),
            ("sss_color", c(self.sss_color)),
            ("foam_threshold", f(self.foam_threshold)),
            ("foam_noise_scale", f(self.foam_noise_scale)),
            ("ramp_steps", f(self.ramp_steps)),
            ("specular_strength", f(self.specular_strength)),
            ("turbidity", f(self.turbidity)),
            ("refraction_strength", f(self.refraction_strength)),
            ("grid_scale", f(self.grid_scale)),
            ("fade_start", f(self.fade_start)),
            ("fade_end", f(self.fade_end)),
        ]
    }
}

pub struct OceanDebugPanel {
    config: OceanPanelConfig,
    original: OceanPanelConfig,
    scene_path: PathBuf,
    ocean_entity_name: String,
    open: bool,
    dirty: bool,
}

impl OceanDebugPanel {
    pub fn new(
        config: OceanPanelConfig,
        scene_path: PathBuf,
        ocean_entity_name: String,
    ) -> Self {
        Self {
            original: config.clone(),
            config,
            scene_path,
            ocean_entity_name,
            open: false,
            dirty: false,
        }
    }

    pub fn config(&self) -> &OceanPanelConfig {
        &self.config
    }

    pub fn entity_name(&self) -> &str {
        &self.ocean_entity_name
    }

    fn commit_to_file(&self) -> Result<(), String> {
        let mut doc = SceneDocument::from_file(&self.scene_path)?;
        for (field, value) in self.config.to_fields() {
            doc.patch_field(&self.ocean_entity_name, "ocean", field, &value)?;
        }
        doc.save(&self.scene_path)
    }

    fn format_toml(&self) -> String {
        self.config
            .to_fields()
            .iter()
            .map(|(name, value)| format!("{name} = {value}\n"))
            .collect()
    }
}

impl DebugPanel for OceanDebugPanel {
    fn name(&self) -> &str {
        "Ocean Debug"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;

        egui::CollapsingHeader::new("Waves")
            .default_open(true)
            .show(ui, |ui| {
                changed |= drag_f32(ui, "Amplitude (m)", &mut self.config.amplitude, 0.01, 0.0..=6.0);
                changed |= drag_f32(ui, "Choppiness", &mut self.config.choppiness, 0.005, 0.0..=1.0);
                changed |= drag_f32(ui, "Wavelength Min", &mut self.config.wavelength_min, 0.1, 0.5..=50.0);
                changed |= drag_f32(ui, "Wavelength Max", &mut self.config.wavelength_max, 0.5, 2.0..=400.0);
                changed |= drag_f32(ui, "Direction (deg)", &mut self.config.direction_deg, 1.0, -360.0..=360.0);
                changed |= drag_f32(ui, "Spread (deg)", &mut self.config.spread_deg, 1.0, 0.0..=180.0);
                changed |= drag_f32(ui, "Speed Scale", &mut self.config.speed_scale, 0.01, 0.0..=8.0);
                changed |= drag_i64(ui, "Wave Count", &mut self.config.num_waves, 1..=16);
                changed |= drag_i64(ui, "Seed", &mut self.config.seed, 0..=9999);
            });

        egui::CollapsingHeader::new("Color & Style")
            .default_open(true)
            .show(ui, |ui| {
                changed |= color_rgba(ui, "Deep", &mut self.config.deep_color);
                changed |= color_rgba(ui, "Shallow", &mut self.config.shallow_color);
                changed |= color_rgba(ui, "Foam", &mut self.config.foam_color);
                changed |= color_rgba(ui, "Subsurface", &mut self.config.sss_color);
                changed |= drag_f32(ui, "Foam Threshold", &mut self.config.foam_threshold, 0.005, 0.0..=1.5);
                changed |= drag_f32(ui, "Foam Noise Scale", &mut self.config.foam_noise_scale, 0.005, 0.01..=4.0);
                changed |= drag_f32(ui, "Ramp Steps", &mut self.config.ramp_steps, 0.05, 1.0..=8.0);
                changed |= drag_f32(ui, "Specular", &mut self.config.specular_strength, 0.01, 0.0..=4.0);
            });

        egui::CollapsingHeader::new("Water Clarity")
            .default_open(true)
            .show(ui, |ui| {
                changed |= drag_f32(ui, "Turbidity", &mut self.config.turbidity, 0.01, 0.0..=10.0);
                changed |= drag_f32(ui, "Refraction", &mut self.config.refraction_strength, 0.01, 0.0..=4.0);
            });

        egui::CollapsingHeader::new("Grid")
            .default_open(false)
            .show(ui, |ui| {
                changed |= drag_f32(ui, "Grid Scale", &mut self.config.grid_scale, 0.5, 10.0..=500.0);
                changed |= drag_f32(ui, "Fade Start", &mut self.config.fade_start, 1.0, 5.0..=1000.0);
                changed |= drag_f32(ui, "Fade End", &mut self.config.fade_end, 1.0, 10.0..=2000.0);
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
            if ui.button("Copy TOML").clicked() {
                let snippet = self.format_toml();
                ui.output_mut(|o| o.copied_text = snippet);
            }
            if ui.button("Commit to File").clicked() {
                if let Err(e) = self.commit_to_file() {
                    tracing::error!("OceanDebugPanel: commit_to_file failed: {}", e);
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
