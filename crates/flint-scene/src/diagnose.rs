//! Everything wrong with a scene, in one pass (ADR 0115).
//!
//! [`diagnose`] reads a scene file and returns a [`Diagnostic`] for each
//! problem it has, never stopping at the first: a file that is not TOML, a
//! prefab that doesn't expand, an archetype or a component no loaded schema
//! declares, what [`flint_schema::validate_component`] finds in each
//! component table, and each reference that leads nowhere: an `entity` field
//! (or an entity's `parent`) naming no entity of the scene, an `asset` field
//! naming no file the project's asset search finds (#684). Every one names
//! its file, entity, component and field.
//!
//! The loaders run the same pass ([`diagnose_scene_in`]) over what they
//! parsed and log a summary, so a scene that plays has still said what is
//! wrong with it; `flint validate` prints the whole list and fails on an
//! error. [`diagnose_scene`] is the pass without the references, for entities
//! that are not a whole scene (a chunk names entities of the scene it joins).

use crate::format::{EntityDef, SceneFile};
use crate::prefab;
use flint_core::diagnostic::{self, codes, Diagnostic};
use flint_core::project::{self, AssetKind, Project};
use flint_core::vfs;
use flint_schema::{references, validate_component, ReferenceKind, SchemaRegistry};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// The component that declares an asset to be made (`flint asset resolve`
/// reads it): its `name` is an asset the scene may name before any file is
/// there.
const ASSET_DEF: &str = "asset_def";

/// Every problem in the scene at `scene_file`, prefab instances expanded as
/// the loader expands them (their entities are named `<prefix>_<name>`).
/// `registry` is the schemas of every loaded directory: a component or
/// archetype declared in any of them is known. `project` finds the prefabs
/// and the assets, and its `[project] strict = true` makes an unknown field
/// an error, as [`diagnostic::promote_strict`] does for
/// `flint validate --strict`.
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
    found.extend(diagnose_scene_in(&scene, scene_file, registry, project));
    // With a prefab missing, an entity that names one of its entities
    // would be called unknown: the prefab is the one thing wrong there
    if !expanded {
        found.retain(|d| d.code != codes::UNKNOWN_ENTITY);
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

/// [`diagnose`] for a whole scene already parsed (and prefab-expanded): the
/// entities' archetypes and components against `registry`, and their
/// references: every `entity` field and `parent` against the scene's own
/// entities, every `asset` field through `project`'s search from the
/// directory of `file`. What the loaders run on every load.
pub fn diagnose_scene_in(
    scene: &SceneFile,
    file: &Path,
    registry: &SchemaRegistry,
    project: &Project,
) -> Vec<Diagnostic> {
    let scene_dir = file
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut lookup = Lookup {
        scene,
        project,
        scene_dir,
        to_be_made: scene
            .entities
            .values()
            .filter_map(|entity| entity.components.get(ASSET_DEF)?.get("name")?.as_str())
            .collect(),
        assets: HashMap::new(),
    };
    diagnose_entities(scene, Some(file), registry, Some(&mut lookup))
}

/// [`diagnose_scene_in`] without the references: the entities' archetypes
/// and components against `registry`, and nothing about whether an `entity`
/// or `asset` field leads anywhere. For entities that are not a whole scene
/// (the player's chunk, whose entities name the main scene's) and for text
/// with no file to search from. `file` is what each diagnostic names.
pub fn diagnose_scene(
    scene: &SceneFile,
    file: Option<&Path>,
    registry: &SchemaRegistry,
) -> Vec<Diagnostic> {
    diagnose_entities(scene, file, registry, None)
}

fn diagnose_entities(
    scene: &SceneFile,
    file: Option<&Path>,
    registry: &SchemaRegistry,
    mut lookup: Option<&mut Lookup>,
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
            diagnose_entity(
                name,
                &scene.entities[name],
                registry,
                lookup.as_deref_mut(),
                &mut found,
            );
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

/// What a reference is looked up in: the scene's entities, and the project's
/// asset search from the scene's directory. One search a (kinds, name), so
/// a thousand trees are one look for `oak`.
struct Lookup<'a> {
    scene: &'a SceneFile,
    project: &'a Project,
    scene_dir: &'a Path,
    /// The `name` of every `asset_def` in the scene: assets it declares
    /// and `flint asset resolve` makes, so no file need be there yet.
    to_be_made: HashSet<&'a str>,
    assets: HashMap<(Vec<AssetKind>, String), Option<String>>,
}

impl Lookup<'_> {
    /// The hint for an entity name that is no entity of the scene.
    fn unknown_entity_hint(&self, name: &str) -> String {
        // A prefab's entity under the name its template gave it
        let suffix = format!("_{name}");
        let mut expanded: Vec<&String> = self
            .scene
            .entities
            .keys()
            .filter(|entity| entity.ends_with(&suffix))
            .collect();
        expanded.sort_unstable();
        diagnostic::did_you_mean(name, self.scene.entities.keys().map(String::as_str))
            .or_else(|| {
                expanded.first().map(|entity| {
                    format!(
                        "did you mean `{entity}`? A prefab's entities are named \
                         `<prefix>_<name>`: `${{PREFIX}}_{name}` inside the prefab"
                    )
                })
            })
            .unwrap_or_else(|| {
                "it names an entity of this scene (a prefab's are `<prefix>_<name>`); leave it \
                 \"\" when a script fills it at run time"
                    .to_string()
            })
    }

    /// `None` when a file of one of `kinds` is found for `name`, or an
    /// `asset_def` of the scene declares it; otherwise every path tried, as
    /// the hint.
    fn missing_asset(&mut self, kinds: &[AssetKind], name: &str) -> Option<String> {
        if self.to_be_made.contains(name) {
            return None;
        }
        let (project, scene_dir) = (self.project, self.scene_dir);
        self.assets
            .entry((kinds.to_vec(), name.to_string()))
            .or_insert_with(|| {
                let mut tried = Vec::new();
                for kind in kinds {
                    let resolution = project.explain(*kind, name, scene_dir);
                    if resolution.found.is_some() {
                        return None;
                    }
                    tried.extend(resolution.tried.iter().map(|path| project::shown(path)));
                }
                Some(format!("tried {}", tried.join(", ")))
            })
            .clone()
    }
}

/// `model 'barrel'`, or `texture or procgen spec 'wall'` for a field that
/// takes either.
fn asset_noun(kinds: &[AssetKind]) -> String {
    let nouns: Vec<&str> = kinds.iter().map(|kind| kind.info().noun).collect();
    nouns.join(" or ")
}

fn diagnose_entity(
    name: &str,
    entity: &EntityDef,
    registry: &SchemaRegistry,
    mut lookup: Option<&mut Lookup>,
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

    if let (Some(lookup), Some(parent)) = (lookup.as_deref(), entity.parent.as_deref()) {
        if !lookup.scene.entities.contains_key(parent) {
            found.push(
                Diagnostic::error(
                    codes::UNKNOWN_ENTITY,
                    format!("no entity named `{parent}`, so the scene does not load"),
                )
                .entity(name)
                .field("parent")
                .hint(lookup.unknown_entity_hint(parent)),
            );
        }
    }

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
        let Some(lookup) = lookup.as_deref_mut() else {
            continue;
        };
        for reference in references(schema, data) {
            let d = match &reference.kind {
                ReferenceKind::Entity => {
                    if lookup.scene.entities.contains_key(&reference.name) {
                        continue;
                    }
                    Diagnostic::error(
                        codes::UNKNOWN_ENTITY,
                        format!("no entity named `{}`", reference.name),
                    )
                    .hint(lookup.unknown_entity_hint(&reference.name))
                }
                ReferenceKind::Asset(kinds) => {
                    let Some(tried) = lookup.missing_asset(kinds, &reference.name) else {
                        continue;
                    };
                    Diagnostic::error(
                        codes::MISSING_ASSET,
                        format!("{} '{}' not found", asset_noun(kinds), reference.name),
                    )
                    .hint(tried)
                }
            };
            found.push(
                d.entity(name)
                    .component(component.clone())
                    .field(reference.field),
            );
        }
        // The tip is found among this entity's children: one that is
        // somewhere else in the scene is never reached
        if component == flint_core::components::IK_TWO_BONE {
            let tip = data.get("tip").and_then(|v| v.as_str()).unwrap_or("");
            let elsewhere = lookup
                .scene
                .entities
                .get(tip)
                .is_some_and(|t| t.parent.as_deref() != Some(name));
            if elsewhere {
                found.push(
                    Diagnostic::error(
                        codes::NOT_A_CHILD,
                        format!(
                            "`{tip}` is not a child of this entity, so the chain is never solved"
                        ),
                    )
                    .entity(name)
                    .component(component.clone())
                    .field("tip")
                    .hint(format!("give `{tip}` parent = \"{name}\"")),
                );
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

    /// #687: an `ik_two_bone.tip` must be a child of the entity naming it,
    /// or the chain is never solved.
    #[test]
    fn an_ik_tip_that_is_not_a_child_is_an_error() {
        let ik = (
            "schemas/components/ik_two_bone.toml",
            "[component.ik_two_bone]\n[component.ik_two_bone.fields]\n\
             target = { type = \"entity\", default = \"\" }\n\
             tip = { type = \"entity\", default = \"\" }\n",
        );
        let scene = "[scene]\nname = \"s\"\n[entities.goal]\n[entities.hand]\n\
                     [entities.foot]\nparent = \"knee\"\n\
                     [entities.elbow.ik_two_bone]\ntarget = \"goal\"\ntip = \"hand\"\n\
                     [entities.knee.ik_two_bone]\ntarget = \"goal\"\ntip = \"foot\"\n";
        let found = with_scene(
            &[ik, ("scenes/a.scene.toml", scene)],
            |registry, project| diagnose(Path::new("scenes/a.scene.toml"), registry, project),
        );
        let said: Vec<String> = found.iter().map(|d| d.to_string()).collect();
        assert_eq!(
            said,
            [
                "error[not-a-child] entity 'elbow' ik_two_bone.tip: `hand` is not a child of this \
              entity, so the chain is never solved"
            ]
        );
        assert_eq!(
            found[0].hint.as_deref(),
            Some("give `hand` parent = \"elbow\"")
        );
    }

    /// With a prefab that doesn't expand, a reference to one of its
    /// entities is not called unknown: the prefab is what is wrong.
    #[test]
    fn a_missing_prefab_is_the_one_thing_said_about_its_entities() {
        let scene = "[scene]\nname = \"s\"\n[prefabs.one]\ntemplate = \"nope\"\nprefix = \"n\"\n\
                     [entities.lid]\nparent = \"n_box\"\n";
        let found = with_scene(&[("scenes/a.scene.toml", scene)], |registry, project| {
            diagnose(Path::new("scenes/a.scene.toml"), registry, project)
        });
        let codes_found: Vec<&str> = found.iter().map(|d| d.code).collect();
        assert_eq!(codes_found, [codes::BAD_PREFAB]);
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

    const RIG: (&str, &str) = (
        "schemas/components/rig.toml",
        r#"
[component.rig.fields]
target = { type = "entity", default = "" }
mesh = { type = "asset", kind = ["models", "specs"], unless = "runtime" }
runtime = "bool"
sound = { type = "asset", kind = "audio" }
"#,
    );

    /// The ticket's first box: an entity reference to nothing fails
    /// validation, naming the field; so does a `parent`.
    #[test]
    fn an_entity_reference_to_nothing_is_an_error_naming_the_field() {
        let scene = r#"
[scene]
name = "s"

[entities.hand]

[entities.elbow]
parent = "sholder"
[entities.elbow.rig]
target = "hnad"

[entities.wrist.rig]
target = "hand"

[entities.knee.rig]
target = ""

[entities.ankle.rig]
target = "nope"
"#;
        let found = with_scene(
            &[RIG, ("scenes/a.scene.toml", scene)],
            |registry, project| diagnose(Path::new("scenes/a.scene.toml"), registry, project),
        );
        let said: Vec<String> = found.iter().map(|d| d.to_string()).collect();
        assert_eq!(
            said,
            [
                "error[unknown-entity] entity 'ankle' rig.target: no entity named `nope`",
                "error[unknown-entity] entity 'elbow' field 'parent': no entity named \
                 `sholder`, so the scene does not load",
                "error[unknown-entity] entity 'elbow' rig.target: no entity named `hnad`",
            ],
            "an empty reference is one a script fills, and passes"
        );
        assert!(
            found[0].hint.as_deref().unwrap().contains("leave it \"\""),
            "{:?}",
            found[0].hint
        );
        assert_eq!(found[2].hint.as_deref(), Some("did you mean `hand`?"));
        assert!(found.iter().all(Diagnostic::is_error));
    }

    /// A prefab's references are written `${PREFIX}_<name>` and checked
    /// after expansion, against the names the loader gives its entities.
    #[test]
    fn a_reference_inside_a_prefab_is_checked_by_its_expanded_name() {
        let found = with_scene(
            &[
                RIG,
                (
                    "scenes/a.scene.toml",
                    "[scene]\nname = \"s\"\n[prefabs.one]\ntemplate = \"arm\"\nprefix = \"left\"\n",
                ),
                (
                    "prefabs/arm.prefab.toml",
                    "[prefab]\nname = \"arm\"\n\n[entities.hand]\n\n\
                     [entities.elbow.rig]\ntarget = \"${PREFIX}_hand\"\n\n\
                     [entities.wrist.rig]\ntarget = \"hand\"\n",
                ),
            ],
            |registry, project| diagnose(Path::new("scenes/a.scene.toml"), registry, project),
        );
        let said: Vec<String> = found.iter().map(|d| d.to_string()).collect();
        assert_eq!(
            said,
            ["error[unknown-entity] entity 'left_wrist' rig.target: no entity named `hand`"]
        );
        assert_eq!(
            found[0].hint.as_deref(),
            Some(
                "did you mean `left_hand`? A prefab's entities are named `<prefix>_<name>`: \
                 `${PREFIX}_hand` inside the prefab"
            )
        );
    }

    /// A missing asset is an error that lists every path the project's one
    /// search tried; a file anywhere on that walk, a procgen spec of the
    /// name, or `runtime = true` passes.
    #[test]
    fn a_missing_asset_is_an_error_listing_the_paths_tried() {
        let scene = r#"
[scene]
name = "s"

[entities.a.rig]
mesh = "barrel"
sound = "hit.ogg"

[entities.b.rig]
mesh = "oak"

[entities.c.rig]
mesh = "blob"
runtime = true

[entities.d.rig]
mesh = "nope"
sound = "gone.ogg"

[entities.e.rig]
mesh = "nope"
"#;
        let found = with_scene(
            &[
                RIG,
                ("flint.toml", "[project]\n"),
                ("scenes/a.scene.toml", scene),
                ("models/barrel.glb", ""),
                ("scenes/audio/hit.ogg", ""),
                (
                    "specs/tree.procgen.toml",
                    "[meta]\nname = \"oak\"\ngenerator = \"tree\"\n",
                ),
            ],
            |registry, project| diagnose(Path::new("scenes/a.scene.toml"), registry, project),
        );
        let said: Vec<String> = found.iter().map(|d| d.to_string()).collect();
        assert_eq!(
            said,
            [
                "error[missing-asset] entity 'd' rig.mesh: model or procgen spec 'nope' not found",
                "error[missing-asset] entity 'd' rig.sound: sound 'gone.ogg' not found",
                "error[missing-asset] entity 'e' rig.mesh: model or procgen spec 'nope' not found",
            ]
        );
        assert_eq!(
            found[0].hint.as_deref(),
            Some(
                "tried scenes/models/nope.glb, scenes/models/nope.gltf, models/nope.glb, \
                 models/nope.gltf, scenes/specs, scenes/models, specs, models"
            )
        );
        assert_eq!(
            found[1].hint.as_deref(),
            Some("tried scenes/gone.ogg, scenes/audio/gone.ogg, gone.ogg, audio/gone.ogg")
        );
    }

    /// An asset an `asset_def` declares is one `flint asset resolve` makes:
    /// naming it before its file exists is the point, not a mistake.
    #[test]
    fn an_asset_the_scene_declares_with_asset_def_is_not_missing() {
        let scene = "[scene]\nname = \"s\"\n\
                     [entities.a.rig]\nmesh = \"oak_table\"\n\
                     [entities.a.asset_def]\nname = \"oak_table\"\n\
                     [entities.b.rig]\nmesh = \"oak_chair\"\n";
        let found = with_scene(
            &[
                RIG,
                (
                    "schemas/components/asset_def.toml",
                    "[component.asset_def]\n",
                ),
                ("scenes/a.scene.toml", scene),
            ],
            |registry, project| diagnose(Path::new("scenes/a.scene.toml"), registry, project),
        );
        let said: Vec<String> = found.iter().map(|d| d.to_string()).collect();
        assert_eq!(
            said,
            ["error[missing-asset] entity 'b' rig.mesh: model or procgen spec 'oak_chair' not found"]
        );
    }

    /// The loaders say the same things: a reference to nothing doesn't stop
    /// a load, and the list a host asks for holds it.
    #[test]
    fn a_load_reports_references_and_a_chunk_pass_does_not() {
        let scene = "[scene]\nname = \"s\"\n[entities.a.rig]\ntarget = \"nope\"\nmesh = \"nope\"\n";
        with_scene(&[RIG, ("scenes/a.scene.toml", scene)], |registry, _| {
            let (world, parsed, found) =
                crate::load_scene_diagnosed("scenes/a.scene.toml", registry).expect("loads");
            assert!(world.contains_name("a"));
            let codes_found: Vec<&str> = found.iter().map(|d| d.code).collect();
            assert_eq!(codes_found, [codes::MISSING_ASSET, codes::UNKNOWN_ENTITY]);
            // Entities that are not a whole scene: nothing about references
            assert_eq!(diagnose_scene(&parsed, None, registry), []);
        });
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
