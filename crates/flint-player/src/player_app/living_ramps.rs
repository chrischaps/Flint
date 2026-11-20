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
use flint_procgen::{KnobValue, LivingDef, LivingKnob, LivingSpeciesDef};

use super::living_props::species_of;
use super::living_subsystem::LivingSubsystem;
use super::living_tuning::{tune_props, tune_species, LivingView, ScriptTune};
use super::subsystem::StageCtx;

/// A knob a timed tune eases rather than steps: one whose value is a number.
pub(crate) type Eased = LivingKnob;

/// Every knob a timed tune eases, for a species, in the panel's order.
fn eased() -> impl Iterator<Item = Eased> {
    LivingKnob::ALL.into_iter().filter(|knob| knob.is_number())
}

/// The ones the whole grove's `[living]` holds: its clock is the field's.
fn eased_grove() -> impl Iterator<Item = Eased> {
    eased().filter(|knob| !knob.is_clock())
}

/// What a knob reads now over a view's props.
fn read(knob: Eased, view: &LivingView) -> f32 {
    view.get(knob).value.as_f32().unwrap_or_default()
}

/// Say `value` for a knob in a species table.
fn put(knob: Eased, def: &mut LivingSpeciesDef, value: f32) {
    knob.set_in_species(def, &KnobValue::Number(value));
}

/// A species table of the grove's knobs as a `[living]` says them.
fn grove_living(def: LivingSpeciesDef) -> LivingDef {
    let mut living = LivingDef::default();
    for knob in LivingKnob::ALL {
        if let Some(value) = knob.in_species(&def) {
            knob.set_in_living(&mut living, &value);
        }
    }
    living
}

/// Knobs to ease, each with the value it's heading for.
type Targets = Vec<(Eased, f32)>;

/// Take every eased knob out of a species table, with the value it asks for.
fn take_eased(def: &mut LivingSpeciesDef) -> Targets {
    eased()
        .filter_map(|knob| {
            let to = knob.take_from_species(def)?.as_f32()?;
            Some((knob, to))
        })
        .collect()
}

/// Take every eased knob out of the grove's `[living]`, with the value it asks
/// for.
fn take_eased_grove(living: &mut LivingDef) -> Targets {
    eased_grove()
        .filter_map(|knob| {
            let to = knob.take_from_living(living)?.as_f32()?;
            Some((knob, to))
        })
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
        self.ramps
            .retain(|r| r.species.is_some() || r.knob.in_living(living).is_none());
    }

    /// Stop easing whatever `def` sets at once for `species`.
    pub(crate) fn cancel_species(&mut self, species: &str, def: &LivingSpeciesDef) {
        self.ramps
            .retain(|r| r.species.as_deref() != Some(species) || r.knob.in_species(def).is_none());
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
            put(ramp.knob, &mut moved[at].1, value);
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
                eased.push((None, take_eased_grove(&mut tune.living)));
                // A species no prop grows from keeps its values at once, and
                // is named, as a step names it
                for (name, def) in &mut tune.living.species {
                    if grown.contains(name) {
                        eased.push((Some(name.clone()), take_eased(def)));
                    }
                }
                if let Err(problem) = self.tune_living(ctx, &tune) {
                    problems.push(problem);
                }
            }
            ScriptTune::Species(name, mut def) => {
                if grown.contains(&name) {
                    eased.push((Some(name.clone()), take_eased(&mut def)));
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
            put(knob, &mut one, to);
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
                .head_for(species, knob, read(knob, &view), to, seconds);
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
        let props = &mut *ctx.living;
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
