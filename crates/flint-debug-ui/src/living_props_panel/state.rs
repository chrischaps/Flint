//! What the host tells the panel each frame, and what the panel asks of it:
//! plain data, so this crate never depends on the living props' runtime.

use flint_procgen::WoozePreset;

use super::knobs::{Knob, KnobFlags, KnobSources, LivingKnobs};

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
    /// How far its crown reaches sideways from the trunk at rest (#248).
    pub reach: f32,
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

/// The tree picked in the world (#248), value by value as it reads them: the
/// spec, the entity, the grove, a zone and its species, together.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LivingPickedState {
    /// Its entity's name, as its row has it.
    pub name: String,
    /// The zones it stands in now, each with how much of itself it gives there.
    pub zones: Vec<(String, f32)>,
    /// Every value a species can set, as this tree runs on it.
    pub knobs: LivingKnobs,
    /// The one layer each of those values comes from, as `flint render --report
    /// json`'s `values` names it.
    pub sources: KnobSources,
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
    /// The tree picked in the world, as the host resolved it (#248).
    pub picked: Option<LivingPickedState>,
    /// Whether the grove's own values differ from what the scene file gives
    /// (#249): its `[wooze]`, `[living]` or boil.
    pub unsaved_global: bool,
    /// What Commit to File would write, one line a value, as
    /// `[living.species.grove_thorn] echoes.count = 4`.
    pub unsaved: Vec<String>,
}

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
pub(super) fn side_name(name: &str) -> &str {
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
