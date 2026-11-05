//! F3 "Living Props" panel (#218): every knob of a grove, live, with the warped
//! clock drawn as the curve it is, freeze and single-step, a pose overlay in the
//! world, and presets saved as TOML a scene's `[wooze]` table can name.
//!
//! The panel holds plain data; the host (`flint-player`) mirrors what's true into
//! it each frame while it's showing and drains what it asked for, so this crate
//! never depends on `flint-procgen`.

use crate::DebugPanel;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Vec2};
use flint_scene::{
    BoilDef, LivingDef, LivingEchoesDef, LivingVarianceDef, WoozeKnobs, WoozePreset,
};

pub const LIVING_PROPS_DEBUG_PANEL: &str = "Living Props";

/// The curve's colour, and the overlay's.
const INK: Color32 = Color32::from_rgb(232, 180, 90);
/// The parts behind the main dot, one per branch level.
const PARTS: Color32 = Color32::from_rgb(150, 120, 74);
const FROZEN: Color32 = Color32::from_rgb(132, 190, 232);

/// How parts fall out of phase, as a spec or a scene names it.
const STAGGERS: [(&str, &str); 3] = [
    ("none", "None: all at once"),
    ("per_level", "Per level: a tree shimmers from the trunk out"),
    ("per_branch", "Per branch: neighbours are never in step"),
];

/// Each branch level's phase, as a fraction of a step. Drawn, not computed with:
/// `flint_procgen::STAGGER_PER_LEVEL` is what the clock itself uses.
const LEVEL_PHASE: f64 = 0.15;
/// How many levels the readout draws.
const LEVELS_DRAWN: usize = 5;

/// Every knob, as the panel shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct LivingKnobs {
    pub step: f32,
    pub glide: f32,
    pub amplitude: f32,
    pub jitter: f32,
    pub stagger: String,
    pub boil_on: bool,
    pub boil_paint: f32,
    pub boil_ink: f32,
    pub boil_cadence: u32,
    pub echo_count: u32,
    pub echo_spread: f32,
    pub echo_alpha: f32,
    pub variance_amount: f32,
    pub variance_steady: f32,
    pub keep_size: bool,
}

impl Default for LivingKnobs {
    fn default() -> Self {
        Self::from_preset(&WoozePreset::default())
    }
}

impl LivingKnobs {
    /// The knobs a preset sets, and a sensible value for any it doesn't.
    pub fn from_preset(preset: &WoozePreset) -> Self {
        let echoes = preset.living.echoes.unwrap_or_default();
        let variance = preset.living.variance.unwrap_or_default();
        let boil = preset.boil.unwrap_or_default();
        Self {
            step: preset.wooze.step.unwrap_or(0.5),
            glide: preset.wooze.glide.unwrap_or(0.3),
            amplitude: preset.wooze.amplitude.unwrap_or(1.0),
            jitter: preset.wooze.jitter.unwrap_or(1.0),
            stagger: preset
                .living
                .stagger
                .clone()
                .unwrap_or_else(|| "per_level".to_string()),
            boil_on: boil.enabled.unwrap_or(true),
            boil_paint: boil.kuwahara.unwrap_or(0.0),
            boil_ink: boil.contour.unwrap_or(0.0),
            boil_cadence: boil.on.unwrap_or(2),
            echo_count: echoes.count.unwrap_or(6),
            echo_spread: echoes.spread.unwrap_or(0.9),
            echo_alpha: echoes.alpha.unwrap_or(0.2),
            variance_amount: variance.amount.unwrap_or(0.35),
            variance_steady: variance.steady.unwrap_or(0.8),
            keep_size: variance.keep_size.unwrap_or(true),
        }
    }

    /// Every knob, as a preset.
    pub fn to_preset(&self) -> WoozePreset {
        WoozePreset {
            wooze: WoozeKnobs {
                amplitude: Some(self.amplitude),
                step: Some(self.step),
                glide: Some(self.glide),
                jitter: Some(self.jitter),
            },
            living: LivingDef {
                shadows: None,
                stagger: Some(self.stagger.clone()),
                echoes: Some(LivingEchoesDef {
                    count: Some(self.echo_count),
                    spread: Some(self.echo_spread),
                    alpha: Some(self.echo_alpha),
                }),
                variance: Some(LivingVarianceDef {
                    amount: Some(self.variance_amount),
                    steady: Some(self.variance_steady),
                    keep_size: Some(self.keep_size),
                }),
            },
            boil: Some(BoilDef {
                enabled: Some(self.boil_on),
                kuwahara: Some(self.boil_paint),
                contour: Some(self.boil_ink),
                on: Some(self.boil_cadence),
            }),
        }
    }

    /// Only the knobs that moved from `before`, as a preset whose unset fields
    /// leave a knob alone: the one a hand turned, and nothing it happened to
    /// carry along. A knob in `touched` counts as moved whatever its number,
    /// since a `mixed` knob shows one prop's value and setting every prop to it
    /// is still a change.
    pub fn moved(&self, before: &LivingKnobs, touched: &KnobFlags) -> WoozePreset {
        fn pick<T: PartialEq + Clone>(now: &T, was: &T, touched: bool) -> Option<T> {
            (touched || now != was).then(|| now.clone())
        }
        let t = touched;
        let echoes = LivingEchoesDef {
            count: pick(&self.echo_count, &before.echo_count, t.echo_count),
            spread: pick(&self.echo_spread, &before.echo_spread, t.echo_spread),
            alpha: pick(&self.echo_alpha, &before.echo_alpha, t.echo_alpha),
        };
        let variance = LivingVarianceDef {
            amount: pick(
                &self.variance_amount,
                &before.variance_amount,
                t.variance_amount,
            ),
            steady: pick(
                &self.variance_steady,
                &before.variance_steady,
                t.variance_steady,
            ),
            keep_size: pick(&self.keep_size, &before.keep_size, t.keep_size),
        };
        let boil = BoilDef {
            enabled: pick(&self.boil_on, &before.boil_on, false),
            kuwahara: pick(&self.boil_paint, &before.boil_paint, false),
            contour: pick(&self.boil_ink, &before.boil_ink, false),
            on: pick(&self.boil_cadence, &before.boil_cadence, false),
        };
        WoozePreset {
            wooze: WoozeKnobs {
                amplitude: pick(&self.amplitude, &before.amplitude, t.amplitude),
                step: pick(&self.step, &before.step, t.step),
                glide: pick(&self.glide, &before.glide, t.glide),
                jitter: pick(&self.jitter, &before.jitter, t.jitter),
            },
            living: LivingDef {
                shadows: None,
                stagger: pick(&self.stagger, &before.stagger, t.stagger),
                echoes: (echoes != LivingEchoesDef::default()).then_some(echoes),
                variance: (variance != LivingVarianceDef::default()).then_some(variance),
            },
            boil: (boil != BoilDef::default()).then_some(boil),
        }
    }
}

/// One flag per knob that can differ from prop to prop: which ones the props
/// disagree on, or which ones a hand touched this frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KnobFlags {
    pub step: bool,
    pub glide: bool,
    pub amplitude: bool,
    pub jitter: bool,
    pub stagger: bool,
    pub echo_count: bool,
    pub echo_spread: bool,
    pub echo_alpha: bool,
    pub variance_amount: bool,
    pub variance_steady: bool,
    pub keep_size: bool,
}

impl KnobFlags {
    /// Whether any of the clock's four is set.
    pub fn any_clock(&self) -> bool {
        self.step || self.glide || self.amplitude || self.jitter
    }

    /// The knobs `preset` sets.
    pub fn set_by(preset: &WoozePreset) -> Self {
        let echoes = preset.living.echoes.unwrap_or_default();
        let variance = preset.living.variance.unwrap_or_default();
        Self {
            step: preset.wooze.step.is_some(),
            glide: preset.wooze.glide.is_some(),
            amplitude: preset.wooze.amplitude.is_some(),
            jitter: preset.wooze.jitter.is_some(),
            stagger: preset.living.stagger.is_some(),
            echo_count: echoes.count.is_some(),
            echo_spread: echoes.spread.is_some(),
            echo_alpha: echoes.alpha.is_some(),
            variance_amount: variance.amount.is_some(),
            variance_steady: variance.steady.is_some(),
            keep_size: variance.keep_size.is_some(),
        }
    }

    /// These, less the ones in `other`.
    pub fn without(self, other: &KnobFlags) -> Self {
        Self {
            step: self.step && !other.step,
            glide: self.glide && !other.glide,
            amplitude: self.amplitude && !other.amplitude,
            jitter: self.jitter && !other.jitter,
            stagger: self.stagger && !other.stagger,
            echo_count: self.echo_count && !other.echo_count,
            echo_spread: self.echo_spread && !other.echo_spread,
            echo_alpha: self.echo_alpha && !other.echo_alpha,
            variance_amount: self.variance_amount && !other.variance_amount,
            variance_steady: self.variance_steady && !other.variance_steady,
            keep_size: self.keep_size && !other.keep_size,
        }
    }
}

/// What a `mixed` knob says when hovered.
const MIXED: &str = "The species disagree here, so this shows the first prop's value. \
Moving it sets it on every prop; the knobs beside it stay each species' own.";

/// One prop's place on its clock.
#[derive(Debug, Clone, PartialEq)]
pub struct LivingPropRow {
    pub name: String,
    /// Which tree it is.
    pub lineage_seed: u64,
    pub position: [f32; 3],
    /// How tall its tree stands at rest.
    pub height: f32,
    /// Where its clock stands, in steps.
    pub beats: f64,
    pub step: f32,
    pub glide: f32,
    /// The pose being held or left, on the trunk side.
    pub pose: u64,
    /// How far toward the next, 0 to 1.
    pub weight: f32,
    /// That pose's shape seed.
    pub shape_seed: u64,
    pub ring_first: u64,
    pub ring_len: usize,
    pub ring_ready: usize,
    pub echoes: usize,
    /// On its old clocks, waiting for poses for new ones.
    pub waiting: bool,
}

/// How the rings are taking it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LivingHealth {
    /// Props whose rings aren't full this frame.
    pub rings_short: usize,
    /// Props waiting on poses for new clocks.
    pub waiting: usize,
    /// Whether poses are being held to a new variance.
    pub reholding: bool,
    /// Frames some prop has gone without a pose its clock was using.
    pub frames_starved: u64,
    /// Times a prop kept its last bones for want of a pose.
    pub blends_starved: u64,
}

/// What's true now, from the host.
#[derive(Debug, Clone, Default)]
pub struct LivingPanelState {
    /// Every knob. Where the props disagree, the first prop's value.
    pub knobs: LivingKnobs,
    /// The knobs the props disagree on, which the panel shows as `mixed`.
    pub mixed: KnobFlags,
    /// Whether the scene has no `[wooze]` table, so each prop runs its spec's
    /// clock until a clock knob gives it a field.
    pub no_field: bool,
    pub frozen: bool,
    pub overlay: bool,
    pub rows: Vec<LivingPropRow>,
    pub preset_names: Vec<String>,
    pub status: Option<String>,
    pub health: LivingHealth,
    /// Whether the paint pass is on, without which its boil shows nothing.
    pub paint_pass: bool,
    /// Whether the ink pass is on.
    pub ink_pass: bool,
}

/// What the user asked for; applied by the host.
#[derive(Debug, Clone, PartialEq)]
pub enum LivingPanelAction {
    /// The knobs a hand moved, and only those: unset fields leave a knob as
    /// each prop has it.
    Set(WoozePreset),
    Freeze(bool),
    /// Move the clock on by exactly one beat.
    Step,
    Overlay(bool),
    /// The knobs as the scene loaded them.
    Reset,
    Save(String),
    Load(String),
}

pub struct LivingPropsDebugPanel {
    open: bool,
    dirty: bool,
    state: LivingPanelState,
    /// Which row the clock readout follows.
    focus: usize,
    /// The preset the combo has picked.
    picked: String,
    /// The name to save under.
    save_as: String,
    actions: Vec<LivingPanelAction>,
}

impl Default for LivingPropsDebugPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl LivingPropsDebugPanel {
    pub fn new() -> Self {
        Self {
            open: false,
            dirty: false,
            state: LivingPanelState::default(),
            focus: 0,
            picked: String::new(),
            save_as: "grove".to_string(),
            actions: Vec::new(),
        }
    }

    /// Live mirror: the host calls this each frame the panel or overlay is showing.
    pub fn refresh(&mut self, state: LivingPanelState) {
        self.state = state;
    }

    /// What's been asked for since the last drain.
    pub fn take_actions(&mut self) -> Vec<LivingPanelAction> {
        std::mem::take(&mut self.actions)
    }

    /// What the panel last heard from the host.
    pub fn state(&self) -> &LivingPanelState {
        &self.state
    }

    /// Queue an action as a click would, for tests that drive the host.
    pub fn send_for_test(&mut self, action: LivingPanelAction) {
        self.push(action);
    }

    fn push(&mut self, action: LivingPanelAction) {
        self.actions.push(action);
        self.dirty = true;
    }

    /// The row the readout follows, if the scene has any.
    fn focused(&self) -> Option<&LivingPropRow> {
        self.state
            .rows
            .get(self.focus)
            .or_else(|| self.state.rows.first())
    }

    fn clock_section(&mut self, ui: &mut egui::Ui) {
        if let Some(row) = self.focused().cloned() {
            draw_clock(ui, &row, &self.state.knobs.stagger, self.state.frozen);
            ui.monospace(format!(
                "{}  pose {} > {}  {:>3.0}%",
                row.name,
                row.pose,
                row.pose + 1,
                row.weight * 100.0
            ));
        }
        ui.horizontal(|ui| {
            let frozen = self.state.frozen;
            if ui.selectable_label(frozen, "Freeze").clicked() {
                self.state.frozen = !frozen;
                self.push(LivingPanelAction::Freeze(!frozen));
            }
            if ui
                .add_enabled(frozen, egui::Button::new("Step"))
                .on_hover_text("Exactly one beat of the warped clock")
                .clicked()
            {
                self.push(LivingPanelAction::Step);
            }
            let mut overlay = self.state.overlay;
            if ui
                .checkbox(&mut overlay, "Pose overlay")
                .on_hover_text("Each prop's lineage and poses, in the world")
                .changed()
            {
                self.state.overlay = overlay;
                self.push(LivingPanelAction::Overlay(overlay));
            }
        });
        let health = self.state.health;
        let starved = health.frames_starved + health.blends_starved;
        let busy = if health.waiting > 0 {
            format!("{} waiting on poses", health.waiting)
        } else if health.reholding {
            "holding poses to a new variance".to_string()
        } else if health.rings_short > 0 {
            format!("{} rings filling", health.rings_short)
        } else {
            "rings full".to_string()
        };
        let line = format!("{} props: {busy}", self.state.rows.len());
        if starved > 0 {
            ui.colored_label(
                Color32::from_rgb(232, 120, 90),
                format!(
                    "{line}. {} frames short of a pose, {} blends kept their bones",
                    health.frames_starved, health.blends_starved
                ),
            );
        } else {
            ui.weak(line);
        }
    }

    fn knobs_ui(&mut self, ui: &mut egui::Ui) {
        let mut k = self.state.knobs.clone();
        let mixed = self.state.mixed;
        let mut touched = KnobFlags::default();

        section(ui, "Clock", true, |ui| {
            touched.step = knob(
                ui,
                egui::Slider::new(&mut k.step, 0.05..=4.0)
                    .logarithmic(true)
                    .text("step (s)"),
                mixed.step,
            );
            touched.glide = knob(
                ui,
                egui::Slider::new(&mut k.glide, 0.0..=1.0).text("glide"),
                mixed.glide,
            );
            touched.amplitude = knob(
                ui,
                egui::Slider::new(&mut k.amplitude, 0.0..=1.0).text("drift"),
                mixed.amplitude,
            );
            touched.jitter = knob(
                ui,
                egui::Slider::new(&mut k.jitter, 0.0..=4.0).text("jitter"),
                mixed.jitter,
            );
            let shown = if mixed.stagger {
                "mixed".to_string()
            } else {
                k.stagger.clone()
            };
            let combo = egui::ComboBox::from_label("stagger")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    let mut picked = false;
                    for (name, says) in STAGGERS {
                        picked |= ui
                            .selectable_value(&mut k.stagger, name.to_string(), name)
                            .on_hover_text(says)
                            .clicked();
                    }
                    picked
                });
            touched.stagger = combo.inner.unwrap_or(false);
            if mixed.stagger {
                combo.response.on_hover_text(MIXED);
            }
            if self.state.no_field && mixed.any_clock() {
                ui.weak(
                    "No [wooze] table: each species runs its spec's clock. Turning any of \
                     these gives the scene one field, from the first prop's clock, so all \
                     four even out across the grove.",
                );
            }
        });

        section(ui, "Shading boil", true, |ui| {
            ui.checkbox(&mut k.boil_on, "Boil");
            ui.add_enabled_ui(k.boil_on, |ui| {
                ui.add(egui::Slider::new(&mut k.boil_paint, 0.0..=1.0).text("paint"));
                ui.add(egui::Slider::new(&mut k.boil_ink, 0.0..=2.0).text("ink"));
                ui.horizontal(|ui| {
                    ui.label("on");
                    for (cadence, name) in [(1, "ones"), (2, "twos"), (3, "threes")] {
                        ui.selectable_value(&mut k.boil_cadence, cadence, name);
                    }
                });
            });
            let (paint, ink) = (self.state.paint_pass, self.state.ink_pass);
            if !paint && !ink {
                ui.weak("Needs Kuwahara for paint and contour for ink: F4.");
            } else if !paint {
                ui.weak("Ink only. Paint needs Kuwahara: F4.");
            } else if !ink {
                ui.weak("Paint only. Ink needs contour: F4.");
            }
        });

        section(ui, "Echoes", true, |ui| {
            touched.echo_count = knob(
                ui,
                egui::Slider::new(&mut k.echo_count, 0..=16).text("count"),
                mixed.echo_count,
            );
            // To where an echo has faded out: flint_procgen::MAX_ECHO_SPREAD.
            touched.echo_spread = knob(
                ui,
                egui::Slider::new(&mut k.echo_spread, 0.0..=1.818).text("spread (steps)"),
                mixed.echo_spread,
            );
            touched.echo_alpha = knob(
                ui,
                egui::Slider::new(&mut k.echo_alpha, 0.0..=1.0).text("alpha"),
                mixed.echo_alpha,
            );
        });

        section(ui, "Variance", true, |ui| {
            touched.variance_amount = knob(
                ui,
                egui::Slider::new(&mut k.variance_amount, 0.0..=1.0).text("amount"),
                mixed.variance_amount,
            );
            touched.variance_steady = knob(
                ui,
                egui::Slider::new(&mut k.variance_steady, 0.0..=1.0).text("steady"),
                mixed.variance_steady,
            );
            let keep = ui.add(
                egui::Checkbox::new(&mut k.keep_size, "keep size").indeterminate(mixed.keep_size),
            );
            touched.keep_size = keep.changed();
            if mixed.keep_size {
                keep.on_hover_text(MIXED);
            }
            if ui
                .small_button("Unheld")
                .on_hover_text("amount 1, steady 0, keep size off: each pose as it grew")
                .clicked()
            {
                k.variance_amount = 1.0;
                k.variance_steady = 0.0;
                k.keep_size = false;
                touched.variance_amount = true;
                touched.variance_steady = true;
                touched.keep_size = true;
            }
        });

        // A touch says a knob moved where its number alone can't: a `mixed` one
        // set to the value it showed, or Unheld on a knob already there.
        let moved = k.moved(&self.state.knobs, &touched);
        if moved != WoozePreset::default() {
            self.state.knobs = k;
            // What moved is now the same on every prop.
            self.state.mixed = mixed.without(&KnobFlags::set_by(&moved));
            self.push(LivingPanelAction::Set(moved));
        }
    }

    fn presets_ui(&mut self, ui: &mut egui::Ui) {
        section(ui, "Presets", true, |ui| {
            if !self.state.preset_names.contains(&self.picked) {
                self.picked = self.state.preset_names.first().cloned().unwrap_or_default();
            }
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("living_preset")
                    .selected_text(self.picked.clone())
                    .show_ui(ui, |ui| {
                        for name in self.state.preset_names.clone() {
                            ui.selectable_value(&mut self.picked, name.clone(), name);
                        }
                    });
                if ui
                    .add_enabled(!self.picked.is_empty(), egui::Button::new("Load"))
                    .clicked()
                {
                    self.push(LivingPanelAction::Load(self.picked.clone()));
                }
                if ui
                    .button("As loaded")
                    .on_hover_text("Every knob back as the scene set it")
                    .clicked()
                {
                    self.push(LivingPanelAction::Reset);
                }
            });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.save_as).desired_width(120.0));
                if ui.button("Save").clicked() {
                    self.push(LivingPanelAction::Save(self.save_as.clone()));
                }
            });
            if let Some(status) = &self.state.status {
                ui.label(egui::RichText::new(status).small());
            }
        });
    }

    fn rows_ui(&mut self, ui: &mut egui::Ui) {
        section(ui, "Props", false, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("living_rows")
                .max_height(220.0)
                .show(ui, |ui| {
                    egui::Grid::new("living_rows_grid")
                        .num_columns(4)
                        .striped(true)
                        .show(ui, |ui| {
                            ui.weak("prop");
                            ui.weak("lineage");
                            ui.weak("pose");
                            ui.weak("ring");
                            ui.end_row();
                            let rows = self.state.rows.clone();
                            for (i, row) in rows.iter().enumerate() {
                                if ui.selectable_label(self.focus == i, &row.name).clicked() {
                                    self.focus = i;
                                }
                                ui.monospace(row.lineage_seed.to_string());
                                ui.monospace(format!("{} {:.0}%", row.pose, row.weight * 100.0));
                                ui.monospace(format!(
                                    "{}+{}{}",
                                    row.ring_first,
                                    row.ring_ready,
                                    if row.waiting { " wait" } else { "" }
                                ));
                                ui.end_row();
                            }
                        });
                });
        });
    }

    /// Paint the pose overlay, if it's on: each prop's lineage and poses over its
    /// crown, after a small dial of where it stands in its step. The nearest get
    /// to speak first and a label that would land on one stays quiet, so a grove
    /// reads as a grove: every prop is in the panel's Props section regardless.
    pub fn paint_overlay(&self, ctx: &egui::Context, view_proj: &[[f32; 4]; 4]) {
        if !self.state.overlay {
            return;
        }
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("living_pose_overlay"),
        ));
        let screen = ctx.screen_rect();
        let mut placed: Vec<(f32, Pos2, &LivingPropRow)> = self
            .state
            .rows
            .iter()
            .filter_map(|row| {
                let (at, depth) = project(row, view_proj, screen)?;
                Some((depth, at, row))
            })
            .collect();
        placed.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut taken: Vec<Rect> = Vec::with_capacity(placed.len());
        for (depth, at, row) in placed {
            let near = (1.0 - (depth - 10.0) / 60.0).clamp(0.25, 1.0);
            let label = Label::of(&painter, at, row, near);
            if taken.iter().any(|other| other.intersects(label.area)) {
                continue;
            }
            taken.push(label.area);
            label.paint(&painter, at, row, near);
        }
    }
}

impl DebugPanel for LivingPropsDebugPanel {
    fn name(&self) -> &str {
        LIVING_PROPS_DEBUG_PANEL
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.clock_section(ui);
        ui.separator();
        self.knobs_ui(ui);
        self.presets_ui(ui);
        self.rows_ui(ui);
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

fn section(ui: &mut egui::Ui, title: &str, default_open: bool, add: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(title)
        .default_open(default_open)
        .show(ui, add);
}

/// A slider that reads `mixed` when the props disagree on it. Returns whether a
/// hand moved it.
fn knob(ui: &mut egui::Ui, slider: egui::Slider<'_>, mixed: bool) -> bool {
    if !mixed {
        return ui.add(slider).changed();
    }
    ui.add(slider.custom_formatter(|_, _| "mixed".to_string()))
        .on_hover_text(MIXED)
        .changed()
}

/// How far toward the next pose a part is, `frac` of the way through a step: the
/// design's t', as `flint_core::wooze::warp` works it out.
fn eased(frac: f64, glide: f32) -> f32 {
    flint_core::wooze::warp(frac.clamp(0.0, 0.999_999), glide).1
}

/// The warped clock as what it is: a step's worth of hold, then the ease to the
/// next pose, with the prop riding it. With a stagger, each branch level rides it
/// a little ahead, so what the dial means by "shimmer" is there to see.
fn draw_clock(ui: &mut egui::Ui, row: &LivingPropRow, stagger: &str, frozen: bool) {
    let width = ui.available_width().min(320.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 76.0), Sense::hover());
    let painter = ui.painter_at(rect);
    // Under the labels, so the curve's top never reads through them.
    let plot = Rect::from_min_max(
        rect.left_top() + Vec2::new(8.0, 20.0),
        rect.right_bottom() - Vec2::new(8.0, 8.0),
    );
    let at = |frac: f64, weight: f32| {
        Pos2::new(
            plot.left() + frac as f32 * plot.width(),
            plot.bottom() - weight * plot.height(),
        )
    };
    let visuals = ui.visuals();
    let faint = visuals.weak_text_color();
    painter.rect_filled(rect, 3.0, visuals.extreme_bg_color);

    // Where the hold ends and the glide begins.
    let hold_end = f64::from(1.0 - row.glide);
    painter.line_segment(
        [at(hold_end, 0.0), at(hold_end, 1.0)],
        Stroke::new(1.0_f32, faint),
    );

    let accent = if frozen { FROZEN } else { INK };
    let points: Vec<Pos2> = (0..=64)
        .map(|i| {
            let frac = f64::from(i) / 64.0;
            at(frac, eased(frac, row.glide))
        })
        .collect();
    painter.add(Shape::line(points, Stroke::new(1.5_f32, accent)));

    if stagger == "per_level" {
        for level in (1..LEVELS_DRAWN).rev() {
            let frac = (row.beats + level as f64 * LEVEL_PHASE).fract();
            painter.circle_filled(at(frac, eased(frac, row.glide)), 2.5, PARTS);
        }
    }
    let frac = row.beats.fract();
    let dot = at(frac, eased(frac, row.glide));
    painter.circle_filled(dot, 4.0, accent);

    let label = FontId::monospace(10.0);
    painter.text(
        rect.left_top() + Vec2::new(5.0, 2.0),
        Align2::LEFT_TOP,
        format!("beat {:.3}", row.beats),
        label.clone(),
        faint,
    );
    painter.text(
        rect.right_top() + Vec2::new(-5.0, 2.0),
        Align2::RIGHT_TOP,
        if frozen {
            "frozen".to_string()
        } else {
            format!("{:.2} s / {:.0}% glide", row.step, row.glide * 100.0)
        },
        label,
        if frozen { FROZEN } else { faint },
    );
}

/// Where a prop's label goes on screen, and how far off it is: the top of its
/// crown, where distance spreads a grove's labels up the screen. Partway up the
/// trunks, they'd all sit in one line at the horizon.
fn project(row: &LivingPropRow, m: &[[f32; 4]; 4], screen: Rect) -> Option<(Pos2, f32)> {
    let p = [
        row.position[0],
        row.position[1] + row.height,
        row.position[2],
    ];
    let clip = |i: usize| m[0][i] * p[0] + m[1][i] * p[1] + m[2][i] * p[2] + m[3][i];
    let w = clip(3);
    if w <= 0.1 {
        return None;
    }
    let (x, y) = (clip(0) / w, clip(1) / w);
    if x.abs() > 1.1 || y.abs() > 1.1 {
        return None;
    }
    Some((
        Pos2::new(
            screen.left() + (x * 0.5 + 0.5) * screen.width(),
            screen.top() + (0.5 - y * 0.5) * screen.height(),
        ),
        w,
    ))
}

/// `c` at `a` of its strength. Blending is in display space, so this premultiplies
/// by hand (see CLAUDE.md).
fn shaded(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_premultiplied(
        (f32::from(c.r()) * a) as u8,
        (f32::from(c.g()) * a) as u8,
        (f32::from(c.b()) * a) as u8,
        (255.0 * a) as u8,
    )
}

/// One prop's label, laid out: a dial, its lineage, and the two poses it's between.
struct Label {
    galley: std::sync::Arc<egui::Galley>,
    /// Where the text starts.
    left: Pos2,
    /// What it covers, dial and all, for telling whether another would land on it.
    area: Rect,
    radius: f32,
}

impl Label {
    fn of(painter: &egui::Painter, at: Pos2, row: &LivingPropRow, near: f32) -> Self {
        let font = FontId::monospace(9.0 + 2.0 * near);
        let radius = 3.0 + 3.0 * near;
        let text = format!(
            "L{}  {}>{}{}",
            row.lineage_seed,
            row.pose,
            row.pose + 1,
            if row.waiting { " ..." } else { "" }
        );
        let colour = shaded(Color32::from_rgb(244, 236, 220), near);
        let galley = painter.layout_no_wrap(text, font, colour);
        let left = at + Vec2::new(radius + 3.0, -galley.size().y * 0.5);
        let area = Rect::from_min_size(left, galley.size())
            .union(Rect::from_center_size(at, Vec2::splat(radius * 2.0)))
            .expand(2.0);
        Self {
            galley,
            left,
            area,
            radius,
        }
    }

    fn paint(self, painter: &egui::Painter, at: Pos2, row: &LivingPropRow, near: f32) {
        painter.rect_filled(
            self.area,
            3.0,
            Color32::from_rgba_premultiplied(0, 0, 0, (150.0 * near) as u8),
        );
        painter.galley(self.left, self.galley, Color32::PLACEHOLDER);
        // A dial for its place in the step: the needle goes round once a beat.
        let radius = self.radius;
        painter.circle_stroke(at, radius, Stroke::new(1.0_f32, shaded(PARTS, near)));
        let turn = row.beats.fract() as f32 * std::f32::consts::TAU;
        let tip = at + Vec2::new(turn.sin(), -turn.cos()) * radius;
        painter.line_segment([at, tip], Stroke::new(1.5_f32, shaded(INK, near)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, position: [f32; 3]) -> LivingPropRow {
        LivingPropRow {
            name: name.to_string(),
            lineage_seed: 7,
            position,
            height: 10.0,
            beats: 3.5,
            step: 0.5,
            glide: 0.3,
            pose: 3,
            weight: 0.0,
            shape_seed: 99,
            ring_first: 2,
            ring_len: 8,
            ring_ready: 8,
            echoes: 6,
            waiting: false,
        }
    }

    fn state() -> LivingPanelState {
        LivingPanelState {
            rows: vec![row("oak_1", [0.0, 0.0, 0.0])],
            preset_names: vec!["linklater".to_string(), "misty".to_string()],
            ..Default::default()
        }
    }

    fn frame(ctx: &egui::Context, panel: &mut LivingPropsDebugPanel, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(420.0, 1400.0))),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| panel.ui(ui));
        });
    }

    #[test]
    fn knobs_read_back_through_a_preset() {
        let knobs = LivingKnobs {
            step: 1.5,
            glide: 0.75,
            amplitude: 0.25,
            jitter: 2.0,
            stagger: "per_branch".to_string(),
            boil_on: false,
            boil_paint: 0.5,
            boil_ink: 1.5,
            boil_cadence: 3,
            echo_count: 3,
            echo_spread: 1.25,
            echo_alpha: 0.5,
            variance_amount: 1.0,
            variance_steady: 0.0,
            keep_size: false,
        };
        assert_eq!(LivingKnobs::from_preset(&knobs.to_preset()), knobs);
    }

    #[test]
    fn looking_at_it_asks_for_nothing() {
        let ctx = egui::Context::default();
        let mut panel = LivingPropsDebugPanel::new();
        panel.refresh(state());
        for _ in 0..3 {
            frame(&ctx, &mut panel, Vec::new());
        }
        assert!(!panel.is_dirty());
        assert!(panel.take_actions().is_empty());
    }

    /// Press the pointer at each point down a column of the panel, as a hand
    /// reaching for whatever's there, and return what that asked for.
    fn press_down(panel: &mut LivingPropsDebugPanel, x: f32) -> Vec<LivingPanelAction> {
        let ctx = egui::Context::default();
        frame(&ctx, panel, Vec::new());
        frame(&ctx, panel, Vec::new());
        let mut asked = Vec::new();
        for y in (0..1400).step_by(9) {
            let at = Pos2::new(x, y as f32);
            let press = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            frame(&ctx, panel, vec![egui::Event::PointerMoved(at)]);
            frame(&ctx, panel, vec![press(true)]);
            frame(&ctx, panel, vec![press(false)]);
            frame(&ctx, panel, vec![egui::Event::PointerGone]);
            asked.extend(panel.take_actions());
            panel.clear_dirty();
        }
        asked
    }

    /// How many knobs a preset sets.
    fn count_set(preset: &WoozePreset) -> usize {
        let f = KnobFlags::set_by(preset);
        let boil = preset.boil.unwrap_or_default();
        [
            f.step,
            f.glide,
            f.amplitude,
            f.jitter,
            f.stagger,
            f.echo_count,
            f.echo_spread,
            f.echo_alpha,
            f.variance_amount,
            f.variance_steady,
            f.keep_size,
            boil.enabled.is_some(),
            boil.kuwahara.is_some(),
            boil.contour.is_some(),
            boil.on.is_some(),
        ]
        .into_iter()
        .filter(|set| *set)
        .count()
    }

    /// Every slider, checkbox and shortcut in the column sends the knob it moved
    /// and nothing else (#244): a whole set would flatten a grove's species.
    #[test]
    fn pressing_down_the_sliders_moves_every_knob_one_at_a_time() {
        let mut panel = LivingPropsDebugPanel::new();
        panel.refresh(state());
        let asked = press_down(&mut panel, 60.0);
        let sets: Vec<WoozePreset> = asked
            .iter()
            .filter_map(|action| match action {
                LivingPanelAction::Set(moved) => Some(moved.clone()),
                _ => None,
            })
            .collect();
        assert!(!sets.is_empty(), "pressing a slider turned it");
        let mut all = KnobFlags::default();
        for moved in &sets {
            // One knob, or Unheld's three.
            let n = count_set(moved);
            assert!(n == 1 || n == 3, "{n} knobs in {moved:?}");
            let f = KnobFlags::set_by(moved);
            all = KnobFlags {
                step: all.step || f.step,
                echo_count: all.echo_count || f.echo_count,
                echo_spread: all.echo_spread || f.echo_spread,
                echo_alpha: all.echo_alpha || f.echo_alpha,
                variance_amount: all.variance_amount || f.variance_amount,
                variance_steady: all.variance_steady || f.variance_steady,
                ..all
            };
        }
        assert!(all.step && all.echo_count && all.echo_spread && all.echo_alpha);
        assert!(all.variance_amount && all.variance_steady, "{sets:?}");
    }

    #[test]
    fn a_knob_moved_from_before_is_the_only_one_set() {
        let before = LivingKnobs::default();
        let mut after = before.clone();
        after.glide = 0.9;
        let moved = after.moved(&before, &KnobFlags::default());
        assert_eq!(moved.wooze.glide, Some(0.9));
        assert_eq!(count_set(&moved), 1, "{moved:?}");
        assert_eq!(
            after.moved(&after, &KnobFlags::default()),
            WoozePreset::default()
        );
        // Touched, it's sent at the value it already had.
        let touched = KnobFlags {
            keep_size: true,
            ..Default::default()
        };
        let moved = after.moved(&after, &touched);
        assert_eq!(moved.living.variance.unwrap().keep_size, Some(true));
        assert_eq!(count_set(&moved), 1);
    }

    /// Every piece of text a frame drew.
    fn texts(output: &egui::FullOutput) -> Vec<String> {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_knob_the_species_disagree_on_reads_mixed() {
        let ctx = egui::Context::default();
        let draw = |panel: &mut LivingPropsDebugPanel| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(420.0, 1400.0))),
                ..Default::default()
            };
            // Collapsing headers open on the second frame.
            let _ = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| panel.ui(ui));
            });
            let output = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| panel.ui(ui));
            });
            texts(&output)
        };
        let mut panel = LivingPropsDebugPanel::new();
        panel.refresh(state());
        assert!(!draw(&mut panel).iter().any(|t| t == "mixed"));

        let mut mixed = state();
        mixed.mixed = KnobFlags {
            stagger: true,
            echo_count: true,
            echo_spread: true,
            variance_amount: true,
            ..Default::default()
        };
        panel.refresh(mixed);
        let said = draw(&mut panel);
        assert_eq!(said.iter().filter(|t| *t == "mixed").count(), 4, "{said:?}");
        assert!(!panel.is_dirty(), "showing mixed asks for nothing");
    }

    #[test]
    fn freeze_is_a_toggle_and_step_needs_the_clock_stopped() {
        // A step of a running clock couldn't be seen, so its button stays off.
        let mut running = LivingPropsDebugPanel::new();
        running.refresh(state());
        let steps = |asked: &[LivingPanelAction]| {
            asked
                .iter()
                .filter(|a| **a == LivingPanelAction::Step)
                .count()
        };
        let mut asked = Vec::new();
        for x in [20.0, 72.0] {
            // Whatever it presses, it can't be Step until Freeze has been.
            let mut panel = LivingPropsDebugPanel::new();
            panel.refresh(state());
            let ctx = egui::Context::default();
            frame(&ctx, &mut panel, Vec::new());
            asked.extend(press_down(&mut running, x));
        }
        assert!(
            asked.contains(&LivingPanelAction::Freeze(true)),
            "the button is in that column: {asked:?}"
        );

        let mut frozen = state();
        frozen.frozen = true;
        let mut panel = LivingPropsDebugPanel::new();
        panel.refresh(frozen);
        let mut pressed = Vec::new();
        for x in (30..110).step_by(10) {
            pressed.extend(press_down(&mut panel, x as f32));
            panel.state.frozen = true;
        }
        assert!(steps(&pressed) >= 1, "{pressed:?}");
    }

    #[test]
    fn a_label_lands_where_the_prop_is_and_never_behind_the_camera() {
        // Looking down -z from the origin, with a plain perspective.
        let view_proj = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0, -1.0],
            [0.0, 0.0, -0.2, 0.0],
        ];
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let mut ahead = row("ahead", [0.0, 0.0, -20.0]);
        ahead.height = 0.0;
        let (at, depth) = project(&ahead, &view_proj, screen).unwrap();
        assert!(
            (at.x - 400.0).abs() < 0.01 && (at.y - 300.0).abs() < 0.01,
            "{at:?}"
        );
        assert_eq!(depth, 20.0);
        assert!(project(&row("behind", [0.0, 0.0, 20.0]), &view_proj, screen).is_none());
        assert!(project(&row("aside", [100.0, 0.0, -2.0]), &view_proj, screen).is_none());
    }

    #[test]
    fn a_label_that_would_land_on_a_nearer_one_stays_quiet() {
        let ctx = egui::Context::default();
        let mut panel = LivingPropsDebugPanel::new();
        let view_proj = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0, -1.0],
            [0.0, 0.0, -0.2, 0.0],
        ];
        let at = |z: f32| {
            let mut row = row("oak", [0.0, 0.0, z]);
            row.height = 0.0;
            row
        };
        let painted = |rows: Vec<LivingPropRow>, panel: &mut LivingPropsDebugPanel| {
            panel.refresh(LivingPanelState {
                rows,
                overlay: true,
                ..Default::default()
            });
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                panel.paint_overlay(ctx, &view_proj);
            });
            output.shapes.len()
        };
        let one = painted(vec![at(-20.0)], &mut panel);
        // Right behind it, so it projects to the same place.
        let behind = painted(vec![at(-40.0), at(-20.0)], &mut panel);
        assert_eq!(one, behind);
        // Off to the side there's room for both.
        let mut aside = at(-20.0);
        aside.position[0] = 10.0;
        assert_eq!(painted(vec![at(-20.0), aside], &mut panel), one * 2);
    }

    #[test]
    fn an_overlay_paints_only_when_on() {
        let ctx = egui::Context::default();
        let mut panel = LivingPropsDebugPanel::new();
        let view_proj = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0, -1.0],
            [0.0, 0.0, -0.2, 0.0],
        ];
        let mut with_prop = state();
        with_prop.rows = vec![row("oak_1", [0.0, 0.0, -20.0])];
        panel.refresh(with_prop.clone());
        let shapes = |ctx: &egui::Context, panel: &LivingPropsDebugPanel| {
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                panel.paint_overlay(ctx, &view_proj);
            });
            output.shapes.len()
        };
        assert_eq!(shapes(&ctx, &panel), 0);
        with_prop.overlay = true;
        panel.refresh(with_prop);
        assert!(shapes(&ctx, &panel) >= 4);
    }
}
