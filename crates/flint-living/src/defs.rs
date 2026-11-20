//! A scene's, a preset's and a script's tables in the runtime's terms (#1025):
//! `[living]` into [`LivingDefaults`], `[living.species.<spec>]` into a
//! [`SpeciesOverride`], and a preset's `[living]` laid over a running grove.
//! Each knob's checks are here, beside the layers they fill, so the player names
//! none of them. A bad value is named and then means "what's below", since a
//! look lab shouldn't refuse to start over one knob.

use flint_procgen::{
    EchoConfig, LivingDef, LivingEchoesDef, LivingSpeciesDef, LivingVarianceDef, Stagger,
    VarianceConfig,
};

use super::layers::{
    ClockOverride, EchoOverride, LivingDefaults, LivingScope, LivingShadows, SpeciesOverride,
    VarianceOverride,
};
use super::LivingProps;

/// A scene's `[living]` table as the props want it. What's wrong is warned.
pub fn defaults_of(table: Option<&LivingDef>) -> LivingDefaults {
    let Some(table) = table else {
        return LivingDefaults::default();
    };
    let shadows = match table.shadows.as_deref() {
        None | Some("blended") => LivingShadows::Blended,
        Some("drift") => LivingShadows::Drift,
        Some(other) => {
            tracing::warn!(
                "[living] shadows = \"{other}\" isn't \"blended\" or \"drift\"; using blended"
            );
            LivingShadows::Blended
        }
    };
    let mut warn = |problem: String| tracing::warn!("[living] {problem}; using each spec's");
    let stagger = table
        .stagger
        .as_deref()
        .and_then(|name| checked_stagger(name, &mut warn));
    let species = table
        .species
        .iter()
        .map(|(name, def)| {
            let mut warn = |problem: String| {
                tracing::warn!("[living.species.{name}] {problem}; leaving it to the layers below")
            };
            (name.clone(), species_of(def, &mut warn))
        })
        .collect();
    LivingDefaults {
        shadows,
        echoes: echoes_of(table.echoes.as_ref(), &mut warn),
        stagger,
        variance: variance_of(table.variance.as_ref(), &mut warn),
        softness: softness_of(table.softness, &mut warn),
        jitter_rate: jitter_rate_of(table.jitter_rate, &mut warn),
        row_rate: row_rate_of(table.row_rate, &mut warn),
        species,
    }
}

/// A jitter rate (#529) as a scene or a tune gives it: 0 to
/// [`JITTER_RATE_MAX`](flint_procgen::JITTER_RATE_MAX), or `None` with the reason
/// sent to `say`.
pub fn jitter_rate_of(value: Option<f32>, say: &mut dyn FnMut(String)) -> Option<f32> {
    let value = value?;
    let max = flint_procgen::JITTER_RATE_MAX;
    if value.is_finite() && (0.0..=max).contains(&value) {
        Some(value)
    } else {
        say(format!("jitter_rate = {value} must be in [0.0, {max:?}]"));
        None
    }
}

/// A row rate (#576) as a scene or a tune gives it: 0 to
/// [`ROW_RATE_MAX`](flint_procgen::ROW_RATE_MAX), or `None` with the reason
/// sent to `say`.
pub fn row_rate_of(value: Option<f32>, say: &mut dyn FnMut(String)) -> Option<f32> {
    let value = value?;
    let max = flint_procgen::ROW_RATE_MAX;
    if value.is_finite() && (0.0..=max).contains(&value) {
        Some(value)
    } else {
        say(format!("row_rate = {value} must be in [0.0, {max:?}]"));
        None
    }
}

/// A softness (#582) as a scene or a tune gives it: 0 to 1, or `None` with the
/// reason sent to `say`.
pub fn softness_of(value: Option<f32>, say: &mut dyn FnMut(String)) -> Option<f32> {
    let value = value?;
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Some(value)
    } else {
        say(format!("softness = {value} must be in [0.0, 1.0]"));
        None
    }
}

/// One species' table (#246) as the props want it. Each bad value goes to
/// `say`, by key, and is left unset, so the layers below decide it.
pub fn species_of(def: &LivingSpeciesDef, say: &mut dyn FnMut(String)) -> SpeciesOverride {
    let mut knob = |name: &str, value: Option<f32>| {
        value.filter(|&value| match flint_wooze::check_knob_named(name, value) {
            Ok(_) => true,
            Err(problem) => {
                say(problem);
                false
            }
        })
    };
    let clock = ClockOverride {
        amplitude: knob("amplitude", def.amplitude),
        step: knob("step", def.step),
        glide: knob("glide", def.glide),
        jitter: knob("jitter", def.jitter),
    };
    SpeciesOverride {
        clock,
        stagger: def
            .stagger
            .as_deref()
            .and_then(|name| checked_stagger(name, say)),
        echoes: echoes_of(def.echoes.as_ref(), say),
        variance: variance_of(def.variance.as_ref(), say),
        softness: softness_of(def.softness, say),
        jitter_rate: jitter_rate_of(def.jitter_rate, say),
        row_rate: row_rate_of(def.row_rate, say),
    }
}

/// Everything wrong with one species' table, for `flint validate`.
pub fn species_problems(def: &LivingSpeciesDef) -> Vec<String> {
    let mut problems = Vec::new();
    species_of(def, &mut |problem| problems.push(problem));
    problems
}

/// A stagger by name, or `None` with the reason sent to `say`.
fn checked_stagger(name: &str, say: &mut dyn FnMut(String)) -> Option<Stagger> {
    let stagger = Stagger::from_name(name);
    if stagger.is_none() {
        say(format!(
            "stagger = \"{name}\" isn't \"none\", \"per_level\" or \"per_branch\""
        ));
    }
    stagger
}

/// A `variance` table as the props want it, with a bad field sent to `say`
/// and then left to the layers below.
pub fn variance_of(
    table: Option<&LivingVarianceDef>,
    say: &mut dyn FnMut(String),
) -> VarianceOverride {
    let Some(table) = table else {
        return VarianceOverride::default();
    };
    let mut fine = |variance: VarianceConfig| match variance.check() {
        Ok(()) => true,
        Err((key, reason)) => {
            say(format!("variance.{key} = {reason}"));
            false
        }
    };
    let base = VarianceConfig::default();
    VarianceOverride {
        amount: table
            .amount
            .filter(|&amount| fine(VarianceConfig { amount, ..base })),
        steady: table
            .steady
            .filter(|&steady| fine(VarianceConfig { steady, ..base })),
        keep_size: table.keep_size,
    }
}

/// An `echoes` table as the props want it, with a bad field sent to `say`
/// and then left to the layers below.
pub fn echoes_of(table: Option<&LivingEchoesDef>, say: &mut dyn FnMut(String)) -> EchoOverride {
    let Some(table) = table else {
        return EchoOverride::default();
    };
    let mut fine = |echoes: EchoConfig| match echoes.check() {
        Ok(()) => true,
        Err((key, reason)) => {
            say(format!("echoes.{key} = {reason}"));
            false
        }
    };
    let base = EchoConfig::default();
    EchoOverride {
        count: table
            .count
            .filter(|&count| fine(EchoConfig { count, ..base })),
        spread: table
            .spread
            .filter(|&spread| fine(EchoConfig { spread, ..base })),
        alpha: table
            .alpha
            .filter(|&alpha| fine(EchoConfig { alpha, ..base })),
    }
}

impl LivingProps {
    /// Turn the knobs a `[living]` table sets, in the global layer: stagger,
    /// echoes, variance, softness and the two rates. A field that's set goes to
    /// every prop, and one that isn't stays as each prop has it: moving echo
    /// `count` in a grove of several species leaves each its own `spread` and
    /// `alpha`. A species' own value still beats whatever this sets (#246).
    /// Returns what couldn't be used.
    pub fn tune_global(&mut self, living: &LivingDef) -> Vec<String> {
        let mut problems = Vec::new();
        let mut say = |problem: String| problems.push(problem);
        let stagger = (living.stagger.as_deref()).and_then(|name| checked_stagger(name, &mut say));
        let echoes = echoes_of(living.echoes.as_ref(), &mut say);
        let variance = variance_of(living.variance.as_ref(), &mut say);
        let softness = softness_of(living.softness, &mut say);
        let jitter_rate = jitter_rate_of(living.jitter_rate, &mut say);
        let row_rate = row_rate_of(living.row_rate, &mut say);
        if let Some(stagger) = stagger {
            self.tune_stagger(LivingScope::All, stagger);
        }
        self.tune_echoes(LivingScope::All, echoes);
        self.tune_variance(LivingScope::All, variance);
        if let Some(softness) = softness {
            self.tune_softness(LivingScope::All, softness);
        }
        if let Some(rate) = jitter_rate {
            self.tune_jitter_rate(LivingScope::All, rate);
        }
        if let Some(rate) = row_rate {
            self.tune_row_rate(LivingScope::All, rate);
        }
        problems
    }
}

/// Lay a preset file's `[living]` over the props, as the player's Load does
/// (#249): its stagger, echoes and variance set the global layer a field at a
/// time, and each species it names takes exactly its values. Its `[wooze]`
/// knobs are the field's. What can't be used is returned.
pub fn apply_preset(props: &mut LivingProps, living: &LivingDef) -> Vec<String> {
    let mut problems = props.tune_global(living);
    let species: Vec<(String, SpeciesOverride)> = (living.species.iter())
        .map(|(name, def)| {
            let mut say = |problem: String| problems.push(format!("species '{name}': {problem}"));
            (name.clone(), species_of(def, &mut say))
        })
        .collect();
    for (name, over) in species {
        if !props.set_species(&name, over) {
            problems.push(format!("no living prop grows from '{name}'"));
        }
    }
    problems
}
