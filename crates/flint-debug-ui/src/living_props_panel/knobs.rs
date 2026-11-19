//! Every knob a species can set, described once (#753): its key, where it sits
//! in a preset, its default, the panel's section and control for it, and what
//! it does. The [`Knob`] enum, the [`LivingKnobs`] and [`KnobFlags`] fields,
//! the preset conversions and the panel's sliders all come from [`knob_table!`]'s
//! one list, so adding a knob is one row there. The boil's knobs aren't in it:
//! a species can't set them, and they're [`LivingKnobs`]' own fields.

use flint_procgen::{BoilDef, LivingEchoesDef, LivingVarianceDef, WoozePreset};

/// How parts fall out of phase, as a spec or a scene names it.
pub(super) const STAGGERS: &[(&str, &str)] = &[
    ("none", "None: all at once"),
    ("per_level", "Per level: a tree shimmers from the trunk out"),
    ("per_branch", "Per branch: neighbours are never in step"),
];

/// Which of the panel's sections a knob is drawn in, in the order they come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnobSection {
    Clock,
    Echoes,
    Variance,
    Silhouette,
}

/// How the panel draws a knob.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KnobControl {
    /// A slider from `min` to `max`, logarithmic or not.
    Slider {
        label: &'static str,
        min: f64,
        max: f64,
        log: bool,
    },
    /// A combo of named choices, each with its hover.
    Choice {
        label: &'static str,
        choices: &'static [(&'static str, &'static str)],
    },
    /// A checkbox.
    Check { label: &'static str },
}

const fn slider(label: &'static str, min: f64, max: f64) -> KnobControl {
    KnobControl::Slider {
        label,
        min,
        max,
        log: false,
    }
}

const fn log_slider(label: &'static str, min: f64, max: f64) -> KnobControl {
    KnobControl::Slider {
        label,
        min,
        max,
        log: true,
    }
}

/// One row of the table, as the panel reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KnobDesc {
    pub knob: Knob,
    /// Its key in `[living.species.<spec>]`, in `clear_living` and in `flint
    /// render --report json`'s `values`.
    pub key: &'static str,
    pub section: KnobSection,
    pub control: KnobControl,
    /// What it does, for its hover.
    pub help: &'static str,
}

/// A knob's value in [`LivingKnobs`], to draw and turn.
pub enum KnobValue<'a> {
    F32(&'a mut f32),
    U32(&'a mut u32),
    Bool(&'a mut bool),
    Name(&'a mut String),
}

trait AsKnobValue {
    fn as_knob_value(&mut self) -> KnobValue<'_>;
}

impl AsKnobValue for f32 {
    fn as_knob_value(&mut self) -> KnobValue<'_> {
        KnobValue::F32(self)
    }
}

impl AsKnobValue for u32 {
    fn as_knob_value(&mut self) -> KnobValue<'_> {
        KnobValue::U32(self)
    }
}

impl AsKnobValue for bool {
    fn as_knob_value(&mut self) -> KnobValue<'_> {
        KnobValue::Bool(self)
    }
}

impl AsKnobValue for String {
    fn as_knob_value(&mut self) -> KnobValue<'_> {
        KnobValue::Name(self)
    }
}

/// Where a knob sits in a [`WoozePreset`]: the `[wooze]` knobs, a `[living]`
/// field, or one of `[living] echoes` / `variance`.
macro_rules! preset_get {
    ($p:expr, wooze . $f:ident) => {
        $p.wooze.$f.clone()
    };
    ($p:expr, living . $f:ident) => {
        $p.living.$f.clone()
    };
    ($p:expr, echoes . $f:ident) => {
        $p.living.echoes.as_ref().and_then(|d| d.$f.clone())
    };
    ($p:expr, variance . $f:ident) => {
        $p.living.variance.as_ref().and_then(|d| d.$f.clone())
    };
}

macro_rules! preset_set {
    ($p:expr, wooze . $f:ident, $v:expr) => {
        $p.wooze.$f = Some($v)
    };
    ($p:expr, living . $f:ident, $v:expr) => {
        $p.living.$f = Some($v)
    };
    ($p:expr, echoes . $f:ident, $v:expr) => {
        $p.living
            .echoes
            .get_or_insert_with(LivingEchoesDef::default)
            .$f = Some($v)
    };
    ($p:expr, variance . $f:ident, $v:expr) => {
        $p.living
            .variance
            .get_or_insert_with(LivingVarianceDef::default)
            .$f = Some($v)
    };
}

/// Whether a preset path is one of the wooze field's four clock knobs.
macro_rules! is_clock {
    (wooze) => {
        true
    };
    ($other:ident) => {
        false
    };
}

/// The one list of knobs. A row is a [`Knob`] variant and everything about
/// it; everything else here is made from the list.
macro_rules! knob_table {
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
        /// One of the knobs a species can set (#247). The boil's aren't among them.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Knob {
            $($knob,)*
        }

        impl Knob {
            /// Every knob, in the order the panel shows them.
            pub const ALL: [Knob; Knob::COUNT] = [$(Knob::$knob,)*];
            /// How many there are.
            pub const COUNT: usize = [$($key,)*].len();
            /// Each knob's row, in [`Self::ALL`]'s order.
            pub const TABLE: [KnobDesc; Knob::COUNT] = [$(KnobDesc {
                knob: Knob::$knob,
                key: $key,
                section: KnobSection::$section,
                control: $control,
                help: $help,
            },)*];

            /// Its row.
            pub fn desc(self) -> &'static KnobDesc {
                &Self::TABLE[self.index()]
            }

            /// Its key in `[living.species.<spec>]`, in `clear_living` and in `flint
            /// render --report json`'s `values`.
            pub fn key(self) -> &'static str {
                self.desc().key
            }

            /// Whether it's one of the wooze field's four: the clock's.
            pub fn is_clock(self) -> bool {
                match self {
                    $(Knob::$knob => is_clock!($group),)*
                }
            }

            /// The knobs drawn in `section`, in the table's order.
            pub fn in_section(section: KnobSection) -> impl Iterator<Item = Knob> {
                Self::ALL.into_iter().filter(move |knob| knob.desc().section == section)
            }

            pub(super) fn index(self) -> usize {
                self as usize
            }
        }

        /// Every knob, as the panel shows it.
        #[derive(Debug, Clone, PartialEq)]
        pub struct LivingKnobs {
            $(pub $field: $ty,)*
            pub boil_on: bool,
            pub boil_paint: f32,
            pub boil_ink: f32,
            pub boil_cadence: u32,
        }

        /// One flag per knob that can differ from prop to prop: which ones the
        /// props disagree on, or which ones a hand touched this frame.
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
        pub struct KnobFlags {
            $(pub $field: bool,)*
        }

        impl KnobFlags {
            /// One knob's flag.
            pub fn get(&self, knob: Knob) -> bool {
                match knob {
                    $(Knob::$knob => self.$field,)*
                }
            }

            /// Set one knob's flag.
            pub fn set_flag(&mut self, knob: Knob, on: bool) {
                match knob {
                    $(Knob::$knob => self.$field = on,)*
                }
            }

            /// The knobs `preset` sets.
            pub fn set_by(preset: &WoozePreset) -> Self {
                Self {
                    $($field: preset_get!(preset, $group . $path).is_some(),)*
                }
            }
        }

        impl LivingKnobs {
            /// The knobs a preset sets, and a sensible value for any it doesn't.
            pub fn from_preset(preset: &WoozePreset) -> Self {
                let boil = preset.boil.unwrap_or_default();
                Self {
                    $($field: preset_get!(preset, $group . $path).unwrap_or_else(|| $default),)*
                    boil_on: boil.enabled.unwrap_or(true),
                    boil_paint: boil.kuwahara.unwrap_or(0.0),
                    boil_ink: boil.contour.unwrap_or(0.0),
                    boil_cadence: boil.on.unwrap_or(2),
                }
            }

            /// Every knob, as a preset.
            pub fn to_preset(&self) -> WoozePreset {
                let mut preset = WoozePreset::default();
                $(preset_set!(preset, $group . $path, self.$field.clone());)*
                preset.boil = Some(self.boil());
                preset
            }

            /// Only the knobs that moved from `before`, as a preset whose unset
            /// fields leave a knob alone: the one a hand turned, and nothing it
            /// happened to carry along. A knob in `touched` counts as moved
            /// whatever its number, since a `mixed` knob shows one prop's value
            /// and setting every prop to it is still a change.
            pub fn moved(&self, before: &LivingKnobs, touched: &KnobFlags) -> WoozePreset {
                let mut preset = WoozePreset::default();
                $(if touched.$field || self.$field != before.$field {
                    preset_set!(preset, $group . $path, self.$field.clone());
                })*
                let boil = self.boil();
                let was = before.boil();
                let boil = BoilDef {
                    enabled: boil.enabled.filter(|_| boil.enabled != was.enabled),
                    kuwahara: boil.kuwahara.filter(|_| boil.kuwahara != was.kuwahara),
                    contour: boil.contour.filter(|_| boil.contour != was.contour),
                    on: boil.on.filter(|_| boil.on != was.on),
                };
                preset.boil = (boil != BoilDef::default()).then_some(boil);
                preset
            }

            /// One knob's value, to draw and turn.
            pub fn value_mut(&mut self, knob: Knob) -> KnobValue<'_> {
                match knob {
                    $(Knob::$knob => self.$field.as_knob_value(),)*
                }
            }
        }
    };
}

knob_table! {
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

impl Default for LivingKnobs {
    fn default() -> Self {
        Self::from_preset(&WoozePreset::default())
    }
}

impl LivingKnobs {
    /// The boil's knobs, as a preset holds them.
    fn boil(&self) -> BoilDef {
        BoilDef {
            enabled: Some(self.boil_on),
            kuwahara: Some(self.boil_paint),
            contour: Some(self.boil_ink),
            on: Some(self.boil_cadence),
        }
    }
}

impl KnobFlags {
    /// Whether any of the clock's four is set.
    pub fn any_clock(&self) -> bool {
        Knob::ALL
            .iter()
            .any(|knob| knob.is_clock() && self.get(*knob))
    }

    /// These, less the ones in `other`.
    pub fn without(self, other: &KnobFlags) -> Self {
        let mut left = self;
        for knob in Knob::ALL {
            left.set_flag(knob, self.get(knob) && !other.get(knob));
        }
        left
    }
}

/// Where each knob's value comes from (#247), over the props a scope covers:
/// each layer once, by the label `flint render --report json` gives it, `spec`,
/// `entity`, `global`, `zone:<name>` or `species`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KnobSources([Vec<String>; Knob::COUNT]);

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
