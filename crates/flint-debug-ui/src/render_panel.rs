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
use crate::widgets::section;
use crate::DebugPanel;
use flint_render::style_mask::{legend, LegendEntry, StyleIds};
use flint_render::stylize::StyleSettings;
use flint_render::{DebugMode, LightingLevers, PostProcessConfig};
use flint_schema::post_effects::{self, PostParam, PostParamMut, PostWidget};

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
    /// Seconds a picked look fades in over (#496; 0 cuts), offered only
    /// by a host that fades looks ([`offer_look_fade`](Self::offer_look_fade)).
    pub look_seconds: f32,
    fades_looks: bool,
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
            look_seconds: 0.0,
            fades_looks: false,
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

    /// [`set_scene_dir`](Self::set_scene_dir) for a host that holds the
    /// scene's project (ADR 0114): the choices are searched from `assets`.
    pub fn set_asset_scope(&mut self, assets: Option<flint_core::project::AssetScope>) {
        self.styles_editor.set_asset_scope(assets);
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

    /// Offer a fade time beside the Look picker (#496): the player eases
    /// to a picked look over it; a host that can only cut (the viewer)
    /// leaves it off.
    pub fn offer_look_fade(&mut self, on: bool) {
        self.fades_looks = on;
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
                if self.fades_looks {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.look_seconds)
                                .range(0.0..=30.0)
                                .speed(0.05)
                                .suffix(" s"),
                        )
                        .on_hover_text(
                            "How long a picked look takes to fade in: numbers ease, a style \
                             in one look alone fades by its blend, options switch halfway. \
                             0 cuts. A script's set_look(name, seconds) does the same.",
                        );
                        ui.label("Fade");
                    });
                }
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
                        "Scripts and a rhythm game's ladder re-stamp several post fields \
                         every frame (exposure, vignette, bloom intensity, chromatic, radial \
                         blur, SSAO intensity, fog density/color, desaturate, DoF, render \
                         mode). While frozen, those stamps are skipped so panel edits stick.",
                    );
                ui.separator();
            }
            f.pp_changed |= ui
                .checkbox(&mut self.pp.enabled, "Post-processing")
                .changed();
        });

        // Every built-in effect, one section each, its widgets drawn from
        // its descriptor (#741); after them, what is the renderer's own
        // and no scene key (ground fog, the boil's clock) or does more
        // than set a field (the neutral grade)
        let fxaa_was_on = self.pp.fxaa;
        for effect in post_effects::EFFECTS {
            section(ui, effect.label, false, |ui| {
                for param in effect.params {
                    f.pp_changed |= param_widget(ui, &mut self.pp, param);
                }
                match effect.name {
                    "fog" => {
                        // Ground fog (ADR 0106): shows where the game has set a map
                        ui.separator();
                        f.pp_changed |= slider_log(
                            ui,
                            "Ground density",
                            &mut self.pp.ground_fog_density,
                            0.0..=2.0,
                        );
                        f.pp_changed |=
                            color_row(ui, "Ground color", &mut self.pp.ground_fog_color);
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
                    }
                    "grade" => {
                        if ui.button("Neutral grade").clicked() {
                            self.pp.grade_lift = [0.0; 3];
                            self.pp.grade_gamma = [1.0; 3];
                            self.pp.grade_gain = [1.0; 3];
                            f.pp_changed = true;
                        }
                    }
                    "timing" => {
                        // The boil's amounts are the Kuwahara and contour
                        // styles' own `boil` (#599), in Styles below
                        ui.label("Boil amounts: Styles > Kuwahara / Contour > Boil");
                        ui.label(boil_status(&self.pp.boil));
                    }
                    "tear" => {
                        // What tears (#746): the render modes' looks are
                        // styles now, and follow the tear by a table key
                        ui.weak(
                            "Shown through it: a style with blend_source = \"tear\" \
                             (Styles: matrix, blood, tron...) or a prop's style component",
                        );
                    }
                    _ => {}
                }
            });
        }
        if !fxaa_was_on && self.pp.fxaa {
            f.fxaa_needs_resources = true;
        }

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
            ui.weak("The shared geometry edges every style reads: Shared edges, above");
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

/// What the wooze field is telling the boil here and now. Its sliders are shares
/// of this, so a scene with no field needs to say why they do nothing.
fn boil_status(boil: &flint_wooze::BoilFrame) -> String {
    if boil.step <= 0.0 {
        "No [wooze] table here, so nothing boils.".to_string()
    } else {
        format!(
            "Field: step {:.3} s, jitter {:.2}, drawing {}",
            boil.step, boil.amount, boil.seed
        )
    }
}

/// One built-in effect parameter's widget, as its descriptor says (#741):
/// a checkbox, a slider over the range it is tuned for (logarithmic for
/// one spanning decades), a drag value, a colour or three channels, its
/// help on hover. A value the scene set past the range is shown as it is
/// and only clamped once the slider is moved.
fn param_widget(ui: &mut egui::Ui, pp: &mut PostProcessConfig, param: &PostParam) -> bool {
    let Some(field) = pp.param_mut(param.key) else {
        return false;
    };
    let range = param.min..=param.max;
    let clamping = egui::SliderClamping::Edits;
    let response = match (field, param.widget) {
        (PostParamMut::Bool(on), _) => {
            return ui
                .checkbox(on, param.label)
                .on_hover_text(param.help)
                .changed()
        }
        (PostParamMut::F32(value), PostWidget::Drag(speed)) => labelled(ui, param, |ui| {
            ui.add(egui::DragValue::new(value).speed(speed))
        }),
        (PostParamMut::F32(value), widget) => labelled(ui, param, |ui| {
            ui.add(
                egui::Slider::new(value, range)
                    .logarithmic(widget == PostWidget::Log)
                    .clamping(clamping),
            )
        }),
        (PostParamMut::U32(value), _) => labelled(ui, param, |ui| {
            ui.add(egui::Slider::new(value, param.min as u32..=param.max as u32).clamping(clamping))
        }),
        (PostParamMut::Rgb(rgb), PostWidget::Channels(speed)) => {
            return ui
                .horizontal(|ui| {
                    ui.label(param.label).on_hover_text(param.help);
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
        (PostParamMut::Rgb(rgb), _) => labelled(ui, param, |ui| ui.color_edit_button_rgb(rgb)),
    };
    response.changed()
}

/// A row of `param`'s label and the widget `add` makes, its help on both.
fn labelled(
    ui: &mut egui::Ui,
    param: &PostParam,
    add: impl FnOnce(&mut egui::Ui) -> egui::Response,
) -> egui::Response {
    ui.horizontal(|ui| {
        ui.label(param.label).on_hover_text(param.help);
        add(ui).on_hover_text(param.help)
    })
    .inner
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

fn color_row(ui: &mut egui::Ui, label: &str, rgb: &mut [f32; 3]) -> bool {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.color_edit_button_rgb(rgb).changed()
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

    fn weight(&self) -> u32 {
        60
    }

    /// Mirror the renderer while clean; while dirty, write the edits
    /// through, each group by its flag (the costly ones only when asked).
    fn sync(&mut self, ctx: &mut crate::PanelCtx) {
        let renderer = &mut *ctx.renderer;
        ctx.host.freeze_scripts = Some(self.freeze_scripts);
        if self.dirty {
            let flags = self.take_flags();
            if flags.look_changed {
                ctx.host.look = Some(self.look.clone());
                ctx.host.look_seconds = if self.fades_looks {
                    self.look_seconds as f64
                } else {
                    0.0
                };
            }
            if flags.styles_committed {
                ctx.host.committed_styles = self.authored_styles.clone();
            }
            if flags.pp_changed {
                renderer.set_post_process_config(self.pp.clone());
                if flags.style_needs_resources {
                    renderer.ensure_style_resources(ctx.device);
                }
                if flags.fxaa_needs_resources {
                    renderer.ensure_fxaa_resources(ctx.device);
                }
            }
            if flags.shadows_changed {
                renderer.set_shadows(self.shadows_enabled);
            }
            if flags.shadow_res_changed {
                renderer.set_shadow_resolution(ctx.device, self.shadow_resolution);
            }
            if flags.lighting_reset {
                renderer.reset_ambient();
            } else if flags.lighting_changed {
                renderer.set_ambient(self.lighting.ambient_sky, self.lighting.ambient_ground);
                renderer.set_diffuse_wrap(self.lighting.diffuse_wrap);
                renderer.set_oren_nayar(self.lighting.oren_nayar);
                renderer.set_sheen(self.lighting.sheen_color, self.lighting.sheen_strength);
            }
            if flags.mode_changed {
                // Its overlays are read from the world at the next frame's
                // `prepare` (#739)
                renderer.set_debug_mode(self.debug_mode);
            }
            if flags.fov_changed {
                ctx.camera.fov = self.fov_deg;
            }
            self.dirty = false;
        } else {
            self.refresh(
                renderer.post_process_config(),
                renderer.debug_state().mode,
                renderer.shadows_enabled(),
                renderer.shadow_resolution(),
                renderer.lighting_levers(),
                ctx.camera.fov,
            );
        }
        self.styles_editor.set_styles(renderer.styles());
        self.set_style_ids(renderer.style_ids());
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
