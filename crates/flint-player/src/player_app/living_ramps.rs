//! Easing Living Props knobs over time (#514): `tune_living(map, seconds)`.
//!
//! A step is a visible jump: an echo spread that widens from 0.4 to 1.8 in
//! one frame pops. So a timed tune splits in three. The grove's four clock
//! knobs are the wooze field's, and ease on its own ramps, as `set_wooze`'s do.
//! The other continuous knobs (each echo's spread and alpha, variance's
//! amount and steady, softness, the jitter rate, and a species' own clock) ease
//! here, on a
//! [`Ramp`] each: the field's own smoothstep, stepped on the living clock, so
//! a freeze or a paused game state holds them where they are. The discrete
//! ones (echo count, stagger, keep size, the boil) switch at the start, as
//! every knob did before.
//!
//! A ramp started on a knob that's already easing starts from where that one
//! stands, and one asked for the place it's already heading leaves it to get
//! there, so a script may ask every frame. A knob set at once, by a script,
//! the panel or a preset, takes over from its ramp.

use flint_core::field::Ramp;
use flint_procgen::{LivingDef, LivingEchoesDef, LivingSpeciesDef, LivingVarianceDef};

use super::living_props::species_of;
use super::living_subsystem::LivingSubsystem;
use super::living_tuning::{tune_props, tune_species, LivingView, ScriptTune};
use super::subsystem::StageCtx;

/// A knob a timed tune eases rather than steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Eased {
    Amplitude,
    Step,
    Glide,
    Jitter,
    EchoSpread,
    EchoAlpha,
    VarianceAmount,
    VarianceSteady,
    Softness,
    JitterRate,
    RowRate,
}

impl Eased {
    /// Every one, in the order a species table lists them.
    pub(crate) const ALL: [Eased; 11] = [
        Eased::Amplitude,
        Eased::Step,
        Eased::Glide,
        Eased::Jitter,
        Eased::EchoSpread,
        Eased::EchoAlpha,
        Eased::VarianceAmount,
        Eased::VarianceSteady,
        Eased::Softness,
        Eased::JitterRate,
        Eased::RowRate,
    ];

    /// The ones the whole grove's `[living]` holds: its clock is the field's.
    pub(crate) const GROVE: [Eased; 7] = [
        Eased::EchoSpread,
        Eased::EchoAlpha,
        Eased::VarianceAmount,
        Eased::VarianceSteady,
        Eased::Softness,
        Eased::JitterRate,
        Eased::RowRate,
    ];

    /// Its key, as `clear_living` names it.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Eased::Amplitude => "amplitude",
            Eased::Step => "step",
            Eased::Glide => "glide",
            Eased::Jitter => "jitter",
            Eased::EchoSpread => "echoes.spread",
            Eased::EchoAlpha => "echoes.alpha",
            Eased::VarianceAmount => "variance.amount",
            Eased::VarianceSteady => "variance.steady",
            Eased::Softness => "softness",
            Eased::JitterRate => "jitter_rate",
            Eased::RowRate => "row_rate",
        }
    }

    /// Whether `key` (a [`SPECIES_KEYS`](super::living_tuning::SPECIES_KEYS)
    /// entry) covers it: its own key, or its table's (`echoes`).
    fn named_by(self, key: &str) -> bool {
        let own = self.key();
        own == key
            || own
                .strip_prefix(key)
                .is_some_and(|rest| rest.starts_with('.'))
    }

    /// Its slot in a species table, if the table has it.
    fn get(self, def: &LivingSpeciesDef) -> Option<f32> {
        let echoes = def.echoes.as_ref();
        let variance = def.variance.as_ref();
        match self {
            Eased::Amplitude => def.amplitude,
            Eased::Step => def.step,
            Eased::Glide => def.glide,
            Eased::Jitter => def.jitter,
            Eased::EchoSpread => echoes.and_then(|e| e.spread),
            Eased::EchoAlpha => echoes.and_then(|e| e.alpha),
            Eased::VarianceAmount => variance.and_then(|v| v.amount),
            Eased::VarianceSteady => variance.and_then(|v| v.steady),
            Eased::Softness => def.softness,
            Eased::JitterRate => def.jitter_rate,
            Eased::RowRate => def.row_rate,
        }
    }

    /// Take it out of a species table, leaving the rest.
    fn take(self, def: &mut LivingSpeciesDef) -> Option<f32> {
        let echoes = def.echoes.as_mut();
        let variance = def.variance.as_mut();
        match self {
            Eased::Amplitude => def.amplitude.take(),
            Eased::Step => def.step.take(),
            Eased::Glide => def.glide.take(),
            Eased::Jitter => def.jitter.take(),
            Eased::EchoSpread => echoes.and_then(|e| e.spread.take()),
            Eased::EchoAlpha => echoes.and_then(|e| e.alpha.take()),
            Eased::VarianceAmount => variance.and_then(|v| v.amount.take()),
            Eased::VarianceSteady => variance.and_then(|v| v.steady.take()),
            Eased::Softness => def.softness.take(),
            Eased::JitterRate => def.jitter_rate.take(),
            Eased::RowRate => def.row_rate.take(),
        }
    }

    /// Say `value` for it in a species table.
    fn put(self, def: &mut LivingSpeciesDef, value: f32) {
        let echoes = LivingEchoesDef::default;
        let variance = LivingVarianceDef::default;
        let slot = match self {
            Eased::Amplitude => &mut def.amplitude,
            Eased::Step => &mut def.step,
            Eased::Glide => &mut def.glide,
            Eased::Jitter => &mut def.jitter,
            Eased::EchoSpread => &mut def.echoes.get_or_insert_with(echoes).spread,
            Eased::EchoAlpha => &mut def.echoes.get_or_insert_with(echoes).alpha,
            Eased::VarianceAmount => &mut def.variance.get_or_insert_with(variance).amount,
            Eased::VarianceSteady => &mut def.variance.get_or_insert_with(variance).steady,
            Eased::Softness => &mut def.softness,
            Eased::JitterRate => &mut def.jitter_rate,
            Eased::RowRate => &mut def.row_rate,
        };
        *slot = Some(value);
    }

    /// What it reads now over a view's props.
    fn read(self, view: &LivingView) -> f32 {
        match self {
            Eased::Amplitude => view.amplitude.value,
            Eased::Step => view.step.value,
            Eased::Glide => view.glide.value,
            Eased::Jitter => view.jitter.value,
            Eased::EchoSpread => view.echo_spread.value,
            Eased::EchoAlpha => view.echo_alpha.value,
            Eased::VarianceAmount => view.variance_amount.value,
            Eased::VarianceSteady => view.variance_steady.value,
            Eased::Softness => view.softness.value,
            Eased::JitterRate => view.jitter_rate.value,
            Eased::RowRate => view.row_rate.value,
        }
    }
}

/// The grove's `[living]` as a species table holds the same knobs, and back.
fn grove_def(living: &LivingDef) -> LivingSpeciesDef {
    LivingSpeciesDef {
        softness: living.softness,
        jitter_rate: living.jitter_rate,
        row_rate: living.row_rate,
        echoes: living.echoes,
        variance: living.variance,
        ..Default::default()
    }
}

fn grove_living(def: LivingSpeciesDef) -> LivingDef {
    LivingDef {
        softness: def.softness,
        jitter_rate: def.jitter_rate,
        row_rate: def.row_rate,
        echoes: def.echoes,
        variance: def.variance,
        ..Default::default()
    }
}

/// Knobs to ease, each with the value it's heading for.
type Targets = Vec<(Eased, f32)>;

/// Take every eased knob out of `def`, with the value it asks for.
fn take_eased(def: &mut LivingSpeciesDef, knobs: &[Eased]) -> Targets {
    (knobs.iter())
        .filter_map(|&knob| knob.take(def).map(|to| (knob, to)))
        .collect()
}

/// Seconds of rounding a ramp's end may fall short by and still arrive.
const ARRIVAL_SLACK: f64 = 1e-9;

/// One knob on its way, for the grove (`species: None`) or one species.
#[derive(Debug, Clone)]
struct KnobRamp {
    species: Option<String>,
    knob: Eased,
    ramp: Ramp,
    /// What the props were last given, so a held clock gives nothing.
    shown: f32,
}

/// Every knob easing now, and the clock they ease on: the living clock,
/// counted from the scene's start, which a freeze or a pause stops.
#[derive(Debug, Clone, Default)]
pub(crate) struct KnobRamps {
    time: f64,
    ramps: Vec<KnobRamp>,
}

impl KnobRamps {
    /// Whether no knob is easing.
    pub(crate) fn is_empty(&self) -> bool {
        self.ramps.is_empty()
    }

    /// How many are.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.ramps.len()
    }

    /// Head `knob` of `species` (`None`: the grove) for `to` over `seconds`,
    /// from `now`, what it reads, or from where a ramp already easing it
    /// stands. One already heading there is left to arrive.
    fn head_for(&mut self, species: Option<&str>, knob: Eased, now: f32, to: f32, seconds: f64) {
        let time = self.time;
        if let Some(running) = self.find(species, knob) {
            if *running.ramp.target() != to {
                running.ramp.head_for(to, time, seconds);
            }
            return;
        }
        if now == to {
            return;
        }
        let mut ramp = Ramp::constant(now);
        ramp.head_for(to, time, seconds);
        self.ramps.push(KnobRamp {
            species: species.map(str::to_string),
            knob,
            ramp,
            shown: now,
        });
    }

    fn find(&mut self, species: Option<&str>, knob: Eased) -> Option<&mut KnobRamp> {
        (self.ramps.iter_mut()).find(|r| r.knob == knob && r.species.as_deref() == species)
    }

    /// Stop easing whatever the grove's `living` sets at once.
    pub(crate) fn cancel_grove(&mut self, living: &LivingDef) {
        let def = grove_def(living);
        self.ramps
            .retain(|r| r.species.is_some() || r.knob.get(&def).is_none());
    }

    /// Stop easing whatever `def` sets at once for `species`.
    pub(crate) fn cancel_species(&mut self, species: &str, def: &LivingSpeciesDef) {
        self.ramps
            .retain(|r| r.species.as_deref() != Some(species) || r.knob.get(def).is_none());
    }

    /// Stop easing the knobs `keys` name for `species` (`None`: the grove),
    /// or all of its knobs with no keys.
    pub(crate) fn cancel_keys(&mut self, species: Option<&str>, keys: &[String]) {
        self.ramps.retain(|r| {
            r.species.as_deref() != species
                || !(keys.is_empty() || keys.iter().any(|key| r.knob.named_by(key)))
        });
    }

    /// Stop easing everything, where it stands.
    pub(crate) fn clear(&mut self) {
        self.ramps.clear();
    }

    /// Move the clock on by `dt` and say what each scope's knobs are now, as a
    /// species table each, leaving out what hasn't moved. A ramp that has
    /// arrived says its target once more and is gone.
    fn advance(&mut self, dt: f64) -> Vec<(Option<String>, LivingSpeciesDef)> {
        self.time += dt;
        // Sixty frames of 1/60 s add up to a hair under a second: a ramp over a
        // whole number of frames lands on its last one, not one frame after
        let time = self.time + ARRIVAL_SLACK;
        let mut moved: Vec<(Option<String>, LivingSpeciesDef)> = Vec::new();
        for ramp in &mut self.ramps {
            let value = ramp.ramp.at(time);
            if value == ramp.shown {
                continue;
            }
            ramp.shown = value;
            let at = match moved.iter().position(|(s, _)| *s == ramp.species) {
                Some(at) => at,
                None => {
                    moved.push((ramp.species.clone(), LivingSpeciesDef::default()));
                    moved.len() - 1
                }
            };
            ramp.knob.put(&mut moved[at].1, value);
        }
        self.ramps.retain(|r| !r.ramp.arrived(time));
        moved
    }
}

impl LivingSubsystem {
    /// A script's `tune_living(map, seconds)` (#514): what
    /// [`Self::tune_living_from_script`] does, with each continuous knob eased
    /// there over `seconds` and the rest switched at once. `seconds` of 0 or
    /// less is that step exactly.
    pub(crate) fn tune_living_over(
        &mut self,
        ctx: &mut StageCtx<'_>,
        knobs: toml::Value,
        seconds: f64,
    ) -> Result<(), String> {
        if !(seconds.is_finite() && seconds > 0.0) {
            return self.tune_living_from_script(ctx, knobs);
        }
        let grown = self.living_species(ctx);
        let mut problems = Vec::new();
        let mut eased: Vec<(Option<String>, Targets)> = Vec::new();
        match ScriptTune::of(knobs)? {
            ScriptTune::Grove(mut tune) => {
                let wooze = std::mem::take(&mut tune.wooze);
                problems.extend(self.tune_wooze_over(ctx, &wooze, seconds));
                let mut def = grove_def(&tune.living);
                eased.push((None, take_eased(&mut def, &Eased::GROVE)));
                tune.living.softness = def.softness;
                tune.living.jitter_rate = def.jitter_rate;
                tune.living.row_rate = def.row_rate;
                tune.living.echoes = def.echoes;
                tune.living.variance = def.variance;
                // A species no prop grows from keeps its values at once, and
                // is named, as a step names it
                for (name, def) in &mut tune.living.species {
                    if grown.contains(name) {
                        eased.push((Some(name.clone()), take_eased(def, &Eased::ALL)));
                    }
                }
                if let Err(problem) = self.tune_living(ctx, &tune) {
                    problems.push(problem);
                }
            }
            ScriptTune::Species(name, mut def) => {
                if grown.contains(&name) {
                    eased.push((Some(name.clone()), take_eased(&mut def, &Eased::ALL)));
                }
                problems.extend(self.tune_species(ctx, &name, &def));
            }
        }
        for (species, knobs) in eased {
            problems.extend(self.ease(ctx, species.as_deref(), &knobs, seconds));
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    /// Start each of `knobs` on its way for `species` (`None`: the grove), from
    /// what its props read now. A value the knob can't take is named and
    /// leaves it where it is, as a step would.
    fn ease(
        &mut self,
        ctx: &StageCtx<'_>,
        species: Option<&str>,
        knobs: &[(Eased, f32)],
        seconds: f64,
    ) -> Vec<String> {
        if knobs.is_empty() {
            return Vec::new();
        }
        let view = self.living_view(ctx, species);
        let mut problems = Vec::new();
        for &(knob, to) in knobs {
            let mut one = LivingSpeciesDef::default();
            knob.put(&mut one, to);
            let mut bad = None;
            species_of(&one, &mut |problem| bad = Some(problem));
            if let Some(problem) = bad {
                problems.push(match species {
                    Some(name) => format!("species '{name}': {problem}"),
                    None => problem,
                });
                continue;
            }
            self.ramps
                .head_for(species, knob, knob.read(&view), to, seconds);
        }
        problems
    }

    /// Move every easing knob on by `dt` of the living clock and give the props
    /// where each now stands. Nothing easing, nothing done, so a grove no
    /// script eases runs exactly as it did.
    pub(super) fn step_knob_ramps(&mut self, ctx: &mut StageCtx<'_>, dt: f64) {
        if self.ramps.is_empty() {
            return;
        }
        let props = ctx.resolver.living_props_mut();
        // Every value was checked when its ramp started, and an ease between
        // two good values stays between them
        for (species, def) in self.ramps.advance(dt) {
            let _ = match species {
                None => tune_props(props, &grove_living(def)),
                Some(name) => tune_species(props, &name, &def).0,
            };
        }
    }
}

#[cfg(test)]
#[path = "living_ramps_tests.rs"]
mod tests;
