//! A parameter field (#747, ADR 0123): named numbers as one function of
//! position and time. Global values, with zones (spheres and boxes, each
//! with a soft falloff) laid over them in order, and every value able to head
//! for a new one over some seconds, so a script's change is a ramp and a
//! sample is still a pure function of time.
//!
//! What the numbers are is the field's [`FieldKeys`]: their names, how each
//! is checked, and any presets. The wooze field (`flint_wooze::WoozeField`) is
//! this with four keys, `amplitude`, `step`, `glide` and `jitter`. Its key set
//! is fixed when the field is made (`N`, at compile time), so a sample is an
//! array on the stack.
//!
//! A field a scene declares (#1014: `[field.weather] keys = { wind = .. }`)
//! is a [`SceneField`], whose keys are read from the file. It has the same
//! zones, ramps and messages, its values held in a list rather than an
//! array, and a script samples it one key at a time
//! ([`SceneField::sample_index`]), which allocates nothing either.
//!
//! A table that makes or changes a zone takes [`SHAPE_KEYS`] and the field's
//! own key names; a table of global values takes the key names and, when the
//! field has presets, `preset`.

use std::fmt;

use crate::toml_util::{toml_f32, toml_vec3};

mod scene;

pub use scene::{FieldIssue, SceneField, SceneFields, SceneKey, SceneZone, FIELD_KEYS, KEY_KEYS};

/// The keys every zone table takes before the field's own.
pub const SHAPE_KEYS: [&str; 6] = ["name", "shape", "center", "radius", "size", "falloff"];

/// A preset by name, as all `N` values, or the message for a name that isn't
/// one.
pub type PresetFn<const N: usize> = fn(&str) -> Result<[f32; N], String>;

/// What a field's numbers are.
pub struct FieldKeys<const N: usize> {
    /// The field's name in messages: `'fog' isn't a wooze knob`.
    pub field: &'static str,
    /// Each number's name, in the order a sample holds them.
    pub names: [&'static str; N],
    /// Check key `i`'s value, the same for the global table, a zone and a
    /// script: the value as it is, or why it can't be.
    pub check: fn(usize, f32) -> Result<f32, String>,
    /// A preset by name, as all `N` values, or the message for a name that
    /// isn't one. `None`: the field has no presets and takes no `preset` key.
    pub preset: Option<PresetFn<N>>,
}

impl<const N: usize> FieldKeys<N> {
    /// The index of the key called `name`.
    pub fn index(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|k| *k == name)
    }

    /// Check key `name`'s value, or say that there is no such key.
    pub fn check_named(&self, name: &str, value: f32) -> Result<f32, String> {
        let i = self.index(name).ok_or_else(|| {
            format!(
                "'{name}' isn't a {} knob; it takes {}",
                self.field,
                self.names.join(", ")
            )
        })?;
        (self.check)(i, value)
    }
}

impl<const N: usize> fmt::Debug for FieldKeys<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FieldKeys")
            .field("field", &self.field)
            .field("names", &self.names)
            .finish_non_exhaustive()
    }
}

/// What the zone readers need of a key set, compile-time or a scene's: its
/// keys' names and each key's check. One set of
/// readers serves both, so a zone is read and refused in the same words
/// whichever field it is in.
trait KeySet {
    fn len(&self) -> usize;
    fn name(&self, i: usize) -> &str;
    fn check(&self, i: usize, value: f32) -> Result<f32, String>;

    fn names(&self) -> Vec<&str> {
        (0..self.len()).map(|i| self.name(i)).collect()
    }
}

impl<const N: usize> KeySet for FieldKeys<N> {
    fn len(&self) -> usize {
        N
    }
    fn name(&self, i: usize) -> &str {
        self.names[i]
    }
    fn check(&self, i: usize, value: f32) -> Result<f32, String> {
        (self.check)(i, value)
    }
}

/// Key `i`'s value from TOML: a number its check takes.
fn read_value(keys: &(impl KeySet + ?Sized), i: usize, value: &toml::Value) -> Result<f32, String> {
    let number =
        toml_f32(value).ok_or_else(|| format!("{} = {value} must be a number", keys.name(i)))?;
    keys.check(i, number)
}

/// A value on its way from one number to another, so that a sample is a pure
/// function of time however a script has moved things. It eases in and out (a
/// smoothstep), and a new heading starts from wherever it stands, so a change
/// of mind never jumps. It is the engine's one ramp (#734,
/// [`crate::tween::Ramp`]), over `f32`; Living Props' knobs ease on it too
/// (#514).
pub use crate::tween::Ramp;

/// What a zone covers.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    /// A ball of `radius` metres around the centre.
    Sphere {
        /// Metres from the centre to the edge.
        radius: f32,
    },
    /// An axis-aligned box, `size` metres across on each axis.
    Box {
        /// Full width on each axis, in metres.
        size: [f32; 3],
    },
}

/// Where a zone is: its shape, its centre and how far past its edge it
/// fades.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Place {
    shape: Shape,
    center: [f32; 3],
    /// Metres past the edge over which the zone fades out. 0 is a hard edge.
    falloff: f32,
}

impl Place {
    /// How far inside it `position` is: 1 inside, 0 past the falloff.
    fn weight(&self, position: [f32; 3]) -> f32 {
        let d = |axis: usize| position[axis] - self.center[axis];
        let past_edge = match self.shape {
            Shape::Sphere { radius } => (d(0) * d(0) + d(1) * d(1) + d(2) * d(2)).sqrt() - radius,
            Shape::Box { size } => {
                let out = |axis: usize| (d(axis).abs() - size[axis] * 0.5).max(0.0);
                (out(0) * out(0) + out(1) * out(1) + out(2) * out(2)).sqrt()
            }
        };
        if past_edge <= 0.0 {
            1.0
        } else if past_edge >= self.falloff {
            0.0
        } else {
            let x = 1.0 - past_edge / self.falloff;
            x * x * (3.0 - 2.0 * x)
        }
    }
}

/// A zone's values, one slot a key, `None` passing through what's below:
/// an array for a compile-time key set, a list for a scene's.
pub trait Slots:
    Clone + PartialEq + fmt::Debug + AsRef<[Option<Ramp>]> + AsMut<[Option<Ramp>]>
{
}

impl<T> Slots for T where
    T: Clone + PartialEq + fmt::Debug + AsRef<[Option<Ramp>]> + AsMut<[Option<Ramp>]>
{
}

/// One part of the world where some of a field's numbers differ, its values
/// held in `S` ([`Zone`] for a [`ParamField`], [`SceneZone`] for a
/// [`SceneField`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneOf<S> {
    name: String,
    place: Place,
    /// The values it sets; one left `None` passes through what's below.
    knobs: S,
    /// How much of itself it applies, so it can fade in and out whole.
    strength: Ramp,
}

/// A zone of a [`ParamField<N>`].
pub type Zone<const N: usize> = ZoneOf<[Option<Ramp>; N]>;

impl<S: Slots> ZoneOf<S> {
    /// Its name, which is how a script finds it. Unnamed ones get `zone <n>`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// How much of itself it gives at `position` and `time`, 0 to 1.
    fn share(&self, position: [f32; 3], time: f64) -> f32 {
        self.place.weight(position) * self.strength.at(time)
    }
}

/// The zone-list work every field shares: adding, changing and removing a
/// zone by its table, and dropping the ones that have faded out. `empty` is
/// a zone's values with nothing set.
struct ZoneList<'a, K: ?Sized, S> {
    keys: &'a K,
    base: &'a [Ramp],
    zones: &'a mut Vec<ZoneOf<S>>,
    empty: S,
}

impl<K: KeySet + ?Sized, S: Slots> ZoneList<'_, K, S> {
    fn add(self, table: &toml::Table) -> Result<(), String> {
        let name = match table.get("name") {
            None => format!("zone {}", self.zones.len() + 1),
            Some(toml::Value::String(name)) if !name.is_empty() => name.clone(),
            Some(other) => return Err(format!("name = {other} must be a name")),
        };
        if self.zones.iter().any(|zone| zone.name == name) {
            return Err("its name is already taken by another zone".to_string());
        }
        let zone = read_zone(
            self.keys, name, table, None, self.empty, self.base, 0.0, 0.0,
        )?;
        self.zones.push(zone);
        Ok(())
    }

    fn set(self, name: &str, table: &toml::Table, time: f64, seconds: f64) -> Result<(), String> {
        if name.is_empty() {
            return Err("a zone needs a name".to_string());
        }
        settle(self.zones, time);
        let found = self.zones.iter().position(|zone| zone.name == name);
        let old = found.map(|i| &self.zones[i]);
        // A new zone is all there and fades in whole, so only an edit ramps values.
        let knob_seconds = if old.is_some() { seconds } else { 0.0 };
        let mut zone = read_zone(
            self.keys,
            name.to_string(),
            table,
            old,
            self.empty,
            self.base,
            time,
            knob_seconds,
        )?;
        match found {
            Some(i) => {
                zone.strength.head_for(1.0, time, seconds);
                self.zones[i] = zone;
            }
            None => {
                zone.strength = Ramp::constant(0.0);
                zone.strength.head_for(1.0, time, seconds);
                self.zones.push(zone);
            }
        }
        Ok(())
    }
}

/// Fade the named zone out over `seconds` and drop it. Returns whether there
/// was one.
fn remove_zone<S: Slots>(zones: &mut Vec<ZoneOf<S>>, name: &str, time: f64, seconds: f64) -> bool {
    settle(zones, time);
    let Some(i) = zones.iter().position(|zone| zone.name == name) else {
        return false;
    };
    if seconds > 0.0 {
        zones[i].strength.head_for(0.0, time, seconds);
    } else {
        zones.remove(i);
    }
    true
}

/// Drop the zones that have finished fading out.
fn settle<S: Slots>(zones: &mut Vec<ZoneOf<S>>, time: f64) {
    zones.retain(|zone| *zone.strength.target() > 0.0 || !zone.strength.arrived(time));
}

/// The zones that reach `position` at `time`, in the order they apply, each
/// with how much of itself it gives there, 0 to 1.
fn zones_in<S: Slots>(zones: &[ZoneOf<S>], position: [f32; 3], time: f64) -> Vec<(&str, f32)> {
    zones
        .iter()
        .filter_map(|zone| {
            let weight = zone.share(position, time);
            (weight > 0.0).then_some((zone.name.as_str(), weight))
        })
        .collect()
}

/// The whole field: global values and zones, over the key set `keys`.
#[derive(Debug, Clone)]
pub struct ParamField<const N: usize> {
    keys: &'static FieldKeys<N>,
    base: [Ramp; N],
    zones: Vec<Zone<N>>,
}

impl<const N: usize> PartialEq for ParamField<N> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.keys, other.keys) && self.base == other.base && self.zones == other.zones
    }
}

impl<const N: usize> ParamField<N> {
    /// A field with these global values everywhere and no zones. The values
    /// are taken as they are: check them first if they came from outside.
    pub fn new(keys: &'static FieldKeys<N>, base: [f32; N]) -> Self {
        Self {
            keys,
            base: base.map(Ramp::constant),
            zones: Vec::new(),
        }
    }

    /// What its numbers are.
    pub fn keys(&self) -> &'static FieldKeys<N> {
        self.keys
    }

    /// The zones, in the order they apply.
    pub fn zones(&self) -> &[Zone<N>] {
        &self.zones
    }

    /// The global values at `time`, before any zone.
    pub fn global(&self, time: f64) -> [f32; N] {
        self.base.map(|ramp| ramp.at(time))
    }

    /// Where each global value is heading: the end of the ramp `set_global`
    /// started, or the value itself when none is running.
    pub fn global_target(&self) -> [f32; N] {
        self.base.map(|ramp| *ramp.target())
    }

    /// Every value at `position` and `time`, in whichever clock the caller
    /// keeps this field on. A pure function: the same arguments give the same
    /// bits, and it allocates nothing.
    pub fn sample(&self, position: [f32; 3], time: f64) -> [f32; N] {
        let mut value = self.base.map(|ramp| ramp.at(time));
        for zone in &self.zones {
            let weight = zone.share(position, time);
            if weight <= 0.0 {
                continue;
            }
            for (out, knob) in value.iter_mut().zip(&zone.knobs) {
                if let Some(knob) = knob {
                    *out = lay(*out, knob.at(time), weight);
                }
            }
        }
        value
    }

    /// One value by name at `position` and `time`, or `None` for a name the
    /// field doesn't have: the form a script asks in.
    pub fn sample_key(&self, key: &str, position: [f32; 3], time: f64) -> Option<f32> {
        let i = self.keys.index(key)?;
        Some(self.sample(position, time)[i])
    }

    /// Which zone has the last word on each value at `position` and `time`:
    /// `None` where the global value stands. For saying where a value came
    /// from, not for every frame.
    pub fn zones_at(&self, position: [f32; 3], time: f64) -> [Option<&str>; N] {
        let mut said = [None; N];
        for zone in &self.zones {
            if zone.share(position, time) <= 0.0 {
                continue;
            }
            for (said, knob) in said.iter_mut().zip(&zone.knobs) {
                if knob.is_some() {
                    *said = Some(zone.name.as_str());
                }
            }
        }
        said
    }

    /// The zones that reach `position` at `time`, in the order they apply,
    /// each with how much of itself it gives there, 0 to 1.
    pub fn zones_in(&self, position: [f32; 3], time: f64) -> Vec<(&str, f32)> {
        zones_in(&self.zones, position, time)
    }

    /// Change global values from a table of any of the key names (and
    /// `preset`, when the field has presets), heading for them over `seconds`
    /// from `time`. A preset applies first and the others on top. Nothing
    /// changes on an error.
    pub fn set_global(
        &mut self,
        table: &toml::Table,
        time: f64,
        seconds: f64,
    ) -> Result<(), String> {
        let keys = self.keys;
        let mut targets: [Option<f32>; N] = [None; N];
        for (key, value) in table {
            if key == "preset" && keys.preset.is_some() {
                continue;
            }
            let Some(i) = keys.index(key) else {
                let preset = if keys.preset.is_some() {
                    "preset, "
                } else {
                    ""
                };
                return Err(format!(
                    "'{key}' isn't a {} knob; it takes {preset}{}",
                    keys.field,
                    keys.names.join(", ")
                ));
            };
            targets[i] = Some(read_value(keys, i, value)?);
        }
        if let (Some(value), Some(preset)) = (table.get("preset"), keys.preset) {
            let name = value
                .as_str()
                .ok_or_else(|| format!("preset = {value} must be a name"))?;
            for (target, value) in targets.iter_mut().zip(preset(name)?) {
                target.get_or_insert(value);
            }
        }
        for (ramp, target) in self.base.iter_mut().zip(targets) {
            if let Some(target) = target {
                ramp.head_for(target, time, seconds);
            }
        }
        Ok(())
    }

    /// Add a zone from its table, or an error saying what's wrong with it. An
    /// unnamed one takes a name from its place in the list.
    pub fn add_zone(&mut self, table: &toml::Table) -> Result<(), String> {
        self.list().add(table)
    }

    /// Change the named zone from a table, or add it if there's none yet,
    /// either way over `seconds` from `time`. A new zone fades in whole; an
    /// existing one keeps what the table leaves out, and its shape and place
    /// move at once. Nothing changes on an error.
    pub fn set_zone(
        &mut self,
        name: &str,
        table: &toml::Table,
        time: f64,
        seconds: f64,
    ) -> Result<(), String> {
        self.list().set(name, table, time, seconds)
    }

    /// Fade the named zone out over `seconds` and drop it. Returns whether
    /// there was one.
    pub fn remove_zone(&mut self, name: &str, time: f64, seconds: f64) -> bool {
        remove_zone(&mut self.zones, name, time, seconds)
    }

    fn list(&mut self) -> ZoneList<'_, FieldKeys<N>, [Option<Ramp>; N]> {
        ZoneList {
            keys: self.keys,
            base: &self.base,
            zones: &mut self.zones,
            empty: [None; N],
        }
    }
}

/// `below` with a zone's `target` laid over it at `weight`: exactly the
/// target inside, so a zone's value is that value to the bit.
fn lay(below: f32, target: f32, weight: f32) -> f32 {
    if weight >= 1.0 {
        target
    } else {
        below + (target - below) * weight
    }
}

/// A zone from its table, with `old` filling in anything left out. A value
/// that's new starts from the global value, so a zone's new value eases in
/// from what that place was, save where zones overlap.
#[allow(clippy::too_many_arguments)]
fn read_zone<K: KeySet + ?Sized, S: Slots>(
    keys: &K,
    name: String,
    table: &toml::Table,
    old: Option<&ZoneOf<S>>,
    empty: S,
    base: &[Ramp],
    time: f64,
    seconds: f64,
) -> Result<ZoneOf<S>, String> {
    for key in table.keys() {
        let known =
            SHAPE_KEYS.contains(&key.as_str()) || (0..keys.len()).any(|i| keys.name(i) == key);
        if !known {
            let mut all: Vec<&str> = SHAPE_KEYS.to_vec();
            all.extend(keys.names());
            return Err(format!(
                "'{key}' isn't a zone key; it takes {}",
                all.join(", ")
            ));
        }
    }

    let center = match table.get("center") {
        Some(value) => {
            read_point(value).ok_or_else(|| format!("center = {value} must be [x, y, z]"))?
        }
        None => old
            .map(|z| z.place.center)
            .ok_or("it needs a center: [x, y, z]")?,
    };
    let falloff = match table.get("falloff") {
        Some(value) => toml_f32(value)
            .filter(|f| f.is_finite() && *f >= 0.0)
            .ok_or_else(|| format!("falloff = {value} must be 0 metres or more"))?,
        None => old.map_or(0.0, |z| z.place.falloff),
    };
    let shape = read_shape(table, old.map(|z| z.place.shape))?;

    let mut knobs = old.map_or(empty, |z| z.knobs.clone());
    for (i, slot) in knobs.as_mut().iter_mut().enumerate() {
        if let Some(value) = table.get(keys.name(i)) {
            let target = read_value(keys, i, value)?;
            let mut ramp = slot.unwrap_or_else(|| Ramp::constant(base[i].at(time)));
            ramp.head_for(target, time, seconds);
            *slot = Some(ramp);
        }
    }
    if knobs.as_ref().iter().all(Option::is_none) {
        return Err(format!(
            "it changes nothing: it needs at least one of {}",
            keys.names().join(", ")
        ));
    }

    Ok(ZoneOf {
        name,
        place: Place {
            shape,
            center,
            falloff,
        },
        knobs,
        strength: old.map_or(Ramp::constant(1.0), |z| z.strength),
    })
}

fn read_shape(table: &toml::Table, old: Option<Shape>) -> Result<Shape, String> {
    let name = match table.get("shape") {
        Some(toml::Value::String(name)) => name.as_str(),
        Some(other) => return Err(format!("shape = {other} must be \"sphere\" or \"box\"")),
        None => match old {
            Some(Shape::Sphere { .. }) => "sphere",
            Some(Shape::Box { .. }) => "box",
            None => return Err("it needs a shape: \"sphere\" or \"box\"".to_string()),
        },
    };
    match name {
        "sphere" => {
            if table.contains_key("size") {
                return Err("a sphere takes radius, not size".to_string());
            }
            let radius = match (table.get("radius"), old) {
                (Some(value), _) => toml_f32(value)
                    .filter(|r| r.is_finite() && *r > 0.0)
                    .ok_or_else(|| format!("radius = {value} must be more than 0 metres"))?,
                (None, Some(Shape::Sphere { radius })) => radius,
                (None, _) => return Err("a sphere needs a radius".to_string()),
            };
            Ok(Shape::Sphere { radius })
        }
        "box" => {
            if table.contains_key("radius") {
                return Err("a box takes size, not radius".to_string());
            }
            let size = match (table.get("size"), old) {
                (Some(value), _) => read_point(value)
                    .filter(|s| s.iter().all(|a| a.is_finite() && *a > 0.0))
                    .ok_or_else(|| {
                        format!("size = {value} must be [x, y, z], each more than 0 metres")
                    })?,
                (None, Some(Shape::Box { size })) => size,
                (None, _) => return Err("a box needs a size: [x, y, z]".to_string()),
            };
            Ok(Shape::Box { size })
        }
        other => Err(format!("shape = \"{other}\" must be \"sphere\" or \"box\"")),
    }
}

/// Exactly three finite numbers, unlike [`toml_vec3`] alone, which takes more.
fn read_point(value: &toml::Value) -> Option<[f32; 3]> {
    if value.as_array()?.len() != 3 {
        return None;
    }
    toml_vec3(value).filter(|p| p.iter().all(|a| a.is_finite()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A field that isn't the wooze one: how windy and how foggy a place is.
    static WEATHER: FieldKeys<2> = FieldKeys {
        field: "weather",
        names: ["wind", "fog"],
        check: |i, v| {
            if v >= 0.0 && v.is_finite() {
                Ok(v)
            } else {
                Err(format!("{} = {v} must be 0 or more", ["wind", "fog"][i]))
            }
        },
        preset: None,
    };

    fn table(src: &str) -> toml::Table {
        toml::from_str(src).unwrap()
    }

    #[test]
    fn any_key_set_takes_zones_and_ramps() {
        let mut field = ParamField::new(&WEATHER, [1.0, 0.0]);
        field
            .add_zone(&table(
                "name = \"valley\"\nshape = \"sphere\"\ncenter = [0, 0, 0]\nradius = 5\nfog = 0.8",
            ))
            .unwrap();
        assert_eq!(field.sample([0.0; 3], 0.0), [1.0, 0.8]);
        assert_eq!(field.sample_key("fog", [20.0, 0.0, 0.0], 0.0), Some(0.0));
        assert_eq!(field.sample_key("rain", [0.0; 3], 0.0), None);
        assert_eq!(field.zones_at([0.0; 3], 0.0), [None, Some("valley")]);

        field
            .set_global(&table("wind = 3"), 10.0, 2.0)
            .expect("a known key");
        assert_eq!(field.global(10.0)[0], 1.0);
        assert_eq!(field.global(11.0)[0], 2.0, "half way, smoothstepped");
        assert_eq!(field.global(12.0)[0], 3.0);
    }

    #[test]
    fn a_field_without_presets_names_only_its_keys() {
        let mut field = ParamField::new(&WEATHER, [1.0, 0.0]);
        assert_eq!(
            field.set_global(&table("preset = \"storm\""), 0.0, 0.0),
            Err("'preset' isn't a weather knob; it takes wind, fog".to_string())
        );
        assert_eq!(
            field.add_zone(&table("shape = \"box\"\ncenter = [0, 0, 0]\nsize = [1, 1, 1]\nrain = 1")),
            Err(
                "'rain' isn't a zone key; it takes name, shape, center, radius, size, falloff, wind, fog"
                    .to_string()
            )
        );
        assert_eq!(
            field.add_zone(&table(
                "shape = \"box\"\ncenter = [0, 0, 0]\nsize = [1, 1, 1]\nfog = -1"
            )),
            Err("fog = -1 must be 0 or more".to_string())
        );
    }
}
