//! The schema of a per-entity style's component, made from its descriptor
//! (#556).
//!
//! A built-in style's component (`ascii_style`) has a schema file in
//! `schemas/components/`, so the scene loader holds an entity's settings to
//! it. A user style loads at run time from a `.shader.toml`, and its
//! `<name>_style` component had no schema, or an open one that checks
//! nothing: `hologram_style { glwo = 2 }` or `glow = "bright"` was dropped
//! by [`read_overrides`](crate::style_mask::read_overrides) without a word.
//! [`component_schema`] is that schema, made from what the style declares:
//!
//! - each parameter an `f32` with the parameter's range, each channel of a
//!   colour (`tint_r`) too, and the colour itself (`tint`) a `vec3` with
//!   the channels' range;
//! - each option an `enum` of its choices (an image option's are the files
//!   it offers);
//! - `enabled` (`bool`), `blend` (`f32`, 0..1) and `blend_source` (`own` or
//!   `tear`), which every per-entity style takes.
//!
//! No field has a default, as in `ascii_style.toml`: the loader writes a
//! schema default into the component, and that would hide the scene's
//! `[post_process.styles.<name>]` setting.
//!
//! Two places use it. A host that checks a scene before it loads
//! (`flint check`, `flint validate`) registers each one into its
//! [`SchemaRegistry`] with [`add_component_schemas`], so the scene's
//! diagnostics name the field with the code a built-in's would get. A host
//! that draws has already loaded the scene by the time it knows the user
//! styles, so [`SceneRenderer::bind_scene`](crate::SceneRenderer::bind_scene)
//! warns with [`user_style_component_warnings`], in the same words.

use super::{StyleDescriptor, StyleRegistry};
use crate::style_mask::{BLEND_FIELD, BLEND_RANGE, BLEND_SOURCES, BLEND_SOURCE_FIELD};
use flint_core::diagnostic::Diagnostic;
use flint_ecs::FlintWorld;
use flint_schema::{ComponentSchema, FieldSchema, FieldType, SchemaRegistry};
use std::collections::HashMap;

/// The field every per-entity style takes that keeps an entity out of it.
pub const ENABLED_FIELD: &str = "enabled";

/// The schema of `d`'s component, or `None` for a whole-frame style (one
/// with no component).
pub fn component_schema(d: &StyleDescriptor) -> Option<ComponentSchema> {
    let component = d.component?;
    let mut fields = HashMap::new();
    let ranged = |field_type: FieldType, min: f32, max: f32, description: String| FieldSchema {
        min: Some(bound(min)),
        max: Some(bound(max)),
        description: Some(description),
        ..FieldSchema::of(field_type)
    };
    for p in d.params.iter() {
        let description = match d.color_of(p.name) {
            Some(color) => format!(
                "One channel of `{}` ({}), 0..1 display value",
                color.base,
                d.label_for(color.base)
            ),
            None => described(d, p.name),
        };
        fields.insert(
            p.name.to_string(),
            ranged(
                FieldType::F32,
                p.min,
                p.max,
                clamped(description, p.min, p.max),
            ),
        );
    }
    for color in d.colors() {
        // The channels' widest range: the schema has one range for a vec3
        let min = color
            .channels
            .iter()
            .map(|c| c.min)
            .fold(f32::MAX, f32::min);
        let max = color
            .channels
            .iter()
            .map(|c| c.max)
            .fold(f32::MIN, f32::max);
        let description = format!(
            "{}: [r, g, b], 0..1 display values; sets `{0}_r`, `{0}_g` and `{0}_b`",
            color.base
        );
        let description = format!("{} ({description})", d.label_for(color.base));
        fields.insert(
            color.base.to_string(),
            ranged(FieldType::Vec3, min, max, description),
        );
    }
    for o in d.options.iter() {
        let mut field = FieldSchema::of(FieldType::Enum {
            values: o.choices.iter().map(|c| c.to_string()).collect(),
        });
        if o.choices.is_empty() || o.discover.is_some() {
            // Any text: what the style finds at run time is its own to judge
            field = FieldSchema::of(FieldType::String);
        }
        field.description = Some(described(d, o.name));
        fields.insert(o.name.to_string(), field);
    }
    fields.insert(
        ENABLED_FIELD.to_string(),
        FieldSchema {
            description: Some(format!(
                "false keeps this entity and its subtree out of {}",
                d.label
            )),
            ..FieldSchema::of(FieldType::Bool)
        },
    );
    fields.insert(
        BLEND_FIELD.to_string(),
        ranged(
            FieldType::F32,
            BLEND_RANGE.min,
            BLEND_RANGE.max,
            format!(
                "How much of {} this entity (and its subtree) is drawn with: 0 its own \
                 shading, 1 the style (the default when left out)",
                d.label
            ),
        ),
    );
    fields.insert(
        BLEND_SOURCE_FIELD.to_string(),
        FieldSchema {
            description: Some(
                "What the blend follows: own (the default), or the reality tear at each pixel"
                    .to_string(),
            ),
            ..FieldSchema::of(FieldType::Enum {
                values: BLEND_SOURCES.iter().map(|s| s.to_string()).collect(),
            })
        },
    );
    Some(ComponentSchema {
        name: component.to_string(),
        description: Some(format!(
            "Per-entity settings of the style '{}' ({}), made from its declared params and \
             options when it loaded (#556). Every field is optional: one left out comes from \
             the scene's [post_process.styles.{}] table, then the style's default",
            d.name, d.label, d.name
        )),
        fields,
        panel: None,
    })
}

/// Register the component schema of each style in `styles` that has a
/// component into `registry`, where the registry has none for it or only
/// an open one (`[component.hologram_style]` with no fields, which names it
/// and checks nothing). A schema a project wrote with its own fields is
/// kept: the author said what the component takes. The names registered,
/// in registry order.
pub fn add_component_schemas(styles: &StyleRegistry, registry: &mut SchemaRegistry) -> Vec<String> {
    let mut added = Vec::new();
    for d in styles.iter() {
        let Some(component) = d.component else {
            continue;
        };
        if registry
            .get_component(component)
            .is_some_and(|s| !s.is_open())
        {
            continue;
        }
        if let Some(schema) = component_schema(d) {
            registry.register_component(schema);
            added.push(component.to_string());
        }
    }
    added
}

/// What is wrong with each entity's user-style component (`hologram_style`)
/// against [`component_schema`]: one diagnostic a field, by entity name,
/// each naming the entity, the component and the field, as the scene
/// loader says it of a built-in's (`warning[unknown-field] entity 'barrel'
/// hologram_style.glwo: ...`). A field past its range is drawn clamped into
/// it, and the hint says so.
///
/// A built-in or a game's style is the schema files' to check, so only
/// user styles are looked at; `only`, when given, narrows them to those
/// names (what a hot reload replaced).
pub fn user_style_component_problems(
    world: &FlintWorld,
    styles: &StyleRegistry,
    only: Option<&[String]>,
) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    for d in styles.iter() {
        if !styles.is_user_style(d.name) || only.is_some_and(|o| !o.iter().any(|n| n == d.name)) {
            continue;
        }
        let Some(schema) = component_schema(d) else {
            continue;
        };
        let mut lines: Vec<(String, Diagnostic)> = Vec::new();
        for &id in world.entities_with_component(&schema.name) {
            let Some(data) = world.get_component(id, &schema.name) else {
                continue;
            };
            let name = world.get_name(id).unwrap_or("?").to_string();
            for mut problem in flint_schema::validate_component(&schema, data) {
                if problem.code == flint_core::diagnostic::codes::OUT_OF_RANGE {
                    problem.hint = Some(format!(
                        "the user style '{}' draws it clamped into its range",
                        d.name
                    ));
                }
                lines.push((name.clone(), problem.entity(name.clone())));
            }
        }
        lines.sort_by(|a, b| a.0.cmp(&b.0));
        found.extend(lines.into_iter().map(|(_, d)| d));
    }
    found
}

/// [`user_style_component_problems`] as the lines a drawing host logs
/// (`bind_scene`, a hot reload), less what the scene's project allows
/// (`[project.diagnostics] allow`, as a load drops it).
pub fn user_style_component_warnings(
    world: &FlintWorld,
    styles: &StyleRegistry,
    allow: &[flint_core::diagnostic::Allow],
    only: Option<&[String]>,
) -> Vec<String> {
    let mut found = user_style_component_problems(world, styles, only);
    flint_core::diagnostic::drop_allowed(&mut found, allow);
    found.iter().map(Diagnostic::one_line).collect()
}

/// A range's end as the schema holds it: the decimal the style wrote
/// (`0.2`), not the f32's exact value widened (`0.20000000298`), which a
/// scene's `0.2` would fall below.
fn bound(x: f32) -> f64 {
    x.to_string().parse().unwrap_or(x as f64)
}

/// A setting's label and help, as F4 shows them.
fn described(d: &StyleDescriptor, name: &str) -> String {
    let label = d.label_for(name);
    match d.help_for(name) {
        "" => label,
        help => format!("{label}: {help}"),
    }
}

/// `description`, saying what a value past `min..max` does.
fn clamped(description: String, min: f32, max: f32) -> String {
    format!("{description} ({min}..{max}; a value past it is drawn clamped into it)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::diagnostic::codes;

    /// A user style's descriptor as a `.shader.toml` gives it.
    fn hologram() -> StyleRegistry {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "flint-style-schema-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        #[allow(clippy::disallowed_methods)] // writes a test style
        {
            std::fs::create_dir_all(dir.join("styles")).unwrap();
            std::fs::write(
                dir.join("styles/holo556.shader.toml"),
                r#"version = 1
name = "holo556"
stage = "display"
shader = "holo556.wgsl"
inputs = ["mask"]

[params.glow]
default = 0.8
min = 0.0
max = 2.0

[params.lines]
default = 3.0
min = 0.2
max = 12.0

[params.tint_r]
default = 1.0
min = 0.0
max = 1.0
[params.tint_g]
default = 1.0
min = 0.0
max = 1.0
[params.tint_b]
default = 1.0
min = 0.0
max = 1.0

[options.pattern]
kind = "choice"
default = "lines"
choices = ["lines", "dots"]
"#,
            )
            .unwrap();
            std::fs::write(
                dir.join("styles/holo556.wgsl"),
                "fn style(p: StylePixel) -> vec4<f32> {\n    return p.color;\n}\n",
            )
            .unwrap();
        }
        let (styles, messages) =
            StyleRegistry::new().for_scene(&flint_core::project::AssetScope::for_dir(&dir));
        assert!(messages.is_empty(), "{messages:?}");
        assert!(styles.is_user_style("holo556"));
        styles
    }

    fn holo(styles: &StyleRegistry) -> ComponentSchema {
        component_schema(styles.iter().find(|d| d.name == "holo556").unwrap()).unwrap()
    }

    #[test]
    fn a_user_styles_schema_has_its_params_options_and_the_per_entity_fields() {
        let styles = hologram();
        let schema = holo(&styles);
        assert_eq!(schema.name, "holo556_style");
        let mut names = schema.field_names();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "blend",
                "blend_source",
                "enabled",
                "glow",
                "lines",
                "pattern",
                "tint",
                "tint_b",
                "tint_g",
                "tint_r"
            ]
        );
        let glow = schema.get_field("glow").unwrap();
        assert_eq!(
            (glow.field_type.clone(), glow.min, glow.max),
            (FieldType::F32, Some(0.0), Some(2.0))
        );
        // The decimal the style wrote, not 0.2f32 widened
        assert_eq!(schema.get_field("lines").unwrap().min, Some(0.2));
        assert_eq!(
            schema.get_field("tint").unwrap().field_type,
            FieldType::Vec3
        );
        assert_eq!(
            schema.get_field("pattern").unwrap().field_type,
            FieldType::Enum {
                values: vec!["lines".into(), "dots".into()]
            }
        );
        assert!(
            schema.fields.values().all(|f| f.default.is_none()),
            "no default hides the scene's table"
        );
        // Every built-in with a component gets one too: a host whose
        // schema list leaves the engine's out still checks them
        for d in crate::stylize::builtins() {
            assert_eq!(
                component_schema(d).is_some(),
                d.component.is_some(),
                "{}",
                d.name
            );
        }
    }

    #[test]
    fn every_mistake_is_named_and_a_good_table_says_nothing() {
        let styles = hologram();
        let schema = holo(&styles);
        let table: toml::Value = toml::from_str(
            r#"
            glwo = 2
            glow = "bright"
            lines = 90
            pattern = "plaid"
            tint = [2.0, 0.5, 0.5]
            blend = 0.5
            "#,
        )
        .unwrap();
        let mut found: Vec<(String, String)> = flint_schema::validate_component(&schema, &table)
            .into_iter()
            .map(|d| (d.code.to_string(), d.field.unwrap_or_default()))
            .collect();
        found.sort();
        let want = [
            (codes::BAD_ENUM_VALUE, "pattern"),
            (codes::BAD_TYPE, "glow"),
            (codes::OUT_OF_RANGE, "lines"),
            (codes::OUT_OF_RANGE, "tint[0]"),
            (codes::UNKNOWN_FIELD, "glwo"),
        ];
        let mut want: Vec<(String, String)> = want
            .iter()
            .map(|(c, f)| (c.to_string(), f.to_string()))
            .collect();
        want.sort();
        assert_eq!(found, want);

        let fine: toml::Value = toml::from_str(
            "glow = 2\nlines = 0.2\ntint_r = 0.5\ntint = [1, 0.5, 0]\npattern = \"dots\"\n\
             enabled = true\nblend = 1\nblend_source = \"tear\"",
        )
        .unwrap();
        let said = flint_schema::validate_component(&schema, &fine);
        assert!(said.is_empty(), "{said:#?}");
    }

    #[test]
    fn an_open_schema_is_replaced_and_a_written_one_kept() {
        let styles = hologram();
        let mut registry = SchemaRegistry::new();
        registry.register_component(ComponentSchema {
            name: "holo556_style".into(),
            description: Some("open".into()),
            fields: HashMap::new(),
            panel: None,
        });
        let added = add_component_schemas(&styles, &mut registry);
        assert!(added.contains(&"holo556_style".to_string()), "{added:?}");
        // With no schema files at all, the built-ins' are made too
        assert!(added.contains(&"ascii_style".to_string()), "{added:?}");
        assert!(!registry.get_component("holo556_style").unwrap().is_open());

        let mut fields = HashMap::new();
        fields.insert("glow".to_string(), FieldSchema::of(FieldType::F64));
        let written = ComponentSchema {
            name: "holo556_style".into(),
            description: None,
            fields,
            panel: None,
        };
        registry.register_component(written.clone());
        let added = add_component_schemas(&styles, &mut registry);
        assert!(!added.contains(&"holo556_style".to_string()));
        assert_eq!(registry.get_component("holo556_style"), Some(&written));
    }

    #[test]
    fn an_entitys_misspelt_field_is_warned_by_name() {
        let styles = hologram();
        let mut world = FlintWorld::new();
        let barrel = world.spawn("barrel").unwrap();
        let table: toml::Value = toml::from_str("glwo = 2\nglow = 9").unwrap();
        world.set_component(barrel, "holo556_style", table).unwrap();
        let crate_ = world.spawn("crate").unwrap();
        let fine: toml::Value = toml::from_str("glow = 1").unwrap();
        world.set_component(crate_, "holo556_style", fine).unwrap();

        let lines = user_style_component_warnings(&world, &styles, &[], None);
        assert_eq!(lines.len(), 2, "{lines:#?}");
        assert!(
            lines.iter().any(|l| l
                .starts_with("warning[unknown-field] entity 'barrel' holo556_style.glwo:")
                && l.contains("glow")),
            "names the field and the nearest one: {lines:#?}"
        );
        assert!(
            lines.iter().any(|l| l
                .starts_with("error[out-of-range] entity 'barrel' holo556_style.glow:")
                && l.contains("clamped")),
            "{lines:#?}"
        );
        // A hot reload of another style says nothing of this one
        let other = ["inkline".to_string()];
        assert!(user_style_component_warnings(&world, &styles, &[], Some(&other)).is_empty());
        // What the project allows is dropped
        let allow = [flint_core::diagnostic::Allow::parse("unknown-field:holo556_style").unwrap()];
        assert_eq!(
            user_style_component_warnings(&world, &styles, &allow, None).len(),
            1
        );
    }
}
