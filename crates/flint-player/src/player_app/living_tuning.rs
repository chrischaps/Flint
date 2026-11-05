//! Tuning a running grove (#218): every knob of Living Props, live, from the F3
//! panel or a script, and saved as a preset a scene can name.
//!
//! One path for both: anything that changes a knob says so as a
//! [`WoozePreset`](flint_scene::WoozePreset), whose unset fields leave a knob
//! alone. So what a preset file loads is exactly what a person turned, and setting
//! a knob to the value it has already costs nothing.
//!
//! Freezing stops the clock the field and the props run on, and nothing else: the
//! game goes on, so the camera can move around a held pose. A knob still works
//! while frozen, and glide and drift arrive on each part's next beat, as they do
//! at speed.

use flint_core::wooze::{preset as builtin_preset, WoozeField, WoozeSample, PRESETS};
use flint_procgen::{
    EchoConfig, LivingLayer, LivingLayers, LivingPropId, LivingScope, Resolved, ResolvedProp,
    Stagger, VarianceConfig,
};
use flint_scene::{
    list_presets, read_preset, write_preset, BoilDef, LivingDef, LivingEchoesDef, LivingSpeciesDef,
    LivingVarianceDef, WoozeKnobs, WoozePreset,
};
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
}

/// The keys [`PlayerApp::clear_living_species`] takes, each a knob or a group.
pub const SPECIES_KEYS: [&str; 13] = [
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
            (config.kuwahara_boil, config.contour_boil, config.boil_on)
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
        let mut problems = self.tune_wooze(&tune.wooze);
        problems.extend(self.tune_props(&tune.living));
        for (name, species) in &tune.living.species {
            problems.extend(self.tune_species(name, species));
        }
        if let Some(boil) = tune.boil {
            if let Some(on) = boil.on.filter(|on| !(1..=3).contains(on)) {
                problems.push(format!("boil.on = {on} must be 1, 2 or 3"));
            }
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
        let mut table = match knobs {
            toml::Value::Table(table) => table,
            other => return Err(format!("{other} must be a map")),
        };
        let Some(species) = table.remove("species") else {
            let tune = toml::Value::Table(table)
                .try_into::<WoozePreset>()
                .map_err(|e| e.to_string())?;
            return self.tune_living(&tune);
        };
        let name = species
            .as_str()
            .ok_or_else(|| format!("species = {species} must be a spec's name"))?;
        let def = toml::Value::Table(table)
            .try_into::<LivingSpeciesDef>()
            .map_err(|e| format!("species '{name}': {e}"))?;
        let problems = self.tune_species(name, &def);
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    fn tune_wooze(&mut self, knobs: &WoozeKnobs) -> Vec<String> {
        let wanted = [
            ("amplitude", knobs.amplitude),
            ("step", knobs.step),
            ("glide", knobs.glide),
            ("jitter", knobs.jitter),
        ];
        if wanted.iter().all(|(_, value)| value.is_none()) {
            return Vec::new();
        }
        // A scene with no [wooze] table has its specs' clocks: turning one of
        // these is what gives it a field, starting from where it stands, under
        // any species' own clock.
        let time = self.wooze_time;
        let start = self
            .procgen_resolver
            .living_props()
            .props()
            .first()
            .map_or(WoozeSample::NEUTRAL, |prop| prop.wooze_below());
        let field = self.wooze.get_or_insert_with(|| WoozeField::new(start));
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

    /// Stagger, echoes and variance. A field that's set goes to every prop, and
    /// one that isn't stays as each prop has it: moving echo `count` in a grove
    /// of several species leaves each its own `spread` and `alpha`.
    fn tune_props(&mut self, living: &LivingDef) -> Vec<String> {
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
        let props = self.procgen_resolver.living_props_mut();
        if let Some(stagger) = stagger {
            props.tune_stagger(LivingScope::All, stagger);
        }
        props.tune_echoes(LivingScope::All, echoes);
        props.tune_variance(LivingScope::All, variance);
        problems
    }

    /// One species' own values (#246), over whatever it has already: they beat
    /// the global ones, the field and its zones. A species no prop grows from
    /// keeps them, and is named, since it's most likely a misspelling.
    pub(super) fn tune_species(&mut self, name: &str, def: &LivingSpeciesDef) -> Vec<String> {
        let mut problems = Vec::new();
        let over = species_of(def, &mut |problem| {
            problems.push(format!("species '{name}': {problem}"));
        });
        let props = self.procgen_resolver.living_props_mut();
        let now = props.species_override(name).unwrap_or_default();
        if !props.set_species(name, over.merged(now)) {
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
        let tune = &mut self.living_tune;
        // Where the amounts live now: the passes, or what off remembers.
        let (mut paint, mut ink) = tune
            .boil_off
            .unwrap_or((config.kuwahara_boil, config.contour_boil));
        paint = boil.kuwahara.unwrap_or(paint);
        ink = boil.contour.unwrap_or(ink);
        let on = boil.enabled.unwrap_or(tune.boil_off.is_none());
        tune.boil_off = (!on).then_some((paint, ink));
        config.kuwahara_boil = if on { paint } else { 0.0 };
        config.contour_boil = if on { ink } else { 0.0 };
        if let Some(cadence) = boil.on {
            config.boil_on = cadence.clamp(1, 3);
        }
        renderer.set_post_process_config(config);
    }

    /// Save every knob as a preset. Returns the file and what a scene needs to
    /// say to load it.
    ///
    /// A knob the props disagree on is left out, so loading the file leaves each
    /// species its own: saving per-species values is #249's.
    pub(super) fn save_living_preset(&mut self, name: &str) -> Result<PathBuf, String> {
        let (knobs, _) = self.living_shared_knobs();
        write_preset(Path::new(&self.scene_path), name, &knobs)
    }

    /// Load a preset file, or a built-in, which holds only the field's knobs. A
    /// file comes first, as it does when a scene names one.
    pub(super) fn load_living_preset(&mut self, name: &str) -> Result<(), String> {
        let found = read_preset(Path::new(&self.scene_path), name)
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
            .ok_or_else(|| format!("there's no preset named '{name}'"))??;
        self.tune_living(&found)
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
        let mut names = list_presets(Path::new(&self.scene_path));
        for (name, _) in PRESETS {
            if !names.iter().any(|taken| taken == name) {
                names.push((*name).to_string());
            }
        }
        names
    }
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
        pub(in super::super) fn living_species_states(&self) -> Vec<LivingSpeciesState> {
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
            let species = self.living_species_states();
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
                        (config.kuwahara_enabled, config.contour_enabled)
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
        let wins = scene.as_ref().map(scene_says).unwrap_or_default();
        if !wins.is_empty() {
            said.push_str(&format!(
                ". This scene's own {} would win over it",
                wins.join(", ")
            ));
        }
        said
    }

    /// The knobs a scene sets itself, which a preset it names can't change.
    fn scene_says(scene: &flint_scene::SceneFile) -> Vec<String> {
        let mut says = Vec::new();
        if let Some(wooze) = &scene.wooze {
            for (key, value) in [
                ("amplitude", wooze.amplitude),
                ("step", wooze.step),
                ("glide", wooze.glide),
                ("jitter", wooze.jitter),
            ] {
                if value.is_some() {
                    says.push(format!("[wooze] {key}"));
                }
            }
        }
        if let Some(living) = &scene.living {
            if living.stagger.is_some() {
                says.push("[living] stagger".to_string());
            }
            if let Some(echoes) = living.echoes {
                for (key, set) in [
                    ("count", echoes.count.is_some()),
                    ("spread", echoes.spread.is_some()),
                    ("alpha", echoes.alpha.is_some()),
                ] {
                    if set {
                        says.push(format!("[living] echoes.{key}"));
                    }
                }
            }
            if let Some(variance) = living.variance {
                for (key, set) in [
                    ("amount", variance.amount.is_some()),
                    ("steady", variance.steady.is_some()),
                    ("keep_size", variance.keep_size.is_some()),
                ] {
                    if set {
                        says.push(format!("[living] variance.{key}"));
                    }
                }
            }
        }
        says
    }
}

#[cfg(test)]
#[path = "living_tuning_tests.rs"]
mod tests;
