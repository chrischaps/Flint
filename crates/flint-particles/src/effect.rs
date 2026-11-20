//! Authored particle effect assets (`*.particles.toml`).
//!
//! A [`ParticleEffect`] is one named effect made of several named
//! [`EmitterDef`]s. The same `EmitterDef` also deserialises an inline
//! `particle_emitter` component table, so both authoring paths share one
//! parser and one set of defaults (ADR 0068).
//!
//! ```toml
//! name = "campfire"
//! seed = 7
//!
//! [[emitters]]
//! name = "flames"
//! emission_rate = 40.0
//! lifetime = [0.3, 0.8]
//! size = { keys = [ { t = 0.0, v = [0.1, 0.1] }, { t = 1.0, v = [0.0, 0.0] } ] }
//! color = { start = [1.0, 0.7, 0.1, 1.0], end = [1.0, 0.1, 0.0, 0.0] }
//! blend_mode = "additive"
//!
//! [emitters.shape]
//! kind = "cone"
//! radius = 0.15
//! angle = 10.0
//! ```
//!
//! Authored values are lenient about scalars vs. ranges: `lifetime = 1.0`,
//! `lifetime = [0.5, 1.5]`, `size = 0.2`, `size = { start = .., end = .. }`
//! and `size = { keys = [...] }` are all valid.

use crate::curves::{Curve, Interp, Lerp};
use crate::emitter::{
    Burst, EmissionShape, EmitterConfig, Force, ParticleBlendMode, SortMode, SubEmitter,
};
use crate::rand::ParticleRng;
use serde::{Deserialize, Serialize};

/// A scalar or a `[min, max]` pair.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(untagged)]
pub enum RangeDef {
    Const(f32),
    MinMax([f32; 2]),
}

impl RangeDef {
    pub fn min(self) -> f32 {
        match self {
            RangeDef::Const(v) => v,
            RangeDef::MinMax([a, b]) => a.min(b),
        }
    }
    pub fn max(self) -> f32 {
        match self {
            RangeDef::Const(v) => v,
            RangeDef::MinMax([a, b]) => a.max(b),
        }
    }
    pub fn pair(self) -> (f32, f32) {
        (self.min(), self.max())
    }
    pub fn sample(self, rng: &mut ParticleRng) -> f32 {
        match self {
            RangeDef::Const(v) => v,
            RangeDef::MinMax([a, b]) => rng.range(a, b),
        }
    }
}

impl From<f32> for RangeDef {
    fn from(v: f32) -> Self {
        RangeDef::Const(v)
    }
}

/// An integer count or a `[min, max]` pair.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(untagged)]
pub enum CountDef {
    Const(u32),
    MinMax([u32; 2]),
}

impl CountDef {
    pub fn pair(self) -> (u32, u32) {
        match self {
            CountDef::Const(v) => (v, v),
            CountDef::MinMax([a, b]) => (a.min(b), a.max(b)),
        }
    }
}

/// One curve key.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Key<T> {
    pub t: f32,
    pub v: T,
}

/// A constant, a start→end ramp, or a full key list.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum CurveDef<T> {
    Const(T),
    StartEnd {
        start: T,
        end: T,
    },
    Keys {
        keys: Vec<Key<T>>,
        #[serde(default)]
        interp: Interp,
    },
}

impl<T: Lerp> CurveDef<T> {
    pub fn to_curve(&self) -> Result<Curve<T>, String> {
        match self {
            CurveDef::Const(v) => Ok(Curve::constant(*v)),
            CurveDef::StartEnd { start, end } => Ok(Curve::start_end(*start, *end)),
            CurveDef::Keys { keys, interp } => {
                Curve::from_keys(keys.iter().map(|k| (k.t, k.v)).collect(), *interp)
            }
        }
    }

    /// The inverse of [`to_curve`](Self::to_curve), choosing the most
    /// compact authored form.
    pub fn from_curve(curve: &Curve<T>) -> Self {
        let keys = curve.keys();
        match (keys.len(), curve.interp()) {
            (1, _) => CurveDef::Const(keys[0].1),
            (2, Interp::Linear) if keys[0].0 == 0.0 && keys[1].0 == 1.0 => CurveDef::StartEnd {
                start: keys[0].1,
                end: keys[1].1,
            },
            (_, interp) => CurveDef::Keys {
                keys: keys.iter().map(|(t, v)| Key { t: *t, v: *v }).collect(),
                interp,
            },
        }
    }
}

/// Emission shape as authored.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ShapeDef {
    #[default]
    Point,
    Sphere {
        #[serde(default = "default_shape_radius")]
        radius: f32,
    },
    Cone {
        #[serde(default = "default_cone_radius")]
        radius: f32,
        #[serde(default = "default_shape_angle")]
        angle: f32,
    },
    Box {
        #[serde(default = "default_shape_extents")]
        extents: [f32; 3],
    },
}

fn default_shape_radius() -> f32 {
    0.5
}
fn default_cone_radius() -> f32 {
    0.0
}
fn default_shape_angle() -> f32 {
    30.0
}
fn default_shape_extents() -> [f32; 3] {
    [0.5, 0.5, 0.5]
}

/// `shape = "cone"` (legacy inline, dimensions in sibling keys) or a
/// `[emitters.shape]` table with `kind = "cone"`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum ShapeField {
    Named(String),
    Def(ShapeDef),
}

impl Default for ShapeField {
    fn default() -> Self {
        ShapeField::Def(ShapeDef::Point)
    }
}

/// A force as authored (`kind = "noise"` etc.).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ForceDef {
    Wind {
        velocity: [f32; 3],
        #[serde(default = "default_one")]
        strength: f32,
    },
    Drag {
        coefficient: f32,
    },
    Noise {
        #[serde(default = "default_one")]
        strength: f32,
        #[serde(default = "default_one")]
        frequency: f32,
        #[serde(default = "default_half")]
        speed: f32,
        #[serde(default = "default_octaves")]
        octaves: u32,
    },
    Vortex {
        #[serde(default)]
        center: [f32; 3],
        #[serde(default = "default_up")]
        axis: [f32; 3],
        #[serde(default = "default_one")]
        strength: f32,
        #[serde(default)]
        falloff: f32,
    },
    Attractor {
        #[serde(default)]
        position: [f32; 3],
        #[serde(default = "default_one")]
        strength: f32,
        #[serde(default)]
        radius: f32,
    },
}

fn default_one() -> f32 {
    1.0
}
fn default_half() -> f32 {
    0.5
}
fn default_octaves() -> u32 {
    1
}
fn default_up() -> [f32; 3] {
    [0.0, 1.0, 0.0]
}

impl ForceDef {
    pub fn resolve(&self) -> Force {
        match *self {
            ForceDef::Wind { velocity, strength } => Force::Wind {
                velocity,
                strength: strength.max(0.0),
            },
            ForceDef::Drag { coefficient } => Force::Drag {
                coefficient: coefficient.max(0.0),
            },
            ForceDef::Noise {
                strength,
                frequency,
                speed,
                octaves,
            } => Force::Noise {
                strength,
                frequency: frequency.max(1e-4),
                speed,
                octaves: octaves.clamp(1, 6),
            },
            ForceDef::Vortex {
                center,
                axis,
                strength,
                falloff,
            } => Force::Vortex {
                center,
                axis,
                strength,
                falloff: falloff.max(0.0),
            },
            ForceDef::Attractor {
                position,
                strength,
                radius,
            } => Force::Attractor {
                position,
                strength,
                radius: radius.max(0.0),
            },
        }
    }
}

/// One entry in an emitter's burst timeline.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct BurstDef {
    #[serde(default)]
    pub time: f32,
    pub count: CountDef,
    /// 0 = repeat forever.
    #[serde(default = "default_cycles")]
    pub cycles: u32,
    #[serde(default)]
    pub interval: f32,
    #[serde(default = "default_one")]
    pub probability: f32,
}

fn default_cycles() -> u32 {
    1
}

impl BurstDef {
    pub fn resolve(&self) -> Burst {
        let (count_min, count_max) = self.count.pair();
        Burst {
            time: self.time.max(0.0),
            count_min,
            count_max,
            cycles: self.cycles,
            interval: self.interval.max(0.0),
            probability: self.probability.clamp(0.0, 1.0),
        }
    }
}

/// Spawn into a sibling emitter on birth or death.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SubEmitterDef {
    pub emitter: String,
    #[serde(default = "default_count_one")]
    pub count: CountDef,
    #[serde(default)]
    pub inherit_velocity: f32,
}

fn default_count_one() -> CountDef {
    CountDef::Const(1)
}

impl SubEmitterDef {
    pub fn resolve(&self) -> SubEmitter {
        let (count_min, count_max) = self.count.pair();
        SubEmitter {
            emitter: self.emitter.clone(),
            count_min,
            count_max,
            inherit_velocity: self.inherit_velocity,
        }
    }
}

/// Reserved for the ribbon/trail pipeline; parsed so assets stay valid,
/// currently ignored with a warning.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct TrailDef {
    #[serde(default)]
    pub length: f32,
    #[serde(default)]
    pub width: f32,
}

/// Which authoring path an [`EmitterDef`] came from; changes a few defaults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolveContext {
    /// Asset files rotate shape axes with the emitter by default.
    pub asset: bool,
}

impl ResolveContext {
    pub fn asset() -> Self {
        Self { asset: true }
    }
    pub fn inline() -> Self {
        Self { asset: false }
    }
}

/// One thing wrong with an emitter as authored, at the key it is under
/// (#690). Displays as `key: message (hint)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmitterProblem {
    /// The key, `bursts[1]` for an entry of a list; `None` when serde
    /// couldn't say.
    pub key: Option<String>,
    pub message: String,
    /// The key is not one the emitter reads (a typo, mostly): the emitter
    /// still plays, without it.
    pub unknown: bool,
    /// What to write instead: "did you mean `emission_rate`?", the forms a
    /// value can take.
    pub hint: Option<String>,
}

impl EmitterProblem {
    /// A value at `key` that can't be used.
    pub fn at(key: &str, message: String) -> Self {
        Self {
            key: Some(key.to_string()),
            message,
            unknown: false,
            hint: None,
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// From what serde said of a table that didn't deserialise:
    /// "data did not match any variant of untagged enum CurveDef\nin `color`".
    /// The key comes from the `in` line, and an untagged enum's mismatch,
    /// which names a Rust type, is said as the forms the key takes.
    fn from_serde(said: &str) -> Self {
        let mut lines = said.lines().map(str::trim).filter(|l| !l.is_empty());
        let first = lines.next().unwrap_or(said).to_string();
        let key = lines
            .find_map(|l| l.strip_prefix("in `"))
            .and_then(|rest| rest.strip_suffix('`'))
            .map(str::to_string);
        let forms = [
            ("RangeDef", "a number or `[min, max]`"),
            ("CountDef", "a whole number or `[min, max]`"),
            (
                "CurveDef",
                "a value, `{ start = .., end = .. }` or `{ keys = [{ t = .., v = .. }, ..] }`",
            ),
            (
                "ShapeField",
                "\"point\", \"sphere\", \"cone\" or \"box\", or a table with `kind`",
            ),
        ];
        let untagged = forms
            .iter()
            .find(|(ty, _)| first.ends_with(&format!("untagged enum {ty}")));
        let (message, hint) = match untagged {
            Some((_, forms)) => (
                "is not a form this key takes".to_string(),
                Some(format!("expected {forms}")),
            ),
            None => (first, None),
        };
        Self {
            key,
            message,
            unknown: false,
            hint,
        }
    }
}

impl std::fmt::Display for EmitterProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(key) = &self.key {
            write!(f, "{key}: ")?;
        }
        write!(f, "{}", self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, " ({hint})")?;
        }
        Ok(())
    }
}

/// The field names serde's derive gives a struct, so a list of known keys
/// is the struct's own and can't drift from it (#690). They are read by
/// asking the type to deserialise from a deserializer that records the
/// names `deserialize_struct` is handed and then stops.
mod field_names {
    use serde::de::{self, Deserialize, Deserializer, Visitor};
    use std::fmt;

    #[derive(Debug)]
    struct Stop;

    impl fmt::Display for Stop {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("field names read")
        }
    }

    impl std::error::Error for Stop {}

    impl de::Error for Stop {
        fn custom<T: fmt::Display>(_: T) -> Self {
            Stop
        }
    }

    struct Names<'a>(&'a mut &'static [&'static str]);

    impl<'de> Deserializer<'de> for Names<'_> {
        type Error = Stop;

        fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, Stop> {
            Err(Stop)
        }

        fn deserialize_struct<V: Visitor<'de>>(
            self,
            _name: &'static str,
            fields: &'static [&'static str],
            _visitor: V,
        ) -> Result<V::Value, Stop> {
            *self.0 = fields;
            Err(Stop)
        }

        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
            bytes byte_buf option unit unit_struct newtype_struct seq tuple
            tuple_struct map enum identifier ignored_any
        }
    }

    /// `T`'s field names; empty for a type that isn't a struct.
    pub fn of<T: for<'de> Deserialize<'de>>() -> &'static [&'static str] {
        let mut fields: &'static [&'static str] = &[];
        let _ = T::deserialize(Names(&mut fields));
        fields
    }
}

/// An emitter as authored (asset or inline component).
///
/// Every field has a default so partial tables load. Legacy inline keys
/// (`lifetime_min`/`lifetime_max`, `speed_min`/`speed_max`, scalar
/// `size_start`/`size_end`, `color_start`/`color_end`, `burst_count`,
/// `shape = "cone"` + `shape_radius`/`shape_angle`/`shape_extents`) are
/// accepted and folded into the modern fields by [`resolve`](Self::resolve).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct EmitterDef {
    pub name: String,

    // --- Emission ---
    pub emission_rate: f32,
    pub emission_per_meter: f32,
    pub max_particles: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifetime: Option<RangeDef>,
    pub duration: f32,
    pub looping: bool,
    pub start_delay: f32,
    pub autoplay: bool,
    pub playing: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bursts: Vec<BurstDef>,

    // --- Shape ---
    pub shape: ShapeField,
    pub shape_offset: [f32; 3],
    pub shape_axis_u: [f32; 3],
    pub shape_axis_v: [f32; 3],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_axes: Option<bool>,

    // --- Motion ---
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<RangeDef>,
    pub direction: [f32; 3],
    pub spread: f32,
    pub gravity: [f32; 3],
    pub damping: f32,
    pub inherit_velocity: f32,
    pub world_space: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub forces: Vec<ForceDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_curve: Option<CurveDef<f32>>,

    // --- Over lifetime ---
    /// Absolute per-axis size over life.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<CurveDef<[f32; 2]>>,
    /// Random per-particle size multiplier.
    pub size_scale: RangeDef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<CurveDef<[f32; 4]>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpha: Option<CurveDef<f32>>,
    /// Random per-particle RGB multiplier.
    pub brightness: RangeDef,
    /// Initial rotation in degrees.
    pub rotation: RangeDef,
    /// Spin in degrees per second.
    pub angular_velocity: RangeDef,

    // --- Rendering ---
    pub texture: String,
    pub frames_x: u32,
    pub frames_y: u32,
    pub animate_frames: bool,
    pub frame_rate: f32,
    pub random_start_frame: bool,
    pub blend_mode: ParticleBlendMode,
    pub sort: SortMode,
    pub soft_distance: f32,
    pub fade_near: f32,
    pub fade_far: f32,
    pub lighting: f32,
    pub fog: bool,
    pub stretch: f32,

    // --- Sub-emitters ---
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_death: Option<SubEmitterDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_birth: Option<SubEmitterDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trail: Option<TrailDef>,

    // --- Legacy inline keys (folded in `resolve`) ---
    #[serde(skip_serializing_if = "Option::is_none")]
    pub burst_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifetime_min: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifetime_max: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_min: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_max: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_start: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_end: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_start: Option<[f32; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_end: Option<[f32; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape_radius: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape_angle: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape_extents: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation_min: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation_max: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub angular_velocity_min: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub angular_velocity_max: Option<f32>,
}

impl Default for EmitterDef {
    fn default() -> Self {
        Self {
            name: String::new(),
            emission_rate: 10.0,
            emission_per_meter: 0.0,
            max_particles: 256,
            lifetime: None,
            duration: 0.0,
            looping: true,
            start_delay: 0.0,
            autoplay: true,
            playing: false,
            bursts: Vec::new(),
            shape: ShapeField::default(),
            shape_offset: [0.0; 3],
            shape_axis_u: [0.0; 3],
            shape_axis_v: [0.0; 3],
            local_axes: None,
            speed: None,
            direction: [0.0, 1.0, 0.0],
            spread: 15.0,
            gravity: [0.0, -9.81, 0.0],
            damping: 0.0,
            inherit_velocity: 0.0,
            world_space: true,
            forces: Vec::new(),
            speed_curve: None,
            size: None,
            size_scale: RangeDef::Const(1.0),
            color: None,
            alpha: None,
            brightness: RangeDef::Const(1.0),
            rotation: RangeDef::MinMax([0.0, 360.0]),
            angular_velocity: RangeDef::Const(0.0),
            texture: String::new(),
            frames_x: 1,
            frames_y: 1,
            animate_frames: false,
            frame_rate: 0.0,
            random_start_frame: false,
            blend_mode: ParticleBlendMode::Alpha,
            sort: SortMode::None,
            soft_distance: 0.0,
            fade_near: 0.0,
            fade_far: 0.0,
            lighting: 0.0,
            fog: false,
            stretch: 0.0,
            on_death: None,
            on_birth: None,
            trail: None,
            burst_count: None,
            lifetime_min: None,
            lifetime_max: None,
            speed_min: None,
            speed_max: None,
            size_start: None,
            size_end: None,
            color_start: None,
            color_end: None,
            shape_radius: None,
            shape_angle: None,
            shape_extents: None,
            rotation_min: None,
            rotation_max: None,
            angular_velocity_min: None,
            angular_velocity_max: None,
        }
    }
}

/// Hard cap on one emitter's pool.
pub const MAX_PARTICLES_PER_EMITTER: u32 = 10_000;

impl EmitterDef {
    /// Every key `EmitterDef` understands: the field names serde's derive
    /// gives the struct, so a new field is a known key the moment it exists
    /// (#690). Asset loading rejects anything else; `flint validate` and
    /// `flint check` report it on an inline component, and the runtime warns.
    pub fn known_keys() -> &'static [&'static str] {
        field_names::of::<EmitterDef>()
    }

    /// Parse an inline `particle_emitter` component table. Unknown keys are
    /// ignored here (see [`problems`](Self::problems)); a value of the wrong
    /// form is an error naming its key.
    pub fn from_component(table: &toml::value::Table) -> Result<Self, EmitterProblem> {
        toml::Value::Table(table.clone())
            .try_into::<EmitterDef>()
            .map_err(|e| EmitterProblem::from_serde(&e.to_string()))
    }

    /// Keys in `table` that `EmitterDef` does not understand.
    pub fn unknown_keys(table: &toml::value::Table) -> Vec<String> {
        let known = Self::known_keys();
        table
            .keys()
            .filter(|k| !known.contains(&k.as_str()))
            .cloned()
            .collect()
    }

    /// [`unknown_keys`](Self::unknown_keys), each as a problem with the
    /// nearest known key as its hint.
    pub fn unknown_key_problems(table: &toml::value::Table) -> Vec<EmitterProblem> {
        let known = Self::known_keys();
        Self::unknown_keys(table)
            .into_iter()
            .map(|key| EmitterProblem {
                message: "not a key particle_emitter reads".to_string(),
                hint: flint_core::diagnostic::did_you_mean(&key, known.iter().copied()),
                key: Some(key),
                unknown: true,
            })
            .collect()
    }

    /// Everything wrong with an inline `particle_emitter` table, each at
    /// its key: every key the emitter doesn't read, then the first value
    /// that doesn't parse or resolve. Empty when the table plays as written.
    pub fn problems(table: &toml::value::Table) -> Vec<EmitterProblem> {
        let mut found = Self::unknown_key_problems(table);
        match Self::from_component(table) {
            Err(problem) => found.push(problem),
            Ok(def) => {
                if let Err(problem) = def.resolve_keyed(ResolveContext::inline()) {
                    found.push(problem);
                }
            }
        }
        found
    }

    /// "emitter 'flames'", or "emitter" for one with no name.
    fn who(&self) -> String {
        if self.name.is_empty() {
            "emitter".to_string()
        } else {
            format!("emitter '{}'", self.name)
        }
    }

    /// Build the runtime configuration. Validates ranges and curves; the
    /// error names the emitter and the key so asset load failures are
    /// actionable.
    pub fn resolve(&self, ctx: ResolveContext) -> Result<EmitterConfig, String> {
        self.resolve_keyed(ctx)
            .map_err(|problem| format!("{}: {problem}", self.who()))
    }

    /// [`resolve`](Self::resolve), with the problem at its key.
    pub fn resolve_keyed(&self, ctx: ResolveContext) -> Result<EmitterConfig, EmitterProblem> {
        let err = EmitterProblem::at;

        let (lifetime_min, lifetime_max) = match self.lifetime {
            Some(r) => r.pair(),
            None => {
                let d = EmitterConfig::default();
                (
                    self.lifetime_min.unwrap_or(d.lifetime_min),
                    self.lifetime_max.unwrap_or(d.lifetime_max),
                )
            }
        };
        let (lifetime_min, lifetime_max) = order(lifetime_min, lifetime_max);
        if lifetime_max <= 0.0 {
            let key = if self.lifetime.is_some() {
                "lifetime"
            } else {
                "lifetime_max"
            };
            return Err(err(key, "must be positive".into()));
        }

        let (speed_min, speed_max) = match self.speed {
            Some(r) => r.pair(),
            None => {
                let d = EmitterConfig::default();
                (
                    self.speed_min.unwrap_or(d.speed_min),
                    self.speed_max.unwrap_or(d.speed_max),
                )
            }
        };
        let (speed_min, speed_max) = order(speed_min, speed_max);

        let size = match &self.size {
            Some(c) => c.to_curve().map_err(|m| err("size", m))?,
            None => {
                let s = self.size_start.unwrap_or(0.1);
                let e = self.size_end.unwrap_or(0.0);
                Curve::start_end([s, s], [e, e])
            }
        };
        let color = match &self.color {
            Some(c) => c.to_curve().map_err(|m| err("color", m))?,
            None => Curve::start_end(
                self.color_start.unwrap_or([1.0, 1.0, 1.0, 1.0]),
                self.color_end.unwrap_or([1.0, 1.0, 1.0, 0.0]),
            ),
        };
        let alpha = match &self.alpha {
            Some(c) => Some(c.to_curve().map_err(|m| err("alpha", m))?),
            None => None,
        };
        let speed_curve = match &self.speed_curve {
            Some(c) => Some(c.to_curve().map_err(|m| err("speed_curve", m))?),
            None => None,
        };

        let shape = match &self.shape {
            ShapeField::Def(def) => shape_from_def(*def),
            ShapeField::Named(name) => {
                let radius = self.shape_radius.unwrap_or(0.5);
                let angle = self.shape_angle.unwrap_or(30.0);
                let extents = self.shape_extents.unwrap_or([0.5, 0.5, 0.5]);
                match name.as_str() {
                    "point" => EmissionShape::Point,
                    "sphere" => EmissionShape::Sphere { radius },
                    // Legacy cones spawned from a point; keep that unless a
                    // radius was authored explicitly.
                    "cone" => EmissionShape::Cone {
                        radius: self.shape_radius.unwrap_or(0.0),
                        angle,
                    },
                    "box" => EmissionShape::Box { extents },
                    other => {
                        return Err(err("shape", format!("unknown shape '{other}'"))
                            .with_hint("\"point\", \"sphere\", \"cone\" or \"box\""))
                    }
                }
            }
        };

        let mut bursts: Vec<Burst> = self.bursts.iter().map(BurstDef::resolve).collect();
        for (i, b) in bursts.iter().enumerate() {
            let key = format!("bursts[{i}]");
            if b.cycles != 1 && b.interval <= 0.0 && b.cycles != 0 {
                return Err(err(&key, "cycles > 1 needs a positive interval".into()));
            }
            if b.cycles == 0 && b.interval <= 0.0 {
                return Err(err(
                    &key,
                    "a repeating burst (cycles = 0) needs a positive interval".into(),
                ));
            }
        }
        if let Some(n) = self.burst_count {
            if n > 0 {
                bursts.insert(
                    0,
                    Burst {
                        time: 0.0,
                        count_min: n,
                        count_max: n,
                        cycles: 1,
                        interval: 0.0,
                        probability: 1.0,
                    },
                );
            }
        }

        let (rotation_min, rotation_max) = match (self.rotation_min, self.rotation_max) {
            (None, None) => self.rotation.pair(),
            (a, b) => order(a.unwrap_or(0.0), b.unwrap_or(360.0)),
        };
        let (av_min, av_max) = match (self.angular_velocity_min, self.angular_velocity_max) {
            (None, None) => self.angular_velocity.pair(),
            (a, b) => order(a.unwrap_or(0.0), b.unwrap_or(0.0)),
        };
        let deg = std::f32::consts::PI / 180.0;

        if let Some(sub) = &self.on_death {
            if sub.emitter == self.name {
                return Err(err("on_death", "cannot target itself".into()));
            }
        }
        if let Some(sub) = &self.on_birth {
            if sub.emitter == self.name {
                return Err(err("on_birth", "cannot target itself".into()));
            }
        }
        if self.trail.is_some() {
            tracing::warn!(
                "{}: `trail` is reserved and not implemented yet",
                self.who()
            );
        }

        Ok(EmitterConfig {
            name: self.name.clone(),
            emission_rate: self.emission_rate.max(0.0),
            emission_per_meter: self.emission_per_meter.max(0.0),
            max_particles: self.max_particles.clamp(1, MAX_PARTICLES_PER_EMITTER) as usize,
            lifetime_min,
            lifetime_max,
            duration: self.duration.max(0.0),
            looping: self.looping,
            start_delay: self.start_delay.max(0.0),
            autoplay: self.autoplay,
            playing: self.playing,
            bursts,
            shape,
            shape_offset: self.shape_offset,
            shape_axis_u: self.shape_axis_u,
            shape_axis_v: self.shape_axis_v,
            local_axes: self.local_axes.unwrap_or(ctx.asset),
            speed_min,
            speed_max,
            direction: self.direction,
            spread: self.spread.clamp(0.0, 180.0),
            gravity: self.gravity,
            damping: self.damping.max(0.0),
            inherit_velocity: self.inherit_velocity,
            world_space: self.world_space,
            forces: self.forces.iter().map(ForceDef::resolve).collect(),
            speed_curve,
            size,
            size_scale_min: self.size_scale.min(),
            size_scale_max: self.size_scale.max(),
            color,
            alpha,
            brightness_min: self.brightness.min(),
            brightness_max: self.brightness.max(),
            rotation_min: rotation_min * deg,
            rotation_max: rotation_max * deg,
            angular_velocity_min: av_min * deg,
            angular_velocity_max: av_max * deg,
            texture: self.texture.clone(),
            frames_x: self.frames_x.max(1),
            frames_y: self.frames_y.max(1),
            animate_frames: self.animate_frames,
            frame_rate: self.frame_rate.max(0.0),
            random_start_frame: self.random_start_frame,
            blend_mode: self.blend_mode,
            sort: self.sort,
            soft_distance: self.soft_distance.max(0.0),
            fade_near: self.fade_near.max(0.0),
            fade_far: self.fade_far.max(0.0),
            lighting: self.lighting.clamp(0.0, 1.0),
            fog: self.fog,
            stretch: self.stretch.max(0.0),
            on_death: self.on_death.as_ref().map(SubEmitterDef::resolve),
            on_birth: self.on_birth.as_ref().map(SubEmitterDef::resolve),
        })
    }

    /// Rebuild an authored definition from a runtime config, normalising
    /// legacy keys into the modern fields. Used by editors that load an
    /// inline component and save an asset.
    pub fn from_config(cfg: &EmitterConfig) -> Self {
        let deg = 180.0 / std::f32::consts::PI;
        let range = |a: f32, b: f32| {
            if (a - b).abs() < 1e-6 {
                RangeDef::Const(a)
            } else {
                RangeDef::MinMax([a, b])
            }
        };
        Self {
            name: cfg.name.clone(),
            emission_rate: cfg.emission_rate,
            emission_per_meter: cfg.emission_per_meter,
            max_particles: cfg.max_particles as u32,
            lifetime: Some(range(cfg.lifetime_min, cfg.lifetime_max)),
            duration: cfg.duration,
            looping: cfg.looping,
            start_delay: cfg.start_delay,
            autoplay: cfg.autoplay,
            playing: cfg.playing,
            bursts: cfg
                .bursts
                .iter()
                .map(|b| BurstDef {
                    time: b.time,
                    count: if b.count_min == b.count_max {
                        CountDef::Const(b.count_min)
                    } else {
                        CountDef::MinMax([b.count_min, b.count_max])
                    },
                    cycles: b.cycles,
                    interval: b.interval,
                    probability: b.probability,
                })
                .collect(),
            shape: ShapeField::Def(match cfg.shape {
                EmissionShape::Point => ShapeDef::Point,
                EmissionShape::Sphere { radius } => ShapeDef::Sphere { radius },
                EmissionShape::Cone { radius, angle } => ShapeDef::Cone { radius, angle },
                EmissionShape::Box { extents } => ShapeDef::Box { extents },
            }),
            shape_offset: cfg.shape_offset,
            shape_axis_u: cfg.shape_axis_u,
            shape_axis_v: cfg.shape_axis_v,
            local_axes: Some(cfg.local_axes),
            speed: Some(range(cfg.speed_min, cfg.speed_max)),
            direction: cfg.direction,
            spread: cfg.spread,
            gravity: cfg.gravity,
            damping: cfg.damping,
            inherit_velocity: cfg.inherit_velocity,
            world_space: cfg.world_space,
            forces: cfg.forces.iter().map(force_to_def).collect(),
            speed_curve: cfg.speed_curve.as_ref().map(CurveDef::from_curve),
            size: Some(CurveDef::from_curve(&cfg.size)),
            size_scale: range(cfg.size_scale_min, cfg.size_scale_max),
            color: Some(CurveDef::from_curve(&cfg.color)),
            alpha: cfg.alpha.as_ref().map(CurveDef::from_curve),
            brightness: range(cfg.brightness_min, cfg.brightness_max),
            rotation: range(cfg.rotation_min * deg, cfg.rotation_max * deg),
            angular_velocity: range(
                cfg.angular_velocity_min * deg,
                cfg.angular_velocity_max * deg,
            ),
            texture: cfg.texture.clone(),
            frames_x: cfg.frames_x,
            frames_y: cfg.frames_y,
            animate_frames: cfg.animate_frames,
            frame_rate: cfg.frame_rate,
            random_start_frame: cfg.random_start_frame,
            blend_mode: cfg.blend_mode,
            sort: cfg.sort,
            soft_distance: cfg.soft_distance,
            fade_near: cfg.fade_near,
            fade_far: cfg.fade_far,
            lighting: cfg.lighting,
            fog: cfg.fog,
            stretch: cfg.stretch,
            on_death: cfg.on_death.as_ref().map(sub_to_def),
            on_birth: cfg.on_birth.as_ref().map(sub_to_def),
            ..Default::default()
        }
    }
}

fn order(a: f32, b: f32) -> (f32, f32) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

fn shape_from_def(def: ShapeDef) -> EmissionShape {
    match def {
        ShapeDef::Point => EmissionShape::Point,
        ShapeDef::Sphere { radius } => EmissionShape::Sphere {
            radius: radius.max(0.0),
        },
        ShapeDef::Cone { radius, angle } => EmissionShape::Cone {
            radius: radius.max(0.0),
            angle: angle.clamp(0.0, 180.0),
        },
        ShapeDef::Box { extents } => EmissionShape::Box { extents },
    }
}

fn force_to_def(f: &Force) -> ForceDef {
    match *f {
        Force::Wind { velocity, strength } => ForceDef::Wind { velocity, strength },
        Force::Drag { coefficient } => ForceDef::Drag { coefficient },
        Force::Noise {
            strength,
            frequency,
            speed,
            octaves,
        } => ForceDef::Noise {
            strength,
            frequency,
            speed,
            octaves,
        },
        Force::Vortex {
            center,
            axis,
            strength,
            falloff,
        } => ForceDef::Vortex {
            center,
            axis,
            strength,
            falloff,
        },
        Force::Attractor {
            position,
            strength,
            radius,
        } => ForceDef::Attractor {
            position,
            strength,
            radius,
        },
    }
}

fn sub_to_def(s: &SubEmitter) -> SubEmitterDef {
    SubEmitterDef {
        emitter: s.emitter.clone(),
        count: if s.count_min == s.count_max {
            CountDef::Const(s.count_min)
        } else {
            CountDef::MinMax([s.count_min, s.count_max])
        },
        inherit_velocity: s.inherit_velocity,
    }
}

/// A named, reusable effect: one or more emitters simulated together.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct ParticleEffect {
    pub name: String,
    /// Base RNG seed (0 = derive from the owning entity).
    #[serde(default)]
    pub seed: u32,
    /// Optional alive-particle cap for this effect instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<u32>,
    #[serde(default)]
    pub emitters: Vec<EmitterDef>,
}

impl ParticleEffect {
    /// Parse from TOML text. Rejects unknown emitter keys and invalid
    /// values; `origin` labels errors (file name).
    pub fn from_toml_str(text: &str, origin: &str) -> Result<Self, String> {
        let value: toml::Value = toml::from_str(text).map_err(|e| format!("{origin}: {e}"))?;
        // Strict key check per emitter before the lenient serde pass.
        if let Some(emitters) = value.get("emitters").and_then(|v| v.as_array()) {
            for (i, em) in emitters.iter().enumerate() {
                if let Some(t) = em.as_table() {
                    let unknown = EmitterDef::unknown_keys(t);
                    if !unknown.is_empty() {
                        let name = t
                            .get("name")
                            .and_then(|v| v.as_str())
                            .map(|s| format!("'{s}'"))
                            .unwrap_or_else(|| format!("#{i}"));
                        return Err(format!(
                            "{origin}: emitter {name} has unknown key(s): {}",
                            unknown.join(", ")
                        ));
                    }
                }
            }
        }
        if let Some(t) = value.as_table() {
            for k in t.keys() {
                if !field_names::of::<ParticleEffect>().contains(&k.as_str()) {
                    return Err(format!("{origin}: unknown top-level key '{k}'"));
                }
            }
        }
        let effect: ParticleEffect = value.try_into().map_err(|e| format!("{origin}: {e}"))?;
        effect.validate().map_err(|e| format!("{origin}: {e}"))?;
        Ok(effect)
    }

    /// Serialise to TOML text (pretty, one `[[emitters]]` table per emitter).
    pub fn to_toml_string(&self) -> Result<String, String> {
        toml::to_string_pretty(self).map_err(|e| e.to_string())
    }

    /// Cross-emitter checks: names present and unique, sub-emitter targets
    /// exist, every emitter resolves.
    pub fn validate(&self) -> Result<(), String> {
        if self.emitters.is_empty() {
            return Err("effect has no emitters".into());
        }
        let mut seen = std::collections::HashSet::new();
        for (i, em) in self.emitters.iter().enumerate() {
            if em.name.is_empty() {
                return Err(format!("emitter #{i} has no name"));
            }
            if !seen.insert(em.name.as_str()) {
                return Err(format!("duplicate emitter name '{}'", em.name));
            }
        }
        for em in &self.emitters {
            em.resolve(ResolveContext::asset())?;
            for sub in [&em.on_death, &em.on_birth].into_iter().flatten() {
                if !seen.contains(sub.emitter.as_str()) {
                    return Err(format!(
                        "emitter '{}' targets unknown sub-emitter '{}'",
                        em.name, sub.emitter
                    ));
                }
            }
        }
        Ok(())
    }

    /// Resolve every emitter (asset defaults).
    pub fn resolve_all(&self) -> Result<Vec<EmitterConfig>, String> {
        self.emitters
            .iter()
            .map(|e| e.resolve(ResolveContext::asset()))
            .collect()
    }

    pub fn emitter_index(&self, name: &str) -> Option<usize> {
        self.emitters.iter().position(|e| e.name == name)
    }

    /// A single-emitter effect built from one authored emitter (editors
    /// opening an inline component; presets).
    pub fn single(name: &str, emitter: EmitterDef) -> Self {
        Self {
            name: name.to_string(),
            seed: 0,
            budget: None,
            emitters: vec![emitter],
        }
    }

    /// Longest emitter preview length — a default scrub range.
    pub fn preview_length(&self) -> f32 {
        self.resolve_all()
            .map(|cfgs| cfgs.iter().map(|c| c.preview_length()).fold(1.0, f32::max))
            .unwrap_or(2.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAMPFIRE: &str = r#"
name = "campfire"
seed = 7

[[emitters]]
name = "flames"
emission_rate = 40.0
max_particles = 200
lifetime = [0.3, 0.8]
speed = [1.5, 3.0]
gravity = [0, 2.0, 0]
blend_mode = "additive"
size = { keys = [ { t = 0.0, v = [1.0, 1.0] }, { t = 0.3, v = [1.3, 1.6] }, { t = 1.0, v = [0.1, 0.1] } ], interp = "smooth" }
color = { keys = [ { t = 0.0, v = [1.0, 0.7, 0.1, 1.0] }, { t = 1.0, v = [0.4, 0.0, 0.0, 0.0] } ] }
angular_velocity = [-60, 60]

[emitters.shape]
kind = "cone"
radius = 0.15
angle = 10.0

[[emitters.forces]]
kind = "noise"
strength = 1.2

[[emitters]]
name = "embers"
emission_rate = 0.0
lifetime = 1.5
speed = 1.0
size = { start = [0.03, 0.03], end = [0.0, 0.0] }
color = [1.0, 0.6, 0.2, 1.0]
on_death = { emitter = "puff", count = [1, 2] }

[[emitters.bursts]]
time = 0.0
count = [3, 6]
cycles = 0
interval = 1.5

[[emitters]]
name = "puff"
emission_rate = 0.0
lifetime = 0.4
"#;

    #[test]
    fn parses_all_three_curve_forms_and_shapes() {
        let fx = ParticleEffect::from_toml_str(CAMPFIRE, "test").unwrap();
        assert_eq!(fx.name, "campfire");
        assert_eq!(fx.emitters.len(), 3);
        let cfgs = fx.resolve_all().unwrap();
        let flames = &cfgs[0];
        assert_eq!(
            flames.shape,
            EmissionShape::Cone {
                radius: 0.15,
                angle: 10.0
            }
        );
        assert_eq!(flames.size.keys().len(), 3);
        assert_eq!(flames.size.interp(), Interp::Smooth);
        assert!(flames.local_axes, "assets default to local axes");
        assert!((flames.angular_velocity_max - 60.0f32.to_radians()).abs() < 1e-5);
        assert!(matches!(flames.forces[0], Force::Noise { .. }));
        let embers = &cfgs[1];
        assert!(embers.color.is_constant());
        assert_eq!(embers.bursts.len(), 1);
        assert_eq!(embers.bursts[0].cycles, 0);
        assert_eq!(embers.on_death.as_ref().unwrap().count_max, 2);
        assert_eq!(fx.emitter_index("puff"), Some(2));
    }

    #[test]
    fn round_trips_through_toml() {
        let fx = ParticleEffect::from_toml_str(CAMPFIRE, "test").unwrap();
        let text = fx.to_toml_string().unwrap();
        let back = ParticleEffect::from_toml_str(&text, "roundtrip").unwrap();
        assert_eq!(fx, back);
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let bad = "name = \"x\"\n[[emitters]]\nname = \"a\"\nemision_rate = 3.0\n";
        let err = ParticleEffect::from_toml_str(bad, "bad.toml").unwrap_err();
        assert!(err.contains("emision_rate"), "{err}");
        let bad_top = "name = \"x\"\nsead = 3\n[[emitters]]\nname = \"a\"\n";
        assert!(ParticleEffect::from_toml_str(bad_top, "t").is_err());
    }

    #[test]
    fn validation_catches_sub_emitter_problems() {
        let missing =
            "name = \"x\"\n[[emitters]]\nname = \"a\"\non_death = { emitter = \"nope\" }\n";
        let err = ParticleEffect::from_toml_str(missing, "t").unwrap_err();
        assert!(err.contains("nope"), "{err}");
        let selfref = "name = \"x\"\n[[emitters]]\nname = \"a\"\non_death = { emitter = \"a\" }\n";
        assert!(ParticleEffect::from_toml_str(selfref, "t").is_err());
        let dup = "name = \"x\"\n[[emitters]]\nname = \"a\"\n[[emitters]]\nname = \"a\"\n";
        assert!(ParticleEffect::from_toml_str(dup, "t").is_err());
        let unnamed = "name = \"x\"\n[[emitters]]\nemission_rate = 1.0\n";
        assert!(ParticleEffect::from_toml_str(unnamed, "t").is_err());
    }

    #[test]
    fn legacy_inline_table_matches_modern_equivalent() {
        let legacy: toml::value::Table = toml::from_str(
            r#"
emission_rate = 5
burst_count = 4
lifetime_min = 0.5
lifetime_max = 1.5
speed_min = 2
speed_max = 2
size_start = 0.2
size_end = 0.4
color_start = [1, 0, 0, 1]
color_end = [0, 0, 1, 0]
shape = "box"
shape_extents = [1, 2, 3]
"#,
        )
        .unwrap();
        let modern: toml::value::Table = toml::from_str(
            r#"
emission_rate = 5.0
lifetime = [0.5, 1.5]
speed = 2.0
size = { start = [0.2, 0.2], end = [0.4, 0.4] }
color = { start = [1.0, 0.0, 0.0, 1.0], end = [0.0, 0.0, 1.0, 0.0] }
shape = { kind = "box", extents = [1.0, 2.0, 3.0] }
[[bursts]]
time = 0.0
count = 4
"#,
        )
        .unwrap();
        let a = EmitterDef::from_component(&legacy)
            .unwrap()
            .resolve(ResolveContext::inline())
            .unwrap();
        let b = EmitterDef::from_component(&modern)
            .unwrap()
            .resolve(ResolveContext::inline())
            .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn inline_components_tolerate_unknown_keys() {
        let t: toml::value::Table = toml::from_str("emission_rate = 1.0\nfoo = 2\n").unwrap();
        assert!(EmitterDef::from_component(&t).is_ok());
        assert_eq!(EmitterDef::unknown_keys(&t), vec!["foo".to_string()]);
    }

    /// #690: each problem with an inline table is at its key, a typo with
    /// the key it was meant to be.
    #[test]
    fn inline_problems_name_the_key() {
        let problems = |src: &str| {
            let t: toml::value::Table = toml::from_str(src).unwrap();
            EmitterDef::problems(&t)
        };
        let only = |src: &str| {
            let found = problems(src);
            assert_eq!(found.len(), 1, "{src}: {found:?}");
            found.into_iter().next().unwrap()
        };

        assert!(problems("emission_rate = 5.0\nlifetime = [1, 2]\n").is_empty());

        let typo = only("emision_rate = 5.0\n");
        assert!(typo.unknown);
        assert_eq!(typo.key.as_deref(), Some("emision_rate"));
        assert_eq!(typo.hint.as_deref(), Some("did you mean `emission_rate`?"));

        let curve = only("color = \"red\"\n");
        assert!(!curve.unknown);
        assert_eq!(curve.key.as_deref(), Some("color"));
        assert!(curve.hint.as_deref().unwrap().contains("start"), "{curve}");

        assert_eq!(
            only("lifetime = [0.0, 0.0]\n").key.as_deref(),
            Some("lifetime")
        );
        assert_eq!(
            only("lifetime_max = 0.0\nlifetime_min = 0.0\n")
                .key
                .as_deref(),
            Some("lifetime_max")
        );
        assert_eq!(only("shape = \"donut\"\n").key.as_deref(), Some("shape"));
        assert_eq!(
            only("bursts = [{ count = 3 }, { count = 2, cycles = 3 }]\n")
                .key
                .as_deref(),
            Some("bursts[1]")
        );
        let missing = only("bursts = [{ time = 0.0 }]\n");
        assert_eq!(missing.key.as_deref(), Some("bursts"));
        assert!(missing.message.contains("count"), "{missing}");

        // A typo and a bad value: both, the typo first
        let both = problems("emision_rate = 5.0\nspeed = \"fast\"\n");
        let keys: Vec<_> = both.iter().filter_map(|p| p.key.as_deref()).collect();
        assert_eq!(keys, ["emision_rate", "speed"]);
        assert_eq!(
            both[1].to_string(),
            "speed: is not a form this key takes (expected a number or `[min, max]`)"
        );
    }

    /// #690: the known keys are the struct's fields, so they can't drift
    /// from it, and every field the component schema declares is one the
    /// emitter reads.
    #[test]
    fn known_keys_are_the_structs_fields() {
        let known = EmitterDef::known_keys();
        for key in [
            "name",
            "emission_rate",
            "lifetime",
            "trail",
            "angular_velocity_max",
        ] {
            assert!(known.contains(&key), "{key}");
        }
        // Everything a def serialises to, every option filled, is known
        let full = EmitterDef {
            lifetime: Some(RangeDef::Const(1.0)),
            speed: Some(RangeDef::Const(1.0)),
            bursts: vec![BurstDef {
                time: 0.0,
                count: CountDef::Const(1),
                cycles: 1,
                interval: 0.0,
                probability: 1.0,
            }],
            local_axes: Some(true),
            burst_count: Some(1),
            ..EmitterDef::from_config(&EmitterConfig::default())
        };
        let table = toml::Value::try_from(&full).unwrap();
        let table = table.as_table().unwrap();
        assert!(!table.is_empty());
        assert_eq!(EmitterDef::unknown_keys(table), Vec::<String>::new());
        assert_eq!(
            field_names::of::<ParticleEffect>(),
            ["name", "seed", "budget", "emitters"]
        );

        let schema = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../schemas/components/particle_emitter.toml"
        );
        #[allow(clippy::disallowed_methods)] // a test reads the engine's schema file
        let text = std::fs::read_to_string(schema).unwrap();
        let schema: toml::Value = toml::from_str(&text).unwrap();
        let fields = schema["component"]["particle_emitter"]["fields"]
            .as_table()
            .unwrap();
        let unread: Vec<_> = fields
            .keys()
            .filter(|k| !known.contains(&k.as_str()))
            .collect();
        assert!(
            unread.is_empty(),
            "the schema declares keys the emitter doesn't read: {unread:?}"
        );
        // ...and every key the emitter reads is declared, so a scene that
        // writes one loads without an `unknown-field` (#1051)
        let undeclared: Vec<_> = known.iter().filter(|k| !fields.contains_key(**k)).collect();
        assert!(
            undeclared.is_empty(),
            "the emitter reads keys the schema doesn't declare: {undeclared:?}"
        );
        // A field's `default` is written into a component that leaves it
        // out, so each must be what the reader takes with the key absent:
        // the table of every default resolves as the empty table does
        // (that a default never outvotes a modern key is
        // `tests/inline_component.rs`, through the real load)
        let defaults: toml::Table = fields
            .iter()
            .filter_map(|(k, f)| Some((k.clone(), f.get("default")?.clone())))
            .collect();
        let resolve = |t: &toml::Table| {
            EmitterDef::from_component(t)
                .unwrap()
                .resolve(ResolveContext::inline())
                .unwrap()
        };
        assert_eq!(resolve(&defaults), resolve(&toml::Table::new()));
        for key in [
            "rotation_min",
            "rotation_max",
            "angular_velocity_min",
            "angular_velocity_max",
            "shape_radius",
        ] {
            assert!(
                !defaults.contains_key(key),
                "{key} has a default, and the reader falls back on another key without it"
            );
        }
    }

    #[test]
    fn from_config_round_trips_resolve() {
        let fx = ParticleEffect::from_toml_str(CAMPFIRE, "test").unwrap();
        for def in &fx.emitters {
            let cfg = def.resolve(ResolveContext::asset()).unwrap();
            let back = EmitterDef::from_config(&cfg)
                .resolve(ResolveContext::asset())
                .unwrap();
            assert_eq!(cfg, back);
        }
    }

    #[test]
    fn repeating_burst_requires_interval() {
        let bad = "name = \"x\"\n[[emitters]]\nname = \"a\"\n[[emitters.bursts]]\ncount = 3\ncycles = 0\n";
        assert!(ParticleEffect::from_toml_str(bad, "t").is_err());
    }
}
