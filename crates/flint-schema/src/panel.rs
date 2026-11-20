//! A component's F3 panel, declared in its schema (#744).
//!
//! A field with a `panel` key is on the panel of every entity that carries
//! the component, so a game gets a debug panel for a script-driven
//! controller without a line of Rust:
//!
//! ```toml
//! [component.harbour_lantern.panel]
//! title = "Lantern"                      # optional; default "Harbour lantern"
//!
//! [component.harbour_lantern.fields]
//! phase = { type = "f32", default = 0.0, panel = { role = "status", labels = ["Dark", "Kindling", "Lit", "Guttering"] } }
//! night = { type = "f32", default = 0.0, panel = { role = "status", format = "night {:.0}" } }
//! trigger_light = { type = "f32", default = -1.0, panel = { role = "trigger", label = "Light now" } }
//! burn_duration_s = { type = "f32", default = 150.0, panel = "tune" }
//! ```
//!
//! The roles ([`PanelRole`]):
//!
//! - `status`: read only, a part of the panel's one status line.
//! - `tune`: an editor (a checkbox, a number, a choice or a text box) that
//!   writes the live component as it changes; the panel's Reset puts back
//!   the values the scene loaded with, and Commit to File writes them all
//!   into the scene file. A tune field with a `format` is on the status line
//!   too.
//! - `trigger`: a button that writes `press` once (a `bool` field: `true`; a
//!   number: `1`), for the script to act on and clear. With `labels`, one
//!   button per name, each writing its index.
//!
//! The rows keep the order the fields are written in. Every key a field's
//! `panel` table may hold is in [`PANEL_FIELD_KEYS`]; a mistake in one is a
//! `bad-panel` warning from `flint validate` (the row is left off), never a
//! failed load.

use crate::component::{FieldSchema, FieldType};
use std::fmt::Write as _;

/// The keys of a `[component.<name>.panel]` table.
pub const PANEL_KEYS: &[&str] = &["title"];

/// The keys of a field's `panel = { .. }` table.
pub const PANEL_FIELD_KEYS: &[&str] = &[
    "role", "label", "labels", "press", "step", "min", "max", "slider", "format", "scale", "unset",
    "from", "presets", "commit",
];

/// What a field is on its component's panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelRole {
    /// Read only, on the status line.
    Status,
    /// An editor that writes the live component.
    Tune,
    /// A button that writes a value once.
    Trigger,
}

impl PanelRole {
    /// Every role, by its spelling in a schema.
    pub const ALL: &'static [(&'static str, PanelRole)] = &[
        ("status", PanelRole::Status),
        ("tune", PanelRole::Tune),
        ("trigger", PanelRole::Trigger),
    ];

    fn named(name: &str) -> Option<PanelRole> {
        Self::ALL.iter().find(|(n, _)| *n == name).map(|(_, r)| *r)
    }

    /// The role's spelling.
    pub fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, r)| *r == self)
            .map(|(n, _)| *n)
            .unwrap_or("")
    }
}

/// The kind of value a row edits or shows, from the field's type.
#[derive(Debug, Clone, PartialEq)]
pub enum PanelValue {
    Bool,
    Int,
    Float,
    /// An `enum` field's values.
    Choice(Vec<String>),
    Text,
    /// Any other type (a vec3, a colour, an array): shown as TOML, never
    /// edited.
    Other,
}

impl PanelValue {
    fn of(field_type: &FieldType) -> PanelValue {
        match field_type {
            FieldType::Bool => PanelValue::Bool,
            FieldType::I32 | FieldType::I64 => PanelValue::Int,
            FieldType::F32 | FieldType::F64 => PanelValue::Float,
            FieldType::Enum { values } => PanelValue::Choice(values.clone()),
            FieldType::String | FieldType::Entity | FieldType::Asset { .. } => PanelValue::Text,
            _ => PanelValue::Other,
        }
    }

    /// A number, whose row can take a range, a step, labels and presets.
    pub fn is_number(&self) -> bool {
        matches!(self, PanelValue::Int | PanelValue::Float)
    }
}

/// One field on a component's panel. See the module docs for each key.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelRow {
    pub field: String,
    pub role: PanelRole,
    pub value: PanelValue,
    /// What the row is called on the panel (default: the field's name,
    /// `min_interval_s` -> "Min interval s").
    pub label: String,
    /// The field's `description`, shown on hover.
    pub description: Option<String>,
    /// Names for a number's values 0, 1, 2...: what the status line shows,
    /// the choices a tune row offers, the buttons of a trigger (an empty
    /// name is no button).
    pub labels: Vec<String>,
    /// What a trigger writes.
    pub press: toml::Value,
    /// A number editor's drag speed.
    pub step: Option<f64>,
    /// A number editor's range: the panel's own `min` / `max`, else the
    /// field's.
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// A slider instead of a drag value (needs `min` and `max`).
    pub slider: bool,
    /// How the status line shows the value: `{}` is the value (a label's
    /// name, if it has labels), `{:.N}` the number to N places, `{:clock}`
    /// hours as `HH:MM`.
    pub format: Option<String>,
    /// The number the status line shows is the value times this.
    pub scale: f64,
    /// A number that means "not set" (`-1` for "the script decides"): the
    /// status line leaves the row out, and a tune row is a checkbox that
    /// writes it when cleared.
    pub unset: Option<f64>,
    /// The field whose live value a tune row with `unset` starts from when
    /// it is checked.
    pub from: Option<String>,
    /// Buttons beside a tune row's editor that jump to a value.
    pub presets: Vec<(String, f64)>,
    /// Whether Commit to File writes a tune row, and Reset puts it back
    /// (default true). `commit = false` is for a field the script owns that
    /// the panel only nudges, as a day counter: the scene's start value stays.
    pub commit: bool,
}

impl PanelRow {
    /// Whether this row is a part of the status line.
    pub fn on_status_line(&self) -> bool {
        self.role == PanelRole::Status || self.format.is_some() && self.role == PanelRole::Tune
    }

    /// The row's part of the status line, for the live `value`: `None`
    /// when there is no value, or it is `unset`.
    pub fn status_text(&self, value: Option<&toml::Value>) -> Option<String> {
        let value = value?;
        let number = number_of(value);
        if let (Some(unset), Some(n)) = (self.unset, number) {
            if (n - unset).abs() < 1e-9 {
                return None;
            }
        }
        let named = number.and_then(|n| self.label_of(n));
        let format = match &self.format {
            Some(format) => format.clone(),
            None if named.is_some() || matches!(self.value, PanelValue::Choice(_)) => "{}".into(),
            None => format!("{} {{}}", self.label.to_lowercase()),
        };
        let (before, spec, after) = split_placeholder(&format).unwrap_or((&format, "", ""));
        let shown = match (spec, number) {
            (":clock", Some(n)) => clock(n * self.scale),
            (spec, Some(n)) if spec.starts_with(":.") => {
                let places = spec[2..].parse::<usize>().unwrap_or(2);
                format!("{:.*}", places, n * self.scale)
            }
            (_, Some(n)) => match named {
                Some(name) => name.to_string(),
                None if self.value == PanelValue::Int => format!("{}", (n * self.scale).round()),
                None => format!("{:.2}", n * self.scale),
            },
            (_, None) => match value {
                toml::Value::Boolean(b) => if *b { "on" } else { "off" }.to_string(),
                toml::Value::String(s) => s.clone(),
                other => other.to_string(),
            },
        };
        Some(format!("{before}{shown}{after}"))
    }

    /// The label naming `n` (rounded), if the row has one for it.
    pub fn label_of(&self, n: f64) -> Option<&str> {
        let i = n.round();
        if i < 0.0 || self.labels.is_empty() {
            return None;
        }
        let i = (i as usize).min(self.labels.len() - 1);
        Some(self.labels[i].as_str()).filter(|s| !s.is_empty())
    }
}

/// A component's panel: its title and its rows, in the order the fields are
/// written.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelSpec {
    pub title: String,
    pub rows: Vec<PanelRow>,
}

impl PanelSpec {
    /// The rows with `role`.
    pub fn rows_with(&self, role: PanelRole) -> impl Iterator<Item = &PanelRow> {
        self.rows.iter().filter(move |r| r.role == role)
    }
}

/// A mistake in a panel's declaration: the field it is about (none for the
/// `[component.x.panel]` table), what is wrong and what to write.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelProblem {
    pub field: Option<String>,
    pub message: String,
    pub hint: Option<String>,
}

/// `harbour_lantern` -> "Harbour lantern".
pub fn words_of(name: &str) -> String {
    let spaced = name.replace('_', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The panel of component `name`, from its `[component.x.panel]` table and
/// each field's `panel` key, `fields` in the order they are written. `None`
/// when no field has a panel role. Each problem leaves its row (or key) out.
pub fn panel_of(
    name: &str,
    table: Option<&toml::Value>,
    fields: &[(&str, &FieldSchema, Option<&toml::Value>)],
) -> (Option<PanelSpec>, Vec<PanelProblem>) {
    let mut problems = Vec::new();
    let mut title = words_of(name);
    if let Some(table) = table {
        match table.as_table() {
            Some(table) => {
                for (key, value) in table {
                    match (key.as_str(), value) {
                        ("title", toml::Value::String(t)) => title = t.clone(),
                        ("title", _) => problems.push(problem(None, "`title` is a string")),
                        _ => problems.push(PanelProblem {
                            field: None,
                            message: format!("`panel.{key}` is no key of a component's panel"),
                            hint: Some(
                                flint_core::diagnostic::did_you_mean(
                                    key,
                                    PANEL_KEYS.iter().copied(),
                                )
                                .unwrap_or_else(|| {
                                    "a component's panel holds `title`; each field says its own \
                                     part with `panel = \"status\" | \"tune\" | \"trigger\"`"
                                        .into()
                                }),
                            ),
                        }),
                    }
                }
            }
            None => problems.push(problem(None, "`panel` is a table (`[component.x.panel]`)")),
        }
    }
    let names: Vec<&str> = fields.iter().map(|(n, _, _)| *n).collect();
    let mut rows = Vec::new();
    for (field, schema, panel) in fields {
        let Some(panel) = panel else { continue };
        let before = problems.len();
        let row = row_of(field, schema, panel, &names, &mut problems);
        for p in &mut problems[before..] {
            p.field.get_or_insert_with(|| field.to_string());
        }
        if let Some(row) = row {
            rows.push(row);
        }
    }
    if rows.is_empty() {
        if table.is_some() {
            problems.push(PanelProblem {
                field: None,
                message: "the panel has no rows, so there is none".into(),
                hint: Some("give a field `panel = \"status\"`, `\"tune\"` or `\"trigger\"`".into()),
            });
        }
        return (None, problems);
    }
    (Some(PanelSpec { title, rows }), problems)
}

fn problem(field: Option<&str>, message: impl Into<String>) -> PanelProblem {
    PanelProblem {
        field: field.map(str::to_string),
        message: message.into(),
        hint: None,
    }
}

fn row_of(
    field: &str,
    schema: &FieldSchema,
    panel: &toml::Value,
    names: &[&str],
    problems: &mut Vec<PanelProblem>,
) -> Option<PanelRow> {
    let empty = toml::Table::new();
    let (role_name, keys) = match panel {
        toml::Value::String(role) => (Some(role.as_str()), &empty),
        toml::Value::Table(keys) => (keys.get("role").and_then(|r| r.as_str()), keys),
        _ => {
            problems.push(problem(
                None,
                "`panel` is a role (\"status\", \"tune\", \"trigger\") or a table with a `role`",
            ));
            return None;
        }
    };
    let roles = || PanelRole::ALL.iter().map(|(n, _)| *n);
    let Some(role_name) = role_name else {
        problems.push(PanelProblem {
            field: None,
            message: "the field's panel has no `role`".into(),
            hint: Some("write role = \"status\", \"tune\" or \"trigger\"".into()),
        });
        return None;
    };
    let Some(role) = PanelRole::named(role_name) else {
        problems.push(PanelProblem {
            field: None,
            message: format!("`{role_name}` is no panel role"),
            hint: Some(
                flint_core::diagnostic::did_you_mean(role_name, roles())
                    .unwrap_or_else(|| "the roles are status, tune and trigger".into()),
            ),
        });
        return None;
    };
    for key in keys.keys() {
        if !PANEL_FIELD_KEYS.contains(&key.as_str()) {
            problems.push(PanelProblem {
                field: None,
                message: format!("`{key}` is no key of a field's panel"),
                hint: Some(
                    flint_core::diagnostic::did_you_mean(key, PANEL_FIELD_KEYS.iter().copied())
                        .unwrap_or_else(|| {
                            format!("a field's panel holds {}", PANEL_FIELD_KEYS.join(", "))
                        }),
                ),
            });
        }
    }
    let value = PanelValue::of(&schema.field_type);
    let type_name = schema.field_type.describe();
    match (role, &value) {
        (PanelRole::Tune, PanelValue::Other) => {
            problems.push(problem(
                None,
                format!(
                    "a {type_name} field can't be tuned on a panel (only shown: role = \"status\")"
                ),
            ));
            return None;
        }
        (PanelRole::Trigger, v) if !matches!(v, PanelValue::Bool) && !v.is_number() => {
            problems.push(problem(
                None,
                format!("a trigger writes a bool or a number, and this field is a {type_name}"),
            ));
            return None;
        }
        _ => {}
    }

    let label = get_string(keys, "label", problems).unwrap_or_else(|| words_of(field));
    let labels = match keys.get("labels") {
        None => Vec::new(),
        Some(_) if !numbers_only(keys, "labels", &value, problems) => Vec::new(),
        Some(toml::Value::Array(items)) if items.iter().all(|i| i.is_str()) => items
            .iter()
            .filter_map(|i| i.as_str().map(str::to_string))
            .collect(),
        Some(_) => {
            bad(problems, "labels", "a list of names");
            Vec::new()
        }
    };
    let press = match keys.get("press") {
        Some(v) => {
            let fits = match value {
                PanelValue::Bool => v.is_bool(),
                _ => number_of(v).is_some(),
            };
            if fits {
                v.clone()
            } else {
                bad(problems, "press", "a value of the field's type");
                default_press(&value)
            }
        }
        None => default_press(&value),
    };
    let step = numbers_only(keys, "step", &value, problems)
        .then(|| get_number(keys, "step", problems))
        .flatten();
    let min = numbers_only(keys, "min", &value, problems)
        .then(|| get_number(keys, "min", problems))
        .flatten()
        .or(schema.min);
    let max = numbers_only(keys, "max", &value, problems)
        .then(|| get_number(keys, "max", problems))
        .flatten()
        .or(schema.max);
    let slider = match keys.get("slider") {
        None => false,
        Some(toml::Value::Boolean(b)) => *b,
        Some(_) => {
            bad(problems, "slider", "true or false");
            false
        }
    };
    if slider && (min.is_none() || max.is_none()) {
        bad(problems, "slider", "for a number with both `min` and `max`");
    }
    let format = get_string(keys, "format", problems);
    if let Some(format) = &format {
        match split_placeholder(format) {
            Some((_, spec, _)) if !spec_is_known(spec) => {
                problems.push(PanelProblem {
                    field: None,
                    message: format!("`{{{spec}}}` in `format` is no placeholder"),
                    hint: Some(
                        "write `{}`, `{:.2}` (places) or `{:clock}` (hours as HH:MM)".into(),
                    ),
                });
            }
            None => problems.push(PanelProblem {
                field: None,
                message: "`format` has no `{}` for the value".into(),
                hint: None,
            }),
            _ => {}
        }
    }
    let scale = get_number(keys, "scale", problems).unwrap_or(1.0);
    let unset = numbers_only(keys, "unset", &value, problems)
        .then(|| get_number(keys, "unset", problems))
        .flatten();
    let from = get_string(keys, "from", problems);
    if let Some(from) = &from {
        if !names.contains(&from.as_str()) {
            problems.push(PanelProblem {
                field: None,
                message: format!("`from` names `{from}`, which is no field of the component"),
                hint: flint_core::diagnostic::did_you_mean(from, names.iter().copied()),
            });
        }
    }
    let presets = match keys.get("presets") {
        None => Vec::new(),
        Some(_) if !numbers_only(keys, "presets", &value, problems) => Vec::new(),
        Some(toml::Value::Array(items)) => {
            let pairs: Option<Vec<(String, f64)>> = items
                .iter()
                .map(|item| match item.as_array().map(Vec::as_slice) {
                    Some([toml::Value::String(name), n]) => number_of(n).map(|n| (name.clone(), n)),
                    _ => None,
                })
                .collect();
            pairs.unwrap_or_else(|| {
                bad(problems, "presets", "a list of [name, value] pairs");
                Vec::new()
            })
        }
        Some(_) => {
            bad(problems, "presets", "a list of [name, value] pairs");
            Vec::new()
        }
    };
    let commit = match keys.get("commit") {
        None => true,
        Some(toml::Value::Boolean(b)) => *b,
        Some(_) => {
            bad(problems, "commit", "true or false");
            true
        }
    };

    Some(PanelRow {
        field: field.to_string(),
        role,
        value,
        label,
        description: schema.description.clone(),
        labels,
        press,
        step,
        min,
        max,
        slider: slider && min.is_some() && max.is_some(),
        format,
        scale,
        unset,
        from,
        presets,
        commit,
    })
}

fn bad(problems: &mut Vec<PanelProblem>, key: &str, what: &str) {
    problems.push(problem(None, format!("`{key}` is {what}")));
}

fn get_number(keys: &toml::Table, key: &str, problems: &mut Vec<PanelProblem>) -> Option<f64> {
    let n = number_of(keys.get(key)?);
    if n.is_none() {
        bad(problems, key, "a number");
    }
    n
}

fn get_string(keys: &toml::Table, key: &str, problems: &mut Vec<PanelProblem>) -> Option<String> {
    match keys.get(key)?.as_str() {
        Some(s) => Some(s.to_string()),
        None => {
            bad(problems, key, "a string");
            None
        }
    }
}

/// False, with a problem, when `key` is given for a field that is no number.
fn numbers_only(
    keys: &toml::Table,
    key: &str,
    value: &PanelValue,
    problems: &mut Vec<PanelProblem>,
) -> bool {
    if keys.contains_key(key) && !value.is_number() {
        bad(problems, key, "for a number field");
        return false;
    }
    true
}

fn default_press(value: &PanelValue) -> toml::Value {
    match value {
        PanelValue::Bool => toml::Value::Boolean(true),
        PanelValue::Int => toml::Value::Integer(1),
        _ => toml::Value::Float(1.0),
    }
}

/// A TOML number as f64 (an integer is taken).
pub fn number_of(value: &toml::Value) -> Option<f64> {
    match value {
        toml::Value::Float(f) => Some(*f),
        toml::Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

/// `"day {:.0}"` -> `("day ", ":.0", "")`.
fn split_placeholder(format: &str) -> Option<(&str, &str, &str)> {
    let open = format.find('{')?;
    let close = open + format[open..].find('}')?;
    Some((
        &format[..open],
        &format[open + 1..close],
        &format[close + 1..],
    ))
}

fn spec_is_known(spec: &str) -> bool {
    spec.is_empty()
        || spec == ":clock"
        || spec
            .strip_prefix(":.")
            .is_some_and(|n| n.parse::<usize>().is_ok())
}

/// Hours as `HH:MM`, wrapped into the day.
fn clock(hours: f64) -> String {
    let minutes = (hours * 60.0).floor() as i64;
    let mut out = String::new();
    let _ = write!(
        out,
        "{:02}:{:02}",
        minutes.div_euclid(60).rem_euclid(24),
        minutes.rem_euclid(60)
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SchemaRegistry;

    fn registry(text: &str) -> SchemaRegistry {
        let mut registry = SchemaRegistry::new();
        registry.load_component_string(text).unwrap();
        registry
    }

    const LANTERN: &str = r#"
[component.harbour_lantern.panel]
title = "Lantern"

[component.harbour_lantern.fields]
phase = { type = "f32", default = 0.0, panel = { role = "status", labels = ["Dark", "Kindling", "Lit", "Guttering"] } }
night = { type = "f32", default = 0.0, panel = { role = "status", format = "night {:.0}" } }
trigger_light = { type = "f32", default = -1.0, panel = { role = "trigger", label = "Light now" } }
burn_duration_s = { type = "f32", default = 150.0, min = 1.0, panel = "tune" }
debug_log = { type = "bool", default = false }
"#;

    #[test]
    fn the_rows_keep_the_order_they_are_written_in() {
        let registry = registry(LANTERN);
        let panel = registry
            .get_component("harbour_lantern")
            .unwrap()
            .panel
            .as_ref()
            .unwrap();
        assert_eq!(panel.title, "Lantern");
        let fields: Vec<&str> = panel.rows.iter().map(|r| r.field.as_str()).collect();
        assert_eq!(
            fields,
            ["phase", "night", "trigger_light", "burn_duration_s"]
        );
        let tune = &panel.rows[3];
        assert_eq!(tune.role, PanelRole::Tune);
        assert_eq!(tune.label, "Burn duration s");
        assert_eq!(tune.min, Some(1.0), "the field's own min is the range");
        assert_eq!(panel.rows[2].press, toml::Value::Float(1.0));
        assert!(registry.lints().is_empty(), "{:?}", registry.lints());
    }

    #[test]
    fn a_component_with_no_panel_row_has_no_panel() {
        let registry = registry("[component.plain.fields]\nx = { type = \"f32\" }\n");
        assert!(registry.get_component("plain").unwrap().panel.is_none());
    }

    #[test]
    fn the_status_line_names_formats_and_leaves_out_unset() {
        let panel = registry(LANTERN)
            .get_component("harbour_lantern")
            .unwrap()
            .panel
            .clone()
            .unwrap();
        let phase = &panel.rows[0];
        let night = &panel.rows[1];
        assert_eq!(
            phase.status_text(Some(&toml::Value::Float(2.2))).as_deref(),
            Some("Lit")
        );
        assert_eq!(
            phase.status_text(Some(&toml::Value::Float(9.0))).as_deref(),
            Some("Guttering")
        );
        assert_eq!(
            night.status_text(Some(&toml::Value::Integer(3))).as_deref(),
            Some("night 3")
        );

        let mut next = night.clone();
        next.format = Some("next in {:.0} s".into());
        next.unset = Some(-1.0);
        assert_eq!(next.status_text(Some(&toml::Value::Float(-1.0))), None);
        assert_eq!(
            next.status_text(Some(&toml::Value::Float(41.6))).as_deref(),
            Some("next in 42 s")
        );

        let mut sea = night.clone();
        sea.format = Some("sea {:.0}%".into());
        sea.scale = 100.0;
        assert_eq!(
            sea.status_text(Some(&toml::Value::Float(0.25))).as_deref(),
            Some("sea 25%")
        );

        let mut time = night.clone();
        time.format = Some("{:clock}".into());
        assert_eq!(
            time.status_text(Some(&toml::Value::Float(14.55)))
                .as_deref(),
            Some("14:33")
        );
        assert_eq!(
            time.status_text(Some(&toml::Value::Float(24.25)))
                .as_deref(),
            Some("00:15")
        );
    }

    #[test]
    fn a_mistake_is_a_lint_and_leaves_the_row_out() {
        let registry = registry(
            r#"
[component.calm.fields]
phase = { type = "f32", panel = "statsu" }
end_now = { type = "bool", panel = { role = "trigger", lable = "End" } }
spot = { type = "vec3", panel = "tune" }
calm = { type = "f32", panel = { role = "status", format = "calm {:.x}" } }
"#,
        );
        let panel = registry
            .get_component("calm")
            .unwrap()
            .panel
            .clone()
            .unwrap();
        let fields: Vec<&str> = panel.rows.iter().map(|r| r.field.as_str()).collect();
        assert_eq!(
            fields,
            ["end_now", "calm"],
            "the bad role and the vec3 are left out"
        );
        let lints = registry.lints();
        let said: Vec<String> = lints.iter().map(|d| d.to_string()).collect();
        assert_eq!(lints.len(), 4, "{said:#?}");
        assert!(lints
            .iter()
            .all(|d| d.code == flint_core::diagnostic::codes::BAD_PANEL));
        assert!(
            said.iter()
                .any(|s| s.contains("phase") && s.contains("`statsu`")),
            "{said:#?}"
        );
        let hint = lints.iter().find(|d| d.message.contains("lable")).unwrap();
        assert!(hint.hint.as_deref().unwrap().contains("label"), "{hint:?}");
        assert!(
            said.iter()
                .any(|s| s.contains("spot") && s.contains("vec3")),
            "{said:#?}"
        );
        assert!(said.iter().any(|s| s.contains("{:.x}")), "{said:#?}");
    }

    #[test]
    fn a_trigger_with_labels_and_a_tune_with_unset_and_presets() {
        let registry = registry(
            r#"
[component.weather.fields]
trigger_front = { type = "f32", default = -1.0, panel = { role = "trigger", labels = ["", "Rain", "Storm"] } }
mix = { type = "f32", default = 0.0, panel = "status" }
mix_override = { type = "f32", default = -1.0, panel = { role = "tune", unset = -1.0, from = "mix", min = 0.0, max = 1.0, slider = true, label = "Pin mix" } }
time_hours = { type = "f32", panel = { role = "tune", presets = [["Dawn", 6.0], ["Noon", 12.5]] } }
"#,
        );
        let panel = registry
            .get_component("weather")
            .unwrap()
            .panel
            .clone()
            .unwrap();
        assert!(registry.lints().is_empty(), "{:?}", registry.lints());
        assert_eq!(panel.title, "Weather");
        assert_eq!(
            panel.rows[0].label_of(0.0),
            None,
            "an empty name is no button"
        );
        assert_eq!(panel.rows[0].label_of(2.0), Some("Storm"));
        let pin = &panel.rows[2];
        assert_eq!(
            (pin.unset, pin.from.as_deref(), pin.slider),
            (Some(-1.0), Some("mix"), true)
        );
        assert_eq!(
            panel.rows[3].presets,
            [("Dawn".into(), 6.0), ("Noon".into(), 12.5)]
        );
    }
}
