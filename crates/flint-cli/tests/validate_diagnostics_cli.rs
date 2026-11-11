//! `flint validate` reports everything wrong with a scene in one run (ADR
//! 0115, #683): the real binary on a small project whose scene misspells a
//! field, a component and an archetype and gives an enum a value it doesn't
//! have. Each is named with its entity, component and field and a
//! suggestion, the exit code is non-zero, `--format json` carries the same
//! list, and `--strict` (or `[project] strict = true`) makes the unknown
//! field an error. The project has no constraint files: validate used to
//! return before loading the scene in that case.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const MANIFEST: &str = "[project]\nid = \"lamps\"\nschemas = [\"schemas\"]\n";

const FILES: &[(&str, &str)] = &[
    (
        "schemas/components/lamp.toml",
        "[component.lamp]\ndescription = \"A lamp\"\n\n[component.lamp.fields]\n\
         brightness = { type = \"f32\", default = 1.0, min = 0.0 }\n\
         kind = { type = \"enum\", values = [\"point\", \"spot\"], default = \"point\" }\n",
    ),
    (
        "schemas/components/place.toml",
        "[component.place]\n\n[component.place.fields]\n\
         position = { type = \"vec3\", default = [0, 0, 0] }\n",
    ),
    // Declared by name only: open, for a component only scripts read
    (
        "schemas/components/inventory.toml",
        "[component.inventory]\ndescription = \"Read by scripts\"\n",
    ),
    (
        "schemas/archetypes/lamp.toml",
        "[archetype.lamp]\ndescription = \"A lamp\"\ncomponents = [\"lamp\", \"place\"]\n\n\
         [archetype.lamp.defaults.lamp]\nbrightness = 2.0\n",
    ),
    (
        "scenes/good.scene.toml",
        "[scene]\nname = \"Good\"\n\n[entities.desk_lamp]\narchetype = \"lamp\"\n\n\
         [entities.desk_lamp.lamp]\nkind = \"spot\"\n\n\
         [entities.chest.inventory]\nslots = 4\n",
    ),
    (
        "scenes/broken.scene.toml",
        "[scene]\nname = \"Broken\"\n\n\
         [entities.desk_lamp]\narchetype = \"lmap\"\n\n\
         [entities.desk_lamp.lamp]\nbrightnes = 3.0\nkind = \"spto\"\n\n\
         [entities.floor_lamp.lapm]\nbrightness = 1.0\n\n\
         [entities.floor_lamp.place]\nposition = \"left\"\n",
    ),
    (
        "scenes/typo_only.scene.toml",
        "[scene]\nname = \"Typo\"\n\n[entities.desk_lamp.lamp]\nbrightnes = 3.0\n",
    ),
    (
        "scenes/not_toml.scene.toml",
        "[scene]\nname = \"Not TOML\"\n\n[entities.desk_lamp\n",
    ),
];

fn project(name: &str, manifest: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("validate_diagnostics_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in FILES.iter().chain(&[("flint.toml", manifest)]) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    dir
}

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

/// The ticket's first two boxes, through the binary.
#[test]
fn every_mistake_is_reported_in_one_run_and_fails() {
    let dir = project("text", MANIFEST);
    let o = flint(&dir, &["validate", "scenes/broken.scene.toml"]);
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(
        out.starts_with("scenes/broken.scene.toml: 4 errors, 1 warning\n"),
        "{out}"
    );
    for expected in [
        // the archetype
        "  error[unknown-archetype] entity 'desk_lamp' field 'archetype': no archetype named \
         `lmap`, so the entity gets no defaults\n      hint: did you mean `lamp`?\n",
        // the enum value
        "  error[bad-enum-value] entity 'desk_lamp' lamp.kind: \"spto\" is not a value of \
         `kind`\n      hint: did you mean `spot`? allowed values: point, spot\n",
        // the component
        "  error[unknown-component] entity 'floor_lamp' component 'lapm': no component named \
         `lapm`, so nothing checks it\n      hint: did you mean `lamp`?\n",
        // a wrong type
        "  error[bad-type] entity 'floor_lamp' place.position: expected vec3, got string\n",
        // the field: a warning
        "  warning[unknown-field] entity 'desk_lamp' lamp.brightnes: `lamp` has no field \
         `brightnes`\n      hint: did you mean `brightness`?\n",
    ] {
        assert!(out.contains(expected), "missing:\n{expected}\nin:\n{out}");
    }
    // Errors come before warnings
    assert!(out.find("warning[").unwrap() > out.rfind("error[").unwrap());
}

#[test]
fn json_carries_the_same_list() {
    let dir = project("json", MANIFEST);
    let o = flint(
        &dir,
        &["validate", "scenes/broken.scene.toml", "--format", "json"],
    );
    assert_eq!(o.status.code(), Some(1));
    let doc: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("one JSON document");
    assert_eq!(doc["valid"], false);
    assert_eq!(doc["errors"], 4);
    assert_eq!(doc["warnings"], 1);
    let found: Vec<(String, String, String, String, String)> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            let s = |key: &str| d[key].as_str().unwrap_or("").to_string();
            (
                s("severity"),
                s("code"),
                s("entity"),
                s("component"),
                s("field"),
            )
        })
        .collect();
    let row = |a: &str, b: &str, c: &str, d: &str, e: &str| {
        (a.into(), b.into(), c.into(), d.into(), e.into())
    };
    assert_eq!(
        found,
        [
            row("error", "unknown-archetype", "desk_lamp", "", "archetype"),
            row("warning", "unknown-field", "desk_lamp", "lamp", "brightnes"),
            row("error", "bad-enum-value", "desk_lamp", "lamp", "kind"),
            row("error", "unknown-component", "floor_lamp", "lapm", ""),
            row("error", "bad-type", "floor_lamp", "place", "position"),
        ]
    );
    let first = &doc["diagnostics"][0];
    assert_eq!(first["file"], "scenes/broken.scene.toml");
    assert_eq!(first["hint"], "did you mean `lamp`?");
}

/// A clean scene passes with no constraint files, and a component declared
/// by name only (no fields) is known and unchecked.
#[test]
fn a_clean_scene_passes_and_an_open_component_is_known() {
    let dir = project("clean", MANIFEST);
    let o = flint(&dir, &["validate", "scenes/good.scene.toml"]);
    let out = stdout(&o);
    assert!(o.status.success(), "{out}");
    assert!(out.starts_with("No problems found.\n"), "{out}");
    assert!(out.contains("No constraints found"), "{out}");
}

/// Decision 1: an unknown field alone is a warning and passes; `--strict`
/// and `[project] strict = true` make it an error.
#[test]
fn strict_makes_an_unknown_field_an_error() {
    let scene = "scenes/typo_only.scene.toml";
    let dir = project("lenient", MANIFEST);
    let o = flint(&dir, &["validate", scene]);
    let out = stdout(&o);
    assert!(o.status.success(), "a warning alone passes: {out}");
    assert!(out.contains("warning[unknown-field]"), "{out}");
    assert!(out.contains(": 1 warning\n"), "{out}");

    let o = flint(&dir, &["validate", scene, "--strict"]);
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(
        out.contains("error[unknown-field] entity 'desk_lamp' lamp.brightnes"),
        "{out}"
    );

    let strict = project(
        "strict_manifest",
        "[project]\nschemas = [\"schemas\"]\nstrict = true\n",
    );
    let o = flint(&strict, &["validate", scene]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(
        stdout(&o).contains("error[unknown-field]"),
        "{}",
        stdout(&o)
    );
}

/// The ticket's fourth box: a file that is not TOML is named, by validate
/// and by a command that loads the scene.
#[test]
fn a_toml_parse_error_names_the_file() {
    let dir = project("parse", MANIFEST);
    let scene = "scenes/not_toml.scene.toml";
    let o = flint(&dir, &["validate", scene]);
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(
        out.starts_with("scenes/not_toml.scene.toml: 1 error\n"),
        "{out}"
    );
    assert!(
        out.contains("error[toml-parse]: TOML parse error at line 4"),
        "{out}"
    );

    let o = flint(&dir, &["validate", scene, "--format", "json"]);
    let doc: serde_json::Value = serde_json::from_str(&stdout(&o)).expect("one JSON document");
    assert_eq!(doc["diagnostics"][0]["code"], "toml-parse");
    assert_eq!(doc["diagnostics"][0]["file"], scene);

    // The loader's own error (here through `flint query`) names it too
    let o = flint(&dir, &["query", "entities", "--scene", scene]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    assert!(
        err.contains("scenes/not_toml.scene.toml: TOML parse error at line 4"),
        "{err}"
    );
}
