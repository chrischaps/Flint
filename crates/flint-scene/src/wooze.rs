//! A scene's `[wooze]` table (#213) as the field every consumer samples.
//!
//! Zones are kept as raw TOML in [`WoozeDef`], so that a malformed one can be
//! told apart and named: read through serde, a bad zone would fail the whole
//! scene with a line number. Loading a scene never fails over a zone. The player
//! warns and goes without it, and `flint validate` fails on it, through
//! [`WoozeDef::check`].

use crate::format::WoozeDef;
use flint_core::wooze::{preset, WoozeField, WoozeSample};
use flint_schema::{validate_component_data, SchemaRegistry};
use std::fmt;

/// Something wrong with a `[wooze]` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WoozeIssue {
    /// The zone it's about, by name, or by its place in the list if it has none.
    /// `None` for the table's global values.
    pub zone: Option<String>,
    /// What's wrong with it.
    pub message: String,
}

impl fmt::Display for WoozeIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.zone {
            Some(zone) => write!(f, "wooze zone '{zone}': {}", self.message),
            None => write!(f, "[wooze]: {}", self.message),
        }
    }
}

impl WoozeDef {
    /// The field this table describes, made of whatever is good in it, and an
    /// issue for each thing that isn't.
    pub fn field(&self) -> (WoozeField, Vec<WoozeIssue>) {
        let mut issues = Vec::new();
        let global = |message: String| WoozeIssue {
            zone: None,
            message,
        };

        let base = match self.preset.as_deref() {
            None => WoozeSample::NEUTRAL,
            Some(name) => preset(name).unwrap_or_else(|| {
                let names: Vec<&str> = flint_core::wooze::PRESETS.iter().map(|p| p.0).collect();
                issues.push(global(format!(
                    "preset '{name}' isn't one of {}",
                    names.join(", ")
                )));
                WoozeSample::NEUTRAL
            }),
        };
        let mut field = WoozeField::new(base);
        let knobs = [
            ("amplitude", self.amplitude),
            ("step", self.step),
            ("glide", self.glide),
            ("jitter", self.jitter),
        ];
        for (name, value) in knobs {
            let Some(value) = value else { continue };
            // One at a time, so one bad knob leaves the others standing.
            let mut table = toml::Table::new();
            table.insert(name.to_string(), toml::Value::Float(f64::from(value)));
            if let Err(message) = field.set_global(&table, 0.0, 0.0) {
                issues.push(global(message));
            }
        }

        for (i, zone) in self.zone.iter().enumerate() {
            let added = match zone.as_table() {
                Some(table) => field.add_zone(table),
                None => Err(format!("{zone} must be a table")),
            };
            if let Err(message) = added {
                issues.push(WoozeIssue {
                    zone: Some(zone_label(zone, i)),
                    message,
                });
            }
        }
        (field, issues)
    }

    /// Everything wrong with this table: first what the field itself can't use,
    /// which says the most, and then anything that passes that but not the `wooze`
    /// and `wooze_zone` schemas, which a game's own schemas can tighten. One issue
    /// a zone at most, so that one mistake isn't reported twice.
    pub fn check(&self, schemas: &SchemaRegistry) -> Vec<WoozeIssue> {
        let (_, mut issues) = self.field();

        let global_is_fine = issues.iter().all(|issue| issue.zone.is_some());
        if let (true, Some(schema)) = (global_is_fine, schemas.get_component("wooze")) {
            let mut table = toml::Table::new();
            if let Some(name) = &self.preset {
                table.insert("preset".into(), toml::Value::String(name.clone()));
            }
            let knobs = [
                ("amplitude", self.amplitude),
                ("step", self.step),
                ("glide", self.glide),
                ("jitter", self.jitter),
            ];
            for (name, value) in knobs {
                if let Some(value) = value {
                    table.insert(name.into(), toml::Value::Float(f64::from(value)));
                }
            }
            if let Err(e) = validate_component_data(schema, &toml::Value::Table(table)) {
                issues.push(WoozeIssue {
                    zone: None,
                    message: e.to_string(),
                });
            }
        }

        if let Some(schema) = schemas.get_component("wooze_zone") {
            for (i, zone) in self.zone.iter().enumerate() {
                let label = zone_label(zone, i);
                if issues
                    .iter()
                    .any(|issue| issue.zone.as_deref() == Some(&label))
                {
                    continue;
                }
                if let Err(e) = validate_component_data(schema, zone) {
                    issues.push(WoozeIssue {
                        zone: Some(label),
                        message: e.to_string(),
                    });
                }
            }
        }
        issues
    }
}

/// A zone's name, or `zone <n>`, counting from 1, if it hasn't got one.
fn zone_label(zone: &toml::Value, index: usize) -> String {
    match zone.get("name").and_then(|name| name.as_str()) {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => format!("zone {}", index + 1),
    }
}

#[cfg(test)]
mod tests {
    use crate::SceneFile;
    use flint_core::wooze::{preset, WoozeSample, AMPLITUDE_MAX, JITTER_MAX, STEP_MIN};
    use flint_schema::SchemaRegistry;

    fn scene(table: &str) -> SceneFile {
        toml::from_str(&format!("[scene]\nname = \"grove\"\n{table}")).unwrap()
    }

    fn schemas() -> SchemaRegistry {
        SchemaRegistry::load_from_directory("../../schemas").unwrap()
    }

    const GROVE: &str = r#"
[wooze]
preset = "linklater"
glide = 0.4
step = 0.25

[[wooze.zone]]
name = "pocket"
shape = "sphere"
center = [0, 0, 12]
radius = 6
falloff = 4
glide = 0.8

[[wooze.zone]]
name = "hush"
shape = "box"
center = [20, 0, 0]
size = [10, 10, 10]
amplitude = 0.2
step = 2
jitter = 0
"#;

    #[test]
    fn the_table_becomes_a_field() {
        let def = scene(GROVE).wooze.unwrap();
        let (field, issues) = def.field();
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(field.zones().len(), 2);

        let linklater = preset("linklater").unwrap();
        let far = field.sample([100.0, 0.0, 0.0], 0.0);
        assert_eq!(far.step, linklater.step, "from the preset");
        assert_eq!(far.glide, 0.4, "a global key beats its preset");
        assert_eq!(field.sample([0.0, 0.0, 12.0], 0.0).glide, 0.8);
        let hush = field.sample([20.0, 0.0, 0.0], 0.0);
        assert_eq!((hush.amplitude, hush.step, hush.jitter), (0.2, 2.0, 0.0));
    }

    #[test]
    fn a_bare_table_opts_in_at_neutral() {
        let (field, issues) = scene("[wooze]\n").wooze.unwrap().field();
        assert!(issues.is_empty());
        assert_eq!(field.sample([0.0; 3], 0.0), WoozeSample::NEUTRAL);
        assert!(scene("").wooze.is_none());
    }

    #[test]
    fn a_grove_passes_its_schema() {
        let issues = scene(GROVE).wooze.unwrap().check(&schemas());
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn a_malformed_zone_is_named_and_the_good_ones_still_stand() {
        let def = scene(
            r#"
[wooze]
[[wooze.zone]]
name = "good"
shape = "sphere"
center = [0, 0, 0]
radius = 1
glide = 0.5

[[wooze.zone]]
name = "broken"
shape = "sphere"
center = [0, 0, 0]
radius = -1
glide = 0.5

[[wooze.zone]]
shape = "sphere"
center = [0, 0, 0]
glide = 0.5
"#,
        )
        .wooze
        .unwrap();
        let (field, issues) = def.field();
        assert_eq!(field.zones().len(), 1);
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert!(issues[0].to_string().contains("'broken'"), "{}", issues[0]);
        assert!(issues[0].to_string().contains("radius"), "{}", issues[0]);
        assert!(issues[1].to_string().contains("'zone 3'"), "{}", issues[1]);

        let checked = def.check(&schemas());
        assert_eq!(checked.len(), 2, "one each: {checked:?}");
        assert!(checked.iter().any(|i| i.zone.as_deref() == Some("broken")));
    }

    #[test]
    fn a_wrong_type_is_named_with_its_key() {
        let def = scene(
            r#"
[wooze]
[[wooze.zone]]
name = "wrong"
shape = "sphere"
center = [0, 0, 0]
radius = "six"
glide = 0.5
"#,
        )
        .wooze
        .unwrap();
        let issues = def.check(&schemas());
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].zone.as_deref(), Some("wrong"));
        assert!(issues[0].message.contains("radius"), "{issues:?}");
    }

    /// A game's schemas load after the engine's, so a game can keep its own world
    /// within narrower limits, and `flint validate` holds a scene to them.
    #[test]
    fn a_games_schema_can_tighten_what_a_zone_may_be() {
        let mut schemas = schemas();
        schemas
            .load_component_string(
                r#"
[component.wooze_zone.fields]
glide = { type = "f32", min = 0.0, max = 0.5 }
[component.wooze.fields]
step = { type = "f32", min = 1.0 }
"#,
            )
            .unwrap();
        let def = scene(GROVE).wooze.unwrap();
        let issues = def.check(&schemas);
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert_eq!(issues[0].zone, None);
        assert!(issues[0].message.contains("step"), "{issues:?}");
        assert_eq!(issues[1].zone.as_deref(), Some("pocket"));
        assert!(issues[1].message.contains("glide"), "{issues:?}");
    }

    #[test]
    fn a_bad_global_knob_or_preset_is_an_issue_with_no_zone() {
        for bad in ["glide = 2.0", "step = 0.0", "preset = \"nope\""] {
            let def = scene(&format!("[wooze]\n{bad}\n")).wooze.unwrap();
            let issues = def.check(&schemas());
            assert_eq!(issues.len(), 1, "{bad}: {issues:?}");
            assert_eq!(issues[0].zone, None);
            assert_eq!(def.field().0.sample([0.0; 3], 0.0), WoozeSample::NEUTRAL);
        }
    }

    #[test]
    fn a_misspelt_global_knob_fails_the_table() {
        let bad = toml::from_str::<SceneFile>("[scene]\nname = \"g\"\n[wooze]\ngilde = 0.3\n");
        assert!(bad.is_err());
    }

    /// The schema is what agents read and games override, the code is what runs,
    /// so they have to say the same thing.
    #[test]
    fn the_schema_and_the_field_agree_on_every_knob() {
        let schemas = schemas();
        let wooze = schemas
            .get_component("wooze")
            .expect("schemas/components/wooze.toml");
        let zone = schemas
            .get_component("wooze_zone")
            .expect("the zone's schema");
        let ranges = [
            ("amplitude", Some(0.0), Some(f64::from(AMPLITUDE_MAX))),
            ("step", Some(f64::from(STEP_MIN)), None),
            ("glide", Some(0.0), Some(1.0)),
            ("jitter", Some(0.0), Some(f64::from(JITTER_MAX))),
        ];
        for schema in [wooze, zone] {
            for (name, min, max) in ranges {
                let field = schema.get_field(name).unwrap_or_else(|| panic!("{name}"));
                let close = |a: Option<f64>, b: Option<f64>| match (a, b) {
                    (Some(a), Some(b)) => (a - b).abs() < 1e-6,
                    (None, None) => true,
                    _ => false,
                };
                assert!(close(field.min, min), "{name} min {:?}", field.min);
                assert!(close(field.max, max), "{name} max {:?}", field.max);
            }
        }
        let mut names: Vec<&str> = zone.field_names();
        names.sort_unstable();
        let mut keys = flint_core::wooze::ZONE_KEYS.to_vec();
        keys.sort_unstable();
        assert_eq!(names, keys);
    }
}
