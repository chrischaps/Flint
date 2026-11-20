//! Fields a scene declares (#1014, ADR 0123 decision 4): `[field.<name>]`
//! with its keys, each a number with an optional range and a default, and
//! `[[field.<name>.zone]]` entries written as `[[wooze.zone]]` ones are
//! ([`SHAPE_KEYS`] and the keys).
//!
//! ```toml
//! [field.weather]
//! keys = { wind = { min = 0, default = 1 }, fog = { min = 0, max = 1, default = 0 } }
//!
//! [[field.weather.zone]]
//! name = "storm"
//! shape = "sphere"
//! center = [20, 0, 0]
//! radius = 8
//! falloff = 4
//! wind = 6
//! ```
//!
//! The keys are known only once the file is read, so a [`SceneField`] holds
//! its values in lists where a [`ParamField`](super::ParamField) holds
//! arrays. The zones are the same zones, read by the same code, so every
//! message is the one the wooze field writes. A script samples one key at a
//! time ([`SceneField::sample_index`]): nothing is allocated, and the value is
//! the bits a `ParamField` with the same keys gives.

use std::fmt;

use super::{lay, read_value, KeySet, Ramp, ZoneList, ZoneOf, SHAPE_KEYS};
use crate::toml_util::toml_f32;

/// What a `[field.<name>]` table takes.
pub const FIELD_KEYS: [&str; 2] = ["keys", "zone"];

/// What one key's table in `keys = { .. }` takes.
pub const KEY_KEYS: [&str; 3] = ["min", "max", "default"];

/// A zone of a [`SceneField`].
pub type SceneZone = ZoneOf<Vec<Option<Ramp>>>;

/// One number a scene's field holds: its name, the range a value must lie in
/// and the global value it starts at.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneKey {
    pub name: String,
    pub min: Option<f32>,
    pub max: Option<f32>,
    /// The global value, before any zone or script.
    pub default: f32,
}

impl SceneKey {
    /// The value as it is, or why it is out of the key's range.
    pub fn check(&self, value: f32) -> Result<f32, String> {
        let name = &self.name;
        let fits = value.is_finite()
            && self.min.is_none_or(|min| value >= min)
            && self.max.is_none_or(|max| value <= max);
        if fits {
            return Ok(value);
        }
        Err(match (self.min, self.max) {
            (Some(min), Some(max)) => format!("{name} = {value} must be from {min} to {max}"),
            (Some(min), None) => format!("{name} = {value} must be {min} or more"),
            (None, Some(max)) => format!("{name} = {value} must be {max} or less"),
            (None, None) => format!("{name} = {value} must be a finite number"),
        })
    }
}

/// A field's name and keys, as its readers see them.
#[derive(Debug, Clone, PartialEq)]
struct Keys {
    field: String,
    keys: Vec<SceneKey>,
}

impl KeySet for Keys {
    fn len(&self) -> usize {
        self.keys.len()
    }
    fn name(&self, i: usize) -> &str {
        &self.keys[i].name
    }
    fn check(&self, i: usize, value: f32) -> Result<f32, String> {
        self.keys[i].check(value)
    }
}

/// A field a scene declared: its keys' global values and its zones.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneField {
    keys: Keys,
    base: Vec<Ramp>,
    zones: Vec<SceneZone>,
}

impl SceneField {
    /// A field called `name` with these keys, each at its default
    /// everywhere, and no zones. The keys are taken as they are:
    /// [`SceneFields::read`] checks a scene's.
    pub fn new(name: impl Into<String>, keys: Vec<SceneKey>) -> Self {
        let base = keys.iter().map(|key| Ramp::constant(key.default)).collect();
        Self {
            keys: Keys {
                field: name.into(),
                keys,
            },
            base,
            zones: Vec::new(),
        }
    }

    /// Its name: `weather` for `[field.weather]`.
    pub fn name(&self) -> &str {
        &self.keys.field
    }

    /// Its keys, in the order the file wrote them.
    pub fn keys(&self) -> &[SceneKey] {
        &self.keys.keys
    }

    /// The index of the key called `name`, for [`SceneField::sample_index`].
    pub fn index(&self, name: &str) -> Option<usize> {
        self.keys.keys.iter().position(|key| key.name == name)
    }

    /// The zones, in the order they apply.
    pub fn zones(&self) -> &[SceneZone] {
        &self.zones
    }

    /// Key `i`'s global value at `time`, before any zone.
    pub fn global(&self, i: usize, time: f64) -> f32 {
        self.base[i].at(time)
    }

    /// Key `i`'s value at `position` and `time`: a pure function that
    /// allocates nothing, and the same bits a `ParamField` with these keys
    /// and zones gives. Panics on an index the field doesn't have; ask
    /// [`SceneField::index`] first.
    pub fn sample_index(&self, i: usize, position: [f32; 3], time: f64) -> f32 {
        let mut value = self.base[i].at(time);
        for zone in &self.zones {
            let Some(knob) = &zone.knobs[i] else {
                continue;
            };
            let weight = zone.share(position, time);
            if weight > 0.0 {
                value = lay(value, knob.at(time), weight);
            }
        }
        value
    }

    /// One value by name at `position` and `time`, or `None` for a name the
    /// field doesn't have.
    pub fn sample_key(&self, key: &str, position: [f32; 3], time: f64) -> Option<f32> {
        Some(self.sample_index(self.index(key)?, position, time))
    }

    /// The zones that reach `position` at `time`, in the order they apply,
    /// each with how much of itself it gives there, 0 to 1.
    pub fn zones_in(&self, position: [f32; 3], time: f64) -> Vec<(&str, f32)> {
        super::zones_in(&self.zones, position, time)
    }

    /// Change global values from a table of any of the key names, heading
    /// for them over `seconds` from `time`. Nothing changes on an error.
    pub fn set_global(
        &mut self,
        table: &toml::Table,
        time: f64,
        seconds: f64,
    ) -> Result<(), String> {
        let mut targets = Vec::with_capacity(table.len());
        for (key, value) in table {
            let i = self.index(key).ok_or_else(|| {
                format!(
                    "'{key}' isn't a {} key; it takes {}",
                    self.keys.field,
                    self.keys.names().join(", ")
                )
            })?;
            targets.push((i, read_value(&self.keys, i, value)?));
        }
        for (i, target) in targets {
            self.base[i].head_for(target, time, seconds);
        }
        Ok(())
    }

    /// Add a zone from its table, or an error saying what's wrong with it. An
    /// unnamed one takes a name from its place in the list.
    pub fn add_zone(&mut self, table: &toml::Table) -> Result<(), String> {
        self.list().add(table)
    }

    /// Change the named zone from a table, or add it if there's none yet,
    /// either way over `seconds` from `time`, as
    /// [`ParamField::set_zone`](super::ParamField::set_zone) does.
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
        super::remove_zone(&mut self.zones, name, time, seconds)
    }

    fn list(&mut self) -> ZoneList<'_, Keys, Vec<Option<Ramp>>> {
        ZoneList {
            empty: vec![None; self.keys.len()],
            keys: &self.keys,
            base: &self.base,
            zones: &mut self.zones,
        }
    }
}

/// Something wrong with a scene's `[field]` table: which field, and which of
/// its keys or zones, when it is about one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldIssue {
    /// The field's name; empty for the `[field]` table itself.
    pub field: String,
    /// The key it's about, for a problem in `keys = { .. }`.
    pub key: Option<String>,
    /// The zone it's about, by name, or by its place in the list if it has none.
    pub zone: Option<String>,
    /// What's wrong.
    pub message: String,
}

impl FieldIssue {
    fn of(field: &str, message: impl Into<String>) -> Self {
        Self {
            field: field.to_string(),
            key: None,
            zone: None,
            message: message.into(),
        }
    }

    /// Where it is, as a diagnostic's dotted field: `weather`,
    /// `weather.keys.wind`, `weather.zone.storm`.
    pub fn place(&self) -> String {
        match (&self.key, &self.zone) {
            (Some(key), _) => format!("{}.keys.{key}", self.field),
            (None, Some(zone)) => format!("{}.zone.{zone}", self.field),
            (None, None) => self.field.clone(),
        }
    }
}

impl fmt::Display for FieldIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.field.is_empty() {
            return write!(f, "[field]: {}", self.message);
        }
        match (&self.key, &self.zone) {
            (Some(key), _) => write!(f, "[field.{}] key '{key}': {}", self.field, self.message),
            (None, Some(zone)) => {
                write!(f, "[field.{}] zone '{zone}': {}", self.field, self.message)
            }
            (None, None) => write!(f, "[field.{}]: {}", self.field, self.message),
        }
    }
}

/// Every field a scene declares, by name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SceneFields {
    fields: Vec<SceneField>,
}

impl SceneFields {
    /// The fields of a scene's `[field]` table, and everything wrong with it,
    /// never stopping at the first. A field whose keys are wrong is left out
    /// (its zones can't be read without them); a zone that is wrong is left
    /// out and the field kept, as the wooze field does.
    pub fn read(table: &toml::Value) -> (Self, Vec<FieldIssue>) {
        let mut issues = Vec::new();
        let mut fields = Vec::new();
        let Some(table) = table.as_table() else {
            issues.push(FieldIssue::of(
                "",
                "it holds one table a field: [field.<name>] keys = { .. }",
            ));
            return (Self { fields }, issues);
        };
        for (name, def) in table {
            if let Some(field) = read_field(name, def, &mut issues) {
                fields.push(field);
            }
        }
        (Self { fields }, issues)
    }

    /// The fields of a scene file's top-level tables (`SceneFile::tables`),
    /// if it has a `[field]` table.
    pub fn of_tables(tables: &toml::Table) -> Option<(Self, Vec<FieldIssue>)> {
        tables.get("field").map(Self::read)
    }

    /// The field called `name`.
    pub fn get(&self, name: &str) -> Option<&SceneField> {
        self.fields.iter().find(|field| field.name() == name)
    }

    /// The field called `name`, to change.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut SceneField> {
        self.fields.iter_mut().find(|field| field.name() == name)
    }

    /// Every field's name, in the file's order.
    pub fn names(&self) -> Vec<&str> {
        self.fields.iter().map(SceneField::name).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &SceneField> {
        self.fields.iter()
    }
}

/// One `[field.<name>]` table, or `None` when its keys can't be read.
fn read_field(name: &str, def: &toml::Value, issues: &mut Vec<FieldIssue>) -> Option<SceneField> {
    let Some(def) = def.as_table() else {
        issues.push(FieldIssue::of(
            name,
            format!("{def} must be a table: [field.{name}] keys = {{ .. }}"),
        ));
        return None;
    };
    let declared = |key: &str| {
        def.get("keys")
            .and_then(toml::Value::as_table)
            .is_some_and(|keys| keys.contains_key(key))
    };
    for key in def.keys().filter(|key| !FIELD_KEYS.contains(&key.as_str())) {
        let note = if declared(key) {
            " (a key's value everywhere is its default)"
        } else {
            ""
        };
        issues.push(FieldIssue::of(
            name,
            format!("'{key}' isn't a field setting; it takes keys and zone{note}"),
        ));
    }

    let keys = match def.get("keys") {
        None => {
            issues.push(FieldIssue::of(
                name,
                "it needs its keys: keys = { wind = { min = 0, default = 1 } }",
            ));
            return None;
        }
        Some(toml::Value::Table(keys)) if keys.is_empty() => {
            issues.push(FieldIssue::of(name, "keys = {} declares no keys"));
            return None;
        }
        Some(toml::Value::Table(keys)) => keys,
        Some(other) => {
            issues.push(FieldIssue::of(
                name,
                format!("keys = {other} must be a table: {{ wind = {{ min = 0, default = 1 }} }}"),
            ));
            return None;
        }
    };
    let before = issues.len();
    let keys: Vec<SceneKey> = keys
        .iter()
        .filter_map(|(key, spec)| match read_key(key, spec) {
            Ok(key) => Some(key),
            Err(message) => {
                issues.push(FieldIssue {
                    key: Some(key.clone()),
                    ..FieldIssue::of(name, message)
                });
                None
            }
        })
        .collect();
    if issues.len() > before {
        return None;
    }

    let mut field = SceneField::new(name, keys);
    let zones = match def.get("zone") {
        None => return Some(field),
        Some(toml::Value::Array(zones)) => zones,
        Some(other) => {
            issues.push(FieldIssue::of(
                name,
                format!("zone = {other} must be a list of zones: [[field.{name}.zone]]"),
            ));
            return Some(field);
        }
    };
    for (n, zone) in zones.iter().enumerate() {
        let label = zone
            .get("name")
            .and_then(toml::Value::as_str)
            .map_or_else(|| format!("zone {}", n + 1), str::to_string);
        let result = match zone.as_table() {
            Some(zone) => field.add_zone(zone),
            None => Err(format!("{zone} must be a table of shape, center and keys")),
        };
        if let Err(message) = result {
            issues.push(FieldIssue {
                zone: Some(label),
                ..FieldIssue::of(name, message)
            });
        }
    }
    Some(field)
}

/// One key of `keys = { .. }`: a table of `min`, `max` and `default`, or a
/// bare number, its default with no range.
fn read_key(name: &str, spec: &toml::Value) -> Result<SceneKey, String> {
    if name.is_empty() {
        return Err("a key needs a name".to_string());
    }
    if SHAPE_KEYS.contains(&name) {
        return Err(format!(
            "'{name}' can't be a key: a zone's {} takes that name",
            SHAPE_KEYS.join(", ")
        ));
    }
    let number = |setting: &str, value: &toml::Value| {
        toml_f32(value)
            .filter(|v| v.is_finite())
            .ok_or_else(|| format!("{setting} = {value} must be a number"))
    };
    let key = match spec {
        toml::Value::Table(spec) => {
            if let Some(other) = spec.keys().find(|k| !KEY_KEYS.contains(&k.as_str())) {
                return Err(format!(
                    "'{other}' isn't a key's setting; it takes {}",
                    KEY_KEYS.join(", ")
                ));
            }
            let read = |setting: &str| spec.get(setting).map(|v| number(setting, v)).transpose();
            let (min, max) = (read("min")?, read("max")?);
            if let (Some(min), Some(max)) = (min, max) {
                if min > max {
                    return Err(format!("min = {min} is more than max = {max}"));
                }
            }
            // Unsaid, the default is 0, or the end of the range nearest it
            let default = read("default")?
                .unwrap_or_else(|| 0.0f32.clamp(min.unwrap_or(f32::MIN), max.unwrap_or(f32::MAX)));
            SceneKey {
                name: name.to_string(),
                min,
                max,
                default,
            }
        }
        value => SceneKey {
            name: name.to_string(),
            min: None,
            max: None,
            default: number("default", value).map_err(|_| {
                format!("{name} = {value} must be a number or a table of min, max and default")
            })?,
        },
    };
    key.check(key.default)
        .map_err(|why| format!("its default is out of range: {why}"))?;
    Ok(key)
}

#[cfg(test)]
#[path = "scene_tests.rs"]
mod tests;
