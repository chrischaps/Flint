//! Weather panel (F3 family).
//!
//! Controls a game-side `weather` component (convention: a weather script
//! walks a continuous `state` 0..4 = clear/scattered/overcast/rain/storm,
//! publishes `wind`/`sea`, and honors `forced_state` (-1 = autonomous),
//! plus one-shot `snap` / `strike_now` flags). The panel is created only
//! when the component exists, so games without weather never see it.
//!
//! The script owns `state`/`wind`/`sea`; the panel shows them read-only
//! via [`WeatherDebugPanel::sync_status`] and never writes them. One-shot
//! buttons queue a single `true` push (see `take_one_shots`) that the
//! script clears after acting on it.

use crate::DebugPanel;
use flint_core::toml_util::toml_f32;
use flint_scene::SceneDocument;
use std::path::PathBuf;

pub const WEATHER_STATE_NAMES: [&str; 5] = ["Clear", "Scattered", "Overcast", "Rain", "Storm"];

#[derive(Debug, Clone, PartialEq)]
pub struct WeatherPanelConfig {
    pub forced_state: f32,
    pub transition_s: f32,
    pub wind_tau_s: f32,
    pub sea_build_tau_s: f32,
    pub sea_decay_tau_s: f32,
    pub amp_storm: f32,
    pub rain_rate_max: f32,
    pub gust_strength: f32,
    pub raft_assist: f32,
}

impl WeatherPanelConfig {
    pub fn from_component(value: &toml::Value) -> Self {
        let f = |name: &str, dv: f32| value.get(name).and_then(toml_f32).unwrap_or(dv);
        Self {
            forced_state: f("forced_state", -1.0),
            transition_s: f("transition_s", 60.0),
            wind_tau_s: f("wind_tau_s", 20.0),
            sea_build_tau_s: f("sea_build_tau_s", 120.0),
            sea_decay_tau_s: f("sea_decay_tau_s", 240.0),
            amp_storm: f("amp_storm", 5.5),
            rain_rate_max: f("rain_rate_max", 4000.0),
            gust_strength: f("gust_strength", 0.15),
            raft_assist: f("raft_assist", 1.0),
        }
    }

    pub fn to_fields(&self) -> Vec<(&'static str, toml::Value)> {
        vec![
            ("forced_state", toml::Value::Float(self.forced_state as f64)),
            ("transition_s", toml::Value::Float(self.transition_s as f64)),
            ("wind_tau_s", toml::Value::Float(self.wind_tau_s as f64)),
            (
                "sea_build_tau_s",
                toml::Value::Float(self.sea_build_tau_s as f64),
            ),
            (
                "sea_decay_tau_s",
                toml::Value::Float(self.sea_decay_tau_s as f64),
            ),
            ("amp_storm", toml::Value::Float(self.amp_storm as f64)),
            (
                "rain_rate_max",
                toml::Value::Float(self.rain_rate_max as f64),
            ),
            (
                "gust_strength",
                toml::Value::Float(self.gust_strength as f64),
            ),
            ("raft_assist", toml::Value::Float(self.raft_assist as f64)),
        ]
    }
}

pub struct WeatherDebugPanel {
    config: WeatherPanelConfig,
    original: WeatherPanelConfig,
    scene_path: PathBuf,
    weather_entity_name: String,
    open: bool,
    dirty: bool,
    // Read-only status published by the weather script.
    state: f32,
    wind: f32,
    sea: f32,
    // One-shot flags queued by buttons, drained by the host each frame.
    pending_snap: bool,
    pending_strike: bool,
}

impl WeatherDebugPanel {
    pub fn new(
        config: WeatherPanelConfig,
        scene_path: PathBuf,
        weather_entity_name: String,
    ) -> Self {
        Self {
            original: config.clone(),
            config,
            scene_path,
            weather_entity_name,
            open: false,
            dirty: false,
            state: 0.0,
            wind: 0.0,
            sea: 0.0,
            pending_snap: false,
            pending_strike: false,
        }
    }

    pub fn config(&self) -> &WeatherPanelConfig {
        &self.config
    }

    pub fn entity_name(&self) -> &str {
        &self.weather_entity_name
    }

    /// Update the read-only displays from the script-published fields.
    pub fn sync_status(&mut self, state: f32, wind: f32, sea: f32) {
        self.state = state;
        self.wind = wind;
        self.sea = sea;
    }

    /// Drain queued one-shot flags as component field writes.
    pub fn take_one_shots(&mut self) -> Vec<(&'static str, toml::Value)> {
        let mut fields = Vec::new();
        if self.pending_snap {
            self.pending_snap = false;
            fields.push(("snap", toml::Value::Boolean(true)));
        }
        if self.pending_strike {
            self.pending_strike = false;
            fields.push(("strike_now", toml::Value::Boolean(true)));
        }
        fields
    }

    fn commit_to_file(&self) -> Result<(), String> {
        let mut doc = SceneDocument::from_file(&self.scene_path)?;
        for (field, value) in self.config.to_fields() {
            doc.patch_field(&self.weather_entity_name, "weather", field, &value)?;
        }
        doc.save(&self.scene_path)
    }
}

fn state_label(state: f32) -> String {
    let clamped = state.clamp(0.0, 4.0);
    let i = (clamped.round() as usize).min(4);
    let name = WEATHER_STATE_NAMES[i];
    if (clamped - clamped.round()).abs() > 0.1 {
        format!("{name}ish ({clamped:.2})")
    } else {
        name.to_string()
    }
}

impl DebugPanel for WeatherDebugPanel {
    fn name(&self) -> &str {
        "Weather"
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;

        // Status line: what the sky/wind/sea are actually doing.
        ui.label(format!(
            "{}  ·  wind {:.1} m/s  ·  sea {:.0}%",
            state_label(self.state),
            self.wind,
            self.sea * 100.0
        ));
        ui.separator();

        // Auto vs forced state.
        let mut auto = self.config.forced_state < 0.0;
        let before_auto = auto;
        ui.checkbox(&mut auto, "Autonomous drift");
        if auto != before_auto {
            self.config.forced_state = if auto {
                -1.0
            } else {
                self.state.round().clamp(0.0, 4.0)
            };
            changed = true;
        }
        if !auto {
            ui.horizontal(|ui| {
                ui.label("Force");
                let mut idx = (self.config.forced_state.round() as usize).min(4);
                let before = idx;
                egui::ComboBox::from_id_salt("weather_force_state")
                    .selected_text(WEATHER_STATE_NAMES[idx])
                    .show_ui(ui, |ui| {
                        for (i, name) in WEATHER_STATE_NAMES.iter().enumerate() {
                            ui.selectable_value(&mut idx, i, *name);
                        }
                    });
                if idx != before {
                    self.config.forced_state = idx as f32;
                    changed = true;
                }
                if ui.button("Snap").clicked() {
                    self.pending_snap = true;
                }
            });
        }

        if ui.button("Strike now").clicked() {
            self.pending_strike = true;
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
            "Transition (s)",
            &mut self.config.transition_s,
            1.0,
            1.0..=600.0,
        );
        changed |= drag(
            ui,
            "Wind tau (s)",
            &mut self.config.wind_tau_s,
            0.5,
            0.5..=300.0,
        );
        changed |= drag(
            ui,
            "Sea build tau (s)",
            &mut self.config.sea_build_tau_s,
            1.0,
            1.0..=1800.0,
        );
        changed |= drag(
            ui,
            "Sea decay tau (s)",
            &mut self.config.sea_decay_tau_s,
            1.0,
            1.0..=1800.0,
        );
        changed |= drag(
            ui,
            "Storm amplitude",
            &mut self.config.amp_storm,
            0.05,
            0.5..=10.0,
        );
        changed |= drag(
            ui,
            "Rain rate max",
            &mut self.config.rain_rate_max,
            25.0,
            0.0..=10000.0,
        );
        changed |= drag(
            ui,
            "Gust strength",
            &mut self.config.gust_strength,
            0.005,
            0.0..=1.0,
        );
        changed |= drag(
            ui,
            "Raft assist",
            &mut self.config.raft_assist,
            0.02,
            0.0..=3.0,
        );

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
                    tracing::error!("WeatherDebugPanel: commit_to_file failed: {}", e);
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
