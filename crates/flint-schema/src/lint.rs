//! What is wrong with a schema file itself (ADR 0115 decision 4, #684).
//!
//! The loader is forgiving: a type it doesn't know is checked as a string,
//! and a key it doesn't know is dropped by serde. Both turn a typo in a
//! schema into checks that silently don't happen, so [`lint_component_file`]
//! reads the file's own TOML and says, as [`Diagnostic`] warnings naming the
//! file, the component and the field:
//!
//! - `old-type-name`: a type spelled the old way (`float` for `f32`;
//!   [`OLD_TYPE_NAMES`]). It still loads.
//! - `unknown-type`: a type that is none ([`TYPE_NAMES`]), an `asset` with no
//!   `kind`, or a `kind` that is no asset kind.
//! - `ignored-schema-key`: a key the loader ignores. The usual one is a field
//!   written under `[component.x]` instead of `[component.x.fields]`, which
//!   leaves the component with no fields: nothing in it is checked and none
//!   of its defaults is applied.
//!
//! [`SchemaRegistry`](crate::SchemaRegistry) runs it on every file it loads
//! and keeps the result ([`SchemaRegistry::lints`](crate::SchemaRegistry::lints)).
//! `flint validate` prints the ones about a game's files
//! ([`SchemaRegistry::game_lints`](crate::SchemaRegistry::game_lints)); the
//! engine's own files are held clean by `tests/engine_schemas.rs`.

use crate::component::{type_name_of, COMPONENT_KEYS, FIELD_KEYS, OLD_TYPE_NAMES, TYPE_NAMES};
use flint_core::diagnostic::{self, codes, Diagnostic};
use flint_core::project::AssetKind;
use std::path::Path;

/// What a schema file declares, which is the one top-level key it holds:
/// `components/*.toml` hold `[component.<name>]` tables, `scene/*.toml`
/// hold `[table.<name>]` ones (a scene-level table such as `[post_process]`,
/// #686). The two are written alike below that key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaKind {
    Component,
    Table,
}

impl SchemaKind {
    /// The top-level key: `component` or `table`.
    pub fn key(self) -> &'static str {
        match self {
            SchemaKind::Component => "component",
            SchemaKind::Table => "table",
        }
    }

    /// The directory of a schema directory its files are in.
    pub fn dir(self) -> &'static str {
        match self {
            SchemaKind::Component => "components",
            SchemaKind::Table => "scene",
        }
    }

    fn file_noun(self) -> &'static str {
        match self {
            SchemaKind::Component => "a component file",
            SchemaKind::Table => "a scene-table file",
        }
    }

    fn other(self) -> SchemaKind {
        match self {
            SchemaKind::Component => SchemaKind::Table,
            SchemaKind::Table => SchemaKind::Component,
        }
    }
}

/// Every lint of one component schema file's text. `file` is what each
/// diagnostic names. Text that is not TOML gives none: the loader's own
/// error says so.
pub fn lint_component_file(text: &str, file: Option<&Path>) -> Vec<Diagnostic> {
    lint_schema_file(SchemaKind::Component, text, file)
}

/// [`lint_component_file`] for a scene-table file (`scene/*.toml`, #686):
/// the same lints, about its `[table.<name>]` tables.
pub fn lint_table_file(text: &str, file: Option<&Path>) -> Vec<Diagnostic> {
    lint_schema_file(SchemaKind::Table, text, file)
}

/// Every lint of one schema file of `kind`. A `[table.*]` in a component
/// file, or a `[component.*]` in a scene-table file, is ignored like any
/// other stray key, and the hint says which directory it belongs in.
pub fn lint_schema_file(kind: SchemaKind, text: &str, file: Option<&Path>) -> Vec<Diagnostic> {
    let Ok(doc) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (key, value) in &doc {
        if key != kind.key() {
            let hint = if key == kind.other().key() {
                Some(format!(
                    "a `[{key}.<name>]` is declared in a file under `{}/` of the schema directory",
                    kind.other().dir()
                ))
            } else {
                diagnostic::did_you_mean(key, [kind.key()])
            };
            found.push(
                Diagnostic::warning(
                    codes::IGNORED_SCHEMA_KEY,
                    format!(
                        "`{key}` is ignored: {} holds `[{}.<name>]` tables",
                        kind.file_noun(),
                        kind.key()
                    ),
                )
                .field(key.clone())
                .hint_opt(hint),
            );
            continue;
        }
        for (name, def) in value.as_table().into_iter().flatten() {
            let Some(def) = def.as_table() else {
                continue;
            };
            let before = found.len();
            lint_component(kind, name, def, &mut found);
            for d in &mut found[before..] {
                match kind {
                    SchemaKind::Component => d.component = Some(name.clone()),
                    SchemaKind::Table => d.table = Some(name.clone()),
                }
            }
        }
    }
    if let Some(file) = file {
        for d in &mut found {
            d.file = Some(file.to_path_buf());
        }
    }
    found
}

fn lint_component(kind: SchemaKind, name: &str, def: &toml::Table, found: &mut Vec<Diagnostic>) {
    let key_word = kind.key();
    for (key, value) in def {
        if key == "fields" {
            for (field, field_def) in value.as_table().into_iter().flatten() {
                lint_field(field, field_def, found);
            }
        } else if !COMPONENT_KEYS.contains(&key.as_str()) {
            let d = if looks_like_a_field(value) {
                Diagnostic::warning(
                    codes::IGNORED_SCHEMA_KEY,
                    format!(
                        "`{key}` is ignored: it is written under `[{key_word}.{name}]`, and \
                         fields are read from `[{key_word}.{name}.fields]`"
                    ),
                )
                .hint(format!(
                    "move it under `[{key_word}.{name}.fields]`; until then `{name}` has one \
                     field fewer, and with none it is unchecked and its defaults are not applied"
                ))
            } else {
                Diagnostic::warning(
                    codes::IGNORED_SCHEMA_KEY,
                    format!("`{key}` is ignored: a {key_word} holds `description` and `fields`"),
                )
                .hint_opt(diagnostic::did_you_mean(
                    key,
                    COMPONENT_KEYS.iter().copied(),
                ))
            };
            found.push(d.field(key.clone()));
        }
    }
}

/// A field definition: a type name, or a table with a `type`.
fn looks_like_a_field(value: &toml::Value) -> bool {
    match value {
        toml::Value::String(name) => type_name_of(name).is_some(),
        toml::Value::Table(table) => table.contains_key("type"),
        _ => false,
    }
}

fn lint_field(path: &str, def: &toml::Value, found: &mut Vec<Diagnostic>) {
    let table = match def {
        toml::Value::String(name) => return lint_type_name(path, name, found),
        toml::Value::Table(table) => table,
        _ => return,
    };
    for key in table.keys() {
        if !FIELD_KEYS.contains(&key.as_str()) {
            found.push(
                Diagnostic::warning(
                    codes::IGNORED_SCHEMA_KEY,
                    format!("`{key}` is ignored: it is no key of a field definition"),
                )
                .field(format!("{path}.{key}"))
                .hint(
                    diagnostic::did_you_mean(key, FIELD_KEYS.iter().copied())
                        .unwrap_or_else(|| format!("a field holds {}", FIELD_KEYS.join(", "))),
                ),
            );
        }
    }
    let type_name = table.get("type").and_then(|v| v.as_str());
    if let Some(type_name) = type_name {
        lint_type_name(path, type_name, found);
    }
    if type_name.and_then(type_name_of) == Some("asset") {
        lint_asset_kind(path, table.get("kind"), found);
    }
    if let Some(element) = table.get("element") {
        lint_field(&format!("{path}[]"), element, found);
    }
    for (name, field) in table
        .get("fields")
        .and_then(|v| v.as_table())
        .into_iter()
        .flatten()
    {
        lint_field(&format!("{path}.{name}"), field, found);
    }
}

fn lint_type_name(path: &str, name: &str, found: &mut Vec<Diagnostic>) {
    if let Some((old, new)) = OLD_TYPE_NAMES.iter().find(|(old, _)| *old == name) {
        found.push(
            Diagnostic::warning(
                codes::OLD_TYPE_NAME,
                format!("`{old}` is an old spelling of `{new}`"),
            )
            .field(path)
            .hint(format!("write type = \"{new}\"; `{old}` still loads")),
        );
    } else if type_name_of(name).is_none() {
        let names = TYPE_NAMES.iter().map(|(name, _)| *name);
        found.push(
            Diagnostic::warning(
                codes::UNKNOWN_TYPE,
                format!("unknown type `{name}`, so the field is checked as a string"),
            )
            .field(path)
            .hint(
                diagnostic::did_you_mean(name, names.clone()).unwrap_or_else(|| {
                    format!("the types are {}", names.collect::<Vec<_>>().join(", "))
                }),
            ),
        );
    }
}

fn lint_asset_kind(path: &str, kind: Option<&toml::Value>, found: &mut Vec<Diagnostic>) {
    let names: Vec<&str> = match kind {
        Some(toml::Value::String(name)) => vec![name.as_str()],
        Some(toml::Value::Array(names)) => names.iter().filter_map(|v| v.as_str()).collect(),
        _ => Vec::new(),
    };
    let labels = || AssetKind::ALL.iter().map(|kind| kind.label());
    if names.is_empty() {
        found.push(
            Diagnostic::warning(
                codes::UNKNOWN_TYPE,
                "an asset field needs a `kind`, so no file is looked for",
            )
            .field(path)
            .hint(format!(
                "kind = one of {} (or a list of them)",
                labels().collect::<Vec<_>>().join(", ")
            )),
        );
    }
    for name in names {
        if AssetKind::from_label(name).is_none() {
            found.push(
                Diagnostic::warning(
                    codes::UNKNOWN_TYPE,
                    format!("unknown asset kind `{name}`, so it is not looked for"),
                )
                .field(format!("{path}.kind"))
                .hint(diagnostic::did_you_mean(name, labels()).unwrap_or_else(|| {
                    format!("the kinds are {}", labels().collect::<Vec<_>>().join(", "))
                })),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(text: &str) -> Vec<String> {
        lint_component_file(text, None)
            .iter()
            .map(|d| d.to_string())
            .collect()
    }

    #[test]
    fn a_clean_file_says_nothing() {
        assert_eq!(
            said(
                r#"
[component.rig]
description = "A rig"

[component.rig.fields]
target = { type = "entity", default = "" }
mesh = { type = "asset", kind = ["models", "specs"], unless = "runtime" }
runtime = "bool"
limits = { type = "vec2", description = "min, max" }
layers = { type = "array", element = { type = "table", fields = { weight = { type = "f32", min = 0.0, max = 1.0 } } } }
"#
            ),
            [] as [&str; 0]
        );
    }

    #[test]
    fn old_and_unknown_type_names_are_reported_wherever_they_are() {
        let found = lint_component_file(
            r#"
[component.rig.fields]
weight = { type = "float" }
on = "boolean"
stops = { type = "array", element = "integer" }
shear = { type = "table", fields = { xy = "flaot" } }
"#,
            Some(Path::new("schemas/components/rig.toml")),
        );
        let lines: Vec<String> = found.iter().map(|d| d.one_line()).collect();
        assert_eq!(
            lines[0],
            "schemas/components/rig.toml: warning[old-type-name] rig.on: `boolean` is an old \
             spelling of `bool` (write type = \"bool\"; `boolean` still loads)"
        );
        let said: Vec<String> = found.iter().map(|d| d.to_string()).collect();
        assert_eq!(
            said[1..],
            [
                "warning[unknown-type] rig.shear.xy: unknown type `flaot`, so the field is \
                 checked as a string",
                "warning[old-type-name] rig.stops[]: `integer` is an old spelling of `i32`",
                "warning[old-type-name] rig.weight: `float` is an old spelling of `f32`",
            ]
        );
        assert!(
            found[1]
                .hint
                .as_deref()
                .unwrap()
                .starts_with("the types are bool, i32, "),
            "{:?}",
            found[1].hint
        );
    }

    /// The hole eleven engine files are in: fields beside `description`
    /// instead of under `fields`.
    #[test]
    fn a_field_outside_the_fields_table_is_reported() {
        let found = lint_component_file(
            r#"
[component.collider]
shape = { type = "enum", values = ["box", "sphere"], default = "box" }
friction = "f32"
desciption = "Typo"
"#,
            None,
        );
        let lines: Vec<String> = found.iter().map(|d| d.to_string()).collect();
        assert_eq!(
            lines,
            [
                "warning[ignored-schema-key] collider.desciption: `desciption` is ignored: a \
                 component holds `description` and `fields`",
                "warning[ignored-schema-key] collider.friction: `friction` is ignored: it is \
                 written under `[component.collider]`, and fields are read from \
                 `[component.collider.fields]`",
                "warning[ignored-schema-key] collider.shape: `shape` is ignored: it is written \
                 under `[component.collider]`, and fields are read from \
                 `[component.collider.fields]`",
            ]
        );
        assert_eq!(
            found[0].hint.as_deref(),
            Some("did you mean `description`?")
        );
    }

    /// A scene-table file is linted like a component file, about its
    /// `[table.*]`; each kind in the other's directory is named with where
    /// it goes.
    #[test]
    fn a_table_file_is_linted_and_each_kind_has_its_directory() {
        let found = lint_table_file(
            r#"
[table.weather]
wind = "f32"

[table.weather.fields]
rain = { type = "float" }

[component.cloud]
"#,
            Some(Path::new("schemas/scene/weather.toml")),
        );
        let said: Vec<String> = found.iter().map(|d| d.to_string()).collect();
        assert_eq!(
            said,
            [
                "warning[ignored-schema-key] field 'component': `component` is ignored: a \
                 scene-table file holds `[table.<name>]` tables",
                "warning[old-type-name] [weather] rain: `float` is an old spelling of `f32`",
                "warning[ignored-schema-key] [weather] wind: `wind` is ignored: it is written \
                 under `[table.weather]`, and fields are read from `[table.weather.fields]`",
            ]
        );
        assert_eq!(
            found[0].hint.as_deref(),
            Some(
                "a `[component.<name>]` is declared in a file under `components/` of the \
                 schema directory"
            )
        );
        let in_components = lint_component_file("[table.weather]\n", None);
        assert_eq!(
            in_components[0].hint.as_deref(),
            Some("a `[table.<name>]` is declared in a file under `scene/` of the schema directory")
        );
    }

    #[test]
    fn a_key_no_field_holds_and_a_bad_asset_kind_are_reported() {
        assert_eq!(
            said(
                r#"
[compnent.x]

[component.rig.fields]
note = { type = "string", optional = true }
mesh = { type = "asset", kind = "modles" }
file = { type = "asset" }
"#
            ),
            [
                "warning[ignored-schema-key] field 'compnent': `compnent` is ignored: a \
                 component file holds `[component.<name>]` tables",
                "warning[unknown-type] rig.file: an asset field needs a `kind`, so no file is \
                 looked for",
                "warning[unknown-type] rig.mesh.kind: unknown asset kind `modles`, so it is not \
                 looked for",
                "warning[ignored-schema-key] rig.note.optional: `optional` is ignored: it is no \
                 key of a field definition",
            ]
        );
    }
}
