//! `flint entity create` and `delete` edit the scene, not rewrite it (#678).
//! The real binary on a small project with a commented scene, a prefab
//! instance and a `[post_process]` block: a create adds exactly its table, a
//! delete takes out exactly its own, a string with a colon survives, a parent
//! with children is refused (or cascades with `--recursive`), and an edit that
//! would break the scene is refused before the file changes.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SCENE: &str = "\
# The hall, written by hand. Comments here must survive every edit.
[scene]
name = \"Hall\"

[post_process]
bloom_intensity = 0.2

[prefabs.lamp_a]
template = \"lamp_pair\"
prefix = \"a\"

# The room everything hangs off
[entities.hall]
archetype = \"room\"

[entities.hall.bounds]
min = [0, 0, 0]
max = [10, 4, 8]

# The door, a child of the hall
[entities.hall_door]
archetype = \"door\"
parent = \"hall\"

# Hinges hang off the door
[entities.hinge]
archetype = \"door\"
parent = \"hall_door\"

[entities.cellar]
archetype = \"room\"
";

const FILES: &[(&str, &str)] = &[
    (
        "flint.toml",
        "[project]\nid = \"hall\"\nschemas = [\"schemas\"]\n",
    ),
    (
        "schemas/components/bounds.toml",
        "[component.bounds]\ndescription = \"A box\"\n\n\
         [component.bounds.fields]\nmin = { type = \"vec3\" }\nmax = { type = \"vec3\" }\n",
    ),
    (
        "schemas/components/door.toml",
        "[component.door]\ndescription = \"A door\"\n\n\
         [component.door.fields]\nlocked = { type = \"bool\", default = false }\n\
         style = { type = \"string\", default = \"plain\" }\n",
    ),
    (
        "schemas/components/model.toml",
        "[component.model]\ndescription = \"A model\"\n\n\
         [component.model.fields]\nasset = { type = \"string\" }\n",
    ),
    (
        "schemas/archetypes/room.toml",
        "[archetype.room]\ndescription = \"A room\"\ncomponents = [\"bounds\"]\n\n\
         [archetype.room.defaults.bounds]\nmin = [0, 0, 0]\nmax = [1, 1, 1]\n",
    ),
    (
        "schemas/archetypes/door.toml",
        "[archetype.door]\ndescription = \"A door\"\ncomponents = [\"door\"]\n\n\
         [archetype.door.defaults.door]\nlocked = false\n",
    ),
    (
        "prefabs/lamp_pair.prefab.toml",
        "[prefab]\nname = \"lamp_pair\"\n\n\
         [entities.left]\narchetype = \"room\"\n\n\
         [entities.right]\narchetype = \"room\"\n",
    ),
    ("hall.scene.toml", SCENE),
];

/// A fresh project under the test's own temp directory.
fn project(case: &str) -> PathBuf {
    let root = std::env::temp_dir().join("flint-entity-cli").join(format!(
        "{}-{}",
        case,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for (path, body) in FILES {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, body).unwrap();
    }
    root
}

fn flint(root: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .args(args)
        .current_dir(root)
        .output()
        .expect("run flint")
}

fn ok(root: &Path, args: &[&str]) -> String {
    let out = flint(root, args);
    assert!(
        out.status.success(),
        "flint {:?} failed:\n{}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn refused(root: &Path, args: &[&str]) -> String {
    let out = flint(root, args);
    assert!(!out.status.success(), "flint {:?} should have failed", args);
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn scene(root: &Path) -> String {
    std::fs::read_to_string(root.join("hall.scene.toml")).unwrap()
}

const CREATE_LAMP: &[&str] = &[
    "entity",
    "create",
    "--archetype",
    "door",
    "--name",
    "porch",
    "--parent",
    "hall",
    "--scene",
    "hall.scene.toml",
    "--props",
    r#"{"model":{"asset":"a:b.glb"},"door":{"style":"ascii:cell=8","locked":true}}"#,
];

/// What a create adds is its table and nothing else, after the last entity,
/// with every byte before it as it was: comments, `[prefabs]` and
/// `[post_process]` included, no archetype defaults baked in.
#[test]
fn create_adds_only_its_table() {
    let root = project("create");
    ok(&root, CREATE_LAMP);
    let after = scene(&root);
    assert!(
        after.starts_with(SCENE),
        "the old text must be untouched:\n{after}"
    );
    let added = &after[SCENE.len()..];
    assert_eq!(
        added,
        "\n[entities.porch]\narchetype = \"door\"\nparent = \"hall\"\n\n\
         [entities.porch.door]\nlocked = true\nstyle = \"ascii:cell=8\"\n\n\
         [entities.porch.model]\nasset = \"a:b.glb\"\n",
    );
}

/// Strings with colons come back exactly as typed, through the loader.
#[test]
fn colons_round_trip() {
    let root = project("colons");
    ok(&root, CREATE_LAMP);
    let show = ok(
        &root,
        &["entity", "show", "porch", "--scene", "hall.scene.toml"],
    );
    let json: serde_json::Value = serde_json::from_str(&show).unwrap();
    assert_eq!(json["data"]["model"]["asset"], "a:b.glb");
    assert_eq!(json["data"]["door"]["style"], "ascii:cell=8");
    assert_eq!(json["parent"], "hall");
}

/// The same edit on two copies of the file gives the same bytes.
#[test]
fn the_same_edit_twice_is_byte_identical() {
    let one = project("twice-1");
    let two = project("twice-2");
    for root in [&one, &two] {
        ok(root, CREATE_LAMP);
        ok(
            root,
            &["entity", "delete", "cellar", "--scene", "hall.scene.toml"],
        );
    }
    assert_eq!(scene(&one), scene(&two));
}

/// A delete takes out the entity's table, its components and the comment
/// above it, and nothing else.
#[test]
fn delete_removes_only_its_table() {
    let root = project("delete");
    ok(&root, CREATE_LAMP);
    let before = scene(&root);
    ok(
        &root,
        &["entity", "delete", "porch", "--scene", "hall.scene.toml"],
    );
    assert_eq!(scene(&root), SCENE);
    assert_ne!(before, SCENE);

    ok(
        &root,
        &["entity", "delete", "cellar", "--scene", "hall.scene.toml"],
    );
    assert_eq!(
        scene(&root),
        SCENE.replace("\n[entities.cellar]\narchetype = \"room\"\n", "")
    );
}

/// A parent with children is refused, naming them, and the file is left
/// alone; `--recursive` takes the whole subtree.
#[test]
fn deleting_a_parent_errors_or_cascades() {
    let root = project("parent");
    let err = refused(
        &root,
        &["entity", "delete", "hall", "--scene", "hall.scene.toml"],
    );
    assert!(
        err.contains("hall_door") && err.contains("--recursive"),
        "{err}"
    );
    assert_eq!(scene(&root), SCENE);

    let out = ok(
        &root,
        &[
            "entity",
            "delete",
            "hall",
            "--recursive",
            "--scene",
            "hall.scene.toml",
        ],
    );
    assert!(out.contains("hinge") && out.contains("hall_door"), "{out}");
    let after = scene(&root);
    for gone in ["[entities.hall]", "hall_door", "hinge", "Hinges hang"] {
        assert!(!after.contains(gone), "{gone} still there:\n{after}");
    }
    assert!(after.contains("[prefabs.lamp_a]") && after.contains("[entities.cellar]"));
    // and it still loads
    ok(&root, &["entity", "list", "--scene", "hall.scene.toml"]);
}

/// Bad input is an error, never a panic or a half-written file.
#[test]
fn bad_input_is_refused_before_writing() {
    let root = project("bad");
    let cases: &[(&[&str], &str)] = &[
        (&["--props", "not json"], "props JSON"),
        (&["--props", "[1, 2]"], "object of components"),
        (&["--props", r#"{"door": 3}"#], "object of fields"),
        (&["--props", r#"{"door": {"style": null}}"#], "null"),
        (
            &["--props", r#"{"door": {"locked": "yes"}}"#],
            "expected bool",
        ),
        (&["--parent", "nowhere"], "nowhere"),
        (&["--name", "a_left"], "already exists"),
    ];
    for (extra, says) in cases {
        let mut args = vec![
            "entity",
            "create",
            "--archetype",
            "door",
            "--name",
            "porch",
            "--scene",
            "hall.scene.toml",
        ];
        // a later --name wins in clap only if repeated; rebuild instead
        if extra[0] == "--name" {
            args[5] = extra[1];
        } else {
            args.extend_from_slice(extra);
        }
        let err = refused(&root, &args);
        assert!(
            err.contains(says),
            "{extra:?}: expected '{says}' in:\n{err}"
        );
        assert!(!err.contains("panicked"), "{extra:?} panicked:\n{err}");
        assert_eq!(scene(&root), SCENE, "{extra:?} wrote the file");
    }

    // A prefab's entity has no table of its own to delete
    let err = refused(
        &root,
        &["entity", "delete", "a_left", "--scene", "hall.scene.toml"],
    );
    assert!(err.contains("[prefabs.lamp_a]"), "{err}");
    assert_eq!(scene(&root), SCENE);
}
