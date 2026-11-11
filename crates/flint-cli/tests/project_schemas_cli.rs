//! One schema resolver for every command (ADR 0114 step 2, #706): the real
//! binary in a small game project whose `flint.toml` names
//! `schemas = ["engine/schemas", "schemas"]`. With no `--schemas`, validate,
//! query, entity, schema and render all load both directories, the game's
//! over the engine's; flags replace the manifest's list; and a directory
//! that is named and isn't there is an error that says its name and who
//! named it.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

/// The project: an engine-like directory with a lamp and a switch, and a
/// game directory that overrides the lamp (brighter, with a colour, and a
/// box to draw), adds a component of its own and holds the one constraint.
/// The switch is the engine's alone, so `schemas/` in the working directory
/// (all that discovery finds from the root) never explains it.
const FILES: &[(&str, &str)] = &[
    (
        "flint.toml",
        "[project]\nid = \"lamps\"\nschemas = [\"engine/schemas\", \"schemas\"]\n",
    ),
    (
        "engine/schemas/components/lamp.toml",
        "[component.lamp]\ndescription = \"A lamp, as the engine knows it\"\n\n\
         [component.lamp.fields]\nbrightness = { type = \"f32\", default = 1.0 }\n",
    ),
    (
        "engine/schemas/archetypes/lamp.toml",
        "[archetype.lamp]\ndescription = \"A lamp\"\ncomponents = [\"lamp\"]\n\n\
         [archetype.lamp.defaults.lamp]\nbrightness = 1.0\n",
    ),
    (
        "engine/schemas/components/switch.toml",
        "[component.switch]\ndescription = \"Only the engine has switches\"\n\n\
         [component.switch.fields]\non = { type = \"bool\", default = false }\n",
    ),
    (
        "engine/schemas/archetypes/switch.toml",
        "[archetype.switch]\ndescription = \"A switch\"\ncomponents = [\"switch\"]\n\n\
         [archetype.switch.defaults.switch]\non = false\n",
    ),
    (
        "schemas/components/lamp.toml",
        "[component.lamp]\ndescription = \"A lamp, as the game overrides it\"\n\n\
         [component.lamp.fields]\nbrightness = { type = \"f32\", default = 5.0 }\n\
         colour = { type = \"string\", default = \"amber\" }\n",
    ),
    (
        "schemas/components/beacon.toml",
        "[component.beacon]\ndescription = \"Only the game has beacons\"\n\n\
         [component.beacon.fields]\nperiod = { type = \"f32\", default = 2.0 }\n",
    ),
    (
        "schemas/archetypes/lamp.toml",
        "[archetype.lamp]\ndescription = \"A lamp\"\ncomponents = [\"lamp\", \"bounds\"]\n\n\
         [archetype.lamp.defaults.lamp]\nbrightness = 5.0\ncolour = \"amber\"\n\n\
         [archetype.lamp.defaults.bounds]\nmin = [-1, 0, -1]\nmax = [1, 2, 1]\n",
    ),
    (
        "schemas/constraints/lamps.toml",
        "[[constraint]]\nname = \"lamps_are_beacons\"\n\
         description = \"Every lamp carries a beacon\"\n\
         query = \"entities where archetype == 'lamp'\"\nseverity = \"error\"\n\
         message = \"Lamp '{name}' has no beacon\"\n\n\
         [constraint.kind]\ntype = \"required_component\"\narchetype = \"lamp\"\n\
         component = \"beacon\"\n",
    ),
    (
        "scenes/hall.scene.toml",
        "[scene]\nname = \"Hall\"\nversion = \"1.0\"\n\n[entities.hall_lamp]\narchetype = \"lamp\"\n\n\
         [entities.wall_switch]\narchetype = \"switch\"\n",
    ),
];

const SCENE: &str = "scenes/hall.scene.toml";

/// The project written fresh under the test's own folder, with `manifest`
/// in place of its `flint.toml` when given.
fn project(name: &str, manifest: Option<&str>) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("project_schemas_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in FILES {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let text = match manifest {
            Some(other) if *path == "flint.toml" => other,
            _ => text,
        };
        std::fs::write(file, text).unwrap();
    }
    dir
}

/// `flint <args>` with `dir` as the working directory.
fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run flint")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A command's JSON with what differs run to run taken out: entity ids
/// (scenes load from a hash map) are dropped, and `components` lists and
/// lists of entities are sorted.
fn json(text: &str) -> serde_json::Value {
    fn settle(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Array(items) => {
                items.iter_mut().for_each(settle);
                items.sort_by_key(|item| item.to_string());
            }
            serde_json::Value::Object(map) => {
                map.remove("id");
                map.values_mut().for_each(settle);
            }
            _ => {}
        }
    }
    let mut value = serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"));
    settle(&mut value);
    value
}

fn ok(o: &Output) -> String {
    assert!(
        o.status.success(),
        "flint failed: {}{}",
        stdout(o),
        stderr(o)
    );
    stdout(o)
}

const BOTH: [&str; 4] = ["--schemas", "engine/schemas", "--schemas", "schemas"];
const ENGINE_ONLY: [&str; 2] = ["--schemas", "engine/schemas"];

fn with(args: &[&str], flags: &[&str]) -> Vec<String> {
    args.iter().chain(flags).map(|s| s.to_string()).collect()
}

fn run(dir: &Path, args: &[&str], flags: &[&str]) -> Output {
    let all = with(args, flags);
    flint(dir, &all.iter().map(String::as_str).collect::<Vec<_>>())
}

#[test]
fn schema_reads_the_manifest_and_the_game_overrides_the_engine() {
    let dir = project("schema", None);
    let lamp = ok(&flint(&dir, &["schema", "lamp"]));
    assert!(lamp.contains("as the game overrides it"), "{lamp}");
    assert!(lamp.contains("colour"), "{lamp}");
    // The game's own component, which `schemas/` in the working directory
    // alone would also find; the engine's lamp fields prove both loaded
    assert!(ok(&flint(&dir, &["schema", "beacon"])).contains("Only the game"));
    assert_eq!(lamp, ok(&run(&dir, &["schema", "lamp"], &BOTH)));

    // Flags are the whole list: the manifest's game directory is gone
    let engine = ok(&run(&dir, &["schema", "lamp"], &ENGINE_ONLY));
    assert!(engine.contains("as the engine knows it"), "{engine}");
    assert!(!engine.contains("colour"), "{engine}");
}

#[test]
fn query_and_entity_read_the_manifest() {
    let dir = project("query", None);
    let bright = [
        "query",
        "entities where lamp.brightness > 2",
        "--scene",
        SCENE,
    ];
    let found = ok(&flint(&dir, &bright));
    assert!(
        found.contains("hall_lamp"),
        "the game's default, 5: {found}"
    );
    assert_eq!(json(&found), json(&ok(&run(&dir, &bright, &BOTH))));
    // Repeatable, and an override: the engine's default is 1
    let engine = ok(&run(&dir, &bright, &ENGINE_ONLY));
    assert!(!engine.contains("hall_lamp"), "{engine}");

    let show = ["entity", "show", "hall_lamp", "--scene", SCENE];
    let shown = ok(&flint(&dir, &show));
    assert!(shown.contains("\"colour\": \"amber\""), "{shown}");
    assert!(shown.contains("\"brightness\": 5.0"), "{shown}");
    assert_eq!(json(&shown), json(&ok(&run(&dir, &show, &BOTH))));
    let engine = ok(&run(&dir, &show, &ENGINE_ONLY));
    assert!(engine.contains("\"brightness\": 1.0"), "{engine}");
    assert!(!engine.contains("colour"), "{engine}");

    // The engine's own archetype: the working directory's `schemas/` alone
    // (all `query` read, and `entity`'s default) knows no switch
    let switch = ok(&flint(
        &dir,
        &["entity", "show", "wall_switch", "--scene", SCENE],
    ));
    assert!(switch.contains("\"on\": false"), "{switch}");
    let off = [
        "query",
        "entities where switch.on == false",
        "--scene",
        SCENE,
    ];
    assert!(ok(&flint(&dir, &off)).contains("wall_switch"));
    assert!(ok(&flint(&dir, &["schema", "switch"])).contains("Only the engine"));

    let list = ["entity", "list", "--scene", SCENE];
    assert_eq!(
        json(&ok(&flint(&dir, &list))),
        json(&ok(&run(&dir, &list, &BOTH)))
    );
}

#[test]
fn the_project_is_the_scenes_not_the_working_directorys() {
    // From the scenes folder, where no `schemas/` is in the working
    // directory: the scene's project still names both
    let dir = project("from_scenes", None);
    let scenes = dir.join("scenes");
    let shown = ok(&flint(
        &scenes,
        &["entity", "show", "hall_lamp", "--scene", "hall.scene.toml"],
    ));
    assert!(shown.contains("\"colour\": \"amber\""), "{shown}");
    let lamp = ok(&flint(&scenes, &["schema", "lamp"]));
    assert!(lamp.contains("as the game overrides it"), "{lamp}");
}

#[test]
fn validate_reads_the_manifest() {
    let dir = project("validate", None);
    // The game's constraint is found, and fails the lamp
    let o = flint(&dir, &["validate", SCENE]);
    assert!(!o.status.success(), "{}", stdout(&o));
    assert!(
        stdout(&o).contains("Lamp 'hall_lamp' has no beacon"),
        "{}",
        stdout(&o)
    );
    let flagged = run(&dir, &["validate", SCENE], &BOTH);
    assert_eq!(stdout(&o), stdout(&flagged));
    assert_eq!(o.status.code(), flagged.status.code());

    // Engine alone: no constraints at all
    let engine = ok(&run(&dir, &["validate", SCENE], &ENGINE_ONLY));
    assert!(engine.contains("No constraints found"), "{engine}");
}

#[test]
fn a_named_directory_that_is_missing_is_an_error_by_name() {
    let dir = project("missing_flag", None);
    let commands: [&[&str]; 5] = [
        &["schema", "lamp"],
        &["query", "entities", "--scene", SCENE],
        &["entity", "list", "--scene", SCENE],
        &["validate", SCENE],
        &["render", SCENE, "--output", "out.png"],
    ];
    for command in commands {
        let o = run(
            &dir,
            command,
            &["--schemas", "engine/schemas", "--schemas", "nope"],
        );
        let said = stderr(&o);
        assert!(!o.status.success(), "{command:?}");
        assert!(
            said.contains("schema directory not found: nope (named by --schemas)"),
            "{command:?}: {said}"
        );
        assert_eq!(stdout(&o), "", "{command:?}: the error is stderr's alone");
    }

    // The manifest names engine/schemas in a project with no engine subtree
    let dir = project("missing_manifest", None);
    std::fs::remove_dir_all(dir.join("engine")).unwrap();
    for command in commands {
        let o = flint(&dir, command);
        let said = stderr(&o);
        assert!(!o.status.success(), "{command:?}");
        assert!(
            said.contains("schema directory not found:"),
            "{command:?}: {said}"
        );
        assert!(said.contains("schemas"), "{command:?}: {said}");
        assert!(said.contains("flint.toml"), "{command:?}: {said}");
        // The flags are the way round it
        if command[0] != "render" {
            let o = run(&dir, command, &["--schemas", "schemas"]);
            assert!(
                !stderr(&o).contains("schema directory not found"),
                "{command:?}: {}",
                stderr(&o)
            );
        }
    }
}

#[test]
fn without_a_schemas_key_discovery_is_what_it_was() {
    // A manifest that is only a marker: `schemas/` in the working directory
    // (and beside the scene, one level up) as before, so the engine's
    // directory in the subtree is not found
    let dir = project("marker_only", Some("[project]\nid = \"lamps\"\n"));
    let shown = ok(&flint(
        &dir,
        &["entity", "show", "hall_lamp", "--scene", SCENE],
    ));
    assert!(shown.contains("\"colour\": \"amber\""), "{shown}");
    let lamp = ok(&flint(&dir, &["schema", "lamp"]));
    assert!(lamp.contains("as the game overrides it"), "{lamp}");

    // And from a folder with no schemas/ at all, `flint schema` still says so
    let o = flint(&dir.join("scenes"), &["schema", "lamp"]);
    assert!(!o.status.success());
    assert!(
        stderr(&o).contains("No schemas directories found"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn render_reads_the_manifest() {
    let dir = project("render", None);
    let render = |out: &str, flags: &[&str]| -> Option<Vec<u8>> {
        let o = run(
            &dir,
            &[
                "render",
                SCENE,
                "--output",
                out,
                "--no-grid",
                "--width",
                "160",
                "--height",
                "90",
            ],
            flags,
        );
        if !o.status.success() && stderr(&o).contains("headless render context") {
            flint_render::gpu_select::skip_without_gpu("no GPU adapter");
            return None;
        }
        let said = ok(&o);
        assert!(
            !said.contains("No schemas directories found"),
            "render found no schemas: {said}"
        );
        Some(std::fs::read(dir.join(out)).unwrap())
    };
    let Some(plain) = render("plain.png", &[]) else {
        return;
    };
    let both = render("both.png", &BOTH).unwrap();
    let engine = render("engine.png", &ENGINE_ONLY).unwrap();
    assert_eq!(plain, both, "the manifest draws what both flags draw");
    // The game's archetype gives the lamp a box; the engine's draws nothing
    assert_ne!(plain, engine, "the game's schemas reached the render");
}
