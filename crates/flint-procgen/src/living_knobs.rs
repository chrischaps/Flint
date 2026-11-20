//! Every knob a living prop's species can set, described once (#753, #1025).
//!
//! [`living_knob_rows!`] is the one list. A row is a knob and everything about
//! it: its field and default, its key in `[living.species.<spec>]`, `clear_living`
//! and `flint render --report json`, where it sits in a preset, and the Living
//! Props panel's section, control and hover for it. This crate makes the
//! [`LivingKnob`] enum and its reads and writes of a preset, a scene's
//! `[living]` and a species table from the list; flint-debug-ui makes the
//! panel's sliders from the same rows, and flint-living resolves each knob for a
//! prop. So adding a knob is one row here plus the runtime's resolution of it,
//! and flint-player names none of them: it loops over [`LivingKnob::ALL`].
//!
//! The panel's columns (`section`, `control`, `help`) are flint-debug-ui's: this
//! crate passes them on unread, and their names (`slider`, `STAGGERS`, ...) are
//! looked up where [`living_knob_rows!`] is called, in that crate.
//!
//! The boil's knobs aren't here: a species can't set them, being one pass over
//! the whole frame.

use flint_wooze::WoozeSample;

use crate::scene_tables::{
    LivingDef, LivingEchoesDef, LivingSpeciesDef, LivingVarianceDef, WoozeKnobs, WoozePreset,
};

/// Every knob, a row each, handed to `$callback!` (a `macro_rules!` of the
/// caller's that takes the rows). A row:
///
/// ```text
/// Variant {
///     field: <panel field>: <type> = <default>,
///     key: "<key>",
///     preset: <wooze | living | echoes | variance> . <field in it>,
///     section: <panel section>,
///     control: <panel control>,
///     help: "<what it does>",
/// }
/// ```
///
/// The type is `f32`, `u32`, `bool` or `String` (a name, as a preset holds it).
#[macro_export]
macro_rules! living_knob_rows {
    ($callback:ident) => {
        $callback! {
            Step {
                field: step: f32 = 0.5,
                key: "step",
                preset: wooze.step,
                section: Clock,
                control: log_slider("step (s)", 0.05, 4.0),
                help: "Seconds from one pose to the next: how long a step of the warped clock lasts.",
            },
            Glide {
                field: glide: f32 = 0.3,
                key: "glide",
                preset: wooze.glide,
                section: Clock,
                control: slider("glide", 0.0, 1.0),
                help: "How much of each step is spent easing to the next pose; the rest holds.",
            },
            Amplitude {
                field: amplitude: f32 = 1.0,
                key: "amplitude",
                preset: wooze.amplitude,
                section: Clock,
                control: slider("drift", 0.0, 1.0),
                help: "How much of each pose's slow drift from the rest shape shows.",
            },
            Jitter {
                field: jitter: f32 = 1.0,
                key: "jitter",
                preset: wooze.jitter,
                section: Clock,
                control: slider("jitter", 0.0, 4.0),
                help: "How far the leaf cards turn and their tints flicker, on the fast clock.",
            },
            JitterRate {
                field: jitter_rate: f32 = 1.0,
                key: "jitter_rate",
                preset: living.jitter_rate,
                section: Clock,
                control: log_slider("jitter rate (x)", 0.125, 8.0),
                help: "How fast the fast clock ticks, times each spec's jitter.step: 2 boils twice as fast. A change holds every card until its next tick.",
            },
            RowRate {
                field: row_rate: f32 = 1.0,
                key: "row_rate",
                preset: living.row_rate,
                section: Clock,
                control: log_slider("row rate (x)", 0.125, 8.0),
                help: "How fast a form's words drift along their lines, times each spec's row_pace: 2 reflows twice as fast. A change goes on from where the words are.",
            },
            Stagger {
                field: stagger: String = "per_level".to_string(),
                key: "stagger",
                preset: living.stagger,
                section: Clock,
                control: KnobControl::Choice { label: "stagger", choices: STAGGERS },
                help: "How a prop's parts fall out of phase with each other.",
            },
            EchoCount {
                field: echo_count: u32 = 6,
                key: "echoes.count",
                preset: echoes.count,
                section: Echoes,
                control: slider("count", 0.0, 16.0),
                help: "How many faint copies of each prop trail and lead it in time.",
            },
            EchoSpread {
                // To where an echo has faded out: flint_procgen::MAX_ECHO_SPREAD.
                field: echo_spread: f32 = 0.9,
                key: "echoes.spread",
                preset: echoes.spread,
                section: Echoes,
                control: slider("spread (steps)", 0.0, 1.818),
                help: "How far the farthest echo is from now, in steps of the clock.",
            },
            EchoAlpha {
                field: echo_alpha: f32 = 0.2,
                key: "echoes.alpha",
                preset: echoes.alpha,
                section: Echoes,
                control: slider("alpha", 0.0, 1.0),
                help: "How opaque an echo is at now; it fades with its distance in time.",
            },
            VarianceAmount {
                field: variance_amount: f32 = 0.35,
                key: "variance.amount",
                preset: variance.amount,
                section: Variance,
                control: slider("amount", 0.0, 1.0),
                help: "How much of each pose's difference from the rest shape is left.",
            },
            VarianceSteady {
                field: variance_steady: f32 = 0.8,
                key: "variance.steady",
                preset: variance.steady,
                section: Variance,
                control: slider("steady", 0.0, 1.0),
                help: "How much of that is held back toward the trunk, so the tips move most.",
            },
            KeepSize {
                field: keep_size: bool = true,
                key: "variance.keep_size",
                preset: variance.keep_size,
                section: Variance,
                control: KnobControl::Check { label: "keep size" },
                help: "Whether every pose keeps the rest shape's size.",
            },
            Softness {
                field: softness: f32 = 0.0,
                key: "softness",
                preset: living.softness,
                section: Silhouette,
                control: slider("softness", 0.0, 1.0),
                help: "How far the body's outline feathers into what's behind it; 0 is a hard edge.",
            },
        }
    };
}

/// One knob's value, whichever kind it is.
#[derive(Debug, Clone, PartialEq)]
pub enum KnobValue {
    Number(f32),
    Count(u32),
    Flag(bool),
    /// A name, as a preset holds it: a stagger.
    Name(String),
}

impl KnobValue {
    /// The number, if it's one.
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            KnobValue::Number(value) => Some(*value),
            _ => None,
        }
    }
}

// A value read back compares with what it should be, as a test writes it.
impl PartialEq<f32> for KnobValue {
    fn eq(&self, other: &f32) -> bool {
        *self == KnobValue::Number(*other)
    }
}

impl PartialEq<u32> for KnobValue {
    fn eq(&self, other: &u32) -> bool {
        *self == KnobValue::Count(*other)
    }
}

impl PartialEq<bool> for KnobValue {
    fn eq(&self, other: &bool) -> bool {
        *self == KnobValue::Flag(*other)
    }
}

impl PartialEq<crate::Stagger> for KnobValue {
    fn eq(&self, other: &crate::Stagger) -> bool {
        matches!(self, KnobValue::Name(name) if name == other.name())
    }
}

impl std::fmt::Display for KnobValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KnobValue::Number(value) => write!(f, "{value}"),
            KnobValue::Count(value) => write!(f, "{value}"),
            KnobValue::Flag(value) => write!(f, "{value}"),
            KnobValue::Name(value) => f.write_str(value),
        }
    }
}

/// A type a knob's value can be: to and from a [`KnobValue`].
pub trait KnobType: Sized + Clone {
    fn into_knob(self) -> KnobValue;
    fn from_knob(value: &KnobValue) -> Option<Self>;
}

impl KnobType for f32 {
    fn into_knob(self) -> KnobValue {
        KnobValue::Number(self)
    }
    fn from_knob(value: &KnobValue) -> Option<Self> {
        match value {
            KnobValue::Number(value) => Some(*value),
            _ => None,
        }
    }
}

impl KnobType for u32 {
    fn into_knob(self) -> KnobValue {
        KnobValue::Count(self)
    }
    fn from_knob(value: &KnobValue) -> Option<Self> {
        match value {
            KnobValue::Count(value) => Some(*value),
            _ => None,
        }
    }
}

impl KnobType for bool {
    fn into_knob(self) -> KnobValue {
        KnobValue::Flag(self)
    }
    fn from_knob(value: &KnobValue) -> Option<Self> {
        match value {
            KnobValue::Flag(value) => Some(*value),
            _ => None,
        }
    }
}

impl KnobType for String {
    fn into_knob(self) -> KnobValue {
        KnobValue::Name(self)
    }
    fn from_knob(value: &KnobValue) -> Option<Self> {
        match value {
            KnobValue::Name(value) => Some(value.clone()),
            _ => None,
        }
    }
}

/// A knob's slot in a table that has the `[living]` shape (a scene's `[living]`
/// or a species table): `living.x` is its own field, `echoes.x` and `variance.x`
/// one of its tables'. The clock's (`wooze.x`) aren't in `[living]`.
macro_rules! living_get {
    ($t:expr, living . $f:ident) => {
        $t.$f.clone()
    };
    ($t:expr, echoes . $f:ident) => {
        $t.echoes.as_ref().and_then(|d| d.$f.clone())
    };
    ($t:expr, variance . $f:ident) => {
        $t.variance.as_ref().and_then(|d| d.$f.clone())
    };
}

macro_rules! living_set {
    ($t:expr, living . $f:ident, $v:expr) => {
        $t.$f = Some($v)
    };
    ($t:expr, echoes . $f:ident, $v:expr) => {
        $t.echoes.get_or_insert_with(LivingEchoesDef::default).$f = Some($v)
    };
    ($t:expr, variance . $f:ident, $v:expr) => {
        $t.variance
            .get_or_insert_with(LivingVarianceDef::default)
            .$f = Some($v)
    };
}

/// Take a knob out of a `[living]`-shaped table, leaving the table (and any
/// sibling fields) where it is.
macro_rules! living_take {
    ($t:expr, living . $f:ident) => {
        $t.$f.take()
    };
    ($t:expr, echoes . $f:ident) => {
        $t.echoes.as_mut().and_then(|d| d.$f.take())
    };
    ($t:expr, variance . $f:ident) => {
        $t.variance.as_mut().and_then(|d| d.$f.take())
    };
}

/// A knob's slot in each shape it's kept in: a preset, a scene's `[living]`, a
/// species table (whose clock knobs are its own fields) and a wooze sample.
macro_rules! knob_slots {
    (get preset $p:expr, wooze . $f:ident) => {
        $p.wooze.$f.clone()
    };
    (get preset $p:expr, $g:ident . $f:ident) => {
        living_get!($p.living, $g.$f)
    };
    (set preset $p:expr, wooze . $f:ident, $v:expr) => {
        $p.wooze.$f = Some($v)
    };
    (set preset $p:expr, $g:ident . $f:ident, $v:expr) => {
        living_set!($p.living, $g.$f, $v)
    };
    (take preset $p:expr, wooze . $f:ident) => {
        $p.wooze.$f.take()
    };
    (take preset $p:expr, $g:ident . $f:ident) => {
        living_take!($p.living, $g.$f)
    };

    (get living $l:expr, wooze . $f:ident) => {
        None
    };
    (get living $l:expr, $g:ident . $f:ident) => {
        living_get!($l, $g.$f)
    };
    (set living $l:expr, wooze . $f:ident, $v:expr) => {{
        let _ = (&$l, $v);
    }};
    (set living $l:expr, $g:ident . $f:ident, $v:expr) => {
        living_set!($l, $g.$f, $v)
    };
    (take living $l:expr, wooze . $f:ident) => {{
        let _ = &$l;
        None
    }};
    (take living $l:expr, $g:ident . $f:ident) => {
        living_take!($l, $g.$f)
    };

    (get species $s:expr, wooze . $f:ident) => {
        $s.$f.clone()
    };
    (get species $s:expr, $g:ident . $f:ident) => {
        living_get!($s, $g.$f)
    };
    (set species $s:expr, wooze . $f:ident, $v:expr) => {
        $s.$f = Some($v)
    };
    (set species $s:expr, $g:ident . $f:ident, $v:expr) => {
        living_set!($s, $g.$f, $v)
    };
    (take species $s:expr, wooze . $f:ident) => {
        $s.$f.take()
    };
    (take species $s:expr, $g:ident . $f:ident) => {
        living_take!($s, $g.$f)
    };

    (get wooze $w:expr, wooze . $f:ident) => {
        $w.$f
    };
    (get wooze $w:expr, $g:ident . $f:ident) => {
        None
    };
    (sample $s:expr, wooze . $f:ident) => {
        Some($s.$f)
    };
    (sample $s:expr, $g:ident . $f:ident) => {
        None
    };
    (is_clock wooze) => {
        true
    };
    (is_clock $g:ident) => {
        false
    };
}

macro_rules! procgen_knobs {
    ($(
        $knob:ident {
            field: $field:ident: $ty:ty = $default:expr,
            key: $key:literal,
            preset: $group:ident . $path:ident,
            section: $section:ident,
            control: $control:expr,
            help: $help:literal $(,)?
        }
    ),* $(,)?) => {
        /// One of the knobs a species can set (#247), in the order the Living
        /// Props panel shows them. The boil's aren't among them.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum LivingKnob {
            $($knob,)*
        }

        impl LivingKnob {
            /// Every knob, in the panel's order.
            pub const ALL: [LivingKnob; LivingKnob::COUNT] = [$(LivingKnob::$knob,)*];
            /// How many there are.
            pub const COUNT: usize = [$($key,)*].len();

            /// Its key in `[living.species.<spec>]`, in `clear_living` and in
            /// `flint render --report json`'s `values`.
            pub const fn key(self) -> &'static str {
                match self {
                    $(LivingKnob::$knob => $key,)*
                }
            }

            /// Its place in [`Self::ALL`], for a table kept by knob.
            pub const fn index(self) -> usize {
                self as usize
            }

            /// Whether it's one of the wooze field's four: the clock's.
            pub const fn is_clock(self) -> bool {
                match self {
                    $(LivingKnob::$knob => knob_slots!(is_clock $group),)*
                }
            }

            /// The value it has where nothing sets it.
            pub fn default_value(self) -> KnobValue {
                match self {
                    $(LivingKnob::$knob => {
                        let value: $ty = $default;
                        value.into_knob()
                    })*
                }
            }

            /// What `preset` sets it to.
            pub fn in_preset(self, preset: &WoozePreset) -> Option<KnobValue> {
                match self {
                    $(LivingKnob::$knob => {
                        let value: Option<$ty> = knob_slots!(get preset preset, $group . $path);
                        value.map(KnobType::into_knob)
                    })*
                }
            }

            /// Set it in `preset`. A value of another kind is left out.
            pub fn set_in_preset(self, preset: &mut WoozePreset, value: &KnobValue) {
                match self {
                    $(LivingKnob::$knob => {
                        if let Some(value) = <$ty as KnobType>::from_knob(value) {
                            knob_slots!(set preset preset, $group . $path, value);
                        }
                    })*
                }
            }

            /// Take it out of `preset`, leaving the table it was in.
            pub fn clear_in_preset(self, preset: &mut WoozePreset) {
                match self {
                    $(LivingKnob::$knob => {
                        let _: Option<$ty> = knob_slots!(take preset preset, $group . $path);
                    })*
                }
            }

            /// What a scene's `[living]` sets it to. The clock's are `[wooze]`'s.
            pub fn in_living(self, living: &LivingDef) -> Option<KnobValue> {
                match self {
                    $(LivingKnob::$knob => {
                        let value: Option<$ty> = knob_slots!(get living living, $group . $path);
                        value.map(KnobType::into_knob)
                    })*
                }
            }

            /// Take it out of a scene's `[living]`, leaving the rest.
            pub fn take_from_living(self, living: &mut LivingDef) -> Option<KnobValue> {
                match self {
                    $(LivingKnob::$knob => {
                        let value: Option<$ty> = knob_slots!(take living living, $group . $path);
                        value.map(KnobType::into_knob)
                    })*
                }
            }

            /// Set it in a scene's `[living]`; the clock's have no place there.
            pub fn set_in_living(self, living: &mut LivingDef, value: &KnobValue) {
                match self {
                    $(LivingKnob::$knob => {
                        if let Some(value) = <$ty as KnobType>::from_knob(value) {
                            knob_slots!(set living living, $group . $path, value);
                        }
                    })*
                }
            }

            /// What a species table sets it to.
            pub fn in_species(self, def: &LivingSpeciesDef) -> Option<KnobValue> {
                match self {
                    $(LivingKnob::$knob => {
                        let value: Option<$ty> = knob_slots!(get species def, $group . $path);
                        value.map(KnobType::into_knob)
                    })*
                }
            }

            /// Set it in a species table. A value of another kind is left out.
            pub fn set_in_species(self, def: &mut LivingSpeciesDef, value: &KnobValue) {
                match self {
                    $(LivingKnob::$knob => {
                        if let Some(value) = <$ty as KnobType>::from_knob(value) {
                            knob_slots!(set species def, $group . $path, value);
                        }
                    })*
                }
            }

            /// Take it out of a species table, leaving the rest.
            pub fn take_from_species(self, def: &mut LivingSpeciesDef) -> Option<KnobValue> {
                match self {
                    $(LivingKnob::$knob => {
                        let value: Option<$ty> = knob_slots!(take species def, $group . $path);
                        value.map(KnobType::into_knob)
                    })*
                }
            }

            /// What a wooze field's knobs set it to: the clock's four only.
            pub fn in_wooze(self, knobs: &WoozeKnobs) -> Option<f32> {
                match self {
                    $(LivingKnob::$knob => knob_slots!(get wooze knobs, $group . $path),)*
                }
            }

            /// Its value in a wooze sample: the clock's four only.
            pub fn in_sample(self, sample: &WoozeSample) -> Option<f32> {
                match self {
                    $(LivingKnob::$knob => knob_slots!(sample sample, $group . $path),)*
                }
            }
        }
    };
}

crate::living_knob_rows!(procgen_knobs);

impl LivingKnob {
    /// The knob a key names, if one does.
    pub fn from_key(key: &str) -> Option<LivingKnob> {
        Self::ALL.into_iter().find(|knob| knob.key() == key)
    }

    /// The table it's in (`echoes`, `variance`), if its key is in one.
    pub fn group(self) -> Option<&'static str> {
        self.key().split_once('.').map(|(group, _)| group)
    }

    /// Whether `key` covers it: its own key, or its table's (`echoes`).
    pub fn named_by(self, key: &str) -> bool {
        self.key() == key || self.group() == Some(key)
    }

    /// Whether its value is a number, so a timed tune can ease it (#514).
    pub fn is_number(self) -> bool {
        matches!(self.default_value(), KnobValue::Number(_))
    }
}

/// The keys `clear_living` takes: every knob's, and each table's (`echoes`,
/// `variance`) before its first knob.
pub fn species_keys() -> Vec<&'static str> {
    let mut keys = Vec::with_capacity(LivingKnob::COUNT + 2);
    for knob in LivingKnob::ALL {
        if let Some(group) = knob.group() {
            if !keys.contains(&group) {
                keys.push(group);
            }
        }
        keys.push(knob.key());
    }
    keys
}

/// A wooze sample's four knobs as a preset's `[wooze]` sets them, every one.
pub fn wooze_knobs_of(sample: &WoozeSample) -> WoozeKnobs {
    let mut preset = WoozePreset::default();
    for knob in LivingKnob::ALL {
        if let Some(value) = knob.in_sample(sample) {
            knob.set_in_preset(&mut preset, &KnobValue::Number(value));
        }
    }
    preset.wooze
}

/// What a species table says for the knobs a preset sets (#247): the shape
/// `[living.species.<spec>]` and a script's `tune_living` take. The boil has no
/// place in it, being one pass over the whole frame.
pub fn species_def_of(preset: &WoozePreset) -> LivingSpeciesDef {
    let mut def = LivingSpeciesDef::default();
    for knob in LivingKnob::ALL {
        if let Some(value) = knob.in_preset(preset) {
            knob.set_in_species(&mut def, &value);
        }
    }
    def
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_names_one_knob_and_back() {
        for knob in LivingKnob::ALL {
            assert_eq!(LivingKnob::from_key(knob.key()), Some(knob));
            assert_eq!(LivingKnob::ALL[knob.index()], knob);
        }
        assert_eq!(LivingKnob::from_key("echoes"), None);
        let clock: Vec<&str> = (LivingKnob::ALL.into_iter())
            .filter(|knob| knob.is_clock())
            .map(LivingKnob::key)
            .collect();
        assert_eq!(clock, ["step", "glide", "amplitude", "jitter"]);
    }

    #[test]
    fn a_knob_set_in_each_shape_reads_back_there_alone() {
        for knob in LivingKnob::ALL {
            let value = knob.default_value();
            let mut preset = WoozePreset::default();
            knob.set_in_preset(&mut preset, &value);
            assert_eq!(knob.in_preset(&preset), Some(value.clone()), "{knob:?}");
            for other in LivingKnob::ALL.into_iter().filter(|o| *o != knob) {
                assert_eq!(other.in_preset(&preset), None, "{knob:?} set {other:?}");
            }
            let def = species_def_of(&preset);
            assert_eq!(knob.in_species(&def), Some(value.clone()), "{knob:?}");
            let mut taken = def.clone();
            assert_eq!(knob.take_from_species(&mut taken), Some(value.clone()));
            assert_eq!(knob.in_species(&taken), None);
            assert_eq!(knob.in_living(&preset.living).is_some(), !knob.is_clock());
            knob.clear_in_preset(&mut preset);
            assert_eq!(knob.in_preset(&preset), None, "{knob:?} cleared");
        }
    }

    #[test]
    fn a_value_of_another_kind_sets_nothing() {
        let mut preset = WoozePreset::default();
        LivingKnob::EchoCount.set_in_preset(&mut preset, &KnobValue::Number(3.0));
        LivingKnob::Stagger.set_in_preset(&mut preset, &KnobValue::Flag(true));
        assert_eq!(preset, WoozePreset::default());
    }

    #[test]
    fn the_species_keys_are_each_knobs_and_each_tables() {
        let keys = species_keys();
        assert_eq!(keys.len(), LivingKnob::COUNT + 2);
        let echoes = keys.iter().position(|k| *k == "echoes").unwrap();
        assert_eq!(keys[echoes + 1], "echoes.count");
        assert!(LivingKnob::VarianceAmount.named_by("variance"));
        assert!(!LivingKnob::VarianceAmount.named_by("var"));
    }

    #[test]
    fn a_sample_is_its_four_clock_knobs() {
        let knobs = wooze_knobs_of(&WoozeSample::NEUTRAL);
        assert_eq!(knobs.step, Some(WoozeSample::NEUTRAL.step));
        assert_eq!(knobs.amplitude, Some(WoozeSample::NEUTRAL.amplitude));
        assert_eq!(knobs.glide, Some(WoozeSample::NEUTRAL.glide));
        assert_eq!(knobs.jitter, Some(WoozeSample::NEUTRAL.jitter));
    }
}
