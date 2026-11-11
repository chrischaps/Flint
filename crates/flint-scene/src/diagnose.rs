//! Everything wrong with a scene, in one pass (ADR 0115).
//!
//! [`diagnose`] reads a scene file and returns a [`Diagnostic`] for each
//! problem it has, never stopping at the first: a file that is not TOML, a
//! prefab that doesn't expand, an archetype or a component no loaded schema
//! declares, what [`flint_schema::validate_component`] finds in each
//! component table, and an entity that names another the scene doesn't have
//! ([`diagnose_references`]). Every one names its file, entity, component
//! and field.
//!
//! The loaders run the same pass ([`diagnose_scene`]) over what they parsed
//! and log a summary, so a scene that plays has still said what is wrong with
//! it; `flint validate` prints the whole list and fails on an error.

use crate::format::{EntityDef, SceneFile};
use crate::prefab;
use flint_core::diagnostic::{self, codes, Diagnostic};
use flint_core::project::Project;
use flint_core::vfs;
use flint_schema::{validate_component, SchemaRegistry};
use std::path::Path;

/// Every problem in the scene at `scene_file`, prefab instances expanded as
/// the loader expands them (their entities are named `<prefix>_<name>`).
/// `registry` is the schemas of every loaded directory: a component or
/// archetype declared in any of them is known. `project` finds the prefabs,
/// and its `[project] strict = true` makes an unknown field an error, as
/// [`diagnostic::promote_strict`] does for `flint validate --strict`.
///
/// Entities are reported in name order, so two runs print the same list.
pub fn diagnose(
    scene_file: &Path,
    registry: &SchemaRegistry,
    project: &Project,
) -> Vec<Diagnostic> {
    let content = match vfs::read_to_string(scene_file) {
        Ok(content) => content,
        Err(e) => {
            return vec![Diagnostic::error(codes::FILE_UNREADABLE, e.to_string()).file(scene_file)]
        }
    };
    let mut scene: SceneFile = match toml::from_str(&content) {
        Ok(scene) => scene,
        Err(e) => return vec![parse_diagnostic(scene_file, &e)],
    };
    let mut found = Vec::new();
    if let Err(e) = prefab::expand_prefabs_in(&mut scene, scene_file, project) {
        found.push(
            Diagnostic::error(codes::BAD_PREFAB, e.to_string())
                .file(scene_file)
                .hint("the entities of the prefabs that did expand are checked below"),
        );
    }
    let expanded = found.is_empty();
    found.extend(diagnose_scene(&scene, Some(scene_file), registry));
    // With a prefab missing, an entity that names one of its entities
    // would be called dangling: the prefab is the one thing wrong there
    if expanded {
        found.extend(diagnose_references(&scene, Some(scene_file)));
    }
    if project.strict {
        diagnostic::promote_strict(&mut found);
    }
    found
}

/// A file that is not TOML, or not a scene, as a diagnostic naming it.
pub(crate) fn parse_diagnostic(file: &Path, error: &toml::de::Error) -> Diagnostic {
    Diagnostic::error(codes::TOML_PARSE, error.to_string().trim_end()).file(file)
}

/// [`diagnose`] for a scene already parsed (and prefab-expanded): the
/// entities' archetypes and components against `registry`. `file` is what
/// each diagnostic names. What the loaders run on every load, and the player
/// on a chunk.
pub fn diagnose_scene(
    scene: &SceneFile,
    file: Option<&Path>,
    registry: &SchemaRegistry,
) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    if scene.entities.is_empty() {
        return found;
    }
    // With no schema at all every table would be "unknown": say the one
    // thing that is wrong instead
    if registry.component_names().is_empty() && registry.archetype_names().is_empty() {
        found.push(
            Diagnostic::warning(
                codes::NO_SCHEMAS,
                "no schema is loaded, so no archetype or component was checked",
            )
            .hint(
                "pass --schemas <dir>, or list the directories under [project] schemas in \
                 flint.toml",
            ),
        );
    } else {
        let mut names: Vec<&String> = scene.entities.keys().collect();
        names.sort_unstable();
        for name in names {
            diagnose_entity(name, &scene.entities[name], registry, &mut found);
        }
    }
    if let Some(file) = file {
        for d in &mut found {
            d.file = Some(file.to_path_buf());
        }
    }
    found
}

/// The fields that name another entity of the scene, by component. An empty
/// string is "none" for each of them (a joint then uses the transform
/// parent; an IK chain waits for a script to name its target).
///
/// Hard-coded until the schema language has an `entity` field type (#684,
/// ADR 0115 decision 4), which will say this in the schema files and check
/// it for a game's own components too.
pub const ENTITY_REFERENCES: &[(&str, &[&str])] = &[
    (flint_core::components::JOINT, &["parent"]),
    (
        flint_core::components::IK_TWO_BONE,
        &["target", "pole", "tip"],
    ),
];

/// Every entity of a whole scene (prefabs expanded) that names an entity the
/// scene doesn't have: its `parent`, which fails the load, and the fields of
/// [`ENTITY_REFERENCES`], which the runtime skips with a warning at best. An
/// `ik_two_bone.tip` must also be a child of the entity that names it.
///
/// Not for a chunk, whose entities may name the base scene's.
pub fn diagnose_references(scene: &SceneFile, file: Option<&Path>) -> Vec<Diagnostic> {
    let mut found = Vec::new();
    let mut names: Vec<&String> = scene.entities.keys().collect();
    names.sort_unstable();
    let dangling = |entity: &str, named: &str| {
        Diagnostic::error(
            codes::DANGLING_ENTITY,
            format!("names the entity `{named}`, and the scene has none of that name"),
        )
        .entity(entity)
        .hint_opt(diagnostic::did_you_mean(
            named,
            names.iter().map(|n| n.as_str()),
        ))
    };
    for name in &names {
        let entity = &scene.entities[*name];
        if let Some(parent) = entity.parent.as_deref() {
            if !scene.entities.contains_key(parent) {
                let d = dangling(name, parent).field("parent");
                found.push(Diagnostic {
                    message: format!("{}; the scene does not load", d.message),
                    ..d
                });
            }
        }
        for (component, fields) in ENTITY_REFERENCES {
            let Some(data) = entity.components.get(*component) else {
                continue;
            };
            for field in *fields {
                let Some(named) = data.get(field).and_then(|v| v.as_str()) else {
                    continue; // unset, or of a type the schema pass names
                };
                if named.is_empty() {
                    continue;
                }
                match scene.entities.get(named) {
                    None => found.push(dangling(name, named).component(*component).field(*field)),
                    Some(tip)
                        if *component == flint_core::components::IK_TWO_BONE
                            && *field == "tip"
                            && tip.parent.as_deref() != Some(name.as_str()) =>
                    {
                        found.push(
                            Diagnostic::error(
                                codes::DANGLING_ENTITY,
                                format!(
                                    "`{named}` is not a child of this entity, so the chain is \
                                     never solved"
                                ),
                            )
                            .entity(*name)
                            .component(*component)
                            .field(*field)
                            .hint(format!("give `{named}` parent = \"{name}\"")),
                        );
                    }
                    Some(_) => {}
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

/// What to do about a table named after a kind of light: nothing reads it,
/// and until #681 nothing said so.
fn light_table_hint(component: &str) -> Option<String> {
    let kind = component.strip_suffix("_light")?;
    ["directional", "point", "spot"].contains(&kind).then(|| {
        format!(
            "no system reads a `{component}` table: a light is a `light` component with \
             type = \"{kind}\" (`flint schema light` lists its fields)"
        )
    })
}

fn diagnose_entity(
    name: &str,
    entity: &EntityDef,
    registry: &SchemaRegistry,
    found: &mut Vec<Diagnostic>,
) {
    let archetype = entity.archetype.as_deref().and_then(|archetype| {
        let schema = registry.get_archetype(archetype);
        if schema.is_none() {
            found.push(
                Diagnostic::error(
                    codes::UNKNOWN_ARCHETYPE,
                    format!("no archetype named `{archetype}`, so the entity gets no defaults"),
                )
                .entity(name)
                .field("archetype")
                .hint(
                    diagnostic::did_you_mean(archetype, registry.archetype_names()).unwrap_or_else(
                        || {
                            "an archetype is declared in a schema directory's archetypes/*.toml; \
                             `flint schema --list` names the ones loaded"
                                .to_string()
                        },
                    ),
                ),
            );
        }
        schema
    });

    let mut components: Vec<(&String, &toml::Value)> = entity.components.iter().collect();
    components.sort_unstable_by_key(|(component, _)| *component);
    for (component, data) in components {
        let Some(schema) = registry.get_component(component) else {
            found.push(
                Diagnostic::error(
                    codes::UNKNOWN_COMPONENT,
                    format!("no component named `{component}`, so nothing checks it"),
                )
                .entity(name)
                .component(component.clone())
                .hint(
                    light_table_hint(component)
                        .or_else(|| diagnostic::did_you_mean(component, registry.component_names()))
                        .unwrap_or_else(|| {
                            format!(
                                "a component a game adds is declared in its schema layer, \
                                 as components/{component}.toml"
                            )
                        }),
                ),
            );
            continue;
        };
        for d in validate_component(schema, data) {
            // A required field that the archetype's defaults or the schema's
            // own default supplies is there once the scene is loaded
            let supplied = d.code == codes::MISSING_FIELD
                && d.field.as_deref().is_some_and(|field| {
                    archetype.is_some_and(|a| a.get_default(component, field).is_some())
                        || schema.get_field(field).is_some_and(|f| f.default.is_some())
                });
            if !supplied {
                found.push(d.entity(name));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flint_core::vfs::MemorySource;
    use flint_core::Severity;
    use std::sync::Arc;

    const SCHEMAS: &[(&str, &str)] = &[
        (
            "schemas/components/light.toml",
            r#"
[component.light]
description = "A light"

[component.light.fields]
type = { type = "enum", values = ["point", "spot", "directional"], default = "point" }
intensity = { type = "f32", default = 1.0, min = 0.0 }
"#,
        ),
        (
            "schemas/components/door.toml",
            r#"
[component.door]
[component.door.fields]
style = { type = "enum", values = ["hinged", "sliding"], required = true }
locked = { type = "bool", default = false }
"#,
        ),
        (
            "schemas/components/transform.toml",
            r#"
[component.transform]
[component.transform.fields]
position = { type = "vec3", default = [0, 0, 0] }
"#,
        ),
        (
            "schemas/archetypes/door.toml",
            r#"
[archetype.door]
components = ["transform", "door"]

[archetype.door.defaults.door]
style = "hinged"
"#,
        ),
        // A game's own layer: a component the engine has never heard of
        (
            "game/schemas/components/loot.toml",
            r#"
[component.loot]
[component.loot.fields]
gold = { type = "i32", default = 0 }
"#,
        ),
    ];

    /// Run `body` over the test schemas plus `files`, with the registry of
    /// both schema directories and the project found from the scenes folder.
    fn with_scene<T>(
        files: &[(&str, &str)],
        body: impl FnOnce(&SchemaRegistry, &Project) -> T,
    ) -> T {
        let mut source = MemorySource::new();
        for (path, text) in SCHEMAS.iter().chain(files) {
            source.insert(*path, *text);
        }
        vfs::scoped(Arc::new(source), || {
            let registry =
                SchemaRegistry::load_from_directories(&["schemas", "game/schemas"]).unwrap();
            let project = Project::for_scene(Path::new("scenes/a.scene.toml"));
            body(&registry, &project)
        })
    }

    fn found_in(scene: &str) -> Vec<Diagnostic> {
        with_scene(&[("scenes/a.scene.toml", scene)], |registry, project| {
            diagnose(Path::new("scenes/a.scene.toml"), registry, project)
        })
    }

    /// The ticket's first two boxes at once: a misspelled field, component,
    /// archetype and enum value in one scene are all reported, each naming
    /// its entity, component and field, with a suggestion.
    #[test]
    fn every_mistake_in_a_scene_is_reported_in_one_pass() {
        let found = found_in(
            r#"
[scene]
name = "Broken"

[entities.lamp.lihgt]
intensity = 2.0

[entities.sun.light]
type = "directonal"
intensty = 3.0

[entities.gate]
archetype = "dooor"
[entities.gate.door]
style = "hinged"
locked = "no"
"#,
        );
        let said: Vec<String> = found.iter().map(|d| d.to_string()).collect();
        assert_eq!(
            said,
            [
                "error[unknown-archetype] entity 'gate' field 'archetype': no archetype named \
                 `dooor`, so the entity gets no defaults",
                "error[bad-type] entity 'gate' door.locked: expected bool, got string",
                "error[unknown-component] entity 'lamp' component 'lihgt': no component named \
                 `lihgt`, so nothing checks it",
                "warning[unknown-field] entity 'sun' light.intensty: `light` has no field \
                 `intensty`",
                "error[bad-enum-value] entity 'sun' light.type: \"directonal\" is not a value \
                 of `type`",
            ]
        );
        let hints: Vec<&str> = found
            .iter()
            .map(|d| d.hint.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(
            hints,
            [
                "did you mean `door`?",
                "",
                "did you mean `light`?",
                "did you mean `intensity`?",
                "did you mean `directional`? allowed values: directional, point, spot",
            ]
        );
        assert!(found
            .iter()
            .all(|d| d.file.as_deref() == Some(Path::new("scenes/a.scene.toml"))));
    }

    #[test]
    fn a_clean_scene_says_nothing() {
        let found = found_in(
            r#"
[scene]
name = "Clean"

[entities.gate]
archetype = "door"
[entities.gate.door]
locked = true
[entities.gate.transform]
position = [1, 0, 2]
"#,
        );
        assert_eq!(found, [], "the archetype supplies door.style");
    }

    /// A component declared in any loaded schema directory is known: the
    /// game's layer counts as the engine's does.
    #[test]
    fn a_component_of_the_games_layer_is_known() {
        let scene = "[scene]\nname = \"s\"\n[entities.chest.loot]\ngold = 5\n";
        assert_eq!(found_in(scene), []);
        // ...and with only the engine's directory it is unknown, and the
        // hint says where to declare it
        let found = with_scene(&[("scenes/a.scene.toml", scene)], |_, project| {
            let engine_only = SchemaRegistry::load_from_directories(&["schemas"]).unwrap();
            diagnose(Path::new("scenes/a.scene.toml"), &engine_only, project)
        });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, codes::UNKNOWN_COMPONENT);
        assert_eq!(found[0].severity, Severity::Error);
        assert_eq!(
            found[0].hint.as_deref(),
            Some(
                "a component a game adds is declared in its schema layer, as \
                 components/loot.toml"
            )
        );
    }

    #[test]
    fn a_missing_required_field_is_an_error_unless_the_archetype_supplies_it() {
        let found = found_in("[scene]\nname = \"s\"\n[entities.gate.door]\nlocked = true\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, codes::MISSING_FIELD);
        assert_eq!(found[0].entity.as_deref(), Some("gate"));
        assert_eq!(found[0].component.as_deref(), Some("door"));
        assert_eq!(found[0].field.as_deref(), Some("style"));
    }

    #[test]
    fn a_number_out_of_range_is_an_error() {
        let found = found_in("[scene]\nname = \"s\"\n[entities.sun.light]\nintensity = -1.0\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, codes::OUT_OF_RANGE);
        assert_eq!(found[0].message, "-1 is below the minimum 0");
    }

    #[test]
    fn a_file_that_is_not_toml_names_the_file() {
        let found = found_in("[scene]\nname = \"s\"\n[entities.lamp\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, codes::TOML_PARSE);
        assert_eq!(
            found[0].file.as_deref(),
            Some(Path::new("scenes/a.scene.toml"))
        );
        assert!(found[0].message.contains("line 3"), "{}", found[0].message);
        assert!(
            found[0]
                .one_line()
                .starts_with("scenes/a.scene.toml: error[toml-parse]: "),
            "{}",
            found[0].one_line()
        );
    }

    #[test]
    fn a_file_that_is_not_there_is_a_diagnostic() {
        let found = with_scene(&[], |registry, project| {
            diagnose(Path::new("scenes/gone.scene.toml"), registry, project)
        });
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, codes::FILE_UNREADABLE);
        assert_eq!(
            found[0].file.as_deref(),
            Some(Path::new("scenes/gone.scene.toml"))
        );
    }

    /// The loader's own error names the file as well (the ticket's fourth
    /// box): the TOML parser's message has only a line and a column.
    #[test]
    fn the_loaders_parse_error_names_the_file() {
        let scene = "[scene]\nname = \"s\"\n[entities.lamp\n";
        with_scene(&[("scenes/a.scene.toml", scene)], |registry, _| {
            let err = crate::load_scene("scenes/a.scene.toml", registry)
                .err()
                .expect("not TOML")
                .to_string();
            assert!(
                err.starts_with(
                    "TOML parse error: scenes/a.scene.toml: TOML parse error at line 3"
                ),
                "{err}"
            );
            // From a string there is no file to name
            let err = crate::load_scene_string(scene, registry)
                .err()
                .expect("not TOML")
                .to_string();
            assert!(
                err.starts_with("TOML parse error: TOML parse error at line 3"),
                "{err}"
            );
        });
    }

    /// A load never fails on what `diagnose_scene` finds, and hands the
    /// list back to a host that asks for it.
    #[test]
    fn a_scene_with_mistakes_still_loads_and_returns_them() {
        let scene = "[scene]\nname = \"s\"\n[entities.gate]\narchetype = \"dooor\"\n\
                     [entities.gate.lihgt]\nintensity = 1.0\n";
        with_scene(&[("scenes/a.scene.toml", scene)], |registry, _| {
            let (world, _, found) =
                crate::load_scene_diagnosed("scenes/a.scene.toml", registry).expect("loads");
            assert!(world.contains_name("gate"));
            let codes_found: Vec<&str> = found.iter().map(|d| d.code).collect();
            assert_eq!(
                codes_found,
                [codes::UNKNOWN_ARCHETYPE, codes::UNKNOWN_COMPONENT]
            );
        });
    }

    /// A prefab's entities are checked as the scene's own, under the names
    /// the loader gives them; one that can't be found doesn't stop the pass.
    #[test]
    fn prefab_entities_are_diagnosed_and_a_missing_prefab_does_not_stop_the_pass() {
        let found = with_scene(
            &[
                (
                    "scenes/a.scene.toml",
                    r#"
[scene]
name = "s"

[prefabs.one]
template = "gate"
prefix = "north"

[entities.lamp.lihgt]
intensity = 1.0
"#,
                ),
                (
                    "prefabs/gate.prefab.toml",
                    "[prefab]\nname = \"gate\"\n\n[entities.leaf.door]\nstyle = \"swinging\"\n",
                ),
            ],
            |registry, project| diagnose(Path::new("scenes/a.scene.toml"), registry, project),
        );
        let said: Vec<(&str, Option<&str>)> = found
            .iter()
            .map(|d| (d.code, d.entity.as_deref()))
            .collect();
        assert_eq!(
            said,
            [
                (codes::UNKNOWN_COMPONENT, Some("lamp")),
                (codes::BAD_ENUM_VALUE, Some("north_leaf")),
            ]
        );

        let missing = found_in(
            "[scene]\nname = \"s\"\n[prefabs.one]\ntemplate = \"nope\"\nprefix = \"n\"\n\
             [entities.lamp.lihgt]\nintensity = 1.0\n",
        );
        let codes_found: Vec<&str> = missing.iter().map(|d| d.code).collect();
        assert_eq!(codes_found, [codes::BAD_PREFAB, codes::UNKNOWN_COMPONENT]);
    }

    /// #687: an entity that names another the scene doesn't have. The
    /// fields are the ones [`ENTITY_REFERENCES`] lists, plus `parent`.
    #[test]
    fn an_entity_reference_to_nothing_is_an_error_naming_the_field() {
        let scene = r#"
[scene]
name = "refs"

[entities.anchor]
[entities.lid]
parent = "ancor"
[entities.door.joint]
parent = "frame"
[entities.gate.joint]
parent = ""
[entities.hinge.joint]
parent = "anchor"

[entities.shoulder]
[entities.elbow]
parent = "shoulder"
[entities.elbow.ik_two_bone]
target = "reach"
pole = "anchor"
tip = "hand"
[entities.hand]
"#;
        let found: Vec<Diagnostic> = toml::from_str::<SceneFile>(scene)
            .map(|scene| diagnose_references(&scene, Some(Path::new("a.scene.toml"))))
            .unwrap();
        let said: Vec<String> = found.iter().map(|d| d.to_string()).collect();
        assert_eq!(
            said,
            [
                "error[dangling-entity] entity 'door' joint.parent: names the entity `frame`, \
                 and the scene has none of that name",
                "error[dangling-entity] entity 'elbow' ik_two_bone.target: names the entity \
                 `reach`, and the scene has none of that name",
                "error[dangling-entity] entity 'elbow' ik_two_bone.tip: `hand` is not a child \
                 of this entity, so the chain is never solved",
                "error[dangling-entity] entity 'lid' field 'parent': names the entity `ancor`, \
                 and the scene has none of that name; the scene does not load",
            ]
        );
        assert_eq!(found[3].hint.as_deref(), Some("did you mean `anchor`?"));
        assert!(found
            .iter()
            .all(|d| d.file.as_deref() == Some(Path::new("a.scene.toml"))));
    }

    /// A reference into a prefab's entities is found once the prefab is
    /// expanded, and `diagnose` is where a whole scene's are checked.
    #[test]
    fn references_are_checked_after_prefabs_expand() {
        let prefab = "[prefab]\nname = \"gate\"\n\n[entities.leaf.door]\nstyle = \"hinged\"\n";
        let scene =
            "[scene]\nname = \"s\"\n[prefabs.one]\ntemplate = \"gate\"\nprefix = \"north\"\n\
                     [entities.latch.joint]\nparent = \"north_leaf\"\n\
                     [entities.bolt.joint]\nparent = \"south_leaf\"\n";
        let found = with_scene(
            &[
                ("scenes/a.scene.toml", scene),
                ("prefabs/gate.prefab.toml", prefab),
            ],
            |registry, project| diagnose(Path::new("scenes/a.scene.toml"), registry, project),
        );
        let said: Vec<(&str, Option<&str>)> = found
            .iter()
            .filter(|d| d.code != codes::UNKNOWN_COMPONENT)
            .map(|d| (d.code, d.entity.as_deref()))
            .collect();
        assert_eq!(said, [(codes::DANGLING_ENTITY, Some("bolt"))]);
    }

    /// A table named after a kind of light is an unknown component whose
    /// hint says what a light is.
    #[test]
    fn a_stale_light_table_says_what_a_light_is() {
        let found =
            found_in("[scene]\nname = \"s\"\n[entities.sun.directional_light]\nintensity = 2.0\n");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, codes::UNKNOWN_COMPONENT);
        let hint = found[0].hint.as_deref().unwrap();
        assert!(hint.contains("type = \"directional\""), "{hint}");
    }

    #[test]
    fn strict_in_the_manifest_makes_an_unknown_field_an_error() {
        let scene = "[scene]\nname = \"s\"\n[entities.sun.light]\nintensty = 3.0\n";
        let lenient = found_in(scene);
        assert_eq!(lenient[0].severity, Severity::Warning);
        let strict = with_scene(
            &[
                ("scenes/a.scene.toml", scene),
                ("flint.toml", "[project]\nstrict = true\n"),
            ],
            |registry, project| {
                assert!(project.strict);
                diagnose(Path::new("scenes/a.scene.toml"), registry, project)
            },
        );
        assert_eq!(strict.len(), 1);
        assert_eq!(strict[0].code, codes::UNKNOWN_FIELD);
        assert_eq!(strict[0].severity, Severity::Error);
    }

    /// With no schema loaded, one warning says so; every table is not
    /// called unknown.
    #[test]
    fn an_empty_registry_is_one_warning() {
        let scene: SceneFile =
            toml::from_str("[scene]\nname = \"s\"\n[entities.a.light]\n[entities.b.door]\n")
                .unwrap();
        let found = diagnose_scene(&scene, None, &SchemaRegistry::new());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, codes::NO_SCHEMAS);
        assert_eq!(found[0].severity, Severity::Warning);
        assert_eq!(found[0].file, None);
    }
}
