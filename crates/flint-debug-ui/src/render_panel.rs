//! Rendering & Effects panel (F4).
//!
//! One consolidated home for every render/post-effect debug control that
//! used to live on scattered function keys (F1 debug-mode cycle, F4 shadows,
//! F5-F10 per-effect toggles, F12 kuwahara / DoF-follow), plus the
//! non-binary parameters those keys could never expose (SSAO radius/samples,
//! DoF focus/range, grade lift/gamma/gain, render mode/mix/params, shadow
//! resolution, lighting levers, FOV).
//!
//! Ownership model (ADR 0053): the host refreshes the panel from live
//! renderer state every frame while the panel is clean ("live mirror"), and
//! applies the panel's state back when the user edits something
//! ("write-through"), routing expensive operations by the per-group change
//! flags. Fields a script drives are visibly reclaimed next frame unless the
//! player-only "freeze script post overrides" switch is on — the panel shows
//! the truth about who owns a value rather than pretending to.

use crate::DebugPanel;
use flint_render::{DebugMode, LightingLevers, PostProcessConfig};

pub const RENDER_DEBUG_PANEL: &str = "Rendering & Effects";

const SHADOW_RESOLUTIONS: [u32; 4] = [512, 1024, 2048, 4096];

/// Per-group change flags the host consumes on apply. Groups map to distinct
/// renderer calls (post config upload vs shadow-pass rebuild vs lighting
/// setters vs debug-mode pipeline swap vs camera FOV).
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderPanelFlags {
    pub pp_changed: bool,
    pub mode_changed: bool,
    pub shadows_changed: bool,
    pub shadow_res_changed: bool,
    pub lighting_changed: bool,
    pub lighting_reset: bool,
    pub fov_changed: bool,
    /// Kuwahara was just enabled — host must call ensure_kuwahara_resources.
    pub kuwahara_needs_resources: bool,
    /// FXAA was just enabled — host must call ensure_fxaa_resources.
    pub fxaa_needs_resources: bool,
}

impl RenderPanelFlags {
    fn any(&self) -> bool {
        self.pp_changed
            || self.mode_changed
            || self.shadows_changed
            || self.shadow_res_changed
            || self.lighting_changed
            || self.lighting_reset
            || self.fov_changed
    }
}

pub struct RenderDebugPanel {
    /// Edited in place; uploaded wholesale by the host on pp_changed.
    pub pp: PostProcessConfig,
    pub debug_mode: DebugMode,
    pub shadows_enabled: bool,
    pub shadow_resolution: u32,
    pub lighting: LightingLevers,
    pub fov_deg: f32,
    /// Player-only: skip the script/ladder post-override stamp while true so
    /// panel edits to contended fields stick. Shown only when `show_freeze`.
    pub freeze_scripts: bool,
    show_freeze: bool,
    flags: RenderPanelFlags,
    open: bool,
    dirty: bool,
}

impl RenderDebugPanel {
    /// `show_freeze`: true in the player (scripts drive post fields there),
    /// false in the viewer (no scripts, nothing to freeze).
    pub fn new(show_freeze: bool) -> Self {
        Self {
            pp: PostProcessConfig::default(),
            debug_mode: DebugMode::Pbr,
            shadows_enabled: true,
            shadow_resolution: 2048,
            lighting: LightingLevers {
                ambient_sky: [0.0; 3],
                ambient_ground: [0.0; 3],
                diffuse_wrap: 0.0,
                oren_nayar: 0.0,
                sheen_color: [0.0; 3],
                sheen_strength: 0.0,
            },
            fov_deg: 65.0,
            freeze_scripts: false,
            show_freeze,
            flags: RenderPanelFlags::default(),
            open: false,
            dirty: false,
        }
    }

    /// Mirror live renderer state into the widgets. No-op while the panel
    /// has unapplied edits so an in-progress drag is never yanked; never
    /// sets dirty (the mirror is not an edit).
    #[allow(clippy::too_many_arguments)]
    pub fn refresh(
        &mut self,
        pp: &PostProcessConfig,
        debug_mode: DebugMode,
        shadows_enabled: bool,
        shadow_resolution: u32,
        lighting: LightingLevers,
        fov_deg: f32,
    ) {
        if self.dirty {
            return;
        }
        self.pp = pp.clone();
        self.debug_mode = debug_mode;
        self.shadows_enabled = shadows_enabled;
        self.shadow_resolution = shadow_resolution;
        self.lighting = lighting;
        self.fov_deg = fov_deg;
    }

    /// Consume the per-group change flags (host apply pass).
    pub fn take_flags(&mut self) -> RenderPanelFlags {
        std::mem::take(&mut self.flags)
    }

    /// The panel body, exposed for hosts that embed it outside the
    /// DebugPanel loop (the viewer draws it inside its own egui Window).
    pub fn ui_contents(&mut self, ui: &mut egui::Ui) {
        let mut f = self.flags;

        section(ui, "Post chain", true, |ui| {
            if self.show_freeze {
                ui.checkbox(&mut self.freeze_scripts, "Freeze script post overrides")
                    .on_hover_text(
                        "Scripts and the reintegration ladder re-stamp several post fields \
                         every frame (exposure, vignette, bloom intensity, chromatic, radial \
                         blur, SSAO intensity, fog density/color, desaturate, DoF, render \
                         mode). While frozen, those stamps are skipped so panel edits stick.",
                    );
                ui.separator();
            }
            f.pp_changed |= ui
                .checkbox(&mut self.pp.enabled, "Post-processing")
                .changed();
            f.pp_changed |= slider_log(ui, "Exposure", &mut self.pp.exposure, 0.1..=4.0);
            f.pp_changed |= ui
                .checkbox(&mut self.pp.vignette_enabled, "Vignette")
                .changed();
            f.pp_changed |= slider(
                ui,
                "Vignette intensity",
                &mut self.pp.vignette_intensity,
                0.0..=1.0,
            );
            f.pp_changed |= slider(
                ui,
                "Vignette smoothness",
                &mut self.pp.vignette_smoothness,
                0.5..=5.0,
            );
            f.pp_changed |= slider(
                ui,
                "Chromatic aberration",
                &mut self.pp.chromatic_aberration,
                0.0..=0.02,
            );
            f.pp_changed |= slider(ui, "Radial blur", &mut self.pp.radial_blur, 0.0..=1.0);
            f.pp_changed |= slider(ui, "Desaturate", &mut self.pp.desaturate, 0.0..=1.0);
        });

        section(ui, "SSAO", false, |ui| {
            f.pp_changed |= ui.checkbox(&mut self.pp.ssao_enabled, "Enabled").changed();
            f.pp_changed |= slider(ui, "Radius", &mut self.pp.ssao_radius, 0.05..=2.0);
            f.pp_changed |= slider(ui, "Intensity", &mut self.pp.ssao_intensity, 0.0..=3.0);
            f.pp_changed |= slider(ui, "Bias", &mut self.pp.ssao_bias, 0.0..=0.1);
            let r = ui.horizontal(|ui| {
                ui.label("Samples");
                ui.add(egui::Slider::new(&mut self.pp.ssao_samples, 1..=64))
                    .on_hover_text("Heaviest per-pixel pass — 16 is the quality/cost sweet spot")
                    .changed()
            });
            f.pp_changed |= r.inner;
        });

        section(ui, "Depth of field", false, |ui| {
            f.pp_changed |= slider(ui, "Strength", &mut self.pp.dof_strength, 0.0..=1.5);
            f.pp_changed |= slider_log(
                ui,
                "Focus distance",
                &mut self.pp.dof_focus_distance,
                0.1..=100.0,
            );
            f.pp_changed |= slider_log(ui, "Focus range", &mut self.pp.dof_focus_range, 0.1..=60.0);
        });

        section(ui, "Fog", false, |ui| {
            f.pp_changed |= ui.checkbox(&mut self.pp.fog_enabled, "Enabled").changed();
            f.pp_changed |= color_row(ui, "Color", &mut self.pp.fog_color);
            f.pp_changed |= slider_log(ui, "Density", &mut self.pp.fog_density, 0.0005..=0.2);
            f.pp_changed |= drag(ui, "Start", &mut self.pp.fog_start, 0.5);
            f.pp_changed |= drag(ui, "End", &mut self.pp.fog_end, 0.5);
            f.pp_changed |= ui
                .checkbox(&mut self.pp.fog_height_enabled, "Height fog")
                .changed();
            f.pp_changed |= slider(
                ui,
                "Height falloff",
                &mut self.pp.fog_height_falloff,
                0.0..=1.0,
            );
            f.pp_changed |= drag(ui, "Height origin", &mut self.pp.fog_height_origin, 0.25);
        });

        section(ui, "Bloom", false, |ui| {
            f.pp_changed |= ui.checkbox(&mut self.pp.bloom_enabled, "Enabled").changed();
            f.pp_changed |= slider(ui, "Intensity", &mut self.pp.bloom_intensity, 0.0..=0.5);
            f.pp_changed |= slider(ui, "Threshold", &mut self.pp.bloom_threshold, 0.0..=3.0);
            f.pp_changed |= slider(
                ui,
                "Soft threshold",
                &mut self.pp.bloom_soft_threshold,
                0.0..=1.0,
            );
        });

        section(ui, "Grade / Grain / FXAA", false, |ui| {
            f.pp_changed |= rgb_drags(ui, "Lift", &mut self.pp.grade_lift, -0.25..=0.25, 0.002);
            f.pp_changed |= rgb_drags(ui, "Gamma", &mut self.pp.grade_gamma, 0.25..=4.0, 0.01);
            f.pp_changed |= rgb_drags(ui, "Gain", &mut self.pp.grade_gain, 0.0..=2.0, 0.01);
            if ui.button("Neutral grade").clicked() {
                self.pp.grade_lift = [0.0; 3];
                self.pp.grade_gamma = [1.0; 3];
                self.pp.grade_gain = [1.0; 3];
                f.pp_changed = true;
            }
            f.pp_changed |= slider(ui, "Film grain", &mut self.pp.film_grain, 0.0..=0.2);
            let was_off = !self.pp.fxaa_enabled;
            if ui.checkbox(&mut self.pp.fxaa_enabled, "FXAA").changed() {
                f.pp_changed = true;
                if was_off && self.pp.fxaa_enabled {
                    f.fxaa_needs_resources = true;
                }
            }
        });

        section(ui, "Kuwahara", false, |ui| {
            let was_off = !self.pp.kuwahara_enabled;
            if ui
                .checkbox(&mut self.pp.kuwahara_enabled, "Enabled")
                .changed()
            {
                f.pp_changed = true;
                if was_off && self.pp.kuwahara_enabled {
                    f.kuwahara_needs_resources = true;
                }
            }
            let r = ui.horizontal(|ui| {
                ui.label("Radius");
                ui.add(egui::Slider::new(&mut self.pp.kuwahara_radius, 1..=8))
                    .changed()
            });
            f.pp_changed |= r.inner;
            f.pp_changed |= slider(ui, "Sharpness", &mut self.pp.kuwahara_sharpness, 1.0..=16.0);
            f.pp_changed |= slider(ui, "Hardness", &mut self.pp.kuwahara_hardness, 1.0..=16.0);
            f.pp_changed |= slider(
                ui,
                "Anisotropy",
                &mut self.pp.kuwahara_anisotropy,
                0.0..=2.0,
            );
        });

        section(ui, "Render mode", false, |ui| {
            let labels = ["None", "Matrix", "Blood", "Drunk", "Tron", "Underwater"];
            let cur = (self.pp.render_mode as usize).min(labels.len() - 1);
            egui::ComboBox::from_label("Mode")
                .selected_text(labels[cur])
                .show_ui(ui, |ui| {
                    for (i, label) in labels.iter().enumerate() {
                        if ui.selectable_label(cur == i, *label).clicked() && cur != i {
                            self.pp.render_mode = i as u32;
                            f.pp_changed = true;
                        }
                    }
                });
            f.pp_changed |= slider(ui, "Mix", &mut self.pp.mode_mix, 0.0..=1.0);
            let hover = "Tears (1-4): x = mask scale, y = mask style (0 fbm / 1 iris), \
                         z = rate, w = spare. Underwater (5): x = signed eye depth m, \
                         y = sea energy, z = daylight, w = biolum.";
            let r = ui.horizontal(|ui| {
                ui.label("Params");
                let mut changed = false;
                for v in self.pp.mode_params.iter_mut() {
                    changed |= ui.add(egui::DragValue::new(v).speed(0.05)).changed();
                }
                changed
            });
            r.response.on_hover_text(hover);
            f.pp_changed |= r.inner;
        });

        section(ui, "Dither / Volumetric", false, |ui| {
            f.pp_changed |= ui.checkbox(&mut self.pp.dither_enabled, "Dither").changed();
            f.pp_changed |= slider(
                ui,
                "Dither intensity",
                &mut self.pp.dither_intensity,
                0.0..=0.1,
            );
            ui.separator();
            f.pp_changed |= ui
                .checkbox(&mut self.pp.volumetric_enabled, "Volumetric")
                .changed();
            let r = ui.horizontal(|ui| {
                ui.label("Samples");
                ui.add(egui::Slider::new(&mut self.pp.volumetric_samples, 8..=64))
                    .changed()
            });
            f.pp_changed |= r.inner;
            f.pp_changed |= slider(ui, "Density", &mut self.pp.volumetric_density, 0.0..=4.0);
            f.pp_changed |= slider(
                ui,
                "Max distance",
                &mut self.pp.volumetric_max_distance,
                10.0..=300.0,
            );
            f.pp_changed |= slider(ui, "Decay", &mut self.pp.volumetric_decay, 0.8..=1.0);
        });

        section(ui, "Shadows", false, |ui| {
            f.shadows_changed |= ui.checkbox(&mut self.shadows_enabled, "Enabled").changed();
            egui::ComboBox::from_label("Resolution")
                .selected_text(format!("{}", self.shadow_resolution))
                .show_ui(ui, |ui| {
                    for res in SHADOW_RESOLUTIONS {
                        if ui
                            .selectable_label(self.shadow_resolution == res, format!("{res}"))
                            .clicked()
                            && self.shadow_resolution != res
                        {
                            self.shadow_resolution = res;
                            f.shadow_res_changed = true;
                        }
                    }
                })
                .response
                .on_hover_text("Rebuilds the shadow pass");
        });

        section(ui, "Lighting", false, |ui| {
            f.lighting_changed |= color_row(ui, "Ambient sky", &mut self.lighting.ambient_sky);
            f.lighting_changed |=
                color_row(ui, "Ambient ground", &mut self.lighting.ambient_ground);
            f.lighting_changed |= slider(
                ui,
                "Diffuse wrap",
                &mut self.lighting.diffuse_wrap,
                0.0..=1.0,
            );
            f.lighting_changed |=
                slider(ui, "Oren-Nayar", &mut self.lighting.oren_nayar, 0.0..=1.0);
            f.lighting_changed |= color_row(ui, "Sheen color", &mut self.lighting.sheen_color);
            f.lighting_changed |= slider(
                ui,
                "Sheen strength",
                &mut self.lighting.sheen_strength,
                0.0..=0.3,
            );
            if ui
                .button("Reset lighting")
                .on_hover_text("Clears all overrides back to the built-in defaults")
                .clicked()
            {
                f.lighting_reset = true;
            }
        });

        section(ui, "Camera", false, |ui| {
            let r = ui.horizontal(|ui| {
                ui.label("Vertical FOV");
                let changed = ui
                    .add(
                        egui::Slider::new(&mut self.fov_deg, 40.0..=110.0)
                            .fixed_decimals(1)
                            .suffix("°"),
                    )
                    .changed();
                ui.label(fov_label(self.fov_deg));
                changed
            });
            f.fov_changed |= r.inner;
        });

        section(ui, "Debug mode", false, |ui| {
            egui::ComboBox::from_label("Shading")
                .selected_text(self.debug_mode.label())
                .show_ui(ui, |ui| {
                    for mode in DebugMode::ALL {
                        if ui
                            .selectable_label(self.debug_mode == mode, mode.label())
                            .clicked()
                            && self.debug_mode != mode
                        {
                            self.debug_mode = mode;
                            f.mode_changed = true;
                        }
                    }
                });
        });

        self.flags = f;
        if f.any() {
            self.dirty = true;
        }
    }
}

fn fov_label(fov_deg: f32) -> &'static str {
    match fov_deg {
        f if f < 50.0 => "narrow",
        f if f < 72.0 => "natural",
        f if f < 90.0 => "wide",
        _ => "fisheye",
    }
}

fn section(ui: &mut egui::Ui, title: &str, default_open: bool, add: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(title)
        .default_open(default_open)
        .show(ui, add);
}

fn slider(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::Slider::new(value, range)).changed()
    })
    .inner
}

fn slider_log(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::Slider::new(value, range).logarithmic(true))
            .changed()
    })
    .inner
}

fn drag(ui: &mut egui::Ui, label: &str, value: &mut f32, speed: f32) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::DragValue::new(value).speed(speed)).changed()
    })
    .inner
}

fn color_row(ui: &mut egui::Ui, label: &str, rgb: &mut [f32; 3]) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.color_edit_button_rgb(rgb).changed()
    })
    .inner
}

fn rgb_drags(
    ui: &mut egui::Ui,
    label: &str,
    rgb: &mut [f32; 3],
    range: std::ops::RangeInclusive<f32>,
    speed: f32,
) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        let mut changed = false;
        for v in rgb.iter_mut() {
            changed |= ui
                .add(egui::DragValue::new(v).speed(speed).range(range.clone()))
                .changed();
        }
        changed
    })
    .inner
}

impl DebugPanel for RenderDebugPanel {
    fn name(&self) -> &str {
        RENDER_DEBUG_PANEL
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.ui_contents(ui);
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
