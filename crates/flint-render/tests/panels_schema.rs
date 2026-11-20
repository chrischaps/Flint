//! `schemas/scene/panels.toml` against `PanelLayout`, the one reader of a
//! scene's `[panels]` table (#880, ADR 0120). flint-scene's
//! `tests/table_schemas.rs` holds `[camera]`, `[environment]` and
//! `[post_process]` to their structs, flint-procgen's
//! `tests/scene_table_schemas.rs` `[living]` and `[wooze]`; `[panels]` came in
//! with #898 and had nothing holding it (#976).
//!
//! The schema is what `flint schema`, `flint validate` and an editor go by;
//! `PanelLayout::parse` is what runs, and it refuses a key it doesn't know.
//! A key in one and not the other is a check that lies.
//!
//! No GPU: these parse tables.

use flint_render::panels::{PanelLayout, DEFAULT_BORDER, DEFAULT_GUTTER, PAGE_KEYS, PANEL_KEYS};
use flint_schema::{FieldType, SchemaRegistry, TableSchema};
use std::collections::BTreeSet;

fn panels(registry: &SchemaRegistry) -> &TableSchema {
    registry
        .get_table("panels")
        .expect("schemas/scene/panels.toml declares [table.panels]")
}

fn registry() -> SchemaRegistry {
    SchemaRegistry::load_from_directory(concat!(env!("CARGO_MANIFEST_DIR"), "/../../schemas"))
        .unwrap()
}

/// The schema declares exactly the keys `[panels]` takes, and a table
/// writing each of them, with values unlike the defaults, parses with every
/// value reaching the layout.
#[test]
fn the_schema_declares_what_the_layout_reads() {
    let registry = registry();
    let schema = panels(&registry);
    let declared: BTreeSet<&str> = schema.fields.keys().map(String::as_str).collect();
    let read: BTreeSet<&str> = PAGE_KEYS.iter().copied().collect();
    assert_eq!(
        declared.difference(&read).collect::<Vec<_>>(),
        [] as [&&str; 0],
        "schemas/scene/panels.toml declares what PanelLayout::parse refuses"
    );
    assert_eq!(
        read.difference(&declared).collect::<Vec<_>>(),
        [] as [&&str; 0],
        "PanelLayout::parse reads what schemas/scene/panels.toml does not declare \
         (flint validate would call it unknown)"
    );

    // Each declared field written in the type the schema gives it
    let mut table = toml::Table::new();
    for (name, field) in &schema.fields {
        let value: toml::Value = match (&field.field_type, name.as_str()) {
            (FieldType::F32, _) => 7.5.into(),
            (FieldType::Color, _) => vec![0.25, 0.5, 0.75].into(),
            (FieldType::Table, "panel") => {
                let mut panel = toml::Table::new();
                for key in PANEL_KEYS {
                    let v: toml::Value = match *key {
                        "rect" => vec![0.0, 0.0, 0.5, 1.0].into(),
                        "order" => 3.into(),
                        "position" | "target" => vec![1.0, 2.0, 3.0].into(),
                        "fov" => 35.0.into(),
                        "follow" => "hero".into(),
                        "look" => "rotoscope".into(),
                        "post_process" => toml::Value::Table(toml::Table::new()),
                        other => panic!("PANEL_KEYS has `{other}`, which this test can't write"),
                    };
                    panel.insert(key.to_string(), v);
                }
                let mut list = toml::Table::new();
                list.insert("wide".into(), panel.into());
                list.into()
            }
            (other, _) => panic!("panels.{name}: a {other:?} field this test can't write"),
        };
        table.insert(name.clone(), value);
    }
    let layout = PanelLayout::parse(&table)
        .unwrap_or_else(|e| panic!("[panels] with every declared field: {e}\n{table:#?}"))
        .expect("a layout: the table names a panel");
    assert_eq!((layout.gutter, layout.border), (7.5, 7.5));
    assert_eq!(layout.gutter_color, [0.25, 0.5, 0.75]);
    assert_eq!(layout.border_color, [0.25, 0.5, 0.75]);
    let wide = layout.panel("wide").expect("panel.wide");
    assert_eq!(wide.rect, [0.0, 0.0, 0.5, 1.0]);
    assert_eq!(wide.order, 3);
    assert_eq!(wide.position, Some([1.0, 2.0, 3.0]));
    assert_eq!(wide.target, Some([1.0, 2.0, 3.0]));
    assert_eq!(wide.fov, Some(35.0));
    assert_eq!(wide.follow.as_deref(), Some("hero"));
    assert_eq!(wide.look.as_deref(), Some("rotoscope"));
    assert!(wide.post_process.is_some());
}

/// A panel is an open `table` in the schema, so its keys are named only in
/// the `panel` field's description: each key a panel takes is named there.
/// And the page's numbers the descriptions give as "Unset: N" are the ones
/// the layout falls back to.
#[test]
fn the_descriptions_name_the_panel_keys_and_the_defaults() {
    let registry = registry();
    let schema = panels(&registry);
    let described = |field: &str| {
        schema
            .get_field(field)
            .and_then(|f| f.description.clone())
            .unwrap_or_else(|| panic!("panels.{field} has no description"))
    };
    let panel = described("panel");
    let words: BTreeSet<&str> = panel
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .collect();
    let unnamed: Vec<&&str> = PANEL_KEYS.iter().filter(|k| !words.contains(**k)).collect();
    assert_eq!(
        unnamed,
        [] as [&&str; 0],
        "a panel takes these, and panels.panel's description in \
         schemas/scene/panels.toml doesn't name them"
    );
    for (field, default) in [("gutter", DEFAULT_GUTTER), ("border", DEFAULT_BORDER)] {
        let said = format!("Unset: {default}");
        assert!(
            described(field).contains(&said),
            "panels.{field}: the layout falls back to {default}, and the schema's \
             description doesn't say `{said}`"
        );
    }
    // No field states a `default`: one would be written into the table at
    // load, and each of these is the reader's to fall back on
    let stated: Vec<&String> = schema
        .fields
        .iter()
        .filter(|(_, f)| f.default.is_some())
        .map(|(n, _)| n)
        .collect();
    assert_eq!(stated, [] as [&String; 0]);
}
