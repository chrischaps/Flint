//! Rendering & Effects panel (F4).
//!
//! One consolidated home for every render/post-effect debug control that
//! used to live on scattered function keys (F1 debug-mode cycle, F4 shadows,
//! F5-F10 per-effect toggles, F12 kuwahara (now F4 -> Styles) / DoF-follow), plus the
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

use crate::styles_ui::StylesEditor;
use crate::DebugPanel;
use flint_render::style_mask::{legend, LegendEntry, StyleIds};
use flint_render::stylize::StyleSettings;
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
    /// A stylize pass (Kuwahara or a registered style) was just enabled —
    /// host must call `ensure_style_resources`.
    pub style_needs_resources: bool,
    /// The style editor wrote a style into the scene file (#364); the
    /// panel's authored settings already match it. A host with another
    /// editor (the player's F3 Styles) passes [`RenderDebugPanel::authored_styles`] on.
    pub styles_committed: bool,
    /// FXAA was just enabled — host must call ensure_fxaa_resources.
    pub fxaa_needs_resources: bool,
    /// A look was picked (#356): the host lays [`RenderDebugPanel::look`]
    /// under the scene's keys and uploads the result.
    pub look_changed: bool,
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
            || self.look_changed
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
    /// The scene's own `[post_process.styles]`, for the style editor's
    /// Authored button and markers; set by the host on scene load.
    authored_styles: Option<StyleSettings>,
    styles_editor: StylesEditor,
    /// The style mask view's legend (#309), shown under the debug mode
    /// while it is Style Mask; set by the host from the renderer's ids.
    style_legend: Vec<LegendEntry>,
    /// The looks the scene can reach (#356), set by the host.
    looks: Vec<LookEntry>,
    /// The look the scene draws with (`None`: its own keys alone); the
    /// picker writes the choice here and raises `look_changed`.
    pub look: Option<String>,
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
            authored_styles: None,
            styles_editor: StylesEditor::default(),
            style_legend: Vec::new(),
            looks: Vec::new(),
            look: None,
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

    /// The scene's authored style settings (its `[post_process.styles]`),
    /// or `None` when the host has no scene to compare against.
    pub fn set_authored_styles(&mut self, authored: Option<StyleSettings>) {
        self.authored_styles = authored;
    }

    /// The scene's authored style settings, as last set or committed.
    pub fn authored_styles(&self) -> Option<&StyleSettings> {
        self.authored_styles.as_ref()
    }

    /// The scene file the style editor's Commit to scene writes (#364);
    /// `None` hides the button (a scene from a bundle has no file).
    pub fn set_scene_file(&mut self, scene_file: Option<std::path::PathBuf>) {
        self.styles_editor.set_scene_file(scene_file);
    }

    /// Grey out Commit to scene, with the reason on hover (#364); `None`
    /// allows it.
    pub fn set_commit_blocked(&mut self, reason: Option<String>) {
        self.styles_editor.set_commit_blocked(reason);
    }

    /// The scene's folder, where the style editor finds the scene's own
    /// palettes and glyph sets to offer (#322); set by the host on scene
    /// load (and reload, to find new files).
    pub fn set_scene_dir(&mut self, scene_dir: Option<std::path::PathBuf>) {
        self.styles_editor.set_scene_dir(scene_dir);
    }

    /// The looks to offer (#356; `flint_render::look::list` of the
    /// scene) and the one in use, set by the host on scene load and after
    /// a script or the picker changes it.
    pub fn set_looks(&mut self, looks: &[flint_render::look::Look], current: Option<&str>) {
        self.looks = looks
            .iter()
            .map(|l| LookEntry {
                name: l.name.clone(),
                hover: match l.description.is_empty() {
                    true => l.source.clone(),
                    false => format!("{}\n\n{}", l.description, l.source),
                },
            })
            .collect();
        self.look = current.map(str::to_string);
    }

    /// The look in use, as the host last said (a script's `set_look`,
    /// #356), without touching the list on offer.
    pub fn set_current_look(&mut self, current: Option<&str>) {
        self.look = current.map(str::to_string);
    }

    /// The names of the looks on offer.
    pub fn look_names(&self) -> impl Iterator<Item = &str> {
        self.looks.iter().map(|l| l.name.as_str())
    }

    /// Pick a look as the menu does, raising `look_changed` for the host.
    /// For hosts and tests that drive the panel without a click.
    pub fn pick_look(&mut self, look: Option<&str>) {
        if self.look.as_deref() == look {
            return;
        }
        self.look = look.map(str::to_string);
        self.flags.look_changed = true;
        self.dirty = true;
    }

    /// Mirror the renderer's style ids into the Style Mask legend. Cheap
    /// every frame: it rebuilds only when the ids changed.
    pub fn set_style_ids(&mut self, ids: &StyleIds) {
        let same = self.style_legend.len() == ids.len()
            && self
                .style_legend
                .iter()
                .zip(ids.iter())
                .all(|(e, (id, name))| e.id == id && e.name == name);
        if !same {
            self.style_legend = legend(ids);
        }
    }

    /// The Style Mask legend's rows, in id order.
    pub fn style_legend(&self) -> &[LegendEntry] {
        &self.style_legend
    }

    /// The block the Styles section's last Copy TOML put on the clipboard.
    pub fn last_copied_toml(&self) -> Option<&str> {
        self.styles_editor.last_copied.as_deref()
    }

    /// Consume the per-group change flags (host apply pass).
    pub fn take_flags(&mut self) -> RenderPanelFlags {
        std::mem::take(&mut self.flags)
    }

    /// The panel body, exposed for hosts that embed it outside the
    /// DebugPanel loop (the viewer draws it inside its own egui Window).
    pub fn ui_contents(&mut self, ui: &mut egui::Ui) {
        let mut f = self.flags;

        // A look (#356): a whole preset, laid under the scene's own keys.
        // Picking one replaces the post config; the sections below then
        // tune what it set
        if !self.looks.is_empty() {
            section(ui, "Look", true, |ui| {
                let none = "None (the scene's keys)";
                let current = self.look.clone();
                let mut picked: Option<Option<String>> = None;
                egui::ComboBox::from_label("Look")
                    .selected_text(current.as_deref().unwrap_or(none))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), none).clicked() {
                            picked = Some(None);
                        }
                        for entry in &self.looks {
                            let on = current.as_deref() == Some(entry.name.as_str());
                            if ui
                                .selectable_label(on, &entry.name)
                                .on_hover_text(&entry.hover)
                                .clicked()
                            {
                                picked = Some(Some(entry.name.clone()));
                            }
                        }
                    })
                    .response
                    .on_hover_text(
                        "A styles/<name>.look.toml beside the scene, or a built-in. It goes \
                         under the scene's own [post_process] keys and replaces what this \
                         menu changed since.",
                    );
                if let Some(look) = current.as_deref() {
                    ui.weak(format!("[post_process] look = \"{look}\""));
                }
                if let Some(choice) = picked {
                    if choice != current {
                        self.look = choice;
                        f.look_changed = true;
                    }
                }
            });
        }

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
            f.pp_changed |= slider_log(
                ui,
                "Height density",
                &mut self.pp.fog_height_density,
                0.0..=0.5,
            );
            f.pp_changed |= color_row(ui, "Tint color", &mut self.pp.fog_tint_color);
            f.pp_changed |= slider(
                ui,
                "Tint strength",
                &mut self.pp.fog_tint_strength,
                0.0..=1.0,
            );
            f.pp_changed |= drag(ui, "Tint start", &mut self.pp.fog_tint_start, 1.0);
            f.pp_changed |= drag(ui, "Tint end", &mut self.pp.fog_tint_end, 1.0);
            // Ground fog (ADR 0106): shows where the game has set a map
            f.pp_changed |= slider_log(
                ui,
                "Ground density",
                &mut self.pp.ground_fog_density,
                0.0..=2.0,
            );
            f.pp_changed |= color_row(ui, "Ground color", &mut self.pp.ground_fog_color);
            f.pp_changed |= slider_log(
                ui,
                "Ground falloff",
                &mut self.pp.ground_fog_falloff,
                0.05..=5.0,
            );
            let r = ui.horizontal(|ui| {
                ui.label("Ground steps");
                ui.add(egui::Slider::new(&mut self.pp.ground_fog_steps, 1..=64))
                    .changed()
            });
            f.pp_changed |= r.inner;
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

        // Every registered style (#262; Kuwahara too since #298), one
        // section each, drawn from their descriptors (#268)
        section(ui, "Styles", false, |ui| {
            let edit = self
                .styles_editor
                .ui(ui, &mut self.pp, self.authored_styles.as_ref());
            f.pp_changed |= edit.changed;
            f.style_needs_resources |= edit.needs_resources;
            if let Some(name) = edit.committed {
                // The scene now says what the config says of the style
                self.authored_styles
                    .get_or_insert_with(StyleSettings::default)
                    .restore(name, &self.pp.styles);
                f.styles_committed = true;
            }
            // The shared geometry edges' thresholds (#301): one pair for
            // every style that reads them (edges_view shows what they find)
            ui.separator();
            ui.label("Shared edges");
            f.pp_changed |= slider(ui, "Silhouette step", &mut self.pp.edge_depth, 0.05..=4.0);
            f.pp_changed |= slider(ui, "Crease", &mut self.pp.edge_normal, 0.2..=8.0);
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
            if self.debug_mode == DebugMode::StyleMask {
                style_legend_ui(ui, &self.style_legend);
            }
            if self.debug_mode == DebugMode::StyleEdges {
                edge_legend_ui(ui);
            }
        });

        self.flags = f;
        if f.any() {
            self.dirty = true;
        }
    }
}

/// A look on offer in the picker.
#[derive(Debug, Clone)]
struct LookEntry {
    name: String,
    /// What it is and where it comes from.
    hover: String,
}

/// The Style Mask view's legend: a swatch of each id's hue, its id and
/// the style variant it stands for.
fn style_legend_ui(ui: &mut egui::Ui, rows: &[LegendEntry]) {
    if rows.is_empty() {
        ui.weak("No styled entities: the mask is empty");
        return;
    }
    for row in rows {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            // The mask view paints these display values
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            let [r, g, b] = row.color;
            ui.painter().rect_filled(
                rect,
                2.0,
                egui::Color32::from_rgb(byte(r), byte(g), byte(b)),
            );
            ui.label(format!("{}  {}", row.id, row.name))
                .on_hover_text(row.color_name);
        });
    }
    ui.weak("Grey: unstyled. White: where ids meet.");
}

/// The Style Edges view's legend (#349): what each colour marks.
fn edge_legend_ui(ui: &mut egui::Ui) {
    for (i, ([r, g, b], name)) in flint_render::stylize::EDGE_VIEW_LEGEND.iter().enumerate() {
        if i == 3 {
            ui.weak("Each glyph cell:");
        } else if i == 0 {
            ui.weak("Each edge pixel:");
        }
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            ui.painter().rect_filled(
                rect,
                2.0,
                egui::Color32::from_rgb(byte(*r), byte(*g), byte(*b)),
            );
            ui.label(*name);
        });
    }
    ui.weak(
        "A cell's foot bar is its edge length over edge_max, the tick edge_threshold. \
         Tune them, edge_depth and edge_normal under Styles > ascii.",
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use flint_render::style_mask::StyleVariant;

    #[test]
    fn the_style_mask_legend_follows_the_renderer_ids() {
        let mut panel = RenderDebugPanel::new(false);
        let mut ids = StyleIds::default();
        panel.set_style_ids(&ids);
        assert!(panel.style_legend().is_empty());
        ids.set(vec![
            ("pencil".into(), StyleVariant::plain("pencil")),
            ("ascii".into(), StyleVariant::plain("ascii")),
        ]);
        panel.set_style_ids(&ids);
        let rows: Vec<_> = panel
            .style_legend()
            .iter()
            .map(|e| (e.id, e.name.as_str()))
            .collect();
        assert_eq!(rows, [(1, "ascii"), (2, "pencil")]);
        ids.set(vec![("pencil".into(), StyleVariant::plain("pencil"))]);
        panel.set_style_ids(&ids);
        assert_eq!(panel.style_legend()[0].name, "pencil");
        assert_eq!(panel.style_legend()[0].id, 1, "ids renumber by name order");
    }

    /// #356: F4 offers the scene's looks and a pick reaches the host.
    #[test]
    fn the_look_picker_lists_looks_and_raises_a_flag() {
        let mut panel = RenderDebugPanel::new(true);
        let looks = flint_render::look::list("nowhere/a.scene.toml");
        panel.set_looks(&looks, Some("graphite"));
        let names: Vec<&str> = panel.look_names().collect();
        assert!(
            names.contains(&"graphite") && names.contains(&"matrix"),
            "{names:?}"
        );
        assert_eq!(panel.look.as_deref(), Some("graphite"));

        panel.pick_look(Some("graphite"));
        assert!(!panel.is_dirty(), "the same look is no change");
        panel.pick_look(Some("matrix"));
        assert!(panel.is_dirty());
        let flags = panel.take_flags();
        assert!(flags.look_changed && !flags.pp_changed);
        assert_eq!(panel.look.as_deref(), Some("matrix"));

        panel.pick_look(None);
        assert!(panel.take_flags().look_changed);
        assert_eq!(panel.look, None);
    }
}
