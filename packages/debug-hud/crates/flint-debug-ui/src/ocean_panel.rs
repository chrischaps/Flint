//! Live-tuning panel for the `ocean` component (F3 family).
//!
//! Edits are pushed by the player into the world's `ocean` component, so the
//! renderer (spectrum regen on param change) and the script API
//! (`ocean_height`) both pick them up on the next frame — one source of
//! truth, three consumers.

use crate::widgets::drag_f32;
use crate::DebugPanel;
use flint_core::world_line::WorldLine;
use flint_ecs::ComponentView;
use flint_geometry::ocean::{OceanParams, WaveSpectrum};
use flint_scene::SceneDocument;
use std::path::PathBuf;

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
    pub wind_speed: f32,
    pub fetch_km: f32,
    pub peak_enhancement: f32,
    // Style
    pub deep_color: [f32; 4],
    pub shallow_color: [f32; 4],
    pub foam_color: [f32; 4],
    pub sss_color: [f32; 4],
    pub foam_threshold: f32,
    pub foam_noise_scale: f32,
    pub ramp_steps: f32,
    pub specular_strength: f32,
    pub band_wobble: f32,
    pub band_dither: f32,
    pub band_dither_scale: f32,
    pub foam_glow_color: [f32; 4],
    pub foam_glow: f32,
    // Contact foam (splash around the ocean_contact hull)
    pub splash_strength: f32,
    pub splash_width: f32,
    pub splash_flicker_speed: f32,
    pub splash_noise_scale: f32,
    pub splash_baseline: f32,
    pub splash_response: f32,
    // Shoreline foam (from the grab pass's depth)
    pub shore_foam_width: f32,
    pub shore_foam_strength: f32,
    pub shore_foam_speed: f32,
    pub shore_foam_color: [f32; 4],
    pub shore_foam_reach: f32,
    // Refraction
    pub turbidity: f32,
    pub refraction_strength: f32,
    pub sky_reflection_strength: f32,
    // Grid
    pub grid_scale: f32,
    pub fade_start: f32,
    pub fade_end: f32,
}

impl OceanPanelConfig {
    /// Seed the editors from the `ocean` component through the typed view
    /// (ADR 0115, #1068): a missing field keeps its default, and one of the
    /// wrong type keeps it too and is reported once.
    pub fn from_component(view: &ComponentView<'_>) -> Self {
        let p = OceanParams::from_component(view);
        let f = |name: &str, dv: f32| view.f32(name, dv);
        let c = |name: &str, dv: [f32; 4]| view.color(name, dv);
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
            wind_speed: p.wind_speed,
            fetch_km: p.fetch_km,
            peak_enhancement: p.peak_enhancement,
            deep_color: c("deep_color", [0.055, 0.22, 0.53, 1.0]),
            shallow_color: c("shallow_color", [0.16, 0.51, 0.83, 1.0]),
            foam_color: c("foam_color", [0.96, 0.99, 1.0, 1.0]),
            sss_color: c("sss_color", [0.10, 0.75, 0.75, 1.0]),
            foam_threshold: f("foam_threshold", 0.62),
            foam_noise_scale: f("foam_noise_scale", 0.35),
            ramp_steps: f("ramp_steps", 3.0),
            specular_strength: f("specular_strength", 1.0),
            band_wobble: f("band_wobble", 0.2),
            band_dither: f("band_dither", 0.0),
            band_dither_scale: f("band_dither_scale", 1.5),
            foam_glow_color: c("foam_glow_color", [0.35, 0.95, 0.75, 1.0]),
            foam_glow: f("foam_glow", 0.0),
            splash_strength: f("splash_strength", 1.0),
            splash_width: f("splash_width", 0.35),
            splash_flicker_speed: f("splash_flicker_speed", 1.6),
            splash_noise_scale: f("splash_noise_scale", 1.8),
            splash_baseline: f("splash_baseline", 0.15),
            splash_response: f("splash_response", 1.0),
            shore_foam_width: f("shore_foam_width", 0.0),
            shore_foam_strength: f("shore_foam_strength", 1.0),
            shore_foam_speed: f("shore_foam_speed", 1.0),
            shore_foam_color: c("shore_foam_color", [0.96, 0.99, 1.0, 1.0]),
            shore_foam_reach: f("shore_foam_reach", 0.0),
            turbidity: f("turbidity", 0.8),
            refraction_strength: f("refraction_strength", 0.6),
            sky_reflection_strength: f("sky_reflection_strength", 0.8),
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
            ("wind_speed", f(self.wind_speed)),
            ("fetch_km", f(self.fetch_km)),
            ("peak_enhancement", f(self.peak_enhancement)),
            ("deep_color", c(self.deep_color)),
            ("shallow_color", c(self.shallow_color)),
            ("foam_color", c(self.foam_color)),
            ("sss_color", c(self.sss_color)),
            ("foam_threshold", f(self.foam_threshold)),
            ("foam_noise_scale", f(self.foam_noise_scale)),
            ("ramp_steps", f(self.ramp_steps)),
            ("specular_strength", f(self.specular_strength)),
            ("band_wobble", f(self.band_wobble)),
            ("band_dither", f(self.band_dither)),
            ("band_dither_scale", f(self.band_dither_scale)),
            ("foam_glow_color", c(self.foam_glow_color)),
            ("foam_glow", f(self.foam_glow)),
            ("splash_strength", f(self.splash_strength)),
            ("splash_width", f(self.splash_width)),
            ("splash_flicker_speed", f(self.splash_flicker_speed)),
            ("splash_noise_scale", f(self.splash_noise_scale)),
            ("splash_baseline", f(self.splash_baseline)),
            ("splash_response", f(self.splash_response)),
            ("shore_foam_width", f(self.shore_foam_width)),
            ("shore_foam_strength", f(self.shore_foam_strength)),
            ("shore_foam_speed", f(self.shore_foam_speed)),
            ("shore_foam_color", c(self.shore_foam_color)),
            ("shore_foam_reach", f(self.shore_foam_reach)),
            ("turbidity", f(self.turbidity)),
            ("refraction_strength", f(self.refraction_strength)),
            ("sky_reflection_strength", f(self.sky_reflection_strength)),
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
    /// The parity probe is shown: the panel's own switch, never the
    /// component's (#749).
    show_probe: bool,
    /// The spectrum the probe samples, made again when the ocean changes.
    probe_spectrum: Option<WaveSpectrum>,
}

/// How far ahead of the camera the probe stands on the water (m).
const PROBE_AHEAD: f32 = 6.0;
/// The probe's colour (display sRGB): an orange no sea is.
const PROBE_COLOR: [f32; 4] = [1.0, 0.42, 0.08, 1.0];

impl OceanDebugPanel {
    pub fn new(config: OceanPanelConfig, scene_path: PathBuf, ocean_entity_name: String) -> Self {
        Self {
            original: config.clone(),
            config,
            scene_path,
            ocean_entity_name,
            open: false,
            dirty: false,
            show_probe: false,
            probe_spectrum: None,
        }
    }

    /// Whether the parity probe is on.
    pub fn probe_shown(&self) -> bool {
        self.show_probe
    }

    pub fn set_probe_shown(&mut self, shown: bool) {
        self.show_probe = shown;
    }

    /// The CPU/GPU parity probe's lines this frame (none while it is off):
    /// a cross and a stem pinned at the CPU's `sample_height` for the
    /// `ocean` component as it stands, at a point on the water
    /// `PROBE_AHEAD` metres in front of the camera, on the waves' clock
    /// `time`. The world lines are depth-tested, so the stem vanishes into
    /// the water exactly at the cross while the two agree; when the cross
    /// floats above the drawn surface or sinks under it, the sampler and
    /// the shader have diverged and nothing that floats can be trusted.
    pub fn probe_lines(
        &mut self,
        ocean: &ComponentView<'_>,
        time: f64,
        camera_pos: [f32; 3],
        camera_forward: [f32; 3],
    ) -> Vec<WorldLine> {
        if !self.show_probe {
            return Vec::new();
        }
        let params = OceanParams::from_component(ocean);
        if self
            .probe_spectrum
            .as_ref()
            .is_none_or(|s| s.params != params)
        {
            self.probe_spectrum = Some(WaveSpectrum::generate(&params));
        }
        let spectrum = self.probe_spectrum.as_ref().expect("made just above");
        let flat = [camera_forward[0], camera_forward[2]];
        let len = (flat[0] * flat[0] + flat[1] * flat[1]).sqrt();
        let dir = if len > 1e-4 {
            [flat[0] / len, flat[1] / len]
        } else {
            [0.0, -1.0]
        };
        let x = camera_pos[0] + dir[0] * PROBE_AHEAD;
        let z = camera_pos[2] + dir[1] * PROBE_AHEAD;
        let h = spectrum.sample_height(x, z, time);
        let line = |a: [f32; 3], b: [f32; 3]| WorldLine {
            a,
            b,
            color: PROBE_COLOR,
            thickness: 2.5,
        };
        let arm = 0.45;
        vec![
            line([x, h - 0.6, z], [x, h + 0.9, z]),
            line([x - arm, h, z], [x + arm, h, z]),
            line([x, h, z - arm], [x, h, z + arm]),
        ]
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

    fn weight(&self) -> u32 {
        46
    }

    /// Push an edit into the world's `ocean` component: the renderer's
    /// extraction and the script API both read from there.
    fn sync(&mut self, ctx: &mut crate::PanelCtx) {
        if !self.dirty {
            return;
        }
        if let Some(id) = ctx.world.get_id(&self.ocean_entity_name) {
            if let Some(mut comps) = ctx.world.get_components_mut(id) {
                for (field, value) in self.config.to_fields() {
                    comps.set_field(flint_geometry::components::OCEAN, field, value);
                }
            }
        }
        self.dirty = false;
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;

        egui::CollapsingHeader::new("Waves")
            .default_open(true)
            .show(ui, |ui| {
                changed |= drag_f32(
                    ui,
                    "Amplitude (m)",
                    &mut self.config.amplitude,
                    0.01,
                    0.0..=6.0,
                );
                changed |= drag_f32(
                    ui,
                    "Choppiness",
                    &mut self.config.choppiness,
                    0.005,
                    0.0..=1.0,
                );
                changed |= drag_f32(
                    ui,
                    "Wavelength Min",
                    &mut self.config.wavelength_min,
                    0.1,
                    0.5..=50.0,
                );
                changed |= drag_f32(
                    ui,
                    "Wavelength Max",
                    &mut self.config.wavelength_max,
                    0.5,
                    2.0..=400.0,
                );
                changed |= drag_f32(
                    ui,
                    "Direction (deg)",
                    &mut self.config.direction_deg,
                    1.0,
                    -360.0..=360.0,
                );
                changed |= drag_f32(
                    ui,
                    "Spread (deg)",
                    &mut self.config.spread_deg,
                    1.0,
                    0.0..=180.0,
                );
                changed |= drag_f32(
                    ui,
                    "Speed Scale",
                    &mut self.config.speed_scale,
                    0.01,
                    0.0..=8.0,
                );
                changed |= drag_f32(
                    ui,
                    "Wind (m/s)",
                    &mut self.config.wind_speed,
                    0.1,
                    0.5..=40.0,
                );
                changed |= drag_f32(
                    ui,
                    "Fetch (km)",
                    &mut self.config.fetch_km,
                    1.0,
                    1.0..=2000.0,
                );
                changed |= drag_f32(
                    ui,
                    "Peak Gamma",
                    &mut self.config.peak_enhancement,
                    0.05,
                    1.0..=10.0,
                );
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
                changed |= drag_f32(
                    ui,
                    "Foam Threshold",
                    &mut self.config.foam_threshold,
                    0.005,
                    0.0..=1.5,
                );
                changed |= drag_f32(
                    ui,
                    "Foam Noise Scale",
                    &mut self.config.foam_noise_scale,
                    0.005,
                    0.01..=4.0,
                );
                changed |= drag_f32(
                    ui,
                    "Ramp Steps",
                    &mut self.config.ramp_steps,
                    0.05,
                    1.0..=8.0,
                );
                changed |= drag_f32(
                    ui,
                    "Specular",
                    &mut self.config.specular_strength,
                    0.01,
                    0.0..=4.0,
                );
                changed |= color_rgba(ui, "Glow Color", &mut self.config.foam_glow_color);
                changed |= drag_f32(
                    ui,
                    "Foam Glow",
                    &mut self.config.foam_glow,
                    0.005,
                    0.0..=2.0,
                );
                changed |= drag_f32(
                    ui,
                    "Edge Wobble",
                    &mut self.config.band_wobble,
                    0.005,
                    0.0..=1.0,
                );
                changed |= drag_f32(
                    ui,
                    "Edge Dither",
                    &mut self.config.band_dither,
                    0.005,
                    0.0..=1.0,
                );
                changed |= drag_f32(
                    ui,
                    "Dither Dots/m",
                    &mut self.config.band_dither_scale,
                    0.01,
                    0.05..=16.0,
                );
            });

        egui::CollapsingHeader::new("Contact Foam")
            .default_open(true)
            .show(ui, |ui| {
                changed |= drag_f32(
                    ui,
                    "Splash Strength",
                    &mut self.config.splash_strength,
                    0.01,
                    0.0..=4.0,
                );
                changed |= drag_f32(
                    ui,
                    "Splash Width (m)",
                    &mut self.config.splash_width,
                    0.005,
                    0.01..=2.0,
                );
                changed |= drag_f32(
                    ui,
                    "Flicker Speed",
                    &mut self.config.splash_flicker_speed,
                    0.01,
                    0.0..=8.0,
                );
                changed |= drag_f32(
                    ui,
                    "Edge Noise Scale",
                    &mut self.config.splash_noise_scale,
                    0.01,
                    0.05..=8.0,
                );
                changed |= drag_f32(
                    ui,
                    "Baseline",
                    &mut self.config.splash_baseline,
                    0.005,
                    0.0..=1.0,
                );
                changed |= drag_f32(
                    ui,
                    "Impact Response",
                    &mut self.config.splash_response,
                    0.01,
                    0.0..=8.0,
                );
            });

        egui::CollapsingHeader::new("Shoreline Foam")
            .default_open(true)
            .show(ui, |ui| {
                changed |= drag_f32(
                    ui,
                    "Shore Width (m)",
                    &mut self.config.shore_foam_width,
                    0.005,
                    0.0..=10.0,
                );
                changed |= drag_f32(
                    ui,
                    "Shore Strength",
                    &mut self.config.shore_foam_strength,
                    0.01,
                    0.0..=2.0,
                );
                changed |= drag_f32(
                    ui,
                    "Lap Speed",
                    &mut self.config.shore_foam_speed,
                    0.01,
                    0.0..=10.0,
                );
                changed |= drag_f32(
                    ui,
                    "Shore Reach (m)",
                    &mut self.config.shore_foam_reach,
                    0.01,
                    0.0..=10.0,
                );
                changed |= color_rgba(ui, "Shore Color", &mut self.config.shore_foam_color);
            });

        egui::CollapsingHeader::new("Water Clarity")
            .default_open(true)
            .show(ui, |ui| {
                changed |= drag_f32(
                    ui,
                    "Turbidity",
                    &mut self.config.turbidity,
                    0.01,
                    0.0..=10.0,
                );
                changed |= drag_f32(
                    ui,
                    "Refraction",
                    &mut self.config.refraction_strength,
                    0.01,
                    0.0..=4.0,
                );
                changed |= drag_f32(
                    ui,
                    "Sky Reflection",
                    &mut self.config.sky_reflection_strength,
                    0.01,
                    0.0..=2.0,
                );
            });

        egui::CollapsingHeader::new("Grid")
            .default_open(false)
            .show(ui, |ui| {
                changed |= drag_f32(
                    ui,
                    "Grid Scale",
                    &mut self.config.grid_scale,
                    0.5,
                    10.0..=500.0,
                );
                changed |= drag_f32(
                    ui,
                    "Fade Start",
                    &mut self.config.fade_start,
                    1.0,
                    5.0..=1000.0,
                );
                changed |= drag_f32(
                    ui,
                    "Fade End",
                    &mut self.config.fade_end,
                    1.0,
                    10.0..=2000.0,
                );
            });

        egui::CollapsingHeader::new("Diagnostics")
            .default_open(false)
            .show(ui, |ui| {
                // The probe is the panel's, not the scene's: no edit to
                // commit, so it doesn't make the panel dirty
                ui.checkbox(&mut self.show_probe, "Wave probe")
                    .on_hover_text(
                        "CPU/GPU parity check: an orange cross pinned to \
                         ocean_height() ahead of the camera. It must sit \
                         exactly on the drawn surface; if it floats or sinks, \
                         the sampler and the shader have diverged.",
                    );
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
                ui.ctx().copy_text(snippet);
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

#[cfg(test)]
mod probe_tests {
    use super::*;
    use flint_ecs::FlintWorld;

    /// A world holding one `ocean` component, on the entity `ocean`.
    fn sea(text: &str) -> (FlintWorld, flint_core::EntityId) {
        let mut world = FlintWorld::new();
        let id = world.spawn("ocean").unwrap();
        let data: toml::Value = toml::from_str(text).unwrap();
        world
            .set_component(id, flint_geometry::components::OCEAN, data)
            .unwrap();
        (world, id)
    }

    fn panel(ocean: &ComponentView<'_>) -> OceanDebugPanel {
        OceanDebugPanel::new(
            OceanPanelConfig::from_component(ocean),
            PathBuf::from("scene.toml"),
            "ocean".into(),
        )
    }

    #[test]
    fn the_probe_is_off_until_ticked() {
        let (world, id) = sea("amplitude = 0.8");
        let ocean = world.view(id, flint_geometry::components::OCEAN);
        let mut p = panel(&ocean);
        assert!(p
            .probe_lines(&ocean, 3.0, [0.0, 2.0, 0.0], [0.0, 0.0, -1.0])
            .is_empty());
    }

    #[test]
    fn a_wrong_field_seeds_its_default_and_is_reported_once() {
        let (world, id) = sea("amplitude = \"high\"\ndeep_color = 3\nfoam_glow = 2");
        let ocean = world.view(id, flint_geometry::components::OCEAN);
        let config = OceanPanelConfig::from_component(&ocean);
        let again = OceanPanelConfig::from_component(&ocean);
        assert_eq!(config, again);
        assert_eq!(config.amplitude, OceanParams::default().amplitude);
        assert_eq!(config.deep_color, [0.055, 0.22, 0.53, 1.0]);
        assert_eq!(config.foam_glow, 2.0);
        let fields: Vec<String> = world
            .read_reports()
            .snapshot()
            .iter()
            .map(|d| {
                format!(
                    "{}.{}",
                    d.entity.as_deref().unwrap(),
                    d.field.as_deref().unwrap()
                )
            })
            .collect();
        assert_eq!(fields, ["ocean.amplitude", "ocean.deep_color"]);
    }

    #[test]
    fn the_probe_crosses_at_the_cpu_height_ahead_of_the_camera() {
        let (world, id) = sea("amplitude = 0.8\nseed = 4");
        let ocean = world.view(id, flint_geometry::components::OCEAN);
        let mut p = panel(&ocean);
        p.set_probe_shown(true);
        let t = 7.25;
        let lines = p.probe_lines(&ocean, t, [1.0, 3.0, 2.0], [0.6, -0.5, -0.8]);
        assert_eq!(lines.len(), 3);
        // Six metres along the camera's forward laid flat: (0.6, -0.8)
        let (x, z) = (1.0 + 0.6 * PROBE_AHEAD, 2.0 - 0.8 * PROBE_AHEAD);
        let h = WaveSpectrum::generate(&OceanParams::from_component(&ocean)).sample_height(x, z, t);
        let cross = lines[1];
        assert!((cross.a[1] - h).abs() < 1e-6 && (cross.b[1] - h).abs() < 1e-6);
        assert!(
            (cross.a[2] - z).abs() < 1e-5 && ((cross.a[0] + cross.b[0]) / 2.0 - x).abs() < 1e-5
        );
        // The stem runs through it, into the water and out
        let stem = lines[0];
        assert!(stem.a[1] < h && stem.b[1] > h);
    }

    #[test]
    fn the_probe_is_not_a_scene_edit() {
        // The component no longer has show_probe, and ticking it commits nothing
        let (world, id) = sea("amplitude = 0.8");
        let p = panel(&world.view(id, flint_geometry::components::OCEAN));
        assert!(p
            .config()
            .to_fields()
            .iter()
            .all(|(name, _)| *name != "show_probe"));
        assert!(!p.is_dirty());
    }
}
