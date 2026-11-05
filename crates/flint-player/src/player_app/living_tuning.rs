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
use flint_procgen::{EchoConfig, LivingPropId, Stagger, VarianceConfig};
use flint_scene::{
    list_presets, read_preset, write_preset, BoilDef, LivingDef, LivingEchoesDef,
    LivingVarianceDef, WoozeKnobs, WoozePreset,
};
use std::path::{Path, PathBuf};

use super::living_props::{stagger_name, stagger_of};
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
    /// While the boil is switched off: the amounts it'll come back to.
    boil_off: Option<(f32, f32)>,
    /// A boil asked for before there was a renderer to give it to.
    boil_waiting: Option<BoilDef>,
    /// The knobs as the scene loaded, to go back to.
    at_load: Option<AtLoad>,
    /// What came of the last save or load, for the panel.
    pub status: Option<String>,
    /// The presets on offer, as of when the panel opened or last saved one. Not
    /// read every frame, since that means reading a folder.
    #[cfg(feature = "debug-hud")]
    preset_names: Option<Vec<String>>,
}

/// Every knob as the scene loaded, prop by prop where props can differ: a grove
/// of several species goes back to several.
#[derive(Debug, Clone)]
struct AtLoad {
    /// The field's knobs and the boil, one of each per scene. In a scene with no
    /// field the clock's are unset, and `clocks` has them instead.
    knobs: WoozePreset,
    /// Each prop's clock, in a scene that had no field to give them one.
    clocks: Option<Vec<WoozeSample>>,
    /// Each prop's echoes, and its stagger.
    echoes: Vec<EchoConfig>,
    staggers: Vec<Stagger>,
    /// Each lineage's variance.
    variances: Vec<VarianceConfig>,
}

/// Whether the values `read` gives for `items` aren't all the same.
fn disagree<T, V: PartialEq>(items: &[T], read: impl Fn(&T) -> V) -> bool {
    let mut values = items.iter().map(read);
    match values.next() {
        Some(first) => values.any(|value| value != first),
        None => false,
    }
}

impl PlayerApp {
    /// Every knob as it stands now, all of them set. Where props disagree, as
    /// [`Self::living_mixed`] says, it's the first prop's value or the first
    /// lineage's.
    pub(super) fn living_knobs(&self) -> WoozePreset {
        let props = self.procgen_resolver.living_props();
        let first = props.props().first();
        let sample = match &self.wooze {
            Some(field) => field.global(self.wooze_time),
            None => first.map_or(WoozeSample::NEUTRAL, |prop| prop.wooze()),
        };
        let echoes = first.map_or_else(EchoConfig::default, |prop| prop.echo_config());
        let variance = props.variance().unwrap_or_default();
        let stagger = first.map_or(flint_procgen::Stagger::default(), |prop| prop.stagger());
        let boil = self.living_boil();
        WoozePreset {
            wooze: WoozeKnobs {
                amplitude: Some(sample.amplitude),
                step: Some(sample.step),
                glide: Some(sample.glide),
                jitter: Some(sample.jitter),
            },
            living: LivingDef {
                shadows: None,
                stagger: Some(stagger_name(stagger).to_string()),
                echoes: Some(LivingEchoesDef {
                    count: Some(echoes.count),
                    spread: Some(echoes.spread),
                    alpha: Some(echoes.alpha),
                }),
                variance: Some(LivingVarianceDef {
                    amount: Some(variance.amount),
                    steady: Some(variance.steady),
                    keep_size: Some(variance.keep_size),
                }),
            },
            boil: Some(boil),
        }
    }

    /// The knobs the props disagree on: echoes and stagger from prop to prop,
    /// variance from lineage to lineage, and in a scene with no field, the
    /// clocks their specs gave them.
    pub(super) fn living_mixed(&self) -> flint_debug_ui::KnobFlags {
        let living = self.procgen_resolver.living_props();
        let props = living.props();
        let variances = living.variances();
        let no_field = self.wooze.is_none();
        let clock =
            |read: fn(&WoozeSample) -> f32| no_field && disagree(props, |p| read(&p.wooze()));
        flint_debug_ui::KnobFlags {
            step: clock(|w| w.step),
            glide: clock(|w| w.glide),
            amplitude: clock(|w| w.amplitude),
            jitter: clock(|w| w.jitter),
            stagger: disagree(props, |p| p.stagger()),
            echo_count: disagree(props, |p| p.echo_config().count),
            echo_spread: disagree(props, |p| p.echo_config().spread),
            echo_alpha: disagree(props, |p| p.echo_config().alpha),
            variance_amount: disagree(&variances, |v| v.amount),
            variance_steady: disagree(&variances, |v| v.steady),
            keep_size: disagree(&variances, |v| v.keep_size),
        }
    }

    /// The knobs every prop agrees on, as a preset, and the names of the ones
    /// left out: what a file can say for the whole grove without flattening its
    /// species. Loading it leaves each species its own of the rest.
    pub(super) fn living_shared_knobs(&self) -> (WoozePreset, Vec<&'static str>) {
        let mut knobs = self.living_knobs();
        let mixed = self.living_mixed();
        let mut left_out = Vec::new();
        let mut out = |is: bool, name: &'static str| {
            if is {
                left_out.push(name);
            }
            is
        };
        let wooze = &mut knobs.wooze;
        for (is, name, value) in [
            (mixed.step, "step", &mut wooze.step),
            (mixed.glide, "glide", &mut wooze.glide),
            (mixed.amplitude, "amplitude", &mut wooze.amplitude),
            (mixed.jitter, "jitter", &mut wooze.jitter),
        ] {
            if out(is, name) {
                *value = None;
            }
        }
        if out(mixed.stagger, "stagger") {
            knobs.living.stagger = None;
        }
        if let Some(echoes) = &mut knobs.living.echoes {
            if out(mixed.echo_count, "echoes.count") {
                echoes.count = None;
            }
            for (is, name, value) in [
                (mixed.echo_spread, "echoes.spread", &mut echoes.spread),
                (mixed.echo_alpha, "echoes.alpha", &mut echoes.alpha),
            ] {
                if out(is, name) {
                    *value = None;
                }
            }
        }
        if let Some(variance) = &mut knobs.living.variance {
            for (is, name, value) in [
                (
                    mixed.variance_amount,
                    "variance.amount",
                    &mut variance.amount,
                ),
                (
                    mixed.variance_steady,
                    "variance.steady",
                    &mut variance.steady,
                ),
            ] {
                if out(is, name) {
                    *value = None;
                }
            }
            if out(mixed.keep_size, "variance.keep_size") {
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
        // these is what gives it a field, starting from where it stands.
        let time = self.wooze_time;
        let start = self
            .procgen_resolver
            .living_props()
            .props()
            .first()
            .map_or(WoozeSample::NEUTRAL, |prop| prop.wooze());
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
        // Once, however many props it's wrong for.
        let mut say = |problem: String| {
            if !problems.contains(&problem) {
                problems.push(problem);
            }
        };
        let props = self.procgen_resolver.living_props_mut();
        if let Some(name) = living.stagger.as_deref() {
            match stagger_of(name) {
                Some(stagger) => props.set_stagger(stagger),
                None => say(format!(
                    "stagger = \"{name}\" isn't \"none\", \"per_level\" or \"per_branch\""
                )),
            }
        }
        if let Some(wanted) = living.echoes {
            for i in 0..props.props().len() {
                let id = LivingPropId(i);
                let now = props.prop(id).echo_config();
                let echoes = EchoConfig {
                    count: wanted.count.unwrap_or(now.count),
                    spread: wanted.spread.unwrap_or(now.spread),
                    alpha: wanted.alpha.unwrap_or(now.alpha),
                };
                match echoes.check() {
                    Ok(()) => props.set_echoes_of(id, echoes),
                    Err((key, reason)) => say(format!("echoes.{key} = {reason}")),
                }
            }
        }
        if let Some(wanted) = living.variance {
            for (lineage, now) in props.variances().into_iter().enumerate() {
                let variance = VarianceConfig {
                    amount: wanted.amount.unwrap_or(now.amount),
                    steady: wanted.steady.unwrap_or(now.steady),
                    keep_size: wanted.keep_size.unwrap_or(now.keep_size),
                };
                match variance.check() {
                    Ok(()) => props.set_variance_of(lineage, variance),
                    Err((key, reason)) => say(format!("variance.{key} = {reason}")),
                }
            }
        }
        problems
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

    /// Every knob as it stands, prop by prop, to go back to.
    fn living_at_load(&self) -> AtLoad {
        let living = self.procgen_resolver.living_props();
        let props = living.props();
        let mut knobs = self.living_knobs();
        knobs.living = LivingDef::default();
        let clocks = self.wooze.is_none().then(|| {
            knobs.wooze = WoozeKnobs::default();
            props.iter().map(|prop| prop.wooze()).collect()
        });
        AtLoad {
            knobs,
            clocks,
            echoes: props.iter().map(|prop| prop.echo_config()).collect(),
            staggers: props.iter().map(|prop| prop.stagger()).collect(),
            variances: living.variances(),
        }
    }

    /// Put the knobs back as the scene loaded them: each prop's echoes and
    /// stagger, each lineage's variance, and in a scene that had no field, each
    /// prop's own clock.
    pub(super) fn reset_living_knobs(&mut self) {
        let Some(at_load) = self.living_tune.at_load.clone() else {
            return;
        };
        if let Err(problem) = self.tune_living(&at_load.knobs) {
            tracing::warn!("Living Props: {problem}");
        }
        if let Some(clocks) = &at_load.clocks {
            // A clock knob may since have given the scene a field it didn't have.
            self.wooze = None;
            let props = self.procgen_resolver.living_props_mut();
            let count = props.props().len();
            for (i, clock) in clocks.iter().enumerate().take(count) {
                props.set_wooze_of(LivingPropId(i), *clock);
            }
        }
        let props = self.procgen_resolver.living_props_mut();
        let count = props.props().len();
        let each = at_load.echoes.iter().zip(&at_load.staggers);
        for (i, (echoes, stagger)) in each.enumerate().take(count) {
            props.set_echoes_of(LivingPropId(i), *echoes);
            props.set_stagger_of(LivingPropId(i), *stagger);
        }
        let lineages = props.variances().len();
        for (lineage, variance) in at_load.variances.iter().enumerate().take(lineages) {
            props.set_variance_of(lineage, *variance);
        }
    }

    /// Queue exactly one beat of the clock.
    pub(super) fn step_living(&mut self) {
        self.living_tune.steps_queued += 1;
    }

    /// Before the clock is read: hand over a boil that had to wait, note where the
    /// knobs started, and apply any single steps. Each is a whole beat for every
    /// prop, however long a beat is where it stands, and for the boil at the
    /// camera. The field's clock moves on by a beat of its global step.
    pub(super) fn before_living_tick(&mut self) {
        if let Some(boil) = self.living_tune.boil_waiting {
            if self.scene_renderer.is_some() {
                self.living_tune.boil_waiting = None;
                self.tune_boil(boil);
            }
        }
        if self.living_tune.at_load.is_none() && !self.procgen_resolver.living_props().is_empty() {
            self.living_tune.at_load = Some(self.living_at_load());
        }
        let steps = std::mem::take(&mut self.living_tune.steps_queued);
        if steps == 0 {
            return;
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

    /// Each prop's place on its clock, for the panel and the overlay.
    pub(super) fn living_rows(&self) -> Vec<flint_debug_ui::LivingPropRow> {
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
                flint_debug_ui::LivingPropRow {
                    name: prop.entity_name().to_string(),
                    lineage_seed: props.lineage_seed(id),
                    position: prop.position(),
                    height: props.rest_height(id).unwrap_or(0.0),
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
}

/// The F3 panel's half: its actions come in here and what's true goes back out.
#[cfg(feature = "debug-hud")]
pub(super) mod panel {
    use super::PlayerApp;
    use flint_debug_ui::{DebugPanel, LivingPanelAction, LivingPropsDebugPanel};

    impl PlayerApp {
        fn living_panel(&mut self) -> Option<&mut LivingPropsDebugPanel> {
            self.debug_panels.iter_mut().find_map(|panel| {
                if panel.name() != flint_debug_ui::LIVING_PROPS_DEBUG_PANEL {
                    return None;
                }
                panel.as_any_mut().downcast_mut::<LivingPropsDebugPanel>()
            })
        }

        /// Apply what the panel asked for last frame and show it what's true now.
        /// It costs nothing while the panel's closed and there's no overlay.
        pub(in super::super) fn drain_living_panel(&mut self) {
            let Some(panel) = self.living_panel() else {
                return;
            };
            let actions = panel.take_actions();
            panel.clear_dirty();
            let showing = panel.is_open();
            for action in actions {
                self.apply_living_action(action);
            }
            if !showing {
                // Closing it is what looks in the folder again next time.
                self.living_tune.preset_names = None;
                if !self.living_tune.overlay {
                    return;
                }
            }
            let knobs = self.living_knobs();
            let mixed = self.living_mixed();
            let no_field = self.wooze.is_none();
            let rows = self.living_rows();
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
            if let Some(panel) = self.living_panel() {
                panel.refresh(flint_debug_ui::LivingPanelState {
                    knobs: flint_debug_ui::LivingKnobs::from_preset(&knobs),
                    mixed,
                    no_field,
                    frozen,
                    overlay,
                    rows,
                    preset_names: names,
                    status,
                    health,
                    paint_pass,
                    ink_pass,
                });
            }
        }

        pub(in super::super) fn apply_living_action(&mut self, action: LivingPanelAction) {
            let tune = &mut self.living_tune;
            match action {
                LivingPanelAction::Set(moved) => {
                    if let Err(problem) = self.tune_living(&moved) {
                        self.living_tune.status = Some(problem);
                    }
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
