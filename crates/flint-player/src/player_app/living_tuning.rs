//! Tuning a running grove (#218): every knob of Living Props, live, from the F3
//! panel or a script, and saved as a preset a scene can name.
//!
//! One path for both: anything that changes a knob says so as a
//! [`WoozePreset`](flint_procgen::WoozePreset), whose unset fields leave a knob
//! alone. So what a preset file loads is exactly what a person turned, and setting
//! a knob to the value it has already costs nothing.
//!
//! Freezing stops the clock the field and the props run on, and nothing else: the
//! game goes on, so the camera can move around a held pose. A knob still works
//! while frozen, and glide and drift arrive on each part's next beat, as they do
//! at speed.

use flint_core::wooze::{preset as builtin_preset, WoozeField, WoozeSample, PRESETS};
use flint_procgen::{
    commit_look, read_preset, BoilDef, LivingDef, LivingEchoesDef, LivingSpeciesDef,
    LivingVarianceDef, WoozeKnobs, WoozePreset,
};
use flint_procgen::{
    EchoConfig, EchoOverride, LivingLayer, LivingLayers, LivingPropId, LivingScope, Resolved,
    ResolvedProp, SpeciesOverride, Stagger, VarianceConfig, VarianceOverride,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::living_props::{echoes_of, species_of, stagger_name, stagger_of, variance_of};
use super::PlayerApp;

/// What's been done to a running grove by hand.
#[derive(Debug, Default)]
pub struct LivingTune {
    /// Whether the clock is stopped.
    pub frozen: bool,
    /// Beats to move on by, the next time the clock is read.
    steps_queued: u32,
    /// Whether the pose overlay is showing.
    pub overlay: bool,
    /// Whether the left button's press picked a tree (#248), so its release
    /// stops there too.
    #[cfg(feature = "debug-hud")]
    pub(super) click_held: bool,
    /// While the boil is switched off: the amounts it'll come back to.
    pub(super) boil_off: Option<(f32, f32)>,
    /// A boil asked for before there was a renderer to give it to.
    boil_waiting: Option<BoilDef>,
    /// The knobs as the scene loaded, to go back to.
    pub(super) at_load: Option<AtLoad>,
    /// The look as last committed to the scene file (#249), which is what the
    /// file gives from then on. `None`: as the scene loaded.
    committed: Option<WoozePreset>,
    /// What came of the last save or load, for the panel.
    pub status: Option<String>,
    /// Two presets on the same grove, if they're being compared (#238).
    pub(super) ab: Option<Box<super::living_ab::LivingAb>>,
    /// What A and B would be: the panel's picks, for F6 and F7 to start with.
    /// `None` until picked, when they're the knobs as they stand and the first
    /// preset on offer.
    pub(super) ab_picks: Option<(String, String)>,
    /// `--compare`: start comparing the picks once there are props, split if true.
    pub(super) compare_at_start: Option<bool>,
    /// The presets on offer, as of when the panel opened or last saved one. Not
    /// read every frame, since that means reading a folder.
    #[cfg(feature = "debug-hud")]
    preset_names: Option<Vec<String>>,
}

/// Every knob as the scene loaded, layer by layer: a grove of several species
/// goes back to several, and each species to its own values (#246).
#[derive(Debug, Clone)]
pub(super) struct AtLoad {
    /// The field's knobs and the boil, one of each per scene. In a scene with no
    /// field the clock's are unset, and `layers` has each prop's instead.
    knobs: WoozePreset,
    /// Whether the scene had no field.
    no_field: bool,
    /// Every prop's and lineage's global layer, and every species' own values.
    layers: LivingLayers,
    /// The same as a look in a scene's terms (#249): what the file gave.
    look: WoozePreset,
}

/// Which levels have values the scene file doesn't give (#249): what a commit
/// would write, and so what closing the window would lose.
#[cfg(any(feature = "debug-hud", test))]
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Unsaved {
    /// The whole grove's: `[wooze]`, `[living]` or the boil.
    pub global: bool,
    /// Each species with a value of its own to write or take out, by name.
    pub species: Vec<String>,
    /// Every value a commit would write, by its place in the file.
    pub edits: Vec<flint_scene::SceneEdit>,
}

#[cfg(any(feature = "debug-hud", test))]
impl Unsaved {
    fn of(edits: Vec<flint_scene::SceneEdit>) -> Self {
        let mut species: Vec<String> = Vec::new();
        for name in edits.iter().filter_map(flint_procgen::edit_species) {
            if !species.iter().any(|s| s == name) {
                species.push(name.to_string());
            }
        }
        Self {
            global: edits
                .iter()
                .any(|edit| flint_procgen::edit_species(edit).is_none()),
            species,
            edits,
        }
    }
}

/// A species' own values as its scene table says them.
pub(crate) fn species_table(over: &SpeciesOverride) -> LivingSpeciesDef {
    LivingSpeciesDef {
        amplitude: over.clock.amplitude,
        step: over.clock.step,
        glide: over.clock.glide,
        jitter: over.clock.jitter,
        stagger: over.stagger.map(|s| stagger_name(s).to_string()),
        echoes: echoes_table(over.echoes),
        variance: variance_table(over.variance),
        softness: over.softness,
    }
}

fn echoes_table(echoes: EchoOverride) -> Option<LivingEchoesDef> {
    (echoes != EchoOverride::default()).then_some(LivingEchoesDef {
        count: echoes.count,
        spread: echoes.spread,
        alpha: echoes.alpha,
    })
}

fn variance_table(variance: VarianceOverride) -> Option<LivingVarianceDef> {
    (variance != VarianceOverride::default()).then_some(LivingVarianceDef {
        amount: variance.amount,
        steady: variance.steady,
        keep_size: variance.keep_size,
    })
}

/// The keys [`PlayerApp::clear_living_species`] takes, each a knob or a group.
pub const SPECIES_KEYS: [&str; 14] = [
    "amplitude",
    "step",
    "glide",
    "jitter",
    "stagger",
    "echoes",
    "echoes.count",
    "echoes.spread",
    "echoes.alpha",
    "variance",
    "variance.amount",
    "variance.steady",
    "variance.keep_size",
    "softness",
];

/// One knob over some props (#247): the first one's value, whether they
/// disagree, and the layers that set it, each once, as `flint render --report
/// json` names them: `spec`, `entity`, `global`, `zone:<name>` or `species`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct KnobRead<T> {
    pub value: T,
    pub mixed: bool,
    pub layers: Vec<String>,
}

/// Every knob a species can set, over the props a scope covers (#247).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LivingView {
    /// How many props that is.
    pub props: usize,
    pub amplitude: KnobRead<f32>,
    pub step: KnobRead<f32>,
    pub glide: KnobRead<f32>,
    pub jitter: KnobRead<f32>,
    pub stagger: KnobRead<Stagger>,
    pub echo_count: KnobRead<u32>,
    pub echo_spread: KnobRead<f32>,
    pub echo_alpha: KnobRead<f32>,
    pub variance_amount: KnobRead<f32>,
    pub variance_steady: KnobRead<f32>,
    pub keep_size: KnobRead<bool>,
    pub softness: KnobRead<f32>,
}

/// One knob over `props`. For the whole grove, a prop whose species sets its
/// own is left out, since the grove's knob doesn't reach it, unless every one
/// does: then the knob reads what they run on.
fn read_knob<T: Clone + PartialEq>(
    props: &[ResolvedProp],
    grove: bool,
    fallback: T,
    pick: impl Fn(&ResolvedProp) -> &Resolved<T>,
) -> KnobRead<T> {
    let reached: Vec<&ResolvedProp> = props
        .iter()
        .filter(|prop| !grove || pick(prop).layer != LivingLayer::Species)
        .collect();
    let read: Vec<&ResolvedProp> = if reached.is_empty() {
        props.iter().collect()
    } else {
        reached
    };
    let Some(first) = read.first() else {
        return KnobRead {
            value: fallback,
            mixed: false,
            layers: Vec::new(),
        };
    };
    let value = pick(first).value.clone();
    let mut layers: Vec<String> = Vec::new();
    for prop in &read {
        let label = pick(prop).layer.label();
        if !layers.contains(&label) {
            layers.push(label);
        }
    }
    KnobRead {
        mixed: read.iter().any(|prop| pick(prop).value != value),
        value,
        layers,
    }
}

impl LivingTune {
    /// Nothing done by hand, for a new scene, keeping only what was asked for
    /// before any scene was: which presets to compare, and whether to at start.
    pub(super) fn for_new_scene(&mut self) {
        *self = Self {
            ab_picks: self.ab_picks.take(),
            compare_at_start: self.compare_at_start.take(),
            ..Self::default()
        };
    }
}

impl LivingView {
    /// Every knob over `props`, for the whole grove or for one species.
    fn of(props: &[ResolvedProp], grove: bool) -> Self {
        let echoes = EchoConfig::default();
        let variance = VarianceConfig::default();
        let clock = WoozeSample::NEUTRAL;
        Self {
            props: props.len(),
            amplitude: read_knob(props, grove, clock.amplitude, |p| &p.amplitude),
            step: read_knob(props, grove, clock.step, |p| &p.step),
            glide: read_knob(props, grove, clock.glide, |p| &p.glide),
            jitter: read_knob(props, grove, clock.jitter, |p| &p.jitter),
            stagger: read_knob(props, grove, Stagger::default(), |p| &p.stagger),
            echo_count: read_knob(props, grove, echoes.count, |p| &p.echo_count),
            echo_spread: read_knob(props, grove, echoes.spread, |p| &p.echo_spread),
            echo_alpha: read_knob(props, grove, echoes.alpha, |p| &p.echo_alpha),
            variance_amount: read_knob(props, grove, variance.amount, |p| &p.variance_amount),
            variance_steady: read_knob(props, grove, variance.steady, |p| &p.variance_steady),
            keep_size: read_knob(props, grove, variance.keep_size, |p| &p.keep_size),
            softness: read_knob(props, grove, 0.0, |p| &p.softness),
        }
    }

    /// The knobs as a preset with every one set, and the boil as given.
    pub(crate) fn knobs(&self, boil: BoilDef) -> WoozePreset {
        WoozePreset {
            wooze: WoozeKnobs {
                amplitude: Some(self.amplitude.value),
                step: Some(self.step.value),
                glide: Some(self.glide.value),
                jitter: Some(self.jitter.value),
            },
            living: LivingDef {
                shadows: None,
                stagger: Some(stagger_name(self.stagger.value).to_string()),
                echoes: Some(LivingEchoesDef {
                    count: Some(self.echo_count.value),
                    spread: Some(self.echo_spread.value),
                    alpha: Some(self.echo_alpha.value),
                }),
                variance: Some(LivingVarianceDef {
                    amount: Some(self.variance_amount.value),
                    steady: Some(self.variance_steady.value),
                    keep_size: Some(self.keep_size.value),
                }),
                softness: Some(self.softness.value),
                ..Default::default()
            },
            boil: Some(boil),
        }
    }
}

/// What a species table says for the knobs a panel edit set (#247): the shape
/// `[living.species.<spec>]` and a script's `tune_living` take. The boil has no
/// place in it, being one pass over the whole frame.
#[cfg(feature = "debug-hud")]
pub(crate) fn species_def_of(moved: &WoozePreset) -> LivingSpeciesDef {
    LivingSpeciesDef {
        amplitude: moved.wooze.amplitude,
        step: moved.wooze.step,
        glide: moved.wooze.glide,
        jitter: moved.wooze.jitter,
        stagger: moved.living.stagger.clone(),
        echoes: moved.living.echoes,
        variance: moved.living.variance,
        softness: moved.living.softness,
    }
}

impl PlayerApp {
    /// Every prop's values, each with the layer that set it, as `flint render
    /// --report json` reads them.
    fn living_resolved(&self) -> Vec<ResolvedProp> {
        let living = self.procgen_resolver.living_props();
        let field = self.wooze.as_ref().map(|field| (field, self.wooze_time));
        (0..living.props().len())
            .map(|i| living.resolved(LivingPropId(i), field))
            .collect()
    }

    /// Every knob over the whole grove, or over one species' props (#247). For
    /// the grove, a species with its own value for a knob is left out of it, and
    /// the clock's four are the field's, in a scene that has one.
    pub(crate) fn living_view(&self, species: Option<&str>) -> LivingView {
        let mut props = self.living_resolved();
        if let Some(species) = species {
            props.retain(|prop| prop.species == species);
        }
        let mut view = LivingView::of(&props, species.is_none());
        if let (None, Some(field)) = (species, &self.wooze) {
            let global = field.global(self.wooze_time);
            for (knob, value) in [
                (&mut view.amplitude, global.amplitude),
                (&mut view.step, global.step),
                (&mut view.glide, global.glide),
                (&mut view.jitter, global.jitter),
            ] {
                knob.value = value;
                knob.mixed = false;
            }
        }
        view
    }

    /// The species any prop grows from, by name.
    pub(crate) fn living_species(&self) -> Vec<String> {
        let living = self.procgen_resolver.living_props();
        let mut names: Vec<String> = (living.species_names().into_iter())
            .filter(|species| living.has_species(species))
            .map(str::to_string)
            .collect();
        names.sort_unstable();
        names
    }

    /// Every knob as it stands now for the whole grove, all of them set. Where
    /// props disagree it's the first prop's value; a species that sets its own
    /// is left out, as [`Self::living_view`] says.
    pub(super) fn living_knobs(&self) -> WoozePreset {
        self.living_view(None).knobs(self.living_boil())
    }

    /// The knobs every prop agrees on, as a preset, and the names of the ones
    /// left out: what a file can say for the whole grove without flattening its
    /// species. Loading it leaves each species its own of the rest. A species'
    /// own value doesn't count as disagreeing: it's a layer of its own.
    pub(super) fn living_shared_knobs(&self) -> (WoozePreset, Vec<&'static str>) {
        let view = self.living_view(None);
        let mut knobs = view.knobs(self.living_boil());
        let mut left_out = Vec::new();
        let mut out = |is: bool, name: &'static str| {
            if is {
                left_out.push(name);
            }
            is
        };
        let wooze = &mut knobs.wooze;
        for (is, name, value) in [
            (view.step.mixed, "step", &mut wooze.step),
            (view.glide.mixed, "glide", &mut wooze.glide),
            (view.amplitude.mixed, "amplitude", &mut wooze.amplitude),
            (view.jitter.mixed, "jitter", &mut wooze.jitter),
        ] {
            if out(is, name) {
                *value = None;
            }
        }
        if out(view.stagger.mixed, "stagger") {
            knobs.living.stagger = None;
        }
        if let Some(echoes) = &mut knobs.living.echoes {
            if out(view.echo_count.mixed, "echoes.count") {
                echoes.count = None;
            }
            for (is, name, value) in [
                (view.echo_spread.mixed, "echoes.spread", &mut echoes.spread),
                (view.echo_alpha.mixed, "echoes.alpha", &mut echoes.alpha),
            ] {
                if out(is, name) {
                    *value = None;
                }
            }
        }
        if let Some(variance) = &mut knobs.living.variance {
            for (is, name, value) in [
                (
                    view.variance_amount.mixed,
                    "variance.amount",
                    &mut variance.amount,
                ),
                (
                    view.variance_steady.mixed,
                    "variance.steady",
                    &mut variance.steady,
                ),
            ] {
                if out(is, name) {
                    *value = None;
                }
            }
            if out(view.keep_size.mixed, "variance.keep_size") {
                variance.keep_size = None;
            }
        }
        if out(view.softness.mixed, "softness") {
            knobs.living.softness = None;
        }
        (knobs, left_out)
    }

    /// The boil's knobs. The amounts it has while switched off are the ones it
    /// remembers, not the zeroes the passes are getting.
    fn living_boil(&self) -> BoilDef {
        let config = self
            .scene_renderer
            .as_ref()
            .map(|renderer| renderer.post_process_config());
        let (paint, ink, on) = config.map_or((0.0, 0.0, 2), |config| {
            let (paint, ink) = config.boil_amounts();
            (paint, ink, config.boil_on)
        });
        let (paint, ink) = self.living_tune.boil_off.unwrap_or((paint, ink));
        BoilDef {
            enabled: Some(self.living_tune.boil_off.is_none()),
            kuwahara: Some(paint),
            contour: Some(ink),
            on: Some(on),
        }
    }

    /// Turn the knobs a preset sets. What can't be used is named and the rest
    /// still happen, since this is a person or a script at work.
    pub(super) fn tune_living(&mut self, tune: &WoozePreset) -> Result<(), String> {
        self.tune_living_as(tune, false)
    }

    /// [`Self::tune_living`], with each species the preset names given exactly
    /// its values if `replace`, as loading a preset does (#249), rather than
    /// those laid over what it has, as a script's tune does.
    fn tune_living_as(&mut self, tune: &WoozePreset, replace: bool) -> Result<(), String> {
        let mut problems = self.tune_wooze(&tune.wooze);
        problems.extend(self.tune_props(&tune.living));
        for (name, species) in &tune.living.species {
            if replace {
                problems.extend(self.replace_species(name, species));
            } else {
                problems.extend(self.tune_species(name, species));
            }
        }
        if let Some(boil) = tune.boil {
            problems.extend(boil_problem(&boil));
            self.tune_boil(boil);
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    /// A script's `tune_living` table: a `.wooze.toml`'s shape, or with a
    /// `species` name, one species' own values in the shape of a scene's
    /// `[living.species.<spec>]` (#246).
    pub(super) fn tune_living_from_script(&mut self, knobs: toml::Value) -> Result<(), String> {
        let problems = match ScriptTune::of(knobs)? {
            ScriptTune::Grove(tune) => return self.tune_living(&tune),
            ScriptTune::Species(name, def) => self.tune_species(&name, &def),
        };
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    fn tune_wooze(&mut self, knobs: &WoozeKnobs) -> Vec<String> {
        let start = self
            .procgen_resolver
            .living_props()
            .props()
            .first()
            .map_or(WoozeSample::NEUTRAL, |prop| prop.wooze_below());
        tune_field(&mut self.wooze, start, self.wooze_time, knobs)
    }

    /// Stagger, echoes and variance. A field that's set goes to every prop, and
    /// one that isn't stays as each prop has it: moving echo `count` in a grove
    /// of several species leaves each its own `spread` and `alpha`.
    fn tune_props(&mut self, living: &LivingDef) -> Vec<String> {
        tune_props(self.procgen_resolver.living_props_mut(), living)
    }

    /// One species' own values (#246), over whatever it has already: they beat
    /// the global ones, the field and its zones. A species no prop grows from
    /// keeps them, and is named, since it's most likely a misspelling.
    pub(super) fn tune_species(&mut self, name: &str, def: &LivingSpeciesDef) -> Vec<String> {
        let (mut problems, grown) =
            tune_species(self.procgen_resolver.living_props_mut(), name, def);
        if !grown {
            problems.push(self.no_such_species(name));
        }
        problems
    }

    /// One species' own values made exactly `def` (#249): what it had and `def`
    /// doesn't say goes back to the layers below.
    fn replace_species(&mut self, name: &str, def: &LivingSpeciesDef) -> Vec<String> {
        let (mut problems, grown) =
            replace_species(self.procgen_resolver.living_props_mut(), name, def);
        if !grown {
            problems.push(self.no_such_species(name));
        }
        problems
    }

    /// Clear some of a species' own values, or all of them with no keys, so the
    /// layers below decide them again. Keys are [`SPECIES_KEYS`].
    pub(super) fn clear_living_species(
        &mut self,
        name: &str,
        keys: &[String],
    ) -> Result<(), String> {
        let props = self.procgen_resolver.living_props_mut();
        let mut over = props.species_override(name).unwrap_or_default();
        if keys.is_empty() {
            over = Default::default();
        }
        let mut unknown = Vec::new();
        for key in keys {
            match key.as_str() {
                "amplitude" => over.clock.amplitude = None,
                "step" => over.clock.step = None,
                "glide" => over.clock.glide = None,
                "jitter" => over.clock.jitter = None,
                "stagger" => over.stagger = None,
                "echoes" => over.echoes = Default::default(),
                "echoes.count" => over.echoes.count = None,
                "echoes.spread" => over.echoes.spread = None,
                "echoes.alpha" => over.echoes.alpha = None,
                "variance" => over.variance = Default::default(),
                "variance.amount" => over.variance.amount = None,
                "variance.steady" => over.variance.steady = None,
                "variance.keep_size" => over.variance.keep_size = None,
                "softness" => over.softness = None,
                other => unknown.push(other.to_string()),
            }
        }
        let any = props.set_species(name, over);
        if !unknown.is_empty() {
            return Err(format!(
                "{} isn't a species value; it takes {}",
                unknown.join(", "),
                SPECIES_KEYS.join(", ")
            ));
        }
        if any {
            Ok(())
        } else {
            Err(self.no_such_species(name))
        }
    }

    /// What to say about a species no prop grows from.
    fn no_such_species(&self, name: &str) -> String {
        let grown = self.living_species();
        format!(
            "no living prop grows from '{name}', so it changes nothing; the species here are {}",
            grown.join(", ")
        )
    }

    fn tune_boil(&mut self, boil: BoilDef) {
        let Some(renderer) = &mut self.scene_renderer else {
            self.living_tune.boil_waiting = Some(boil);
            return;
        };
        let mut config = renderer.post_process_config().clone();
        tune_boil(&mut config, &mut self.living_tune.boil_off, boil);
        renderer.set_post_process_config(config);
    }

    /// Save every knob as a preset. Returns the file and what a scene needs to
    /// say to load it.
    ///
    /// A knob the props disagree on is left out of the grove's tables. Each
    /// species with values of its own gets a `[living.species.<spec>]` (#249),
    /// and one the scene gave values that now has none an empty one, so that
    /// loading the file gives every species what it has now.
    pub(super) fn save_living_preset(&mut self, name: &str) -> Result<PathBuf, String> {
        let (mut knobs, _) = self.living_shared_knobs();
        knobs.living.species = self.living_species_tables();
        flint_procgen::write_preset_in(&self.project, Path::new(&self.scene_path), name, &knobs)
    }

    /// Each species' own values as a preset holds them: every one that has some,
    /// and an empty table for each the file gave values to that now has none.
    fn living_species_tables(&self) -> BTreeMap<String, LivingSpeciesDef> {
        let mut tables = self.living_look().living.species;
        if let Some(base) = self.living_base() {
            for name in base.living.species.into_keys() {
                tables.entry(name).or_default();
            }
        }
        tables
    }

    /// The look as it stands now, in a scene's terms (#249): the field's global
    /// knobs, if there's a field, the global layer of `[living]` as it was set,
    /// a field at a time, each species' own values, and the boil.
    pub(super) fn living_look(&self) -> WoozePreset {
        let props = self.procgen_resolver.living_props();
        let wooze = match &self.wooze {
            Some(field) => {
                let global = field.global(self.wooze_time);
                WoozeKnobs {
                    amplitude: Some(global.amplitude),
                    step: Some(global.step),
                    glide: Some(global.glide),
                    jitter: Some(global.jitter),
                }
            }
            None => WoozeKnobs::default(),
        };
        let (echoes, stagger, variance) = props.global_layer();
        let species = (self.living_species().into_iter())
            .filter_map(|name| {
                let over = props.species_override(&name)?;
                (!over.is_empty()).then(|| (name, species_table(&over)))
            })
            .collect();
        WoozePreset {
            wooze,
            living: LivingDef {
                shadows: None,
                stagger: stagger.map(|s| stagger_name(s).to_string()),
                echoes: echoes_table(echoes),
                variance: variance_table(variance),
                softness: props.global_softness(),
                species,
            },
            boil: Some(self.living_boil()),
        }
    }

    /// The look the scene file gives: as last committed, or as it loaded.
    fn living_base(&self) -> Option<WoozePreset> {
        let tune = &self.living_tune;
        (tune.committed.clone()).or_else(|| tune.at_load.as_ref().map(|at| at.look.clone()))
    }

    /// What a commit would write now, level by level.
    #[cfg(any(feature = "debug-hud", test))]
    pub(crate) fn living_unsaved(&self) -> Unsaved {
        match self.living_base() {
            Some(base) => Unsaved::of(flint_procgen::look_edits(&base, &self.living_look())),
            None => Unsaved::default(),
        }
    }

    /// Write every value the scene file doesn't give into it (#249): species
    /// values to `[living.species.<spec>]`, the grove's to `[wooze]` and
    /// `[living]`, the boil to `[post_process]`, each on its own line and the rest
    /// of the file as it was. Returns what was said about it. On the web it's an
    /// error, as saving a preset is.
    pub(super) fn commit_living(&mut self) -> Result<String, String> {
        let base = self
            .living_base()
            .ok_or_else(|| "there are no living props to commit".to_string())?;
        let now = self.living_look();
        let path = PathBuf::from(&self.scene_path);
        let report = commit_look(&path, &base, &now)?;
        self.living_tune.committed = Some(now);
        let file = path.file_name().map_or_else(
            || self.scene_path.clone(),
            |name| name.to_string_lossy().into_owned(),
        );
        let mut said = match report.edits.len() {
            0 => format!("{file} already says all of it."),
            1 => format!("Committed 1 value to {file}: {}", report.edits[0]),
            n => format!(
                "Committed {n} values to {file}: {}",
                (report.edits.iter())
                    .map(|edit| edit.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        };
        if report.boil_skipped {
            said.push_str(". The boil isn't written: the scene has no [post_process]");
        }
        if !report.preset_still_sets.is_empty() {
            said.push_str(&format!(
                ". The preset it names still sets {}, so take them out there too",
                report.preset_still_sets.join(", ")
            ));
        }
        Ok(said)
    }

    /// Load a preset file, or a built-in, which holds only the field's knobs. A
    /// file comes first, as it does when a scene names one.
    pub(super) fn load_living_preset(&mut self, name: &str) -> Result<(), String> {
        let found = find_preset(Path::new(&self.scene_path), name)?;
        self.tune_living_as(&found, true)
    }

    /// Every knob as it stands, layer by layer, to go back to.
    pub(super) fn living_at_load(&self) -> AtLoad {
        let mut knobs = self.living_knobs();
        knobs.living = LivingDef::default();
        let no_field = self.wooze.is_none();
        if no_field {
            knobs.wooze = WoozeKnobs::default();
        }
        AtLoad {
            knobs,
            no_field,
            layers: self.procgen_resolver.living_props().layers(),
            look: self.living_look(),
        }
    }

    /// Put the knobs back as the scene loaded them: each prop's echoes and
    /// stagger, each lineage's variance, each species' own values, and in a scene
    /// that had no field, each prop's own clock.
    pub(super) fn reset_living_knobs(&mut self) {
        let Some(at_load) = self.living_tune.at_load.clone() else {
            return;
        };
        if let Err(problem) = self.tune_living(&at_load.knobs) {
            tracing::warn!("Living Props: {problem}");
        }
        if at_load.no_field {
            // A clock knob may since have given the scene a field it didn't have.
            self.wooze = None;
        }
        self.procgen_resolver
            .living_props_mut()
            .restore_layers(&at_load.layers);
    }

    /// Queue exactly one beat of the clock.
    pub(super) fn step_living(&mut self) {
        self.living_tune.steps_queued += 1;
    }

    /// Before the clock is read: hand over a boil that had to wait, note where the
    /// knobs started, and apply any single steps. Each is a whole beat for every
    /// prop, however long a beat is where it stands, and for the boil at the
    /// camera. The field's clock moves on by a beat of its global step.
    ///
    /// While two presets are compared (#238) a step is that many seconds of the
    /// ordinary clock instead, returned to be added to the frame's `dt`, so both
    /// sides move on by the same time: a whole beat of the side being shown,
    /// wherever its step is the global one.
    pub(super) fn before_living_tick(&mut self) -> f64 {
        if let Some(boil) = self.living_tune.boil_waiting {
            if self.scene_renderer.is_some() {
                self.living_tune.boil_waiting = None;
                self.tune_boil(boil);
            }
        }
        if self.living_tune.at_load.is_none() && !self.procgen_resolver.living_props().is_empty() {
            self.living_tune.at_load = Some(self.living_at_load());
        }
        self.start_asked_comparison();
        let steps = std::mem::take(&mut self.living_tune.steps_queued);
        if steps == 0 {
            return 0.0;
        }
        let beats = f64::from(steps);
        let seconds = match &self.wooze {
            Some(field) => f64::from(field.global(self.wooze_time).step),
            None => self
                .procgen_resolver
                .living_props()
                .props()
                .first()
                .map_or(0.0, |prop| f64::from(prop.clock().step)),
        } * beats;
        if self.living_tune.ab.is_some() {
            return seconds;
        }
        self.procgen_resolver
            .living_props_mut()
            .advance_beats(beats, seconds);
        if let Some(field) = &self.wooze {
            let sample = field.sample(self.camera.position_array(), self.wooze_time);
            self.boil_clock
                .tick(f64::from(sample.step) * beats, &sample);
            self.wooze_time += seconds;
            // Shallow wooze runs its own clocks in seconds, so inside a zone with
            // its own step this isn't a whole beat of theirs.
            self.shallow_wooze
                .advance(&self.world, field, self.wooze_time, seconds);
        }
        0.0
    }

    /// The names a preset can go by here: this scene's files, then the built-ins
    /// that no file has taken.
    pub(super) fn living_preset_names(&self) -> Vec<String> {
        let mut names = flint_procgen::list_presets_in(&self.project, Path::new(&self.scene_path));
        for (name, _) in PRESETS {
            if !names.iter().any(|taken| taken == name) {
                names.push((*name).to_string());
            }
        }
        names
    }
}

/// What a script's `tune_living` table asks for: the whole grove's knobs in a
/// `.wooze.toml`'s shape, or with a `species` name, one species' own values in
/// the shape of a scene's `[living.species.<spec>]` (#246). The player and
/// `flint render --scripts` (#8) read it the same way.
#[derive(Debug, Clone)]
pub(crate) enum ScriptTune {
    Grove(WoozePreset),
    Species(String, LivingSpeciesDef),
}

impl ScriptTune {
    pub(crate) fn of(knobs: toml::Value) -> Result<Self, String> {
        let mut table = match knobs {
            toml::Value::Table(table) => table,
            other => return Err(format!("{other} must be a map")),
        };
        let Some(species) = table.remove("species") else {
            let tune = toml::Value::Table(table)
                .try_into::<WoozePreset>()
                .map_err(|e| e.to_string())?;
            return Ok(Self::Grove(tune));
        };
        let name = species
            .as_str()
            .ok_or_else(|| format!("species = {species} must be a spec's name"))?;
        let def = toml::Value::Table(table)
            .try_into::<LivingSpeciesDef>()
            .map_err(|e| format!("species '{name}': {e}"))?;
        Ok(Self::Species(name.to_string(), def))
    }
}

/// Turn the field's global knobs to `knobs` at `time`. A scene with no `[wooze]`
/// table has its specs' clocks: turning one of these is what gives it a field,
/// starting from `start` (where the first prop stands), under any species' own
/// clock. Returns what couldn't be used.
pub(crate) fn tune_field(
    field: &mut Option<WoozeField>,
    start: WoozeSample,
    time: f64,
    knobs: &WoozeKnobs,
) -> Vec<String> {
    let wanted = [
        ("amplitude", knobs.amplitude),
        ("step", knobs.step),
        ("glide", knobs.glide),
        ("jitter", knobs.jitter),
    ];
    if wanted.iter().all(|(_, value)| value.is_none()) {
        return Vec::new();
    }
    tune_global(
        field.get_or_insert_with(|| WoozeField::new(start)),
        time,
        knobs,
    )
}

/// [`tune_field`] on a field there is.
pub(crate) fn tune_global(field: &mut WoozeField, time: f64, knobs: &WoozeKnobs) -> Vec<String> {
    let wanted = [
        ("amplitude", knobs.amplitude),
        ("step", knobs.step),
        ("glide", knobs.glide),
        ("jitter", knobs.jitter),
    ];
    let now = field.global(time);
    let current = [now.amplitude, now.step, now.glide, now.jitter];
    // Only what's moved, so that the rest of a full set of knobs doesn't cut
    // short a ramp a script has going, and one at a time, so that one bad knob
    // leaves the others standing.
    let mut problems = Vec::new();
    for ((name, value), current) in wanted.into_iter().zip(current) {
        if let Some(value) = value.filter(|value| *value != current) {
            let mut table = toml::Table::new();
            table.insert(name.to_string(), toml::Value::Float(f64::from(value)));
            if let Err(problem) = field.set_global(&table, time, 0.0) {
                problems.push(problem);
            }
        }
    }
    problems
}

/// Stagger, echoes, variance and softness. A field that's set goes to every prop, and one
/// that isn't stays as each prop has it: moving echo `count` in a grove of
/// several species leaves each its own `spread` and `alpha`.
pub(crate) fn tune_props(
    props: &mut flint_procgen::LivingProps,
    living: &LivingDef,
) -> Vec<String> {
    let mut problems = Vec::new();
    let mut say = |problem: String| problems.push(problem);
    // The global layer, a field at a time (#246): a species' own value still
    // beats whatever this sets.
    let stagger = living.stagger.as_deref().and_then(|name| {
        let stagger = stagger_of(name);
        if stagger.is_none() {
            say(format!(
                "stagger = \"{name}\" isn't \"none\", \"per_level\" or \"per_branch\""
            ));
        }
        stagger
    });
    let echoes = echoes_of(living.echoes.as_ref(), &mut say);
    let variance = variance_of(living.variance.as_ref(), &mut say);
    if let Some(stagger) = stagger {
        props.tune_stagger(LivingScope::All, stagger);
    }
    props.tune_echoes(LivingScope::All, echoes);
    props.tune_variance(LivingScope::All, variance);
    if let Some(softness) = super::living_props::softness_of(living.softness, &mut say) {
        props.tune_softness(LivingScope::All, softness);
    }
    problems
}

/// Turn the shading boil's knobs on `config`. `off` is what the amounts come back
/// to while the boil is switched off, which the passes see as zeroes. The player
/// and `flint render --scripts` (#579) tune it the same way.
pub(crate) fn tune_boil(
    config: &mut flint_render::PostProcessConfig,
    off: &mut Option<(f32, f32)>,
    boil: BoilDef,
) {
    // Where the amounts live now: the passes, or what off remembers.
    let (mut paint, mut ink) = off.unwrap_or(config.boil_amounts());
    paint = boil.kuwahara.unwrap_or(paint);
    ink = boil.contour.unwrap_or(ink);
    let on = boil.enabled.unwrap_or(off.is_none());
    *off = (!on).then_some((paint, ink));
    if on {
        config.set_boil_amounts(paint, ink);
    } else {
        config.set_boil_amounts(0.0, 0.0);
    }
    if let Some(cadence) = boil.on {
        config.boil_on = cadence.clamp(1, 3);
    }
}

/// What's wrong with a tune's boil, if anything.
pub(crate) fn boil_problem(boil: &BoilDef) -> Option<String> {
    boil.on
        .filter(|on| !(1..=3).contains(on))
        .map(|on| format!("boil.on = {on} must be 1, 2 or 3"))
}

/// A preset that names `name`: this scene's file of that name, else the built-in.
pub(crate) fn find_preset(scene_path: &Path, name: &str) -> Result<WoozePreset, String> {
    read_preset(scene_path, name)
        .or_else(|| {
            builtin_preset(name).map(|sample| {
                Ok(WoozePreset {
                    wooze: WoozeKnobs {
                        amplitude: Some(sample.amplitude),
                        step: Some(sample.step),
                        glide: Some(sample.glide),
                        jitter: Some(sample.jitter),
                    },
                    ..Default::default()
                })
            })
        })
        .ok_or_else(|| format!("there's no preset named '{name}'"))?
}

/// One species' own values made exactly `def` (#249): what it had and `def`
/// doesn't say goes back to the layers below. Returns what couldn't be used, and
/// whether any prop grows from the species.
pub(crate) fn replace_species(
    props: &mut flint_procgen::LivingProps,
    name: &str,
    def: &LivingSpeciesDef,
) -> (Vec<String>, bool) {
    let mut problems = Vec::new();
    let over = species_of(def, &mut |problem| {
        problems.push(format!("species '{name}': {problem}"));
    });
    let grown = props.set_species(name, over);
    (problems, grown)
}

/// One species' own values (#246), over whatever it has already. Returns what
/// couldn't be used, and whether any prop grows from the species.
pub(crate) fn tune_species(
    props: &mut flint_procgen::LivingProps,
    name: &str,
    def: &LivingSpeciesDef,
) -> (Vec<String>, bool) {
    let mut problems = Vec::new();
    let over = species_of(def, &mut |problem| {
        problems.push(format!("species '{name}': {problem}"));
    });
    let now = props.species_override(name).unwrap_or_default();
    let grown = props.set_species(name, over.merged(now));
    (problems, grown)
}

/// The knobs a scene sets itself, which a preset it names can't change: its
/// `[wooze]` knobs, its `[living]` and each `[living.species.<spec>]` value
/// (#249), named as a commit names them.
#[cfg(any(feature = "debug-hud", test))]
pub(crate) fn scene_says(scene: &flint_scene::SceneFile) -> Vec<String> {
    let tables = flint_procgen::LivingTables::or_warn(scene);
    let wooze = tables.wooze.as_ref();
    let own = WoozePreset {
        wooze: WoozeKnobs {
            amplitude: wooze.and_then(|w| w.amplitude),
            step: wooze.and_then(|w| w.step),
            glide: wooze.and_then(|w| w.glide),
            jitter: wooze.and_then(|w| w.jitter),
        },
        living: LivingDef {
            shadows: None,
            ..tables.living.clone().unwrap_or_default()
        },
        boil: None,
    };
    flint_procgen::look_edits(&WoozePreset::default(), &own)
        .iter()
        .map(flint_scene::SceneEdit::label)
        .collect()
}

/// The F3 panel's half: its actions come in here and what's true goes back out.
#[cfg(feature = "debug-hud")]
pub(super) mod panel {
    use super::{species_def_of, KnobRead, LivingView, PlayerApp};
    use flint_debug_ui::{
        DebugPanel, Knob, KnobFlags, KnobSources, LivingKnobs, LivingPanelAction,
        LivingPickedState, LivingPropRow, LivingPropsDebugPanel, LivingSpeciesState, PanelScope,
    };
    use flint_procgen::{LivingPropId, LivingShadows};

    /// Which knobs a view's props disagree on.
    pub(crate) fn mixed_of(view: &LivingView) -> KnobFlags {
        KnobFlags {
            step: view.step.mixed,
            glide: view.glide.mixed,
            amplitude: view.amplitude.mixed,
            jitter: view.jitter.mixed,
            stagger: view.stagger.mixed,
            echo_count: view.echo_count.mixed,
            echo_spread: view.echo_spread.mixed,
            echo_alpha: view.echo_alpha.mixed,
            variance_amount: view.variance_amount.mixed,
            variance_steady: view.variance_steady.mixed,
            keep_size: view.keep_size.mixed,
            softness: view.softness.mixed,
        }
    }

    /// The layers that set each knob of a view, as the report names them.
    pub(crate) fn sources_of(view: &LivingView) -> KnobSources {
        fn layers<T>(read: &KnobRead<T>) -> Vec<String> {
            read.layers.clone()
        }
        let mut sources = KnobSources::default();
        for (knob, labels) in [
            (Knob::Amplitude, layers(&view.amplitude)),
            (Knob::Step, layers(&view.step)),
            (Knob::Glide, layers(&view.glide)),
            (Knob::Jitter, layers(&view.jitter)),
            (Knob::Stagger, layers(&view.stagger)),
            (Knob::EchoCount, layers(&view.echo_count)),
            (Knob::EchoSpread, layers(&view.echo_spread)),
            (Knob::EchoAlpha, layers(&view.echo_alpha)),
            (Knob::VarianceAmount, layers(&view.variance_amount)),
            (Knob::VarianceSteady, layers(&view.variance_steady)),
            (Knob::KeepSize, layers(&view.keep_size)),
            (Knob::Softness, layers(&view.softness)),
        ] {
            sources.set(knob, labels);
        }
        sources
    }

    impl PlayerApp {
        /// The knobs the props disagree on, over the whole grove: echoes and
        /// stagger from prop to prop, variance from lineage to lineage, and in
        /// a scene with no field, the clocks their specs gave them. A species
        /// with its own value for a knob is left out of it.
        #[cfg(test)]
        pub(in super::super) fn living_mixed(&self) -> KnobFlags {
            mixed_of(&self.living_view(None))
        }

        /// Each species the props grow from, with its knobs, where its own
        /// props disagree, and the layer behind each value (#247).
        pub(in super::super) fn living_species_states(
            &self,
            unsaved: &super::Unsaved,
        ) -> Vec<LivingSpeciesState> {
            let boil = self.living_boil();
            self.living_species()
                .into_iter()
                .map(|name| {
                    let view = self.living_view(Some(&name));
                    LivingSpeciesState {
                        props: view.props,
                        knobs: LivingKnobs::from_preset(&view.knobs(boil)),
                        mixed: mixed_of(&view),
                        sources: sources_of(&view),
                        unsaved: unsaved.species.contains(&name),
                        name,
                    }
                })
                .collect()
        }

        /// Each prop's place on its clock, for the panel and the overlay.
        pub(in super::super) fn living_rows(&self) -> Vec<LivingPropRow> {
            let props = self.procgen_resolver.living_props();
            props
                .props()
                .iter()
                .enumerate()
                .map(|(i, prop)| {
                    let id = LivingPropId(i);
                    let clock = prop.clock();
                    let beats = prop.beats();
                    let (index, weight) = flint_core::wooze::warp(beats, clock.glide);
                    LivingPropRow {
                        name: prop.entity_name().to_string(),
                        species: props.species_of(id).to_string(),
                        lineage_seed: props.lineage_seed(id),
                        position: prop.position(),
                        height: props.rest_height(id).unwrap_or(0.0),
                        reach: props.rest_reach(id).unwrap_or(0.0),
                        beats,
                        step: clock.step,
                        glide: clock.glide,
                        pose: index,
                        weight,
                        shape_seed: prop.shape_seed(index),
                        ring_first: prop.first_step().unwrap_or(0),
                        ring_len: prop.ring_len(),
                        ring_ready: prop.ready(),
                        echoes: prop.echo_count(),
                        waiting: prop.is_waiting(),
                    }
                })
                .collect()
        }
        fn living_panel(&mut self) -> Option<&mut LivingPropsDebugPanel> {
            self.debug_panels.iter_mut().find_map(|panel| {
                if panel.name() != flint_debug_ui::LIVING_PROPS_DEBUG_PANEL {
                    return None;
                }
                panel.as_any_mut().downcast_mut::<LivingPropsDebugPanel>()
            })
        }

        /// The tree picked in the world (#248), value by value as it reads them
        /// now, by the same `LivingProps::resolved` that `flint render --report
        /// json` reads, and the zones it stands in.
        pub(in super::super) fn living_picked_state(
            &self,
            name: &str,
        ) -> Option<LivingPickedState> {
            let living = self.procgen_resolver.living_props();
            let i = (living.props().iter()).position(|prop| prop.entity_name() == name)?;
            let prop = &living.props()[i];
            let field = self.wooze.as_ref().map(|field| (field, self.wooze_time));
            let resolved = living.resolved(LivingPropId(i), field);
            let view = LivingView::of(std::slice::from_ref(&resolved), false);
            let zones = field.map_or_else(Vec::new, |(field, time)| {
                (field.zones_in(prop.position(), time).into_iter())
                    .map(|(zone, weight)| (zone.to_string(), weight))
                    .collect()
            });
            Some(LivingPickedState {
                name: name.to_string(),
                zones,
                knobs: LivingKnobs::from_preset(&view.knobs(self.living_boil())),
                sources: sources_of(&view),
            })
        }

        /// Pick the living prop under `at`, in egui points, the nearest if
        /// several are, as a click in the world does (#248). Returns whether one
        /// was there: a click that finds none goes on to the game.
        pub(in super::super) fn pick_living_at(&mut self, at: egui::Pos2) -> bool {
            let rows = self.living_rows();
            let screen = self.living_screen();
            let view_proj = self.camera.view_projection_matrix();
            let Some(i) = flint_debug_ui::pick_prop(&rows, &view_proj, screen, at) else {
                return false;
            };
            self.pick_living(Some(&rows[i].name))
        }

        /// Pick the prop at the view centre, as F8 does with the cursor captured.
        pub(in super::super) fn pick_living_at_centre(&mut self) -> bool {
            let centre = self.living_screen().center();
            self.pick_living_at(centre)
        }

        /// The screen in egui points, as the overlay projects onto it.
        fn living_screen(&self) -> egui::Rect {
            let (w, h) = self.input.screen_size();
            let ppp = self.egui_ctx.pixels_per_point();
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32 / ppp, h as f32 / ppp))
        }

        /// Pick a prop by name, or let go with `None`. The panel turns to its
        /// species. Returns whether one is picked.
        pub(in super::super) fn pick_living(&mut self, name: Option<&str>) -> bool {
            self.mirror_living_panel();
            let picked = self.living_panel().is_some_and(|panel| panel.select(name));
            self.mirror_living_panel();
            picked
        }

        /// The next prop by name, or the one before, as Shift+F8 does.
        pub(in super::super) fn pick_next_living(&mut self, forward: bool) -> bool {
            self.mirror_living_panel();
            let picked = (self.living_panel()).is_some_and(|panel| panel.select_next(forward));
            self.mirror_living_panel();
            picked
        }

        /// The name of the prop picked in the world, if one is.
        pub(in super::super) fn picked_living(&self) -> Option<String> {
            let panel = self.debug_panels.iter().find_map(|panel| {
                (panel.name() == flint_debug_ui::LIVING_PROPS_DEBUG_PANEL)
                    .then(|| panel.as_any().downcast_ref::<LivingPropsDebugPanel>())
                    .flatten()
            })?;
            panel.selected().map(str::to_string)
        }

        /// Whether a click is the Living Props panel's to take (#248): its panel
        /// is open, the cursor is free, and there are props to pick.
        pub(in super::super) fn living_wants_clicks(&self) -> bool {
            !self.cursor_captured
                && !self.procgen_resolver.living_props().is_empty()
                && self.debug_panels.iter().any(|panel| {
                    panel.name() == flint_debug_ui::LIVING_PROPS_DEBUG_PANEL && panel.is_open()
                })
        }

        /// Apply what the panel asked for last frame and show it what's true now.
        /// It costs nothing while the panel's closed, there's no overlay and no
        /// tree is picked.
        pub(in super::super) fn drain_living_panel(&mut self) {
            let Some(panel) = self.living_panel() else {
                return;
            };
            let actions = panel.take_actions();
            panel.clear_dirty();
            let showing = panel.is_open();
            let picking = panel.selected().is_some();
            for action in actions {
                self.apply_living_action(action);
            }
            if !showing {
                // Closing it is what looks in the folder again next time.
                self.living_tune.preset_names = None;
                if !self.living_tune.overlay && !picking {
                    return;
                }
            }
            self.mirror_living_panel();
        }

        /// Show the panel what's true now.
        fn mirror_living_panel(&mut self) {
            let picked = self.picked_living();
            let picked = picked.and_then(|name| self.living_picked_state(&name));
            let view = self.living_view(None);
            let knobs = view.knobs(self.living_boil());
            let mixed = mixed_of(&view);
            let unsaved = self.living_unsaved();
            let species = self.living_species_states(&unsaved);
            let no_field = self.wooze.is_none();
            let rows = self.living_rows();
            let shadows = match self.procgen_resolver.living_props().shadows() {
                LivingShadows::Blended => "blended",
                LivingShadows::Drift => "drift",
            };
            let names = match self.living_tune.preset_names.clone() {
                Some(names) => names,
                None => {
                    let names = self.living_preset_names();
                    self.living_tune.preset_names = Some(names.clone());
                    names
                }
            };
            let frozen = self.living_tune.frozen;
            let overlay = self.living_tune.overlay;
            let status = self.living_tune.status.clone();
            let props = self.procgen_resolver.living_props();
            let health = flint_debug_ui::LivingHealth {
                rings_short: props
                    .props()
                    .iter()
                    .filter(|p| p.ready() < p.ring_len())
                    .count(),
                waiting: props.props().iter().filter(|p| p.is_waiting()).count(),
                reholding: props.is_reholding(),
                frames_starved: props.stats().frames_starved,
                blends_starved: props.stats().blends_starved,
            };
            let (paint_pass, ink_pass) =
                self.scene_renderer
                    .as_ref()
                    .map_or((false, false), |renderer| {
                        let config = renderer.post_process_config();
                        (
                            config.styles.is_enabled("kuwahara"),
                            config.styles.is_enabled("contour"),
                        )
                    });
            let ab = self
                .living_ab_state()
                .map(|ab| flint_debug_ui::LivingAbState {
                    a: ab.a,
                    b: ab.b,
                    showing_b: ab.showing_b,
                    split: ab.split,
                });
            if let Some(panel) = self.living_panel() {
                panel.refresh(flint_debug_ui::LivingPanelState {
                    ab,
                    knobs: LivingKnobs::from_preset(&knobs),
                    mixed,
                    species,
                    shadows: shadows.to_string(),
                    no_field,
                    frozen,
                    overlay,
                    rows,
                    preset_names: names,
                    status,
                    health,
                    paint_pass,
                    ink_pass,
                    picked,
                    unsaved_global: unsaved.global,
                    unsaved: unsaved.edits.iter().map(|edit| edit.to_string()).collect(),
                });
            }
        }

        pub(in super::super) fn apply_living_action(&mut self, action: LivingPanelAction) {
            let tune = &mut self.living_tune;
            match action {
                LivingPanelAction::Set(PanelScope::Global, moved) => {
                    if let Err(problem) = self.tune_living(&moved) {
                        self.living_tune.status = Some(problem);
                    }
                }
                // A species' own values (#247), through the layer #246 added:
                // they beat the grove's, the field and its zones.
                LivingPanelAction::Set(PanelScope::Species(name), moved) => {
                    let problems = self.tune_species(&name, &species_def_of(&moved));
                    if !problems.is_empty() {
                        self.living_tune.status = Some(problems.join("; "));
                    }
                }
                LivingPanelAction::Clear(name, knob) => {
                    let keys: Vec<String> = knob.iter().map(|k| k.key().to_string()).collect();
                    let said = match self.clear_living_species(&name, &keys) {
                        Ok(()) => match knob {
                            Some(knob) => format!("{name} takes the grove's {} again.", knob.key()),
                            None => format!("{name} takes every value from the grove again."),
                        },
                        Err(problem) => problem,
                    };
                    self.living_tune.status = Some(said);
                }
                LivingPanelAction::Freeze(frozen) => tune.frozen = frozen,
                LivingPanelAction::Step => self.step_living(),
                LivingPanelAction::Overlay(on) => tune.overlay = on,
                LivingPanelAction::Reset => {
                    self.reset_living_knobs();
                    self.living_tune.status = Some("Back as the scene loaded.".to_string());
                }
                LivingPanelAction::Save(name) => {
                    let said = match self.save_living_preset(&name) {
                        Ok(path) => saved_message(self, &name, &path),
                        Err(problem) => format!("Couldn't save: {problem}"),
                    };
                    println!("[living] {said}");
                    self.living_tune.status = Some(said);
                    self.living_tune.preset_names = None;
                }
                LivingPanelAction::Commit => {
                    let said = match self.commit_living() {
                        Ok(said) => said,
                        Err(problem) => format!("Couldn't commit: {problem}"),
                    };
                    println!("[living] {said}");
                    self.living_tune.status = Some(said);
                }
                LivingPanelAction::Load(name) => {
                    self.living_tune.status = Some(match self.load_living_preset(&name) {
                        Ok(()) => format!("Loaded {name}."),
                        Err(problem) => format!("{name}: {problem}"),
                    });
                }
                LivingPanelAction::PickAb(a, b) => tune.ab_picks = Some((a, b)),
                LivingPanelAction::Compare(a, b) => {
                    self.living_tune.ab_picks = Some((a.clone(), b.clone()));
                    let label = super::super::living_ab::side_label;
                    self.living_tune.status = Some(match self.compare_living(&a, &b) {
                        Ok(()) => format!(
                            "Comparing A ({}) with B ({}). F6 flips, F7 splits.",
                            label(&a),
                            label(&b)
                        ),
                        Err(problem) => problem,
                    });
                }
                LivingPanelAction::Flip => {
                    self.flip_living();
                }
                LivingPanelAction::Split(split) => {
                    self.split_living(split);
                }
                LivingPanelAction::StopComparing => self.stop_comparing_living(),
            }
        }
    }

    /// What was saved, how to load it, and which of the scene's own settings would
    /// still win over it. Those can't be read off the loaded scene, which has had
    /// any preset it names filled in, so this reads its text.
    fn saved_message(app: &PlayerApp, name: &str, path: &std::path::Path) -> String {
        let mut said = format!(
            "Saved {}. In a scene: [wooze] preset = \"{name}\"",
            path.display()
        );
        let (_, left_out) = app.living_shared_knobs();
        if !left_out.is_empty() {
            said.push_str(&format!(
                ". Left out, as the species differ: {}",
                left_out.join(", ")
            ));
        }
        let scene = flint_core::vfs::read_to_string(&app.scene_path)
            .ok()
            .and_then(|text| toml::from_str::<flint_scene::SceneFile>(&text).ok());
        let wins = scene.as_ref().map(super::scene_says).unwrap_or_default();
        if !wins.is_empty() {
            said.push_str(&format!(
                ". This scene's own {} would win over it",
                wins.join(", ")
            ));
        }
        said
    }
}

#[cfg(test)]
#[path = "living_tuning_tests.rs"]
mod tests;
