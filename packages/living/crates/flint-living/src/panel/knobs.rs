//! Every knob a species can set, described once (#753, #1025). The list is
//! flint-procgen's [`living_knob_rows!`](flint_procgen::living_knob_rows): a
//! row is a knob's key, where it sits in a preset, its default, the panel's
//! section and control for it, and what it does. [`Knob`] is that crate's
//! [`LivingKnob`](flint_procgen::LivingKnob), so the panel, the runtime and the
//! player name one enum; the [`LivingKnobs`] and [`KnobFlags`] fields, the
//! [`KnobDesc`] rows and the panel's sliders come from the same rows here, so
//! adding a knob is one row there. The boil's knobs aren't in it: a species
//! can't set them, and they're [`LivingKnobs`]' own fields.

use flint_procgen::{BoilDef, KnobType, WoozePreset};

/// One of the knobs a species can set (#247), in the order the panel shows them.
pub use flint_procgen::LivingKnob as Knob;

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

/// The one list of knobs is flint-procgen's ([`flint_procgen::living_knob_rows!`]):
/// a row is a [`Knob`] and everything about it. This makes the panel's side of
/// each row: its [`KnobDesc`], its [`LivingKnobs`] and [`KnobFlags`] fields,
/// and their preset conversions.
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
        /// Each knob's row, in [`Knob::ALL`]'s order.
        pub static KNOB_TABLE: [KnobDesc; Knob::COUNT] = [$(KnobDesc {
            knob: Knob::$knob,
            key: Knob::$knob.key(),
            section: KnobSection::$section,
            control: $control,
            help: $help,
        },)*];

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
        }

        impl LivingKnobs {
            /// The knobs a preset sets, and a sensible value for any it doesn't.
            pub fn from_preset(preset: &WoozePreset) -> Self {
                let boil = preset.boil.unwrap_or_default();
                Self {
                    $($field: Knob::$knob
                        .in_preset(preset)
                        .and_then(|value| <$ty as KnobType>::from_knob(&value))
                        .unwrap_or_else(|| $default),)*
                    boil_on: boil.enabled.unwrap_or(true),
                    boil_paint: boil.kuwahara.unwrap_or(0.0),
                    boil_ink: boil.contour.unwrap_or(0.0),
                    boil_cadence: boil.on.unwrap_or(2),
                }
            }

            /// One knob's value as a preset holds it.
            pub fn value_of(&self, knob: Knob) -> flint_procgen::KnobValue {
                match knob {
                    $(Knob::$knob => self.$field.clone().into_knob(),)*
                }
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

flint_procgen::living_knob_rows!(knob_table);

/// The panel's side of a [`Knob`]: its row, and the knobs of a section.
pub trait KnobPanel: Sized {
    /// Its row.
    fn desc(self) -> &'static KnobDesc;
    /// The knobs drawn in `section`, in the table's order.
    fn in_section(section: KnobSection) -> impl Iterator<Item = Self>;
}

impl KnobPanel for Knob {
    fn desc(self) -> &'static KnobDesc {
        &KNOB_TABLE[self.index()]
    }

    fn in_section(section: KnobSection) -> impl Iterator<Item = Knob> {
        Knob::ALL
            .into_iter()
            .filter(move |knob| knob.desc().section == section)
    }
}

impl KnobFlags {
    /// The knobs `preset` sets.
    pub fn set_by(preset: &WoozePreset) -> Self {
        let mut flags = Self::default();
        for knob in Knob::ALL {
            flags.set_flag(knob, knob.in_preset(preset).is_some());
        }
        flags
    }
}

impl LivingKnobs {
    /// Every knob, as a preset.
    pub fn to_preset(&self) -> WoozePreset {
        let mut preset = WoozePreset::default();
        for knob in Knob::ALL {
            knob.set_in_preset(&mut preset, &self.value_of(knob));
        }
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
        for knob in Knob::ALL {
            let value = self.value_of(knob);
            if touched.get(knob) || value != before.value_of(knob) {
                knob.set_in_preset(&mut preset, &value);
            }
        }
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
