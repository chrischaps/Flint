//! Scene-level tables through the real binary (#686; ADR 0115 decision 4).
//!
//! A small game project lays its own schema layer over the engine's: a
//! `[table.weather]` in `schemas/scene/weather.toml`. Then:
//!
//! - `flint validate` names a misspelled top-level table (`[enviroment]`)
//!   with the one it most likely is, a misspelled field of an engine table
//!   (`[post_process] bloom_intesity`) and of the game's own, and fails on a
//!   wrong type; warnings alone pass, and `--strict` fails on them
//! - `flint schema` lists the tables beside the components, shows one, and
//!   exports them as data and in the scene's JSON Schema

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

const WEATHER: &str = r#"
[table.weather]
description = "What the sky is doing in this scene."

[table.weather.fields]
rain = { type = "f32", min = 0.0, max = 1.0, default = 0.0, description = "How hard it rains" }
wind = { type = "vec3", description = "Metres a second, in world space" }
kind = { type = "enum", values = ["clear", "storm"], default = "clear" }
"#;

const GOOD: &str = r#"
[scene]
name = "Storm"

[environment]
shadows = false

[post_process]
bloom_intensity = 0.1

[weather]
rain = 0.7
wind = [3.0, 0.0, -1.0]
kind = "storm"
"#;

const TYPOS: &str = r#"
[scene]
name = "Typos"

[enviroment]
shadows = false

[post_process]
bloom_intesity = 1

[weather]
rian = 0.7
"#;

const WRONG: &str = r#"
[scene]
name = "Wrong"

[camera]
projection = "ortho"

[weather]
rain = 3.0
kind = "strom"
wind = "north"
"#;

/// A wrong type in one of the tables flint-scene reads into a struct never
/// reached the schemas: serde refuses the file, with the line.
const UNPARSED: &str = r#"
[scene]
name = "Unparsed"

[post_process]
exposure = "bright"
"#;

/// The game: the engine's schemas by path, its own layer over them.
fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("scene_tables_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let engine = repo()
        .join("schemas")
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/");
    let manifest = format!("[project]\nschemas = [\"{engine}\", \"schemas\"]\n");
    for (path, text) in [
        ("flint.toml", manifest.as_str()),
        ("schemas/scene/weather.toml", WEATHER),
        ("scenes/good.scene.toml", GOOD),
        ("scenes/typos.scene.toml", TYPOS),
        ("scenes/wrong.scene.toml", WRONG),
        ("scenes/unparsed.scene.toml", UNPARSED),
    ] {
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

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&o.stderr)))
}

/// The first Done-when box, and the fields of a table: each typo is a
/// warning with the name it most likely meant; the scene still passes, and
/// `--strict` fails it.
#[test]
fn a_misspelled_table_and_field_warn_with_a_suggestion() {
    let dir = project("typos");
    let o = flint(&dir, &["validate", "scenes/good.scene.toml"]);
    assert!(o.status.success(), "{}", stdout(&o));
    assert!(!stdout(&o).contains("warning"), "{}", stdout(&o));

    let o = flint(&dir, &["validate", "scenes/typos.scene.toml"]);
    let out = stdout(&o);
    assert!(o.status.success(), "warnings alone pass: {out}");
    assert!(
        out.starts_with("scenes/typos.scene.toml: 3 warnings\n"),
        "{out}"
    );
    for expected in [
        "  warning[unknown-table] [enviroment]: no scene table named `enviroment`, so nothing \
         reads it\n      hint: did you mean `environment`?\n",
        "  warning[unknown-field] [post_process] bloom_intesity: `post_process` has no field \
         `bloom_intesity`\n      hint: did you mean `bloom_intensity`?\n",
        "  warning[unknown-field] [weather] rian: `weather` has no field `rian`\n      hint: \
         did you mean `rain`?\n",
    ] {
        assert!(out.contains(expected), "missing:\n{expected}\nin:\n{out}");
    }

    let o = flint(&dir, &["validate", "scenes/typos.scene.toml", "--strict"]);
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("error[unknown-table] [enviroment]"), "{out}");
    assert!(out.contains(": 3 errors\n"), "{out}");
}

/// A wrong type, a number out of range and a value that is none of an
/// enum's fail the scene, each naming its table and field; JSON carries the
/// table's name.
#[test]
fn a_wrong_value_in_a_table_is_an_error() {
    let dir = project("wrong");
    let o = flint(&dir, &["validate", "scenes/wrong.scene.toml"]);
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    for expected in [
        "  error[bad-enum-value] [camera] projection: \"ortho\" is not a value of \
         `projection`\n      hint: allowed values: orthographic, perspective\n",
        "  error[bad-type] [weather] wind: expected vec3, got string\n",
        "  error[bad-enum-value] [weather] kind: \"strom\" is not a value of `kind`\n      \
         hint: did you mean `storm`? allowed values: clear, storm\n",
        "  error[out-of-range] [weather] rain: 3 is above the maximum 1\n",
    ] {
        assert!(out.contains(expected), "missing:\n{expected}\nin:\n{out}");
    }

    let o = flint(
        &dir,
        &["validate", "scenes/wrong.scene.toml", "--format", "json"],
    );
    let report = json(&o);
    let diagnostics = report["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(diagnostics.len(), 4, "{report}");
    assert!(diagnostics
        .iter()
        .any(|d| d["table"] == "weather" && d["field"] == "rain" && d["code"] == "out-of-range"));
    assert!(diagnostics.iter().all(|d| d["entity"].is_null()));

    let o = flint(&dir, &["validate", "scenes/unparsed.scene.toml"]);
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("error[toml-parse]"), "{out}");
    assert!(out.contains("6 | exposure = \"bright\""), "{out}");
}

/// `flint schema` knows a table as it knows a component: in the list, by
/// name, as data, and in the JSON Schema of a scene.
#[test]
fn flint_schema_lists_shows_and_exports_tables() {
    let dir = project("schema");
    let list = stdout(&flint(&dir, &["schema", "--list"]));
    assert!(list.contains("\nScene tables (6):\n"), "{list}");
    for name in [
        "camera",
        "environment",
        "living",
        "post_process",
        "weather",
        "wooze",
    ] {
        assert!(
            list.lines()
                .any(|line| line.split_whitespace().next() == Some(name)),
            "`{name}` is not in the list:\n{list}"
        );
    }
    let names = json(&flint(&dir, &["schema", "--list", "--json"]));
    assert_eq!(names["tables"].as_array().unwrap().len(), 6);
    assert_eq!(names["tables"][4]["name"], "weather");

    let text = stdout(&flint(&dir, &["schema", "weather"]));
    assert!(
        text.starts_with("Scene table: weather\nDescription: What the sky is doing"),
        "{text}"
    );
    assert!(text.contains("  rain : f32 = 0\n"), "{text}");
    assert!(text.contains("    # Range: 0 .. 1\n"), "{text}");
    // The engine's own, and a nested table's fields under it
    let wooze = stdout(&flint(&dir, &["schema", "wooze"]));
    assert!(
        wooze.contains("      shape : enum[sphere, box] (required)\n"),
        "{wooze}"
    );

    let one = json(&flint(
        &dir,
        &["schema", "post_process", "--format", "json"],
    ));
    assert_eq!(one["kind"], "table");
    assert_eq!(one["fields"]["bloom_intensity"]["type"], "f32");
    let all = json(&flint(&dir, &["schema", "--format", "json"]));
    assert_eq!(all["tables"]["weather"]["fields"]["rain"]["max"], 1.0);
    assert!(all["components"].get("wooze").is_none());

    let schema = json(&flint(&dir, &["schema", "--format", "json-schema"]));
    for name in ["camera", "post_process", "weather"] {
        assert_eq!(
            schema["properties"][name]["$ref"],
            format!("#/definitions/tables/{name}")
        );
    }
    assert_eq!(
        schema["definitions"]["tables"]["weather"]["properties"]["kind"]["enum"],
        serde_json::json!(["clear", "storm"])
    );
    let one = json(&flint(
        &dir,
        &["schema", "weather", "--format", "json-schema"],
    ));
    assert_eq!(one, schema["definitions"]["tables"]["weather"]);
}
