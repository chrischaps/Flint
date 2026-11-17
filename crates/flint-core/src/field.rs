//! A parameter field (#747, ADR 0123): named numbers as one function of
//! position and time. Global values, with zones (spheres and boxes, each
//! with a soft falloff) laid over them in order, and every value able to head
//! for a new one over some seconds, so a script's change is a ramp and a
//! sample is still a pure function of time.
//!
//! What the numbers are is the field's [`FieldKeys`]: their names, how each
//! is checked, and any presets. The wooze field (`flint_wooze::WoozeField`) is
//! this with four keys, `amplitude`, `step`, `glide` and `jitter`; a game's
//! fog, wind or time scale over a level would be another key set on the same
//! mechanics. The key set is fixed when a field is made (`N`, at compile
//! time), which is what keeps a sample free of allocation.
//!
//! A table that makes or changes a zone takes [`SHAPE_KEYS`] and the field's
//! own key names; a table of global values takes the key names and, when the
//! field has presets, `preset`.

use std::fmt;

use crate::toml_util::{toml_f32, toml_vec3};

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

    fn read(&self, i: usize, value: &toml::Value) -> Result<f32, String> {
        let number = toml_f32(value)
            .ok_or_else(|| format!("{} = {value} must be a number", self.names[i]))?;
        (self.check)(i, number)
    }

    fn is_zone_key(&self, key: &str) -> bool {
        SHAPE_KEYS.contains(&key) || self.names.contains(&key)
    }

    fn zone_keys(&self) -> String {
        let mut all: Vec<&str> = SHAPE_KEYS.to_vec();
        all.extend(self.names);
        all.join(", ")
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

/// One part of the world where some of the field's numbers differ.
#[derive(Debug, Clone, PartialEq)]
pub struct Zone<const N: usize> {
    name: String,
    shape: Shape,
    center: [f32; 3],
    /// Metres past the edge over which the zone fades out. 0 is a hard edge.
    falloff: f32,
    /// The values it sets; one left `None` passes through what's below.
    knobs: [Option<Ramp>; N],
    /// How much of itself it applies, so it can fade in and out whole.
    strength: Ramp,
}

impl<const N: usize> Zone<N> {
    /// Its name, which is how a script finds it. Unnamed ones get `zone <n>`.
    pub fn name(&self) -> &str {
        &self.name
    }

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
            let weight = zone.weight(position) * zone.strength.at(time);
            if weight <= 0.0 {
                continue;
            }
            for (out, knob) in value.iter_mut().zip(&zone.knobs) {
                if let Some(knob) = knob {
                    // Exact inside, so a zone's value is that value to the bit.
                    let target = knob.at(time);
                    *out = if weight >= 1.0 {
                        target
                    } else {
                        *out + (target - *out) * weight
                    };
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
            if zone.weight(position) * zone.strength.at(time) <= 0.0 {
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
        self.zones
            .iter()
            .filter_map(|zone| {
                let weight = zone.weight(position) * zone.strength.at(time);
                (weight > 0.0).then_some((zone.name.as_str(), weight))
            })
            .collect()
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
            targets[i] = Some(keys.read(i, value)?);
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
        let name = match table.get("name") {
            None => format!("zone {}", self.zones.len() + 1),
            Some(toml::Value::String(name)) if !name.is_empty() => name.clone(),
            Some(other) => return Err(format!("name = {other} must be a name")),
        };
        if self.zones.iter().any(|zone| zone.name == name) {
            return Err("its name is already taken by another zone".to_string());
        }
        let zone = read_zone(self.keys, name, table, None, &self.base, 0.0, 0.0)?;
        self.zones.push(zone);
        Ok(())
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
        if name.is_empty() {
            return Err("a zone needs a name".to_string());
        }
        self.settle(time);
        let found = self.zones.iter().position(|zone| zone.name == name);
        let old = found.map(|i| &self.zones[i]);
        // A new zone is all there and fades in whole, so only an edit ramps values.
        let knob_seconds = if old.is_some() { seconds } else { 0.0 };
        let mut zone = read_zone(
            self.keys,
            name.to_string(),
            table,
            old,
            &self.base,
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

    /// Fade the named zone out over `seconds` and drop it. Returns whether
    /// there was one.
    pub fn remove_zone(&mut self, name: &str, time: f64, seconds: f64) -> bool {
        self.settle(time);
        let Some(i) = self.zones.iter().position(|zone| zone.name == name) else {
            return false;
        };
        if seconds > 0.0 {
            self.zones[i].strength.head_for(0.0, time, seconds);
        } else {
            self.zones.remove(i);
        }
        true
    }

    /// Drop the zones that have finished fading out.
    fn settle(&mut self, time: f64) {
        self.zones
            .retain(|zone| *zone.strength.target() > 0.0 || !zone.strength.arrived(time));
    }
}

/// A zone from its table, with `old` filling in anything left out. A value
/// that's new starts from the global value, so a zone's new value eases in
/// from what that place was, save where zones overlap.
fn read_zone<const N: usize>(
    keys: &FieldKeys<N>,
    name: String,
    table: &toml::Table,
    old: Option<&Zone<N>>,
    base: &[Ramp; N],
    time: f64,
    seconds: f64,
) -> Result<Zone<N>, String> {
    for key in table.keys() {
        if !keys.is_zone_key(key) {
            return Err(format!(
                "'{key}' isn't a zone key; it takes {}",
                keys.zone_keys()
            ));
        }
    }

    let center = match table.get("center") {
        Some(value) => {
            read_point(value).ok_or_else(|| format!("center = {value} must be [x, y, z]"))?
        }
        None => old
            .map(|z| z.center)
            .ok_or("it needs a center: [x, y, z]")?,
    };
    let falloff = match table.get("falloff") {
        Some(value) => toml_f32(value)
            .filter(|f| f.is_finite() && *f >= 0.0)
            .ok_or_else(|| format!("falloff = {value} must be 0 metres or more"))?,
        None => old.map_or(0.0, |z| z.falloff),
    };
    let shape = read_shape(table, old.map(|z| z.shape))?;

    let mut knobs = old.map_or([None; N], |z| z.knobs);
    for (i, slot) in knobs.iter_mut().enumerate() {
        if let Some(value) = table.get(keys.names[i]) {
            let target = keys.read(i, value)?;
            let mut ramp = slot.unwrap_or_else(|| Ramp::constant(base[i].at(time)));
            ramp.head_for(target, time, seconds);
            *slot = Some(ramp);
        }
    }
    if knobs.iter().all(Option::is_none) {
        return Err(format!(
            "it changes nothing: it needs at least one of {}",
            keys.names.join(", ")
        ));
    }

    Ok(Zone {
        name,
        shape,
        center,
        falloff,
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
