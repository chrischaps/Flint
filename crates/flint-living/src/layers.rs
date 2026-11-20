//! Where a prop's values come from (#246): the spec, the entity, the scene's
//! globals and every knob moved live for the grove, the wooze field and its
//! zones, and a species' own, lowest first. Pure data, apart from the
//! `impl LivingProps` at the bottom, which turns a knob and brings the props in
//! its reach up to it.
//!
//! Every knob resolves through one [`Layer`]: a value, or a config's fields one
//! at a time, said over what's below it. A prop keeps a [`Layered`] per knob
//! (its lowest word and the global layer's), its species an override of each,
//! so turning one is the same three lines for every knob (#753).

use std::collections::BTreeMap;

use flint_wooze::{WoozeField, WoozeSample};

use super::prop::PendingClocks;
use super::{LivingPropId, LivingProps};
use flint_procgen::{
    EchoConfig, KnobType, KnobValue, LivingDef, LivingKnob, LivingSpeciesDef, Stagger,
    VarianceConfig,
};

// The two rates' ceilings are flint-procgen's, which holds a preset file's
// rates to them too.
pub use flint_procgen::living::{JITTER_RATE_MAX, ROW_RATE_MAX};

/// A jitter rate in the range one can be: 0 (held) to [`JITTER_RATE_MAX`]. Not a
/// number is 1, the spec's pace.
pub fn jitter_rate_clamped(rate: f32) -> f32 {
    if rate.is_nan() {
        1.0
    } else {
        rate.clamp(0.0, JITTER_RATE_MAX)
    }
}

/// A row rate in the range one can be: 0 (held) to [`ROW_RATE_MAX`]. Not a
/// number is 1, the spec's pace.
pub fn row_rate_clamped(rate: f32) -> f32 {
    if rate.is_nan() {
        1.0
    } else {
        rate.clamp(0.0, ROW_RATE_MAX)
    }
}

/// What the shadows of a scene's living props follow, which is its own choice:
/// a whole tree's shadow boiling can read as life or as noise.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LivingShadows {
    /// Exactly what's drawn, stagger and leaf jitter included. No extra cost.
    #[default]
    Blended,
    /// A slower pose with no stagger and no jitter, in bones of its own.
    Drift,
}

/// One layer's word on a knob, over what the layers below it said: `Option<T>`
/// for a single value, or an override of a config a field at a time.
pub trait Layer: Copy + Default + PartialEq {
    /// What the knob runs on once every layer has spoken.
    type Value: Copy + PartialEq;

    /// This layer over what's below it.
    fn over_value(&self, below: Self::Value) -> Self::Value;

    /// A newer word from this layer over an older one, a field at a time.
    fn merged_with(self, under: Self) -> Self;
}

impl<T: Copy + PartialEq> Layer for Option<T> {
    type Value = T;

    fn over_value(&self, below: T) -> T {
        self.unwrap_or(below)
    }

    fn merged_with(self, under: Self) -> Self {
        self.or(under)
    }
}

/// An override of a config, each field `None` for "what's below", with the
/// three ways a layer is read. One list of fields, so nothing is kept equal by
/// hand.
macro_rules! field_layer {
    (
        $(#[$meta:meta])*
        $name:ident over $config:ident {
            $( $(#[$fmeta:meta])* $field:ident: $ty:ty ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, Default, PartialEq)]
        pub struct $name {
            $( $(#[$fmeta])* pub $field: Option<$ty>, )*
        }

        impl $name {
            /// These fields over a whole config.
            pub fn over(&self, below: $config) -> $config {
                $config { $( $field: self.$field.unwrap_or(below.$field), )* }
            }

            /// Every field set, to these.
            pub fn full(config: $config) -> Self {
                Self { $( $field: Some(config.$field), )* }
            }

            /// These over `under`, a field at a time.
            pub fn merged(self, under: Self) -> Self {
                Self { $( $field: self.$field.or(under.$field), )* }
            }
        }

        impl Layer for $name {
            type Value = $config;

            fn over_value(&self, below: $config) -> $config {
                self.over(below)
            }

            fn merged_with(self, under: Self) -> Self {
                self.merged(under)
            }
        }
    };
}

field_layer! {
    /// A species' word on the four knobs a wooze field has (#246).
    ClockOverride over WoozeSample {
        /// How much of its slow drift shows.
        amplitude: f32,
        /// Seconds between pose targets.
        step: f32,
        /// How much of a step is spent easing.
        glide: f32,
        /// How much of its fast jitter shows.
        jitter: f32,
    }
}

field_layer! {
    /// A scene's `[living] echoes`: whichever of a spec's fields it sets. The
    /// wooze field (#213) has no knob for these yet.
    EchoOverride over EchoConfig {
        /// How many copies. `None`: the spec's.
        count: u32,
        /// How far the farthest copy is from now, in steps. `None`: the spec's.
        spread: f32,
        /// The opacity of a copy at now. `None`: the spec's.
        alpha: f32,
    }
}

field_layer! {
    /// A scene's `[living] variance`: whichever of a spec's fields it sets.
    VarianceOverride over VarianceConfig {
        /// How much of each pose's difference is left. `None`: the spec's.
        amount: f32,
        /// How much of that is held back from the trunk. `None`: the spec's.
        steady: f32,
        /// Whether poses keep the base's size. `None`: the spec's.
        keep_size: bool,
    }
}

/// One knob's layers below a species: the lowest word (the spec's, or the
/// entity's) and the global layer's, which a scene's `[living]` and every knob
/// moved live for the whole grove write.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layered<L: Layer> {
    /// The spec's value, or the entity's where it gives one.
    pub base: L::Value,
    /// The global layer's word over it.
    pub global: L,
}

impl<L: Layer> Layered<L> {
    /// `base` with the global layer's word over it.
    pub fn new(base: L::Value, global: L) -> Self {
        Self { base, global }
    }

    /// What it runs on, with a species' own word over the rest.
    pub fn value(&self, species: L) -> L::Value {
        species.over_value(self.global.over_value(self.base))
    }

    /// Say more in the global layer: whatever `change` sets, over what it said.
    pub fn tune(&mut self, change: L) {
        self.global = change.merged_with(self.global);
    }
}

/// What a scene says about its living props, ahead of any one spec. How woozy
/// they are is the wooze field's to say (#213).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LivingDefaults {
    /// What the shadows follow.
    pub shadows: LivingShadows,
    /// The scene's word on echoes, over each spec's, a field at a time.
    pub echoes: EchoOverride,
    /// The scene's word on stagger, over each spec's and each prop's own: it's
    /// what a tuned preset holds, and has to look the same when it loads.
    pub stagger: Option<Stagger>,
    /// The scene's word on variance, over each spec's, a field at a time.
    pub variance: VarianceOverride,
    /// The scene's word on softness (#582), over each spec's.
    pub softness: Option<f32>,
    /// The scene's word on how fast the boil ticks (#529), times each spec's
    /// `jitter.step` pace: 2 is twice as fast, 0.5 half.
    pub jitter_rate: Option<f32>,
    /// The scene's word on how fast a form's words drift along their lines
    /// (#576), times each spec's `row_pace`: 2 is twice as fast, 0 holds them.
    pub row_rate: Option<f32>,
    /// `[living.species.<spec>]` (#246): each species' own values, over all of
    /// the above and over the wooze field, zones included. Keyed by spec name.
    pub species: BTreeMap<String, SpeciesOverride>,
}

/// One species' own values (#246): a layer over everything the scene says for
/// every prop, so a global knob moved live leaves them standing. A species is a
/// spec, with all its lineage seeds. Unset fields are whatever's below.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SpeciesOverride {
    /// Its clock's knobs, over what the field says where each prop stands, or
    /// over its spec's in a scene with no field.
    pub clock: ClockOverride,
    /// Its stagger, over the scene's, each prop's own and the spec's.
    pub stagger: Option<Stagger>,
    /// Its echoes, a field at a time.
    pub echoes: EchoOverride,
    /// Its variance, a field at a time.
    pub variance: VarianceOverride,
    /// Its softness (#582), over the scene's and the spec's.
    pub softness: Option<f32>,
    /// Its jitter rate (#529), over the scene's.
    pub jitter_rate: Option<f32>,
    /// Its row rate (#576), over the scene's.
    pub row_rate: Option<f32>,
}

impl SpeciesOverride {
    /// These values over `under`'s, a field at a time.
    pub fn merged(self, under: SpeciesOverride) -> SpeciesOverride {
        SpeciesOverride {
            clock: self.clock.merged(under.clock),
            stagger: self.stagger.or(under.stagger),
            echoes: self.echoes.merged(under.echoes),
            variance: self.variance.merged(under.variance),
            softness: self.softness.or(under.softness),
            jitter_rate: self.jitter_rate.or(under.jitter_rate),
            row_rate: self.row_rate.or(under.row_rate),
        }
    }

    /// Whether it says nothing, and the species is as the scene would have it.
    pub fn is_empty(&self) -> bool {
        *self == SpeciesOverride::default()
    }

    /// What it says of one knob, if anything: the runtime's own field for each
    /// of [`LivingKnob::ALL`] (#1025).
    pub fn get(&self, knob: LivingKnob) -> Option<KnobValue> {
        let (clock, echoes, variance) = (self.clock, self.echoes, self.variance);
        match knob {
            LivingKnob::Amplitude => clock.amplitude.map(KnobType::into_knob),
            LivingKnob::Step => clock.step.map(KnobType::into_knob),
            LivingKnob::Glide => clock.glide.map(KnobType::into_knob),
            LivingKnob::Jitter => clock.jitter.map(KnobType::into_knob),
            LivingKnob::Stagger => self.stagger.map(|s| KnobValue::Name(s.name().to_string())),
            LivingKnob::EchoCount => echoes.count.map(KnobType::into_knob),
            LivingKnob::EchoSpread => echoes.spread.map(KnobType::into_knob),
            LivingKnob::EchoAlpha => echoes.alpha.map(KnobType::into_knob),
            LivingKnob::VarianceAmount => variance.amount.map(KnobType::into_knob),
            LivingKnob::VarianceSteady => variance.steady.map(KnobType::into_knob),
            LivingKnob::KeepSize => variance.keep_size.map(KnobType::into_knob),
            LivingKnob::Softness => self.softness.map(KnobType::into_knob),
            LivingKnob::JitterRate => self.jitter_rate.map(KnobType::into_knob),
            LivingKnob::RowRate => self.row_rate.map(KnobType::into_knob),
        }
    }

    /// Forget its word on one knob, so the layers below decide it again.
    pub fn clear(&mut self, knob: LivingKnob) {
        match knob {
            LivingKnob::Amplitude => self.clock.amplitude = None,
            LivingKnob::Step => self.clock.step = None,
            LivingKnob::Glide => self.clock.glide = None,
            LivingKnob::Jitter => self.clock.jitter = None,
            LivingKnob::Stagger => self.stagger = None,
            LivingKnob::EchoCount => self.echoes.count = None,
            LivingKnob::EchoSpread => self.echoes.spread = None,
            LivingKnob::EchoAlpha => self.echoes.alpha = None,
            LivingKnob::VarianceAmount => self.variance.amount = None,
            LivingKnob::VarianceSteady => self.variance.steady = None,
            LivingKnob::KeepSize => self.variance.keep_size = None,
            LivingKnob::Softness => self.softness = None,
            LivingKnob::JitterRate => self.jitter_rate = None,
            LivingKnob::RowRate => self.row_rate = None,
        }
    }

    /// Forget what `keys` name, each a knob's key or a table's (`echoes`), or
    /// everything with none. Returns the keys that name nothing, if any.
    pub fn clear_keys(&mut self, keys: &[String]) -> Result<(), Vec<String>> {
        if keys.is_empty() {
            *self = SpeciesOverride::default();
            return Ok(());
        }
        let mut unknown = Vec::new();
        for key in keys {
            let named: Vec<LivingKnob> = (LivingKnob::ALL.into_iter())
                .filter(|knob| knob.named_by(key))
                .collect();
            if named.is_empty() {
                unknown.push(key.clone());
            }
            for knob in named {
                self.clear(knob);
            }
        }
        if unknown.is_empty() {
            Ok(())
        } else {
            Err(unknown)
        }
    }

    /// These values as a scene's `[living.species.<spec>]` says them.
    pub fn to_def(&self) -> LivingSpeciesDef {
        let mut def = LivingSpeciesDef::default();
        for knob in LivingKnob::ALL {
            if let Some(value) = self.get(knob) {
                knob.set_in_species(&mut def, &value);
            }
        }
        def
    }
}

/// Which props a live change reaches (#246): every one, or one species'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivingScope<'a> {
    /// The global layer: every prop, under any species' own value.
    All,
    /// One species' layer, by spec name, which beats the global one.
    Species(&'a str),
}

/// Where a prop's value came from (#246), lowest first. A layer beats every one
/// before it in this list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LivingLayer {
    /// The spec's `[living]` table, or for amplitude and jitter, as authored.
    Spec,
    /// The entity's `living_prop` component.
    Entity,
    /// The scene's `[living]` or `[wooze]` globals, or a knob moved live for the
    /// whole grove.
    Global,
    /// A wooze zone, by name, where the prop stands.
    Zone(String),
    /// `[living.species.<spec>]`, or a knob moved live for that species.
    Species,
}

impl LivingLayer {
    /// How a report names it: `spec`, `entity`, `global`, `zone:<name>` or
    /// `species`.
    pub fn label(&self) -> String {
        match self {
            LivingLayer::Spec => "spec".to_string(),
            LivingLayer::Entity => "entity".to_string(),
            LivingLayer::Global => "global".to_string(),
            LivingLayer::Zone(name) => format!("zone:{name}"),
            LivingLayer::Species => "species".to_string(),
        }
    }

    /// The layer a field came from, out of a species', the global one and
    /// `lowest`.
    fn of(species: bool, global: bool, lowest: LivingLayer) -> LivingLayer {
        if species {
            LivingLayer::Species
        } else if global {
            LivingLayer::Global
        } else {
            lowest
        }
    }
}

/// A value and the layer that set it.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved<T> {
    /// What the prop runs on.
    pub value: T,
    /// Who said so.
    pub layer: LivingLayer,
}

/// Every value a species can set on one prop, resolved, each with its layer,
/// kept by [`LivingKnob`] (#1025): a reader loops over [`LivingKnob::ALL`]
/// rather than naming each knob.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedProp {
    /// Its spec's name.
    pub species: String,
    values: [Resolved<KnobValue>; LivingKnob::COUNT],
}

impl ResolvedProp {
    /// One knob's value and the layer that set it.
    pub fn get(&self, knob: LivingKnob) -> &Resolved<KnobValue> {
        &self.values[knob.index()]
    }

    /// Every knob's, in [`LivingKnob::ALL`]'s order.
    pub fn iter(&self) -> impl Iterator<Item = (LivingKnob, &Resolved<KnobValue>)> {
        LivingKnob::ALL.into_iter().zip(&self.values)
    }
}

/// Every layer the grove's values are made of, apart from the field, to put back
/// with [`LivingProps::restore_layers`]: what "As loaded" goes back to.
#[derive(Debug, Clone, PartialEq)]
pub struct LivingLayers {
    pub(super) echoes: Vec<EchoOverride>,
    pub(super) staggers: Vec<Option<Stagger>>,
    pub(super) softnesses: Vec<Option<f32>>,
    pub(super) jitter_rates: Vec<Option<f32>>,
    pub(super) row_rates: Vec<Option<f32>>,
    pub(super) clocks: Vec<(WoozeSample, bool)>,
    pub(super) variances: Vec<VarianceOverride>,
    pub(super) species: Vec<(String, SpeciesOverride)>,
}

impl LivingProps {
    /// Turn a knob that every prop keeps layers of: in each prop's global layer,
    /// or in one species' own. Returns whether any prop is in reach.
    fn tune_props<L: Layer>(
        &mut self,
        scope: LivingScope,
        change: L,
        layers: fn(&mut super::prop::LivingProp) -> &mut Layered<L>,
        own: fn(&mut SpeciesOverride) -> &mut L,
    ) -> bool {
        match scope {
            LivingScope::All => self.props.iter_mut().for_each(|p| layers(p).tune(change)),
            LivingScope::Species(name) => {
                let species = self.species_index(name);
                let own = own(&mut self.species[species].over);
                *own = change.merged_with(*own);
            }
        }
        self.refresh_props(scope)
    }

    /// Set whichever echo fields `echoes` sets, for every prop in the global
    /// layer or in one species' own (#246). A prop takes them as soon as its
    /// ring can show them, keeping the ones it has until then. Returns whether
    /// any prop has them: a species no prop grows from keeps them, for none.
    pub fn tune_echoes(&mut self, scope: LivingScope, echoes: EchoOverride) -> bool {
        self.tune_props(scope, echoes, |p| &mut p.echo_layers, |s| &mut s.echoes)
    }

    /// Put every prop on this stagger in the global layer, or one species in its
    /// own (#246), as soon as its ring can show it. A part jumps to its new place
    /// on the clock: no stagger can change without that. Returns whether any prop
    /// is of it.
    pub fn tune_stagger(&mut self, scope: LivingScope, stagger: Stagger) -> bool {
        self.tune_props(
            scope,
            Some(stagger),
            |p| &mut p.stagger_layers,
            |s| &mut s.stagger,
        )
    }

    /// Give every prop this softness in the global layer, or one species this
    /// softness of its own (#582). It changes no pose and fills no ring: only how
    /// the body draws, from the next upload. Returns whether any prop has it.
    pub fn tune_softness(&mut self, scope: LivingScope, softness: f32) -> bool {
        let softness = Some(softness.clamp(0.0, 1.0));
        self.tune_props(
            scope,
            softness,
            |p| &mut p.softness_layers,
            |s| &mut s.softness,
        )
    }

    /// Run every prop's boil at this rate in the global layer, or one species' at
    /// a rate of its own (#529): how fast its jitter clock ticks, times its spec's
    /// `jitter.step` pace, from 0 (held) to [`JITTER_RATE_MAX`]. The clock moves on
    /// at the new rate from where it stands, so no card turns or shade changes
    /// at the moment it's set: each channel holds what it shows until its next
    /// tick, which comes sooner or later. It changes no pose and fills no ring.
    /// Returns whether any prop has it.
    pub fn tune_jitter_rate(&mut self, scope: LivingScope, rate: f32) -> bool {
        let rate = Some(jitter_rate_clamped(rate));
        self.tune_props(
            scope,
            rate,
            |p| &mut p.jitter_rate_layers,
            |s| &mut s.jitter_rate,
        )
    }

    /// Set how fast a form's words drift along their lines (#576), times its
    /// spec's `row_pace`, from 0 (held) to [`ROW_RATE_MAX`]. Like the jitter
    /// rate, the row clock moves on at the new rate from where it stands, so no
    /// word jumps on the frame it's set: it only goes on faster or slower. It
    /// changes no pose and fills no ring. Returns whether any prop has it.
    pub fn tune_row_rate(&mut self, scope: LivingScope, rate: f32) -> bool {
        let rate = Some(row_rate_clamped(rate));
        self.tune_props(scope, rate, |p| &mut p.row_rate_layers, |s| &mut s.row_rate)
    }

    /// Set whichever variance fields `variance` sets, for every lineage in the
    /// global layer or one species' own (#246). What the rings have is re-held
    /// from its full form, a few poses a frame, and what's made from here on
    /// starts at it. A lineage belongs to one spec, so a species' change re-holds
    /// only its lineages. Returns whether any prop is of it.
    pub fn tune_variance(&mut self, scope: LivingScope, variance: VarianceOverride) -> bool {
        match scope {
            LivingScope::All => (self.lineages.iter_mut())
                .for_each(|lineage| lineage.variance_layers.tune(variance)),
            LivingScope::Species(name) => {
                let species = self.species_index(name);
                let over = &mut self.species[species].over;
                over.variance = variance.merged(over.variance);
            }
        }
        self.refresh_props(scope)
    }

    /// Set whichever clock knobs `clock` sets for one species (#246), over the
    /// field and its zones. A species is the only scope this has: the global
    /// clock is the field's. Returns whether any prop is of it.
    pub fn tune_clock(&mut self, species: &str, clock: ClockOverride) -> bool {
        let index = self.species_index(species);
        let over = &mut self.species[index].over;
        over.clock = clock.merged(over.clock);
        self.refresh_props(LivingScope::Species(species))
    }

    /// Replace one species' own values whole (#246), which is how one is cleared,
    /// a knob or all of them at a time. Returns whether any prop is of it.
    pub fn set_species(&mut self, species: &str, over: SpeciesOverride) -> bool {
        let index = self.species_index(species);
        self.species[index].over = over;
        self.refresh_props(LivingScope::Species(species))
    }

    /// Put one prop's clock where `wooze` says, as the field would in a scene
    /// that has one. For giving a scene with no field its specs' clocks back:
    /// a field, while there is one, says again each tick. Its species' own clock
    /// values (#246) still beat this.
    pub fn set_wooze_of(&mut self, id: LivingPropId, wooze: WoozeSample) {
        let prop = &mut self.props[id.0];
        prop.wooze_below = wooze;
        prop.clock_live = true;
        self.refresh_clock(id.0);
    }

    /// How soft a prop's silhouette draws, 0 to 1, from its layers: its
    /// species' own, then the global layer's, then its spec's.
    pub fn softness(&self, id: LivingPropId) -> f32 {
        let prop = &self.props[id.0];
        (prop.softness_layers).value(self.species[prop.species].over.softness)
    }

    /// How fast a prop's boil ticks against its spec's pace (#529), from its
    /// layers: its species' own, then the global layer's, then 1.
    pub fn jitter_rate(&self, id: LivingPropId) -> f32 {
        let prop = &self.props[id.0];
        (prop.jitter_rate_layers).value(self.species[prop.species].over.jitter_rate)
    }

    /// How fast a prop's words drift against its spec's `row_pace` (#576),
    /// from its layers: its species' own, then the global layer's, then 1.
    pub fn row_rate(&self, id: LivingPropId) -> f32 {
        let prop = &self.props[id.0];
        (prop.row_rate_layers).value(self.species[prop.species].over.row_rate)
    }

    /// What the global layer says of the row rate: the first prop's, which a
    /// scene's `[living] row_rate` and every live change set alike.
    pub fn global_row_rate(&self) -> Option<f32> {
        self.props
            .first()
            .and_then(|prop| prop.row_rate_layers.global)
    }

    /// What the global layer says of the jitter rate: the first prop's, which a
    /// scene's `[living] jitter_rate` and every live change set alike.
    pub fn global_jitter_rate(&self) -> Option<f32> {
        self.props
            .first()
            .and_then(|prop| prop.jitter_rate_layers.global)
    }

    /// What the global layer says of softness: the first prop's, which a scene's
    /// `[living] softness` and every live change set alike for all of them.
    pub fn global_softness(&self) -> Option<f32> {
        self.props
            .first()
            .and_then(|prop| prop.softness_layers.global)
    }

    /// What the global layer says (#249): the first prop's echoes and stagger and
    /// the first lineage's variance, which every live change and a scene's
    /// `[living]` set alike for all of them. Unset fields are each spec's.
    pub fn global_layer(&self) -> (EchoOverride, Option<Stagger>, VarianceOverride) {
        let prop = self.props.first();
        (
            prop.map(|p| p.echo_layers.global).unwrap_or_default(),
            prop.and_then(|p| p.stagger_layers.global),
            (self.lineages.first())
                .map(|l| l.variance_layers.global)
                .unwrap_or_default(),
        )
    }

    /// The global layer as a scene's `[living]` says it (#249), a field at a
    /// time: what a scene file would hold for it. No species, no shadows.
    pub fn global_def(&self) -> LivingDef {
        let (echoes, stagger, variance) = self.global_layer();
        let global = SpeciesOverride {
            clock: ClockOverride::default(),
            stagger,
            echoes,
            variance,
            softness: self.global_softness(),
            jitter_rate: self.global_jitter_rate(),
            row_rate: self.global_row_rate(),
        };
        let mut living = LivingDef::default();
        for knob in LivingKnob::ALL {
            if let Some(value) = global.get(knob) {
                knob.set_in_living(&mut living, &value);
            }
        }
        living
    }

    /// Every layer as it stands, apart from the field, to go back to.
    pub fn layers(&self) -> LivingLayers {
        LivingLayers {
            echoes: self.props.iter().map(|p| p.echo_layers.global).collect(),
            staggers: self.props.iter().map(|p| p.stagger_layers.global).collect(),
            softnesses: (self.props.iter())
                .map(|p| p.softness_layers.global)
                .collect(),
            jitter_rates: (self.props.iter())
                .map(|p| p.jitter_rate_layers.global)
                .collect(),
            row_rates: (self.props.iter())
                .map(|p| p.row_rate_layers.global)
                .collect(),
            clocks: (self.props.iter())
                .map(|p| (p.wooze_below, p.clock_live))
                .collect(),
            variances: (self.lineages.iter())
                .map(|l| l.variance_layers.global)
                .collect(),
            species: (self.species.iter())
                .map(|s| (s.name.clone(), s.over))
                .collect(),
        }
    }

    /// Put every layer back as [`Self::layers`] found it. In a scene with a
    /// field the clocks come from it again on the next tick.
    pub fn restore_layers(&mut self, layers: &LivingLayers) {
        for (prop, echoes) in self.props.iter_mut().zip(&layers.echoes) {
            prop.echo_layers.global = *echoes;
        }
        for (prop, stagger) in self.props.iter_mut().zip(&layers.staggers) {
            prop.stagger_layers.global = *stagger;
        }
        for (prop, softness) in self.props.iter_mut().zip(&layers.softnesses) {
            prop.softness_layers.global = *softness;
        }
        for (prop, rate) in self.props.iter_mut().zip(&layers.jitter_rates) {
            prop.jitter_rate_layers.global = *rate;
        }
        for (prop, rate) in self.props.iter_mut().zip(&layers.row_rates) {
            prop.row_rate_layers.global = *rate;
        }
        for (prop, (below, live)) in self.props.iter_mut().zip(&layers.clocks) {
            prop.wooze_below = *below;
            prop.clock_live = *live;
        }
        for (lineage, variance) in self.lineages.iter_mut().zip(&layers.variances) {
            lineage.variance_layers.global = *variance;
        }
        for species in &mut self.species {
            species.over = (layers.species.iter())
                .find(|(name, _)| *name == species.name)
                .map_or_else(SpeciesOverride::default, |(_, over)| *over);
        }
        self.refresh_props(LivingScope::All);
    }

    /// Bring every prop in `scope`, and its lineage, up to what its layers say
    /// now. Returns whether there were any.
    fn refresh_props(&mut self, scope: LivingScope) -> bool {
        let wanted = match scope {
            LivingScope::All => None,
            LivingScope::Species(name) => match self.species.iter().position(|s| s.name == name) {
                Some(index) => Some(index),
                None => return false,
            },
        };
        let mut any = false;
        for i in 0..self.props.len() {
            if wanted.is_some_and(|species| self.props[i].species != species) {
                continue;
            }
            any = true;
            self.refresh_echoes(i);
            self.refresh_stagger(i);
            self.refresh_clock(i);
        }
        for lineage in 0..self.lineages.len() {
            if wanted.is_some_and(|species| self.lineages[lineage].species != species) {
                continue;
            }
            self.refresh_variance(lineage);
        }
        any
    }

    /// One prop's echoes, from its layers.
    fn refresh_echoes(&mut self, i: usize) {
        let prop = &self.props[i];
        let echoes = (prop.echo_layers).value(self.species[prop.species].over.echoes);
        let drift = self.defaults.shadows == LivingShadows::Drift;
        let prop = &mut self.props[i];
        if prop.echo_config == echoes {
            return;
        }
        prop.echo_config = echoes;
        let stagger = prop.stagger();
        prop.pending = Some(PendingClocks {
            stagger,
            copies: echoes.copies_placed(&prop.echo_places),
        });
        if prop.ring.is_none() {
            prop.start_pending();
        }
        prop.fit_ring(drift);
    }

    /// One prop's stagger, from its layers.
    fn refresh_stagger(&mut self, i: usize) {
        let prop = &self.props[i];
        let stagger = (prop.stagger_layers).value(self.species[prop.species].over.stagger);
        let drift = self.defaults.shadows == LivingShadows::Drift;
        let prop = &mut self.props[i];
        if prop.stagger() == stagger {
            return;
        }
        let copies = match &prop.pending {
            Some(pending) => pending.copies.clone(),
            None => prop.echoes.iter().map(|e| e.copy).collect(),
        };
        prop.pending = Some(PendingClocks { stagger, copies });
        if prop.ring.is_none() {
            prop.start_pending();
        }
        prop.fit_ring(drift);
    }

    /// One prop's clock, from what's below its species and its species' own.
    pub(super) fn refresh_clock(&mut self, i: usize) {
        let clock = self.species[self.props[i].species].over.clock;
        let prop = &mut self.props[i];
        prop.wooze = clock.over(prop.wooze_below);
        prop.clock.step = prop.wooze.step;
        prop.clock.glide = prop.wooze.glide;
    }

    /// One lineage's variance, from its layers. A change re-holds its poses.
    fn refresh_variance(&mut self, index: usize) {
        let lineage = &self.lineages[index];
        let variance = (lineage.variance_layers).value(self.species[lineage.species].over.variance);
        let lineage = &mut self.lineages[index];
        if lineage.variance == variance {
            return;
        }
        lineage.variance = variance;
        lineage.generation += 1;
        self.reholding = true;
        self.rehold_cursor = 0;
        self.rehold_pass = 0;
    }

    /// Every value a species can set on one prop, with the layer that set it
    /// (#246). `field` is the wooze field and the moment the props last read it,
    /// in a scene that has one, to name a zone where one has the last word.
    pub fn resolved(&self, id: LivingPropId, field: Option<(&WoozeField, f64)>) -> ResolvedProp {
        let prop = &self.props[id.0];
        let species = &self.species[prop.species];
        let over = species.over;
        let lineage = &self.lineages[prop.lineage];
        let zones = field.map(|(field, time)| field.zones_at(prop.position, time));
        let clock_layer = |knob: usize, set: Option<f32>| {
            if set.is_some() {
                LivingLayer::Species
            } else if let Some(zones) = &zones {
                zones[knob].map_or(LivingLayer::Global, |zone| LivingLayer::Zone(zone.into()))
            } else if prop.clock_live {
                LivingLayer::Global
            } else {
                LivingLayer::Spec
            }
        };
        fn resolved(value: impl KnobType, layer: LivingLayer) -> Resolved<KnobValue> {
            Resolved {
                value: value.into_knob(),
                layer,
            }
        }
        let from =
            |species: bool, global: bool| LivingLayer::of(species, global, LivingLayer::Spec);
        let stagger_lowest = if prop.stagger_entity {
            LivingLayer::Entity
        } else {
            LivingLayer::Spec
        };
        let (echoes, global) = (prop.echo_config, prop.echo_layers.global);
        let (variance, global_variance) = (lineage.variance, lineage.variance_layers.global);
        let (own_echoes, own_variance) = (over.echoes, over.variance);
        // Each knob's own resolution: its value where the prop runs on it, and
        // which layer's word that is.
        let values = LivingKnob::ALL.map(|knob| match knob {
            LivingKnob::Amplitude => {
                resolved(prop.wooze.amplitude, clock_layer(0, over.clock.amplitude))
            }
            LivingKnob::Step => resolved(prop.wooze.step, clock_layer(1, over.clock.step)),
            LivingKnob::Glide => resolved(prop.wooze.glide, clock_layer(2, over.clock.glide)),
            LivingKnob::Jitter => resolved(prop.wooze.jitter, clock_layer(3, over.clock.jitter)),
            LivingKnob::Stagger => resolved(
                prop.stagger().name().to_string(),
                LivingLayer::of(
                    over.stagger.is_some(),
                    prop.stagger_layers.global.is_some(),
                    stagger_lowest.clone(),
                ),
            ),
            LivingKnob::EchoCount => resolved(
                echoes.count,
                from(own_echoes.count.is_some(), global.count.is_some()),
            ),
            LivingKnob::EchoSpread => resolved(
                echoes.spread,
                from(own_echoes.spread.is_some(), global.spread.is_some()),
            ),
            LivingKnob::EchoAlpha => resolved(
                echoes.alpha,
                from(own_echoes.alpha.is_some(), global.alpha.is_some()),
            ),
            LivingKnob::VarianceAmount => resolved(
                variance.amount,
                from(
                    own_variance.amount.is_some(),
                    global_variance.amount.is_some(),
                ),
            ),
            LivingKnob::VarianceSteady => resolved(
                variance.steady,
                from(
                    own_variance.steady.is_some(),
                    global_variance.steady.is_some(),
                ),
            ),
            LivingKnob::KeepSize => resolved(
                variance.keep_size,
                from(
                    own_variance.keep_size.is_some(),
                    global_variance.keep_size.is_some(),
                ),
            ),
            LivingKnob::Softness => resolved(
                self.softness(id),
                from(
                    over.softness.is_some(),
                    prop.softness_layers.global.is_some(),
                ),
            ),
            LivingKnob::JitterRate => resolved(
                self.jitter_rate(id),
                from(
                    over.jitter_rate.is_some(),
                    prop.jitter_rate_layers.global.is_some(),
                ),
            ),
            LivingKnob::RowRate => resolved(
                self.row_rate(id),
                from(
                    over.row_rate.is_some(),
                    prop.row_rate_layers.global.is_some(),
                ),
            ),
        });
        ResolvedProp {
            species: species.name.clone(),
            values,
        }
    }

    /// What the first lineage's poses are held to. Every lineage has this once a
    /// knob has set it; until then each has its spec's: see [`Self::variances`].
    pub fn variance(&self) -> Option<VarianceConfig> {
        self.lineages.first().map(|lineage| lineage.variance)
    }

    /// What each lineage's poses are held to, in the order they were first
    /// named.
    pub fn variances(&self) -> Vec<VarianceConfig> {
        self.lineages
            .iter()
            .map(|lineage| lineage.variance)
            .collect()
    }

    /// Whether poses are still being held to a new variance.
    pub fn is_reholding(&self) -> bool {
        self.reholding
    }
}

/// Settings resolution on its own, without a generator: plain values, then
/// props added to a resolver whose lineages are never built (#753).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_grove::Grove;
    use crate::LivingPropRequest;
    use flint_core::EntityId;
    use flint_procgen::ProcGenSpec;

    const SPEC: &str = r#"
generator = "tree_v1"
[meta]
name = "birch"
version = "1.0.0"
[seed]
mode = "fixed"
value = 1
[params]
branch_method = "lsystem"
lsystem_iterations = 1
[living]
lineage_seed = 3
stagger = "per_level"
softness = 0.25
echoes = { count = 2, spread = 0.5, alpha = 0.3 }
variance = { amount = 0.5, steady = 0.5, keep_size = false }
"#;

    fn spec(name: &str) -> ProcGenSpec {
        ProcGenSpec::parse(&SPEC.replace("\"birch\"", &format!("\"{name}\""))).unwrap()
    }

    fn request(n: u64, spec: &str, stagger: Option<Stagger>) -> LivingPropRequest {
        LivingPropRequest {
            entity: EntityId::from_raw(n),
            entity_name: format!("{spec}_{n}"),
            position: [0.0; 3],
            spec_name: spec.to_string(),
            lineage_seed: None,
            pose_ring: None,
            phase_offset: None,
            stagger,
        }
    }

    /// Two birches (one with a stagger of its own) and an alder, added and
    /// never built: no lineage, no pose.
    fn grove(defaults: LivingDefaults) -> Grove {
        let mut resolver = Grove::new();
        resolver.index_spec(spec("birch"));
        resolver.index_spec(spec("alder"));
        resolver.living_props_mut().set_defaults(defaults);
        resolver.add_living_prop(request(1, "birch", None)).unwrap();
        let own = Some(Stagger::PerBranch);
        resolver.add_living_prop(request(2, "birch", own)).unwrap();
        resolver.add_living_prop(request(3, "alder", None)).unwrap();
        resolver
    }

    fn built_nothing(props: &LivingProps) {
        for i in 0..props.props().len() {
            assert!(props.rig_of(LivingPropId(i)).is_none(), "prop {i} built");
            assert_eq!(props.prop(LivingPropId(i)).ring_len(), 0);
        }
    }

    #[test]
    fn a_single_value_layer_is_its_value_or_whats_below() {
        assert_eq!(Some(2.0_f32).over_value(1.0), 2.0);
        assert_eq!(None::<f32>.over_value(1.0), 1.0);
        assert_eq!(Some(2.0_f32).merged_with(Some(3.0)), Some(2.0));
        assert_eq!(None.merged_with(Some(3.0_f32)), Some(3.0));
    }

    #[test]
    fn an_override_speaks_a_field_at_a_time() {
        let spec = EchoConfig {
            count: 6,
            spread: 0.9,
            alpha: 0.2,
        };
        let count = EchoOverride {
            count: Some(2),
            ..Default::default()
        };
        assert_eq!(count.over(spec), EchoConfig { count: 2, ..spec });
        assert_eq!(EchoOverride::full(spec).over(EchoConfig::default()), spec);
        let alpha = EchoOverride {
            alpha: Some(0.5),
            count: Some(9),
            ..Default::default()
        };
        // The newer word wins where both speak; each keeps its other fields.
        let both = count.merged(alpha);
        assert_eq!(
            (both.count, both.alpha, both.spread),
            (Some(2), Some(0.5), None)
        );
    }

    #[test]
    fn a_species_beats_the_global_layer_which_beats_the_base() {
        let mut softness = Layered::new(0.1_f32, None);
        assert_eq!(softness.value(None), 0.1);
        softness.tune(Some(0.4));
        assert_eq!(softness.value(None), 0.4);
        assert_eq!(softness.value(Some(0.9)), 0.9);

        let base = VarianceConfig {
            amount: 0.5,
            steady: 0.5,
            keep_size: false,
        };
        let mut variance = Layered::new(base, VarianceOverride::default());
        variance.tune(VarianceOverride {
            amount: Some(1.0),
            ..Default::default()
        });
        variance.tune(VarianceOverride {
            keep_size: Some(true),
            ..Default::default()
        });
        let own = VarianceOverride {
            amount: Some(0.0),
            ..Default::default()
        };
        // A second tune adds a field and keeps the first.
        assert_eq!(
            variance.value(VarianceOverride::default()),
            VarianceConfig {
                amount: 1.0,
                steady: 0.5,
                keep_size: true,
            }
        );
        assert_eq!(variance.value(own).amount, 0.0);
    }

    #[test]
    fn a_layer_label_is_what_the_report_prints() {
        let labels: Vec<String> = [
            LivingLayer::Spec,
            LivingLayer::Entity,
            LivingLayer::Global,
            LivingLayer::Zone("hush".into()),
            LivingLayer::Species,
        ]
        .iter()
        .map(LivingLayer::label)
        .collect();
        assert_eq!(labels, ["spec", "entity", "global", "zone:hush", "species"]);
    }

    #[test]
    fn a_prop_resolves_its_spec_and_entity_before_anything_is_built() {
        let resolver = grove(LivingDefaults::default());
        let props = resolver.living_props();
        built_nothing(props);
        let first = props.resolved(LivingPropId(0), None);
        assert_eq!(first.species, "birch");
        assert_eq!(first.get(LivingKnob::Stagger).value, Stagger::PerLevel);
        assert_eq!(first.get(LivingKnob::Stagger).layer, LivingLayer::Spec);
        assert_eq!(first.get(LivingKnob::EchoCount).value, 2);
        assert_eq!(first.get(LivingKnob::EchoCount).layer, LivingLayer::Spec);
        assert_eq!(first.get(LivingKnob::Softness).value, 0.25);
        assert_eq!(first.get(LivingKnob::VarianceAmount).value, 0.5);
        assert_eq!(first.get(LivingKnob::Step).layer, LivingLayer::Spec);
        let own = props.resolved(LivingPropId(1), None);
        assert_eq!(own.get(LivingKnob::Stagger).value, Stagger::PerBranch);
        assert_eq!(own.get(LivingKnob::Stagger).layer, LivingLayer::Entity);
    }

    #[test]
    fn the_scene_speaks_globally_and_a_species_over_it() {
        let mut species = BTreeMap::new();
        species.insert(
            "alder".to_string(),
            SpeciesOverride {
                softness: Some(0.75),
                echoes: EchoOverride {
                    alpha: Some(0.9),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let resolver = grove(LivingDefaults {
            echoes: EchoOverride {
                count: Some(5),
                ..Default::default()
            },
            stagger: Some(Stagger::None),
            softness: Some(0.5),
            species,
            ..Default::default()
        });
        let props = resolver.living_props();
        built_nothing(props);
        let birch = props.resolved(LivingPropId(1), None);
        // The scene's stagger beats the entity's own.
        assert_eq!(birch.get(LivingKnob::Stagger).value, Stagger::None);
        assert_eq!(birch.get(LivingKnob::Stagger).layer, LivingLayer::Global);
        assert_eq!(birch.get(LivingKnob::EchoCount).value, 5);
        assert_eq!(birch.get(LivingKnob::EchoAlpha).value, 0.3);
        assert_eq!(birch.get(LivingKnob::EchoAlpha).layer, LivingLayer::Spec);
        assert_eq!(birch.get(LivingKnob::Softness).value, 0.5);
        let alder = props.resolved(LivingPropId(2), None);
        assert_eq!(alder.get(LivingKnob::Softness).value, 0.75);
        assert_eq!(alder.get(LivingKnob::Softness).layer, LivingLayer::Species);
        assert_eq!(alder.get(LivingKnob::EchoAlpha).value, 0.9);
        assert_eq!(alder.get(LivingKnob::EchoCount).layer, LivingLayer::Global);
    }

    #[test]
    fn every_knob_tunes_the_same_way_and_restores() {
        let mut resolver = grove(LivingDefaults::default());
        let props = resolver.living_props_mut();
        let loaded = props.layers();
        let echoes = EchoOverride {
            count: Some(4),
            ..Default::default()
        };
        assert!(props.tune_echoes(LivingScope::All, echoes));
        assert!(props.tune_stagger(LivingScope::Species("alder"), Stagger::PerBranch));
        assert!(
            props.tune_softness(LivingScope::All, 3.0),
            "clamped, not refused"
        );
        assert!(props.tune_variance(
            LivingScope::Species("birch"),
            VarianceOverride {
                steady: Some(0.0),
                ..Default::default()
            }
        ));
        assert!(props.tune_clock(
            "birch",
            ClockOverride {
                step: Some(2.0),
                ..Default::default()
            }
        ));
        assert!(!props.tune_echoes(LivingScope::Species("rowan"), echoes));
        built_nothing(props);

        let birch = props.resolved(LivingPropId(0), None);
        assert_eq!(
            (
                birch.get(LivingKnob::EchoCount).value.clone(),
                birch.get(LivingKnob::EchoCount).layer.clone()
            ),
            (KnobValue::Count(4), LivingLayer::Global)
        );
        assert_eq!(birch.get(LivingKnob::Softness).value, 1.0);
        assert_eq!(birch.get(LivingKnob::VarianceSteady).value, 0.0);
        assert_eq!(
            birch.get(LivingKnob::VarianceSteady).layer,
            LivingLayer::Species
        );
        assert_eq!(birch.get(LivingKnob::Step).value, 2.0);
        assert_eq!(birch.get(LivingKnob::Step).layer, LivingLayer::Species);
        assert_eq!(birch.get(LivingKnob::Stagger).value, Stagger::PerLevel);
        let alder = props.resolved(LivingPropId(2), None);
        assert_eq!(alder.get(LivingKnob::Stagger).value, Stagger::PerBranch);
        assert_eq!(alder.get(LivingKnob::Stagger).layer, LivingLayer::Species);
        assert_eq!(alder.get(LivingKnob::VarianceSteady).value, 0.5);
        assert_eq!(props.global_layer().0, echoes);
        assert_eq!(props.global_softness(), Some(1.0));
        // A species nobody grows from is kept, for none.
        assert_eq!(props.species_override("rowan").unwrap().echoes, echoes);

        props.restore_layers(&loaded);
        let birch = props.resolved(LivingPropId(0), None);
        assert_eq!(
            (
                birch.get(LivingKnob::EchoCount).value.clone(),
                birch.get(LivingKnob::EchoCount).layer.clone()
            ),
            (KnobValue::Count(2), LivingLayer::Spec)
        );
        assert_eq!(birch.get(LivingKnob::Softness).value, 0.25);
        assert_eq!(birch.get(LivingKnob::Step).layer, LivingLayer::Spec);
        let alder = props.resolved(LivingPropId(2), None);
        assert_eq!(alder.get(LivingKnob::Stagger).value, Stagger::PerLevel);
        built_nothing(props);
    }
}
