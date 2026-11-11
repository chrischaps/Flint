//! `flint schema` as data, and the references `flint validate` follows
//! (ADR 0115 decision 4, #684): the real binary.
//!
//! - `flint schema --list --json` names every component and archetype of
//!   the schema directories, and a name that is neither exits non-zero.
//! - `flint schema --format json-schema` emits a JSON Schema that every
//!   scene under `examples/` validates against (and that a scene with a
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
    scenes_under(&repo().join("examples"), &mut scenes);
    assert!(scenes.len() > 60, "only {} example scenes", scenes.len());
    let mut failures = Vec::new();
    for scene in &scenes {
        if let Err(e) = schemas.validate(&scene_as_json(scene), compiled) {
            failures.push(format!("{}:\n{e}", scene.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} example scenes do not validate:\n{}",
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
        let engine = repo().join("schemas");
        let mut args = vec![
            "validate",
            scene,
            "--schemas",
            engine.to_str().unwrap(),
            "--schemas",
            "schemas",
        ];
        args.extend(extra);
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
