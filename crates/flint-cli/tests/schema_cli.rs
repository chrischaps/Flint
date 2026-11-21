//! `flint schema` as data, and the references `flint validate` follows
//! (ADR 0115 decision 4, #684): the real binary.
//!
//! - `flint schema --list --json` names every component and archetype of
//!   the schema directories, and a name that is neither exits non-zero.
//! - `flint schema --format json-schema` emits a JSON Schema that every
//!   frozen scene under `tests/fixtures/scenes/` validates against (and that a scene with a
//!   mistake does not), checked with a JSON Schema validator.
//! - `flint validate` fails on `ik_two_bone.target = "nope"`, naming the
//!   field, and on a model that isn't there, listing the paths tried.
//! - An old type spelling in a game's schema file is a warning that names
//!   the file; the engine's own files are not the game's to be told about.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `flint <args>` from the repository root, whose `flint.toml` names the
/// engine's schemas and the examples' layer.
fn flint(args: &[&str]) -> Output {
    Command::new(FLINT)
        .args(args)
        .current_dir(repo())
        .output()
        .expect("run flint")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n")
}

fn json(out: &Output) -> serde_json::Value {
    assert!(out.status.success(), "{}", stderr(out));
    serde_json::from_str(&stdout(out)).unwrap_or_else(|e| panic!("{e}\n{}", stdout(out)))
}

/// The names under `[<header>.<name>]` in every file of the two layers.
fn declared(kind: &str, header: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for dir in ["schemas", "schemas/examples"] {
        let Ok(entries) = std::fs::read_dir(repo().join(dir).join(kind)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                let doc: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
                names.extend(
                    doc.get(header)
                        .and_then(|t| t.as_table())
                        .into_iter()
                        .flat_map(|t| t.keys().cloned()),
                );
            }
        }
    }
    names
}

fn names_in(list: &serde_json::Value, key: &str) -> BTreeSet<String> {
    list[key]
        .as_array()
        .unwrap_or_else(|| panic!("no `{key}` array"))
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn list_json_names_every_component_and_archetype() {
    let list = json(&flint(&["schema", "--list", "--json"]));
    let components = declared("components", "component");
    let archetypes = declared("archetypes", "archetype");
    assert!(components.len() > 40 && archetypes.len() > 5);
    assert_eq!(names_in(&list, "components"), components);
    assert_eq!(names_in(&list, "archetypes"), archetypes);
    assert_eq!(
        list["schema_dirs"],
        serde_json::json!(["schemas", "schemas/examples"])
    );
    // `--format json` is the same flag
    assert_eq!(
        list,
        json(&flint(&["schema", "--list", "--format", "json"]))
    );

    // An open component says so; one with fields names them
    let find = |name: &str| {
        list["components"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap_or_else(|| panic!("no `{name}`"))
            .clone()
    };
    assert_eq!(find("material_overrides")["open"], true);
    // ...and `collider`, open until #792 moved its fields under `fields`
    assert_eq!(find("collider")["open"], false);
    assert_eq!(
        find("collider")["fields"],
        serde_json::json!([
            "friction",
            "is_sensor",
            "layer",
            "mask",
            "model",
            "offset",
            "restitution",
            "rotation",
            "shape",
            "size"
        ])
    );
    assert_eq!(
        find("ik_two_bone")["fields"],
        serde_json::json!(["enabled", "pole", "target", "tip", "weight"])
    );

    // The text list has the same names
    let text = stdout(&flint(&["schema", "--list"]));
    assert!(
        text.contains(&format!("Components ({}):", components.len())),
        "{text}"
    );
    for name in components.iter().chain(&archetypes) {
        assert!(
            text.lines()
                .any(|line| line.split_whitespace().next() == Some(name)),
            "`{name}` is not in the list:\n{text}"
        );
    }
}

#[test]
fn an_unknown_name_exits_non_zero_and_suggests() {
    for args in [
        &["schema", "lihgt"][..],
        &["schema", "lihgt", "--format", "json"],
        &["schema", "lihgt", "--format", "json-schema"],
    ] {
        let out = flint(args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(stdout(&out), "", "{args:?}");
        let said = stderr(&out);
        assert!(
            said.contains(
                "no component, scene table or archetype named `lihgt`: did you mean `light`?"
            ),
            "{said}"
        );
        assert!(said.contains("flint schema --list"), "{said}");
    }
    // ...and a name there is still prints, as text and as data
    let text = stdout(&flint(&["schema", "ik_two_bone"]));
    assert!(text.contains("target : entity = \"\""), "{text}");
    let one = json(&flint(&["schema", "model", "--format", "json"]));
    assert_eq!(one["kind"], "component");
    assert_eq!(
        one["fields"]["asset"]["kinds"],
        serde_json::json!(["models", "specs"])
    );
    assert_eq!(one["fields"]["asset"]["unless"], "runtime");
    let archetype = json(&flint(&["schema", "player", "--format", "json"]));
    assert_eq!(archetype["kind"], "archetype");
    // No name and no --list is an error that says what to pass
    let out = flint(&["schema"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--list"), "{}", stderr(&out));
    let out = flint(&["schema", "--format", "yaml"]);
    assert_eq!(out.status.code(), Some(1));
}

/// Every `*.scene.toml` under `dir`.
fn scenes_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            scenes_under(&path, out);
        } else if path.to_string_lossy().ends_with(".scene.toml") {
            out.push(path);
        }
    }
}

fn scene_as_json(path: &Path) -> serde_json::Value {
    let doc: toml::Value = std::fs::read_to_string(path)
        .unwrap()
        .parse()
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::to_value(doc).unwrap()
}

/// The ticket's second box. The schema is compiled by a JSON Schema
/// validator (the `boon` crate, draft-07) and each example scene, its TOML
/// converted to JSON value for value, is validated against it. Then the
/// same schema refuses scenes with one mistake each, so it is not a schema
/// that accepts anything.
#[test]
fn the_json_schema_validates_every_example_scene() {
    let schema = json(&flint(&["schema", "--format", "json-schema"]));
    assert_eq!(schema["$schema"], "http://json-schema.org/draft-07/schema#");
    let mut schemas = boon::Schemas::new();
    let mut compiler = boon::Compiler::new();
    compiler
        .add_resource("scene.schema.json", schema.clone())
        .expect("the schema is a resource");
    let compiled = compiler
        .compile("scene.schema.json", &mut schemas)
        .unwrap_or_else(|e| panic!("the JSON Schema does not compile: {e}"));

    let mut scenes = Vec::new();
    scenes_under(&repo().join("tests/fixtures/scenes"), &mut scenes);
    assert!(scenes.len() > 25, "only {} fixture scenes", scenes.len());
    let mut failures = Vec::new();
    for scene in &scenes {
        if let Err(e) = schemas.validate(&scene_as_json(scene), compiled) {
            failures.push(format!("{}:\n{e}", scene.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} fixture scenes do not validate:\n{}",
        failures.len(),
        scenes.len(),
        failures.join("\n")
    );

    // One mistake each, in a scene that otherwise validates
    let good = r#"
[scene]
name = "Lamp"

[entities.sun.light]
type = "directional"
intensity = 3.0
direction = [0.3, 1.0, 0.2]

[entities.elbow.ik_two_bone]
target = "sun"

[entities.crate]
archetype = "furniture"
[entities.crate.material]
color = [1.0, 0.5, 0.2]
blend_mode = "additive"
"#;
    let check = |text: &str| {
        let doc: toml::Value = text.parse().unwrap();
        schemas
            .validate(&serde_json::to_value(doc).unwrap(), compiled)
            .is_ok()
    };
    assert!(check(good));
    // A field the component doesn't declare is allowed, as the engine allows it
    assert!(check(
        &good.replace("intensity = 3.0", "intensity = 3.0\nwarmth = 0.5")
    ));
    for (what, from, to) in [
        ("a wrong type", "intensity = 3.0", "intensity = \"bright\""),
        ("an unknown enum value", "\"directional\"", "\"directonal\""),
        ("a vec3 of two", "[0.3, 1.0, 0.2]", "[0.3, 1.0]"),
        (
            "an entity that is no string",
            "target = \"sun\"",
            "target = 3",
        ),
        ("a colour of two", "[1.0, 0.5, 0.2]", "[1.0, 0.5]"),
        (
            "an unknown component",
            "[entities.sun.light]",
            "[entities.sun.lihgt]",
        ),
        ("an unknown archetype", "\"furniture\"", "\"furnture\""),
    ] {
        assert!(good.contains(from), "{from}");
        assert!(!check(&good.replace(from, to)), "{what} validates");
    }

    // One component's table alone, and --output writes plain UTF-8
    let light = json(&flint(&["schema", "light", "--format", "json-schema"]));
    assert_eq!(
        light["properties"]["type"]["enum"],
        serde_json::json!(["directional", "point", "spot"])
    );
    let file = std::env::temp_dir().join(format!("flint_schema_{}.json", std::process::id()));
    let out = flint(&[
        "schema",
        "--format",
        "json-schema",
        "--output",
        file.to_str().unwrap(),
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "");
    let written = std::fs::read(&file).unwrap();
    std::fs::remove_file(&file).ok();
    assert_eq!(written.first(), Some(&b'{'), "no byte-order mark");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&written).unwrap(),
        schema
    );
}

/// #1062: what taplo (Even Better TOML's engine) needs of the schema
/// beyond a validator's. Its completion follows a `$ref` by setting the
/// schema URL's fragment and reading it as a JSON pointer (taplo-common
/// `reference_url` / `resolve_schema`), so every `$ref` is a local pointer
/// that leads to a schema. It inserts a field's `default` as written, so a
/// default is the decimal the schema meant, never an f32 widened
/// (`0.03999999910593033` for 0.04).
#[test]
fn the_json_schema_is_one_taplo_can_follow() {
    let schema = json(&flint(&["schema", "--format", "json-schema"]));
    fn walk<'a>(
        v: &'a serde_json::Value,
        at: String,
        out: &mut Vec<(String, &'a serde_json::Value)>,
    ) {
        match v {
            serde_json::Value::Object(map) => {
                for (k, x) in map {
                    walk(x, format!("{at}/{k}"), out);
                }
            }
            serde_json::Value::Array(items) => {
                for (i, x) in items.iter().enumerate() {
                    walk(x, format!("{at}/{i}"), out);
                }
            }
            _ => out.push((at, v)),
        }
    }
    let mut leaves = Vec::new();
    walk(&schema, String::new(), &mut leaves);

    let refs: Vec<&str> = leaves
        .iter()
        .filter(|(at, _)| at.ends_with("/$ref"))
        .filter_map(|(_, v)| v.as_str())
        .collect();
    assert!(refs.len() > 20, "only {} refs", refs.len());
    for r in refs {
        let pointer = r
            .strip_prefix('#')
            .unwrap_or_else(|| panic!("{r} is not local"));
        assert!(
            schema.pointer(pointer).is_some_and(|s| s.is_object()),
            "{r} leads to no schema"
        );
    }

    let noisy: Vec<String> = leaves
        .iter()
        .filter(|(at, _)| at.contains("/default"))
        .filter_map(|(at, v)| v.as_f64().map(|n| (at, n)))
        .filter(|(_, n)| n.to_string().len() > (*n as f32).to_string().len())
        .map(|(at, n)| format!("{at} = {n}"))
        .collect();
    assert!(
        noisy.is_empty(),
        "defaults widened from f32:\n{}",
        noisy.join("\n")
    );
    assert_eq!(
        schema["definitions"]["tables"]["post_process"]["properties"]["bloom_intensity"]["default"],
        serde_json::json!(0.04)
    );
}

/// A small game outside the engine checkout (the system temp folder), with
/// the engine's schemas named by flag and a layer of its own.
struct Game {
    root: PathBuf,
}

impl Game {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("flint_684_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Self { root }
    }

    fn validate(&self, scene: &str, extra: &[&str]) -> Output {
        let mut args = vec!["validate", scene];
        args.extend(extra);
        self.flint(&args)
    }

    /// `flint <args>` in the game, over the engine's schemas and its own.
    fn flint(&self, args: &[&str]) -> Output {
        let engine = repo().join("schemas");
        let mut args = args.to_vec();
        args.extend([
            "--schemas",
            engine.to_str().unwrap(),
            "--schemas",
            "schemas",
        ]);
        Command::new(FLINT)
            .args(&args)
            .current_dir(&self.root)
            .output()
            .expect("run flint")
    }
}

impl Drop for Game {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

const LOOT: &str = "[component.loot]\ndescription = \"What a chest holds\"\n\n\
                    [component.loot.fields]\ngold = { type = \"i32\", default = 0 }\n\
                    owner = { type = \"entity\", default = \"\" }\n";

/// The ticket's first box, and the missing asset beside it.
#[test]
fn validate_fails_on_a_reference_to_nothing_naming_the_field() {
    let game = Game::new(
        "refs",
        &[
            ("flint.toml", "[project]\nid = \"refs\"\n"),
            ("schemas/components/loot.toml", LOOT),
            ("models/crate.glb", ""),
            ("scripts/chest.rhai", "fn on_init() {}\n"),
            (
                "scenes/reach.scene.toml",
                "[scene]\nname = \"Reach\"\n\n\
                 [entities.shoulder.transform]\nposition = [0, 1.4, 0]\n\n\
                 [entities.elbow]\nparent = \"shoulder\"\n\n\
                 [entities.elbow.ik_two_bone]\ntarget = \"nope\"\npole = \"\"\n\n\
                 [entities.barrel.model]\nasset = \"barel\"\n\n\
                 [entities.crate.model]\nasset = \"crate\"\n\n\
                 [entities.chest.script]\nsource = \"chest.rhai\"\n\n\
                 [entities.chest.loot]\ngold = 12\nowner = \"elbow\"\n",
            ),
            (
                "scenes/good.scene.toml",
                "[scene]\nname = \"Good\"\n\n[entities.hand]\n\n\
                 [entities.elbow.ik_two_bone]\ntarget = \"hand\"\n\n\
                 [entities.crate.model]\nasset = \"crate\"\n",
            ),
        ],
    );
    let out = game.validate("scenes/reach.scene.toml", &[]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(1), "{text}\n{}", stderr(&out));
    assert!(
        text.starts_with("scenes/reach.scene.toml: 2 errors\n"),
        "{text}"
    );
    assert!(
        text.contains(
            "error[unknown-entity] entity 'elbow' ik_two_bone.target: no entity named `nope`"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "error[missing-asset] entity 'barrel' model.asset: model or procgen spec 'barel' \
             not found\n      hint: tried scenes/models/barel.glb, scenes/models/barel.gltf, \
             models/barel.glb, models/barel.gltf, scenes/specs, scenes/models, specs, models"
        ),
        "{text}"
    );

    let report = json_of_failure(&game.validate("scenes/reach.scene.toml", &["--format", "json"]));
    let found: Vec<(&str, &str, &str)> = report["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["code"].as_str().unwrap(),
                d["component"].as_str().unwrap(),
                d["field"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            ("missing-asset", "model", "asset"),
            ("unknown-entity", "ik_two_bone", "target"),
        ]
    );

    let good = game.validate("scenes/good.scene.toml", &[]);
    assert_eq!(good.status.code(), Some(0), "{}", stdout(&good));
}

/// `flint validate --format json` exits 1 on an error and still prints.
fn json_of_failure(out: &Output) -> serde_json::Value {
    assert_eq!(out.status.code(), Some(1), "{}", stderr(out));
    serde_json::from_str(&stdout(out)).unwrap_or_else(|e| panic!("{e}\n{}", stdout(out)))
}

/// #794's done-when: a game's file of a few lines adds a field to the
/// engine's `material`, the scene using it validates with nothing said, and
/// `flint schema material` names the layer of each field.
#[test]
fn a_game_adds_one_field_to_an_engine_component() {
    const SCENE: &str = "[scene]\nname = \"Street\"\n\n\
                         [entities.cobbles.material]\nroughness = 0.35\nwetness = 0.8\n";
    let bare = Game::new(
        "extend_bare",
        &[
            ("flint.toml", "[project]\nid = \"street\"\n"),
            ("schemas/components/.gitkeep", ""),
            ("scenes/street.scene.toml", SCENE),
        ],
    );
    let out = bare.validate("scenes/street.scene.toml", &[]);
    let text = stdout(&out);
    assert!(
        text.contains("`material` has no field `wetness`")
            && text.contains("`[component.material]` with `extend = true`"),
        "without the game's file the field is undeclared, and the hint says how: {text}"
    );

    let game = Game::new(
        "extend",
        &[
            ("flint.toml", "[project]\nid = \"street\"\n"),
            (
                "schemas/components/material.toml",
                "[component.material]\nextend = true\n\n\
                 [component.material.fields]\nwetness = { type = \"f32\", default = 0.0 }\n",
            ),
            ("scenes/street.scene.toml", SCENE),
        ],
    );
    let out = game.validate("scenes/street.scene.toml", &[]);
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert!(text.starts_with("No problems found."), "{text}");

    let out = game.flint(&["schema", "material"]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        text.contains("\nExtended by: [2] schemas/components/material.toml\n"),
        "{text}"
    );
    assert!(text.contains("\n  wetness : f32 = 0   [2]\n"), "{text}");
    assert!(text.contains("\n  roughness : f32 = 0.7   [1]\n"), "{text}");

    let out = game.flint(&["schema", "material", "--json"]);
    let material = json(&out);
    assert_eq!(
        material["extended_by"],
        serde_json::json!(["schemas/components/material.toml"])
    );
    assert_eq!(
        material["fields"]["wetness"]["file"],
        "schemas/components/material.toml"
    );
    assert!(material["declared_in"]
        .as_str()
        .unwrap()
        .ends_with("schemas/components/material.toml"));
    assert_eq!(
        material["fields"]["roughness"]["file"],
        material["declared_in"]
    );
}

/// #1054: a game adds a component and its defaults to the engine's
/// `player` archetype with a file of a few lines, and an entity of the
/// archetype gets the engine's components and defaults and the game's.
#[test]
fn a_game_adds_a_component_to_an_engine_archetype() {
    let game = Game::new(
        "extend_archetype",
        &[
            ("flint.toml", "[project]\nid = \"run\"\n"),
            (
                "schemas/components/stamina.toml",
                "[component.stamina]\ndescription = \"How long the player can sprint\"\n\n\
                 [component.stamina.fields]\nmax = { type = \"f32\", default = 100.0, min = 0.0 }\n",
            ),
            (
                "schemas/archetypes/player.toml",
                "[archetype.player]\nextend = true\ncomponents = [\"stamina\"]\n\n\
                 [archetype.player.defaults.stamina]\nmax = 120.0\n\n\
                 [archetype.player.defaults.character_controller]\nmove_speed = 7.5\n",
            ),
            (
                "scenes/run.scene.toml",
                "[scene]\nname = \"Run\"\n\n[entities.hero]\narchetype = \"player\"\n",
            ),
        ],
    );
    let out = game.validate("scenes/run.scene.toml", &[]);
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert!(text.starts_with("No problems found."), "{text}");

    let out = game.flint(&["entity", "show", "hero", "--scene", "scenes/run.scene.toml"]);
    let hero = json(&out);
    let data = &hero["data"];
    assert_eq!(
        data["stamina"]["max"], 120.0,
        "the game's component: {data}"
    );
    assert_eq!(
        data["character_controller"]["move_speed"], 7.5,
        "the game's default: {data}"
    );
    assert_eq!(
        data["character_controller"]["jump_force"], 6.0,
        "the engine's default beside it: {data}"
    );
    assert_eq!(
        data["collider"]["shape"], "capsule",
        "the engine's component: {data}"
    );

    let out = game.flint(&["schema", "player"]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        text.contains("\nExtended by: [2] schemas/archetypes/player.toml\n"),
        "{text}"
    );
    assert!(text.contains("\n  - transform   [1]\n"), "{text}");
    assert!(text.contains("\n  - stamina   [2]\n"), "{text}");
    assert!(text.contains("\n    move_speed = 7.5   [2]\n"), "{text}");
    assert!(text.contains("\n    jump_force = 6   [1]\n"), "{text}");

    let out = game.flint(&["schema", "player", "--json"]);
    let player = json(&out);
    assert_eq!(
        player["extended_by"],
        serde_json::json!(["schemas/archetypes/player.toml"])
    );
    assert_eq!(
        player["components"],
        serde_json::json!([
            "transform",
            "collider",
            "rigidbody",
            "character_controller",
            "stamina"
        ])
    );
    assert_eq!(
        player["component_files"]["stamina"],
        "schemas/archetypes/player.toml"
    );
    assert_eq!(player["component_files"]["collider"], player["declared_in"]);
    assert_eq!(
        player["default_files"]["character_controller"]["move_speed"],
        "schemas/archetypes/player.toml"
    );
    assert_eq!(
        player["default_files"]["character_controller"]["jump_force"],
        player["declared_in"]
    );
}

/// Decision 3 of ADR 0115: an old spelling is warned about in a game's
/// schema file. The engine's files are inside an engine checkout, which is
/// how they are told apart: nothing is said about them. (A field written
/// beside `description` is no lint now but a load error, #792, so the
/// ignored key here is a misspelled key of a field.)
#[test]
fn a_games_old_type_spelling_is_a_warning_naming_the_file() {
    let game = Game::new(
        "spelling",
        &[
            ("flint.toml", "[project]\nid = \"spelling\"\n"),
            (
                "schemas/components/loot.toml",
                "[component.loot.fields]\ngold = { type = \"integer\", default = 0 }\n\
                 weight = { type = \"float\" }\n\n\
                 [component.lock.fields]\npicked = { type = \"bool\", defualt = false }\n",
            ),
            (
                "scenes/a.scene.toml",
                "[scene]\nname = \"A\"\n\n[entities.chest.loot]\ngold = 12\nweight = 2.5\n",
            ),
        ],
    );
    let out = game.validate("scenes/a.scene.toml", &[]);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(0), "warnings pass: {text}");
    assert!(
        text.starts_with("scenes/a.scene.toml: 3 warnings\n"),
        "{text}"
    );
    for line in [
        "schemas/components/loot.toml: warning[old-type-name] loot.gold: `integer` is an old \
         spelling of `i32`",
        "schemas/components/loot.toml: warning[old-type-name] loot.weight: `float` is an old \
         spelling of `f32`",
        "schemas/components/loot.toml: warning[ignored-schema-key] lock.picked.defualt: \
         `defualt` is ignored",
    ] {
        assert!(text.contains(line), "missing `{line}` in:\n{text}");
    }
    assert!(
        text.contains("hint: write type = \"f32\"; `float` still loads"),
        "{text}"
    );
    assert!(
        !text.contains("collider") && !text.contains("rigidbody"),
        "the engine's own files are not the game's to fix:\n{text}"
    );
}

/// #1020: `flint schema <component>` shows the F3 panel its fields declare
/// (#744), and `--format json` carries it as `panel`. The component is a
/// fog bank a script rolls in and lifts: its panel rows and one tune row.
#[test]
fn a_components_panel_is_shown_and_exported() {
    let game = Game::new(
        "panel",
        &[
            ("flint.toml", "[project]\nid = \"fog\"\n"),
            (
                "schemas/components/fog_bank.toml",
                r#"[component.fog_bank.fields]
hold_hours = { type = "f32", default = 4.0, min = 0.5, max = 12.0, panel = { role = "tune", step = 0.25, presets = [["short", 1.0], ["long", 8.0]], commit = false } }
trigger_fog = { type = "f32", default = -1.0, panel = { role = "trigger", label = "Fog now" } }
end_now = { type = "bool", default = false, panel = { role = "trigger", label = "End now" } }
phase = { type = "f32", default = 0.0, panel = { role = "status", labels = ["Idle", "Rolling in", "Fogbound", "Lifting"] } }
fog = { type = "f32", default = 0.0, panel = { role = "status", format = "fog {:.2}" } }
next_in_s = { type = "f32", default = -1.0, panel = { role = "status", format = "next in {:.0}s", unset = -1.0 } }
debug_log = { type = "bool", default = false }
"#,
            ),
        ],
    );

    let out = game.flint(&["schema", "fog_bank"]);
    let text = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        text.contains(
            "\nPanel \"Fog bank\": status phase, fog, next_in_s · tune hold_hours · \
             trigger trigger_fog \"Fog now\", end_now\n"
        ),
        "{text}"
    );
    // `end_now`'s label is how its name reads anyway, so it isn't repeated;
    // a component with no panel says nothing about one
    let out = game.flint(&["schema", "transform"]);
    assert!(!stdout(&out).contains("Panel"), "{}", stdout(&out));

    let out = game.flint(&["schema", "fog_bank", "--format", "json"]);
    let panel = &json(&out)["panel"];
    assert_eq!(panel["title"], "Fog bank");
    let rows = panel["rows"].as_array().unwrap();
    let fields: Vec<&str> = rows.iter().map(|r| r["field"].as_str().unwrap()).collect();
    assert_eq!(
        fields,
        [
            "hold_hours",
            "trigger_fog",
            "end_now",
            "phase",
            "fog",
            "next_in_s"
        ],
        "the rows keep the order the fields are written in"
    );
    assert_eq!(
        rows[0],
        serde_json::json!({
            "field": "hold_hours", "role": "tune", "label": "Hold hours",
            "step": 0.25, "min": 0.5, "max": 12.0,
            "presets": [["short", 1.0], ["long", 8.0]], "commit": false,
        })
    );
    assert_eq!(
        rows[1],
        serde_json::json!({ "field": "trigger_fog", "role": "trigger", "label": "Fog now", "press": 1.0 })
    );
    assert_eq!(rows[2]["press"], true);
    assert_eq!(
        rows[3]["labels"],
        serde_json::json!(["Idle", "Rolling in", "Fogbound", "Lifting"])
    );
    assert_eq!(
        rows[5],
        serde_json::json!({
            "field": "next_in_s", "role": "status", "label": "Next in s",
            "format": "next in {:.0}s", "unset": -1.0,
        })
    );

    // The whole export carries it too, and only where there is one
    let out = game.flint(&["schema", "--format", "json"]);
    let all = json(&out);
    assert_eq!(all["components"]["fog_bank"]["panel"], *panel);
    assert!(all["components"]["transform"].get("panel").is_none());
}
