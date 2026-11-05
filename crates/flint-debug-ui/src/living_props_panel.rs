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
                ..Default::default()
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
                ..Default::default()
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
    /// One knob's flag.
    pub fn get(&self, knob: Knob) -> bool {
        match knob {
            Knob::Amplitude => self.amplitude,
            Knob::Step => self.step,
            Knob::Glide => self.glide,
            Knob::Jitter => self.jitter,
            Knob::Stagger => self.stagger,
            Knob::EchoCount => self.echo_count,
            Knob::EchoSpread => self.echo_spread,
            Knob::EchoAlpha => self.echo_alpha,
            Knob::VarianceAmount => self.variance_amount,
            Knob::VarianceSteady => self.variance_steady,
            Knob::KeepSize => self.keep_size,
        }
    }

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

/// One of the knobs a species can set (#247). The boil's aren't among them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Knob {
    Amplitude,
    Step,
    Glide,
    Jitter,
    Stagger,
    EchoCount,
    EchoSpread,
    EchoAlpha,
    VarianceAmount,
    VarianceSteady,
    KeepSize,
}

impl Knob {
    pub const ALL: [Knob; 11] = [
        Knob::Amplitude,
        Knob::Step,
        Knob::Glide,
        Knob::Jitter,
        Knob::Stagger,
        Knob::EchoCount,
        Knob::EchoSpread,
        Knob::EchoAlpha,
        Knob::VarianceAmount,
        Knob::VarianceSteady,
        Knob::KeepSize,
    ];

    /// Its key in `[living.species.<spec>]`, in `clear_living` and in `flint
    /// render --report json`'s `values`.
    pub fn key(self) -> &'static str {
        match self {
            Knob::Amplitude => "amplitude",
            Knob::Step => "step",
            Knob::Glide => "glide",
            Knob::Jitter => "jitter",
            Knob::Stagger => "stagger",
            Knob::EchoCount => "echoes.count",
            Knob::EchoSpread => "echoes.spread",
            Knob::EchoAlpha => "echoes.alpha",
            Knob::VarianceAmount => "variance.amount",
            Knob::VarianceSteady => "variance.steady",
            Knob::KeepSize => "variance.keep_size",
        }
    }

    fn index(self) -> usize {
        Knob::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }
}

/// Where each knob's value comes from (#247), over the props a scope covers:
/// each layer once, by the label `flint render --report json` gives it, `spec`,
/// `entity`, `global`, `zone:<name>` or `species`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KnobSources([Vec<String>; 11]);

impl KnobSources {
    pub fn of(&self, knob: Knob) -> &[String] {
        &self.0[knob.index()]
    }

    pub fn set(&mut self, knob: Knob, layers: Vec<String>) {
        self.0[knob.index()] = layers;
    }

    /// Whether the species' own layer sets this knob.
    pub fn is_own(&self, knob: Knob) -> bool {
        self.of(knob).iter().any(|layer| layer == "species")
    }
}

/// Which values the knobs read and turn (#247): the whole grove's, or one
/// species', by spec name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PanelScope {
    /// The global layer, under any species' own value.
    #[default]
    Global,
    /// One species' own layer, which beats the global one, the field and its
    /// zones.
    Species(String),
}

/// One species the props grow from, as the host resolved it (#247).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LivingSpeciesState {
    /// Its spec's name.
    pub name: String,
    /// How many props grow from it.
    pub props: usize,
    /// What its props run on, the first one's where they disagree. The boil's
    /// knobs are the grove's: a species can't set them.
    pub knobs: LivingKnobs,
    /// The knobs its own props disagree on: a zone some stand in, or lineages
    /// held differently.
    pub mixed: KnobFlags,
    /// The layer behind each value.
    pub sources: KnobSources,
    /// Whether it has values the scene file doesn't (#249): set, changed or
    /// cleared since the scene loaded or was last committed.
    pub unsaved: bool,
}

impl LivingSpeciesState {
    /// Whether it sets any value of its own.
    pub fn has_own(&self) -> bool {
        Knob::ALL.iter().any(|knob| self.sources.is_own(*knob))
    }
}

/// What a `mixed` knob says when hovered, for the whole grove.
const MIXED: &str = "The species disagree here, so this shows the first prop's value. \
Moving it sets it for the whole grove, apart from a species that sets its own; the \
knobs beside it stay each species' own.";

/// What a `mixed` knob says when hovered, for one species.
const MIXED_SPECIES: &str = "This species' trees disagree here: a zone some stand in, a \
tree's own value, or lineages held differently. It shows the first one's. Moving it \
makes it the species' own, on every one of its trees.";

/// What each layer means, for a mark's hover.
fn layer_says(layer: &str) -> String {
    match layer {
        "spec" => "spec: the species' .procgen.toml".to_string(),
        "entity" => "entity: a tree's own living_prop component".to_string(),
        "global" => "global: the scene's [living] or [wooze], or this panel at Global".to_string(),
        "species" => "species: this species' own value".to_string(),
        zone => match zone.strip_prefix("zone:") {
            Some(name) => format!("zone:{name}: the wooze zone some of its trees stand in"),
            None => zone.to_string(),
        },
    }
}

/// What one knob's mark says, beside it.
#[derive(Debug, Clone, PartialEq)]
enum Mark {
    /// Nothing: Global, with no species setting its own.
    Quiet,
    /// At Global: these species set their own, so the knob doesn't reach them.
    Owned(Vec<String>),
    /// At a species: the layers its value comes from.
    From(Vec<String>),
}

/// One prop's place on its clock.
#[derive(Debug, Clone, PartialEq)]
pub struct LivingPropRow {
    pub name: String,
    /// Its spec's name: the species it's one of.
    pub species: String,
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
    /// Every knob for the whole grove. Where the props disagree, the first
    /// prop's value, leaving out a species that sets its own.
    pub knobs: LivingKnobs,
    /// The knobs the props disagree on, which the panel shows as `mixed`.
    pub mixed: KnobFlags,
    /// Each species the props grow from, by name (#247).
    pub species: Vec<LivingSpeciesState>,
    /// What the scene's living shadows follow, `blended` or `drift`: one for
    /// every prop, so a species can't set it.
    pub shadows: String,
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
    /// Two presets being compared (#238), if they are.
    pub ab: Option<LivingAbState>,
    /// Whether the grove's own values differ from what the scene file gives
    /// (#249): its `[wooze]`, `[living]` or boil.
    pub unsaved_global: bool,
    /// What Commit to File would write, one line a value, as
    /// `[living.species.grove_thorn] echoes.count = 4`.
    pub unsaved: Vec<String>,
}

/// What an unsaved level's mark says when hovered.
const UNSAVED: &str = "*: values the scene file doesn't have yet. Commit to File writes \
them; As loaded puts the file's back.";

/// How an A/B comparison stands (#238). A name of "" is the knobs as they stood.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LivingAbState {
    pub a: String,
    pub b: String,
    /// Whether B is the side shown and tuned.
    pub showing_b: bool,
    /// Whether both are on screen, A left.
    pub split: bool,
}

/// What a side is called in the panel.
fn side_name(name: &str) -> &str {
    if name.is_empty() {
        "as it was"
    } else {
        name
    }
}

/// What the user asked for; applied by the host.
#[derive(Debug, Clone, PartialEq)]
pub enum LivingPanelAction {
    /// The knobs a hand moved, and only those, for the grove or one species:
    /// unset fields leave a knob as each prop has it. A species' gets no boil.
    Set(PanelScope, WoozePreset),
    /// Take a species' own value away, or all of them with `None`, so its trees
    /// take the layers below again (#247).
    Clear(String, Option<Knob>),
    Freeze(bool),
    /// Move the clock on by exactly one beat.
    Step,
    Overlay(bool),
    /// The knobs as the scene loaded them.
    Reset,
    Save(String),
    Load(String),
    /// Write every value the scene file doesn't give into it (#249).
    Commit,
    /// Compare preset A with preset B (#238); "" is the knobs as they stand.
    Compare(String, String),
    /// The presets F6 and F7 would compare, as picked.
    PickAb(String, String),
    /// Show the other side.
    Flip,
    /// Both sides at once, or one.
    Split(bool),
    /// End the comparison, keeping what's shown.
    StopComparing,
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
    /// What A and B are picked to be, before a comparison starts. `None` until
    /// the presets on offer are known.
    ab_picks: Option<(String, String)>,
    /// Whose values the knobs turn (#247).
    scope: PanelScope,
    /// What the Props rows are narrowed to: part of a name or a species.
    filter: String,
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
            ab_picks: None,
            scope: PanelScope::Global,
            filter: String::new(),
            actions: Vec::new(),
        }
    }

    /// Live mirror: the host calls this each frame the panel or overlay is showing.
    /// A species that's gone, as after a scene change, gives the grove back.
    pub fn refresh(&mut self, state: LivingPanelState) {
        self.state = state;
        if matches!(self.scope, PanelScope::Species(_)) && self.picked_species().is_none() {
            self.scope = PanelScope::Global;
        }
    }

    /// Whose values the knobs turn.
    pub fn scope(&self) -> &PanelScope {
        &self.scope
    }

    /// Turn the grove's knobs or one species', as a click on the selector would.
    pub fn set_scope(&mut self, scope: PanelScope) {
        self.scope = scope;
    }

    /// The species picked, if one is and the host knows it.
    fn picked_species(&self) -> Option<&LivingSpeciesState> {
        match &self.scope {
            PanelScope::Global => None,
            PanelScope::Species(name) => self.state.species.iter().find(|s| &s.name == name),
        }
    }

    /// What a knob's mark says, for the scope picked.
    fn mark(&self, knob: Knob) -> Mark {
        match self.picked_species() {
            Some(species) => Mark::From(species.sources.of(knob).to_vec()),
            None => {
                let owned: Vec<String> = (self.state.species.iter())
                    .filter(|s| s.sources.is_own(knob))
                    .map(|s| s.name.clone())
                    .collect();
                if owned.is_empty() {
                    Mark::Quiet
                } else {
                    Mark::Owned(owned)
                }
            }
        }
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

    /// The row the readout follows, if the scene has any: one of the picked
    /// species', if one is.
    fn focused(&self) -> Option<&LivingPropRow> {
        let rows = &self.state.rows;
        let row = rows.get(self.focus).or_else(|| rows.first());
        match &self.scope {
            PanelScope::Species(name) if row.is_some_and(|row| &row.species != name) => {
                rows.iter().find(|row| &row.species == name).or(row)
            }
            _ => row,
        }
    }

    /// Whether the overlay labels this row: every row, or the picked species'.
    fn labelled(&self, row: &LivingPropRow) -> bool {
        match &self.scope {
            PanelScope::Global => true,
            PanelScope::Species(name) => &row.species == name,
        }
    }

    /// Global, or a species, picked with one click. A species with values of
    /// its own wears a dot.
    fn scope_ui(&mut self, ui: &mut egui::Ui) {
        if self.state.species.is_empty() {
            return;
        }
        let mut picked = self.scope.clone();
        ui.horizontal_wrapped(|ui| {
            ui.weak("Tuning");
            let global = unsaved_text("Global".to_string(), self.state.unsaved_global);
            let hover = "The whole grove, under any species' own value";
            ui.selectable_value(&mut picked, PanelScope::Global, global)
                .on_hover_text(with_unsaved(hover, self.state.unsaved_global));
            for species in &self.state.species {
                let own = if species.has_own() { " \u{2022}" } else { "" };
                let text = format!("{} {}{own}", species.name, species.props);
                let text = unsaved_text(text, species.unsaved);
                let unsaved = species.unsaved;
                ui.selectable_value(&mut picked, PanelScope::Species(species.name.clone()), text)
                    .on_hover_text(with_unsaved(
                        &format!(
                            "{}: {} trees. A knob moved here is the species' own: it beats the \
                         grove's, the field and its zones{}",
                            species.name,
                            species.props,
                            if own.is_empty() {
                                "."
                            } else {
                                ". The dot: it already has some."
                            }
                        ),
                        unsaved,
                    ));
            }
        });
        self.scope = picked;
        let Some(species) = self.picked_species().cloned() else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(
                INK,
                format!("{}: its own values beat the grove's", species.name),
            );
            if ui
                .add_enabled(species.has_own(), egui::Button::new("Clear all").small())
                .on_hover_text("Every value of its own gone: its trees take the grove's again")
                .clicked()
            {
                self.push(LivingPanelAction::Clear(species.name.clone(), None));
            }
        });
        if self.state.overlay {
            ui.weak("The pose overlay labels only its trees.");
        }
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
        let species = self.picked_species().cloned();
        let (mut k, mixed) = match &species {
            Some(species) => (species.knobs.clone(), species.mixed),
            None => (self.state.knobs.clone(), self.state.mixed),
        };
        let before = k.clone();
        let says = if species.is_some() {
            MIXED_SPECIES
        } else {
            MIXED
        };
        let marks: Vec<Mark> = Knob::ALL.iter().map(|knob| self.mark(*knob)).collect();
        let mut touched = KnobFlags::default();
        let mut clear: Option<Knob> = None;
        // A knob, then its mark: where its value comes from, at a species, or
        // which species it doesn't reach, at Global.
        let mut row =
            |ui: &mut egui::Ui, knob: Knob, add: &mut dyn FnMut(&mut egui::Ui) -> bool| {
                ui.horizontal(|ui| {
                    let moved = add(ui);
                    if paint_mark(ui, &marks[knob.index()]) {
                        clear = Some(knob);
                    }
                    moved
                })
                .inner
            };

        section(ui, "Clock", true, |ui| {
            touched.step = row(ui, Knob::Step, &mut |ui| {
                knob(
                    ui,
                    egui::Slider::new(&mut k.step, 0.05..=4.0)
                        .logarithmic(true)
                        .text("step (s)"),
                    mixed.step,
                    says,
                )
            });
            touched.glide = row(ui, Knob::Glide, &mut |ui| {
                knob(
                    ui,
                    egui::Slider::new(&mut k.glide, 0.0..=1.0).text("glide"),
                    mixed.glide,
                    says,
                )
            });
            touched.amplitude = row(ui, Knob::Amplitude, &mut |ui| {
                knob(
                    ui,
                    egui::Slider::new(&mut k.amplitude, 0.0..=1.0).text("drift"),
                    mixed.amplitude,
                    says,
                )
            });
            touched.jitter = row(ui, Knob::Jitter, &mut |ui| {
                knob(
                    ui,
                    egui::Slider::new(&mut k.jitter, 0.0..=4.0).text("jitter"),
                    mixed.jitter,
                    says,
                )
            });
            touched.stagger = row(ui, Knob::Stagger, &mut |ui| {
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
                if mixed.stagger {
                    combo.response.on_hover_text(says);
                }
                combo.inner.unwrap_or(false)
            });
            if species.is_none() && self.state.no_field && mixed.any_clock() {
                ui.weak(
                    "No [wooze] table: each species runs its spec's clock. Turning any of \
                     these gives the scene one field, from the first prop's clock, so all \
                     four even out across the grove.",
                );
            }
        });

        if species.is_some() {
            // What a species can't set, still in view, and why.
            section(ui, "Whole scene only", true, |ui| {
                ui.add_enabled_ui(false, |ui| boil_ui(ui, &mut k.clone()));
                ui.weak(
                    "The boil is one screen pass over the whole frame, so every species \
                     boils together. Tune it at Global.",
                );
                ui.add_enabled(
                    false,
                    egui::Label::new(format!("shadows: {}", self.state.shadows)),
                );
                ui.weak(
                    "Shadows are the scene's [living] shadows, one choice for every living \
                     prop, so a species can't set them.",
                );
            });
        } else {
            section(ui, "Shading boil", true, |ui| {
                boil_ui(ui, &mut k);
                let (paint, ink) = (self.state.paint_pass, self.state.ink_pass);
                if !paint && !ink {
                    ui.weak("Needs Kuwahara for paint and contour for ink: F4.");
                } else if !paint {
                    ui.weak("Ink only. Paint needs Kuwahara: F4.");
                } else if !ink {
                    ui.weak("Paint only. Ink needs contour: F4.");
                }
            });
        }

        section(ui, "Echoes", true, |ui| {
            touched.echo_count = row(ui, Knob::EchoCount, &mut |ui| {
                knob(
                    ui,
                    egui::Slider::new(&mut k.echo_count, 0..=16).text("count"),
                    mixed.echo_count,
                    says,
                )
            });
            // To where an echo has faded out: flint_procgen::MAX_ECHO_SPREAD.
            touched.echo_spread = row(ui, Knob::EchoSpread, &mut |ui| {
                knob(
                    ui,
                    egui::Slider::new(&mut k.echo_spread, 0.0..=1.818).text("spread (steps)"),
                    mixed.echo_spread,
                    says,
                )
            });
            touched.echo_alpha = row(ui, Knob::EchoAlpha, &mut |ui| {
                knob(
                    ui,
                    egui::Slider::new(&mut k.echo_alpha, 0.0..=1.0).text("alpha"),
                    mixed.echo_alpha,
                    says,
                )
            });
        });

        section(ui, "Variance", true, |ui| {
            touched.variance_amount = row(ui, Knob::VarianceAmount, &mut |ui| {
                knob(
                    ui,
                    egui::Slider::new(&mut k.variance_amount, 0.0..=1.0).text("amount"),
                    mixed.variance_amount,
                    says,
                )
            });
            touched.variance_steady = row(ui, Knob::VarianceSteady, &mut |ui| {
                knob(
                    ui,
                    egui::Slider::new(&mut k.variance_steady, 0.0..=1.0).text("steady"),
                    mixed.variance_steady,
                    says,
                )
            });
            touched.keep_size = row(ui, Knob::KeepSize, &mut |ui| {
                let keep = ui.add(
                    egui::Checkbox::new(&mut k.keep_size, "keep size")
                        .indeterminate(mixed.keep_size),
                );
                let changed = keep.changed();
                if mixed.keep_size {
                    keep.on_hover_text(says);
                }
                changed
            });
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
        let moved = k.moved(&before, &touched);
        if moved != WoozePreset::default() {
            // What moved is now the same on every prop it reached, and at a
            // species, its own, until the host says so itself next frame.
            let set = KnobFlags::set_by(&moved);
            match &species {
                Some(picked) => {
                    if let Some(own) =
                        (self.state.species.iter_mut()).find(|s| s.name == picked.name)
                    {
                        own.knobs = k;
                        own.mixed = mixed.without(&set);
                        for knob in Knob::ALL.into_iter().filter(|knob| set.get(*knob)) {
                            own.sources.set(knob, vec!["species".to_string()]);
                        }
                    }
                    let scope = PanelScope::Species(picked.name.clone());
                    self.push(LivingPanelAction::Set(scope, moved));
                }
                None => {
                    self.state.knobs = k;
                    self.state.mixed = mixed.without(&set);
                    self.push(LivingPanelAction::Set(PanelScope::Global, moved));
                }
            }
        }
        if let (Some(knob), Some(picked)) = (clear, &species) {
            self.push(LivingPanelAction::Clear(picked.name.clone(), Some(knob)));
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
                if ui
                    .button("Save")
                    .on_hover_text(
                        "A preset: the grove's knobs, and each species' own values as \
                         [living.species.<spec>] tables",
                    )
                    .clicked()
                {
                    self.push(LivingPanelAction::Save(self.save_as.clone()));
                }
            });
            self.commit_ui(ui);
            if let Some(status) = &self.state.status {
                ui.label(egui::RichText::new(status).small());
            }
        });
    }

    /// Commit to File (#249), and how much it would write.
    fn commit_ui(&mut self, ui: &mut egui::Ui) {
        let unsaved = self.state.unsaved.clone();
        ui.horizontal_wrapped(|ui| {
            let hover = if unsaved.is_empty() {
                "The scene file already gives every value as it stands.".to_string()
            } else {
                let mut lines: Vec<String> = unsaved.iter().take(12).cloned().collect();
                if unsaved.len() > 12 {
                    lines.push(format!("and {} more", unsaved.len() - 12));
                }
                format!(
                    "Writes into the scene file, each on its own line, the rest of the \
                     file as it is:\n{}",
                    lines.join("\n")
                )
            };
            if ui
                .add_enabled(!unsaved.is_empty(), egui::Button::new("Commit to File"))
                .on_hover_text(hover.clone())
                .on_disabled_hover_text(hover)
                .clicked()
            {
                self.push(LivingPanelAction::Commit);
            }
            match unsaved.len() {
                0 => ui.weak("the scene has it all"),
                1 => ui.colored_label(INK, "* 1 value unsaved"),
                n => ui.colored_label(INK, format!("* {n} values unsaved")),
            };
        });
    }

    /// Two presets on the same grove: pick them, compare, flip, split.
    fn ab_ui(&mut self, ui: &mut egui::Ui) {
        section(ui, "A/B", true, |ui| {
            let names = self.state.preset_names.clone();
            if let Some(ab) = self.state.ab.clone() {
                ui.horizontal(|ui| {
                    let tuned = if ab.showing_b { "B" } else { "A" };
                    ui.label(format!("A {}  ·  B {}", side_name(&ab.a), side_name(&ab.b)));
                    ui.weak(format!("tuning {tuned}"));
                });
                ui.horizontal(|ui| {
                    if ui
                        .button(if ab.showing_b { "Show A" } else { "Show B" })
                        .on_hover_text("F6: the other preset, same camera, same moment")
                        .clicked()
                    {
                        self.push(LivingPanelAction::Flip);
                    }
                    let mut split = ab.split;
                    if ui
                        .checkbox(&mut split, "Split")
                        .on_hover_text("F7: A on the left, B on the right, at once")
                        .changed()
                    {
                        self.push(LivingPanelAction::Split(split));
                    }
                    if ui
                        .button("Stop")
                        .on_hover_text("Keep the side that's shown")
                        .clicked()
                    {
                        self.push(LivingPanelAction::StopComparing);
                    }
                });
                if ab.split {
                    ui.weak("The knobs above turn the side with the dot.");
                }
                return;
            }
            let (mut a, mut b) = self.ab_picks.clone().unwrap_or_else(|| {
                let b = names
                    .iter()
                    .find(|n| *n == "linklater")
                    .or(names.first())
                    .cloned()
                    .unwrap_or_default();
                (String::new(), b)
            });
            let was = (a.clone(), b.clone());
            for (letter, pick, with_now) in [("A", &mut a, true), ("B", &mut b, false)] {
                egui::ComboBox::from_label(letter)
                    .selected_text(side_name(pick).to_string())
                    .show_ui(ui, |ui| {
                        if with_now {
                            ui.selectable_value(pick, String::new(), "as it was")
                                .on_hover_text("The knobs as they stand when it starts");
                        }
                        for name in &names {
                            ui.selectable_value(pick, name.clone(), name.as_str());
                        }
                    });
            }
            // The first frame only learns the defaults; a pick is a change.
            if (a.clone(), b.clone()) != was {
                self.push(LivingPanelAction::PickAb(a.clone(), b.clone()));
            }
            self.ab_picks = Some((a.clone(), b.clone()));
            if ui
                .add_enabled(!b.is_empty(), egui::Button::new("Compare"))
                .on_hover_text("F6 flips between them, F7 splits the view")
                .clicked()
            {
                self.push(LivingPanelAction::Compare(a, b));
            }
        });
    }

    /// Every prop, grouped by species (#247), narrowed by a filter on its name or
    /// its species'. A group's `tune` picks that species above.
    fn rows_ui(&mut self, ui: &mut egui::Ui) {
        section(ui, "Props", false, |ui| {
            ui.horizontal(|ui| {
                ui.weak("filter");
                ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .hint_text("name or species")
                        .desired_width(140.0),
                );
                if !self.filter.is_empty() && ui.small_button("\u{d7}").clicked() {
                    self.filter.clear();
                }
            });
            let filter = self.filter.to_lowercase();
            let rows = self.state.rows.clone();
            let mut groups: Vec<(&str, Vec<(usize, &LivingPropRow)>)> = Vec::new();
            for (i, row) in rows.iter().enumerate() {
                let shown = filter.is_empty()
                    || row.name.to_lowercase().contains(&filter)
                    || row.species.to_lowercase().contains(&filter);
                if !shown {
                    continue;
                }
                match groups
                    .iter_mut()
                    .find(|(species, _)| *species == row.species)
                {
                    Some((_, members)) => members.push((i, row)),
                    None => groups.push((&row.species, vec![(i, row)])),
                }
            }
            groups.sort_by(|a, b| a.0.cmp(b.0));
            if groups.is_empty() {
                ui.weak("No prop matches.");
            }
            egui::ScrollArea::vertical()
                .id_salt("living_rows")
                .max_height(260.0)
                .show(ui, |ui| {
                    for (species, members) in groups {
                        self.species_rows(ui, species, &members, !filter.is_empty());
                    }
                });
        });
    }

    /// One species' rows, under a header that folds them and can pick it.
    fn species_rows(
        &mut self,
        ui: &mut egui::Ui,
        species: &str,
        members: &[(usize, &LivingPropRow)],
        filtering: bool,
    ) {
        let picked = self.scope == PanelScope::Species(species.to_string());
        let id = ui.make_persistent_id(("living_species_rows", species));
        let mut fold =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, picked);
        if filtering {
            fold.set_open(true);
        }
        fold.show_header(ui, |ui| {
            let title = egui::RichText::new(format!("{species}  {}", members.len()));
            ui.label(if picked {
                title.strong().color(INK)
            } else {
                title
            });
            if ui
                .selectable_label(picked, egui::RichText::new("tune").small())
                .on_hover_text("Turn this species' own values above")
                .clicked()
            {
                self.scope = if picked {
                    PanelScope::Global
                } else {
                    PanelScope::Species(species.to_string())
                };
            }
        })
        .body(|ui| {
            egui::Grid::new(("living_rows_grid", species))
                .num_columns(4)
                .striped(true)
                .show(ui, |ui| {
                    ui.weak("prop");
                    ui.weak("lineage");
                    ui.weak("pose");
                    ui.weak("ring");
                    ui.end_row();
                    for (i, row) in members {
                        if ui.selectable_label(self.focus == *i, &row.name).clicked() {
                            self.focus = *i;
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
            .filter(|row| self.labelled(row))
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
        self.scope_ui(ui);
        self.knobs_ui(ui);
        self.presets_ui(ui);
        self.ab_ui(ui);
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

/// A scope's name, with a `*` and in ink if it has values the file doesn't.
fn unsaved_text(text: String, unsaved: bool) -> egui::RichText {
    if unsaved {
        egui::RichText::new(format!("{text} *")).color(INK)
    } else {
        egui::RichText::new(text)
    }
}

/// A scope's hover, saying what its `*` means if it has one.
fn with_unsaved(hover: &str, unsaved: bool) -> String {
    if unsaved {
        format!("{hover}\n\n{UNSAVED}")
    } else {
        hover.to_string()
    }
}

fn section(ui: &mut egui::Ui, title: &str, default_open: bool, add: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(title)
        .default_open(default_open)
        .show(ui, add);
}

/// A slider that reads `mixed` when the props disagree on it, and says `says`
/// about that when hovered. Returns whether a hand moved it.
fn knob(ui: &mut egui::Ui, slider: egui::Slider<'_>, mixed: bool, says: &str) -> bool {
    if !mixed {
        return ui.add(slider).changed();
    }
    ui.add(slider.custom_formatter(|_, _| "mixed".to_string()))
        .on_hover_text(says)
        .changed()
}

/// The boil's knobs: on or off, paint, ink, and its cadence.
fn boil_ui(ui: &mut egui::Ui, k: &mut LivingKnobs) {
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
}

/// Layers as a mark says them, short enough to sit beside a knob: the report's
/// labels, with two or more zones counted, `global · 3 zones`. The hover has
/// each one.
fn short_mark(layers: &[String]) -> String {
    let (zones, mut said): (Vec<&String>, Vec<&String>) =
        layers.iter().partition(|l| l.starts_with("zone:"));
    let counted = format!("{} zones", zones.len());
    match zones.len() {
        0 => {}
        1 => said.push(zones[0]),
        _ => said.push(&counted),
    }
    let said: Vec<&str> = said.iter().map(|s| s.as_str()).collect();
    said.join(" \u{b7} ")
}

/// A knob's mark, after it. Returns whether its clear was pressed.
///
/// At Global it's quiet unless a species sets its own value, and then says how
/// many, since the knob doesn't reach them. At a species it names the layers its
/// value comes from, as `flint render --report json` does, and the species' own
/// gets a clear beside it.
fn paint_mark(ui: &mut egui::Ui, mark: &Mark) -> bool {
    let small = |text: String, colour: Color32| egui::RichText::new(text).small().color(colour);
    match mark {
        Mark::Quiet => false,
        Mark::Owned(species) => {
            ui.label(small(format!("{} own", species.len()), PARTS))
                .on_hover_text(format!(
                    "{} {} their own, so this doesn't reach {}. Pick one under Tuning to \
                     see or clear it.",
                    species.join(", "),
                    if species.len() == 1 { "sets" } else { "set" },
                    if species.len() == 1 { "it" } else { "them" },
                ));
            false
        }
        Mark::From(layers) if layers.iter().any(|layer| layer == "species") => {
            ui.label(small("species".to_string(), INK))
                .on_hover_text(layer_says("species"));
            ui.add(egui::Button::new(small("\u{d7}".to_string(), INK)).small())
                .on_hover_text("Clear: its trees take the grove's value again")
                .clicked()
        }
        Mark::From(layers) => {
            let hover: Vec<String> = layers.iter().map(|layer| layer_says(layer)).collect();
            let faint = ui.visuals().weak_text_color();
            ui.label(small(short_mark(layers), faint))
                .on_hover_text(hover.join("\n"));
            false
        }
    }
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
            species: "oak".to_string(),
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
        // A hand that keeps to one scope: a press on the selector is put back.
        let scope = panel.scope().clone();
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
            panel.set_scope(scope.clone());
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
                LivingPanelAction::Set(_, moved) => Some(moved.clone()),
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

    // ---- Scopes (#247) ----

    /// Every knob from `layer`.
    fn all_from(layer: &str) -> KnobSources {
        let mut sources = KnobSources::default();
        for knob in Knob::ALL {
            sources.set(knob, vec![layer.to_string()]);
        }
        sources
    }

    /// One oak and two thorns. The thorns set their own echo count, and one
    /// stands in a zone with its own step.
    fn grove() -> LivingPanelState {
        let mut thorn_1 = row("thorn_1", [10.0, 0.0, -20.0]);
        thorn_1.species = "thorn".to_string();
        let mut thorn_2 = row("thorn_2", [-10.0, 0.0, -20.0]);
        thorn_2.species = "thorn".to_string();
        let mut thorn = LivingSpeciesState {
            name: "thorn".to_string(),
            props: 2,
            knobs: LivingKnobs {
                echo_count: 2,
                ..Default::default()
            },
            mixed: KnobFlags {
                step: true,
                ..Default::default()
            },
            sources: all_from("global"),
            unsaved: false,
        };
        thorn
            .sources
            .set(Knob::EchoCount, vec!["species".to_string()]);
        thorn.sources.set(
            Knob::Step,
            vec!["zone:hush".to_string(), "global".to_string()],
        );
        LivingPanelState {
            rows: vec![row("oak_1", [0.0, 0.0, -20.0]), thorn_1, thorn_2],
            shadows: "blended".to_string(),
            species: vec![
                LivingSpeciesState {
                    name: "oak".to_string(),
                    props: 1,
                    knobs: LivingKnobs::default(),
                    mixed: KnobFlags::default(),
                    sources: all_from("spec"),
                    unsaved: false,
                },
                thorn,
            ],
            ..state()
        }
    }

    /// Every piece of text the panel draws, and where, once its headers are open.
    fn shown(ctx: &egui::Context, panel: &mut LivingPropsDebugPanel) -> Vec<(String, Rect)> {
        frame(ctx, panel, Vec::new());
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(420.0, 1400.0))),
            ..Default::default()
        };
        let output = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| panel.ui(ui));
        });
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                Shape::Text(text) => Some((
                    text.galley.text().to_string(),
                    Rect::from_min_size(text.pos, text.galley.size()),
                )),
                _ => None,
            })
            .collect()
    }

    /// Click the first text that reads `text`, and return what that asked for.
    fn click_text(
        ctx: &egui::Context,
        panel: &mut LivingPropsDebugPanel,
        text: &str,
    ) -> Vec<LivingPanelAction> {
        let said = shown(ctx, panel);
        let (_, rect) = said
            .iter()
            .find(|(t, _)| t == text)
            .unwrap_or_else(|| panic!("no {text:?} in {said:?}"));
        let at = rect.center();
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(ctx, panel, vec![egui::Event::PointerMoved(at)]);
        frame(ctx, panel, vec![press(true)]);
        frame(ctx, panel, vec![press(false)]);
        frame(ctx, panel, vec![egui::Event::PointerGone]);
        panel.take_actions()
    }

    fn says(said: &[(String, Rect)], text: &str) -> usize {
        said.iter().filter(|(t, _)| t.contains(text)).count()
    }

    #[test]
    fn global_looks_as_it_did_with_a_mark_only_where_a_species_sets_its_own() {
        let ctx = egui::Context::default();
        let mut panel = LivingPropsDebugPanel::new();
        let mut plain = grove();
        for species in &mut plain.species {
            species.sources = all_from("global");
        }
        panel.refresh(plain);
        let said = shown(&ctx, &mut panel);
        for mark in ["own", "species", "zone:", "Whole scene only", "shadows"] {
            assert_eq!(says(&said, mark), 0, "{mark:?} in {said:?}");
        }
        assert_eq!(says(&said, "Shading boil"), 1);

        // The thorns' own count is the one thing Global has to say about them.
        panel.refresh(grove());
        let said = shown(&ctx, &mut panel);
        assert_eq!(says(&said, "1 own"), 1, "{said:?}");
        assert_eq!(says(&said, " own"), 1);
        assert!(!panel.is_dirty(), "showing marks asks for nothing");
    }

    #[test]
    fn a_species_knob_is_sent_for_that_species_and_its_clear_for_that_knob() {
        let ctx = egui::Context::default();
        let mut panel = LivingPropsDebugPanel::new();
        panel.refresh(grove());
        assert_eq!(
            click_text(&ctx, &mut panel, "thorn 2 \u{2022}"),
            Vec::new(),
            "picking a species asks the host for nothing"
        );
        assert_eq!(panel.scope(), &PanelScope::Species("thorn".to_string()));
        let said = shown(&ctx, &mut panel);
        assert_eq!(says(&said, "global \u{b7} zone:hush"), 1, "{said:?}");
        assert_eq!(said.iter().filter(|(t, _)| t == "species").count(), 1);

        let asked = press_down(&mut panel, 60.0);
        let sets: Vec<&LivingPanelAction> = asked
            .iter()
            .filter(|a| matches!(a, LivingPanelAction::Set(..)))
            .collect();
        assert!(!sets.is_empty());
        for set in sets {
            let LivingPanelAction::Set(scope, moved) = set else {
                unreachable!()
            };
            assert_eq!(scope, &PanelScope::Species("thorn".to_string()));
            assert_eq!(moved.boil, None, "a species gets no boil: {moved:?}");
        }

        let ctx = egui::Context::default();
        let mut panel = LivingPropsDebugPanel::new();
        panel.refresh(grove());
        panel.set_scope(PanelScope::Species("thorn".to_string()));
        assert_eq!(
            click_text(&ctx, &mut panel, "\u{d7}"),
            vec![LivingPanelAction::Clear(
                "thorn".to_string(),
                Some(Knob::EchoCount)
            )]
        );
        assert_eq!(
            click_text(&ctx, &mut panel, "Clear all"),
            vec![LivingPanelAction::Clear("thorn".to_string(), None)]
        );
    }

    #[test]
    fn at_a_species_the_boil_and_shadows_are_shown_off_with_the_reason() {
        let ctx = egui::Context::default();
        let mut panel = LivingPropsDebugPanel::new();
        panel.refresh(grove());
        panel.set_scope(PanelScope::Species("oak".to_string()));
        let said = shown(&ctx, &mut panel);
        assert_eq!(says(&said, "Whole scene only"), 1);
        assert_eq!(says(&said, "shadows: blended"), 1);
        assert_eq!(says(&said, "every species boils together"), 1, "{said:?}");
        assert_eq!(says(&said, "a species can't set them"), 1);
        assert_eq!(says(&said, "Clear all"), 1, "shown, if off: oak has none");

        // However hard it's pressed, the boil doesn't move.
        let mut asked = Vec::new();
        for x in [24.0, 60.0, 110.0, 160.0] {
            asked.extend(press_down(&mut panel, x));
        }
        for action in &asked {
            match action {
                LivingPanelAction::Set(scope, moved) => {
                    assert_eq!(scope, &PanelScope::Species("oak".to_string()));
                    assert_eq!(moved.boil, None);
                }
                // Once a knob is the species' own, its clear is there to press.
                LivingPanelAction::Clear(name, _) => assert_eq!(name, "oak"),
                _ => {}
            }
        }
    }

    #[test]
    fn a_picked_species_is_the_only_one_the_overlay_labels() {
        let ctx = egui::Context::default();
        let view_proj = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, -1.0, -1.0],
            [0.0, 0.0, -0.2, 0.0],
        ];
        let mut panel = LivingPropsDebugPanel::new();
        let mut on = grove();
        on.overlay = true;
        panel.refresh(on);
        let labels = |panel: &LivingPropsDebugPanel| {
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                panel.paint_overlay(ctx, &view_proj);
            });
            texts(&output)
        };
        let all = labels(&panel);
        assert_eq!(all.len(), 3, "{all:?}");
        panel.set_scope(PanelScope::Species("thorn".to_string()));
        assert_eq!(labels(&panel).len(), 2);
        panel.set_scope(PanelScope::Species("oak".to_string()));
        assert_eq!(labels(&panel).len(), 1);
    }

    #[test]
    fn the_props_are_grouped_by_species_and_a_filter_narrows_them() {
        let ctx = egui::Context::default();
        let mut panel = LivingPropsDebugPanel::new();
        panel.refresh(grove());
        click_text(&ctx, &mut panel, "Props");
        let said = shown(&ctx, &mut panel);
        assert_eq!(says(&said, "oak  1"), 1, "{said:?}");
        assert_eq!(says(&said, "thorn  2"), 1);
        assert_eq!(says(&said, "thorn_1"), 0, "folded until opened");

        panel.filter = "thorn".to_string();
        let said = shown(&ctx, &mut panel);
        assert_eq!(says(&said, "oak  1"), 0);
        assert_eq!(says(&said, "thorn_1"), 1, "a filter opens what it finds");
        assert_eq!(says(&said, "thorn_2"), 1);

        // A group's tune picks it.
        panel.filter.clear();
        click_text(&ctx, &mut panel, "tune");
        assert_eq!(panel.scope(), &PanelScope::Species("oak".to_string()));
    }

    #[test]
    fn a_species_that_goes_away_gives_the_grove_back() {
        let mut panel = LivingPropsDebugPanel::new();
        panel.refresh(grove());
        panel.set_scope(PanelScope::Species("thorn".to_string()));
        panel.refresh(state());
        assert_eq!(panel.scope(), &PanelScope::Global);
    }

    #[test]
    fn a_mark_counts_zones_once_there_are_two() {
        let labels = |l: &[&str]| -> Vec<String> { l.iter().map(|s| s.to_string()).collect() };
        assert_eq!(short_mark(&labels(&["species"])), "species");
        assert_eq!(
            short_mark(&labels(&["zone:hush", "global"])),
            "global \u{b7} zone:hush"
        );
        assert_eq!(
            short_mark(&labels(&[
                "zone:shiver",
                "global",
                "zone:drowse",
                "zone:hush"
            ])),
            "global \u{b7} 3 zones"
        );
    }

    #[test]
    fn every_knob_has_the_key_its_layer_report_gives_it() {
        let keys: Vec<&str> = Knob::ALL.iter().map(|k| k.key()).collect();
        assert_eq!(
            keys,
            [
                "amplitude",
                "step",
                "glide",
                "jitter",
                "stagger",
                "echoes.count",
                "echoes.spread",
                "echoes.alpha",
                "variance.amount",
                "variance.steady",
                "variance.keep_size",
            ]
        );
        for knob in Knob::ALL {
            assert_eq!(Knob::ALL[knob.index()], knob);
        }
    }

    #[test]
    fn with_nothing_unsaved_there_are_no_marks_and_commit_asks_for_nothing() {
        let ctx = egui::Context::default();
        let mut panel = LivingPropsDebugPanel::new();
        panel.refresh(grove());
        let said = shown(&ctx, &mut panel);
        assert_eq!(says(&said, " *"), 0, "{said:?}");
        assert_eq!(says(&said, "the scene has it all"), 1, "{said:?}");
        assert!(click_text(&ctx, &mut panel, "Commit to File").is_empty());
    }

    #[test]
    fn an_unsaved_level_wears_a_star_and_commit_sends_once() {
        let ctx = egui::Context::default();
        let mut panel = LivingPropsDebugPanel::new();
        let mut state = grove();
        state.species[1].unsaved = true;
        state.unsaved = vec!["[living.species.thorn] echoes.count = 4".to_string()];
        panel.refresh(state.clone());
        let said = shown(&ctx, &mut panel);
        assert_eq!(says(&said, "thorn 2 \u{2022} *"), 1, "{said:?}");
        assert_eq!(says(&said, "Global *"), 0, "the grove's own are saved");
        assert_eq!(says(&said, "* 1 value unsaved"), 1, "{said:?}");

        state.unsaved_global = true;
        state.unsaved.push("[wooze] glide = 0.8".to_string());
        panel.refresh(state);
        let said = shown(&ctx, &mut panel);
        assert_eq!(says(&said, "Global *"), 1, "{said:?}");
        assert_eq!(says(&said, "* 2 values unsaved"), 1, "{said:?}");
        let asked = click_text(&ctx, &mut panel, "Commit to File");
        assert_eq!(asked, [LivingPanelAction::Commit]);
    }
}
