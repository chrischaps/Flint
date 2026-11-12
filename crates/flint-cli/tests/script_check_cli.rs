//! #694's done-when, with the real binary on a small project.
//!
//! - A typo'd function name, a wrong argument count and a misspelt callback
//!   each fail `flint script check` with the file, line and column, as text
//!   and as JSON in `flint check`'s shape.
//! - `flint validate` and `flint check` run the same pass and fail on it.
//! - A game's own functions pass with `--allow` or with its `flint script
//!   api --json` listing (`--api`).
//! - A `script.source` that isn't there is named, with the paths tried.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SCHEMA: &str = "[component.script]\n\n[component.script.fields]\n\
    source = { type = \"string\", default = \"\" }\n";

const SCENE: &str = "[scene]\nname = \"Dock\"\n\n\
    [entities.keeper.script]\nsource = \"keeper.rhai\"\n";

/// Three mistakes, one a line.
const KEEPER: &str = "import \"tide\" as tide;\n\
    \n\
    fn on_update() {\n\
    \x20   let id = get_entity(\"lamp\");\n\
    \x20   set_postion(id, 0.0, 1.0, 0.0);\n\
    \x20   draw_line(0.0, 0.0, 10.0, 10.0, 1.0, 1.0, 1.0, 1.0);\n\
    \x20   tide::rise(2.0);\n\
    \x20   game_ring_bell(3);\n\
    }\n\
    \n\
    fn on_colision(other) {\n\
    \x20   log(\"bump\");\n\
    }\n";

const TIDE: &str = "fn rise(by) { by * 2.0 }\n";

fn project(name: &str, keeper: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("script_check_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in [
        (
            "flint.toml",
            "[project]\nid = \"dock\"\nschemas = [\"schemas\"]\n",
        ),
        ("schemas/components/script.toml", SCHEMA),
        ("scenes/dock.scene.toml", SCENE),
        ("scripts/keeper.rhai", keeper),
        ("scripts/lib/tide.rhai", TIDE),
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
    String::from_utf8_lossy(&o.stdout).replace("\r\n", "\n")
}

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_str(&stdout(o))
        .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}):\n{}", stdout(o)))
}

/// The game's binary would register this: its `flint script api --json`.
const GAME_API: &str = r#"{ "functions": [
    { "module": "game", "name": "game_ring_bell", "declared": true,
      "params": [{ "name": "times", "kind": "int" }] }
] }"#;

#[test]
fn a_typo_a_wrong_count_and_a_misspelt_callback_fail_with_file_and_line() {
    let dir = project("text", KEEPER);
    let o = flint(&dir, &["script", "check", "."]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    let out = stdout(&o);
    for line in [
        "scripts/keeper.rhai:5:5: error[unknown-function]: no script function `set_postion`\n    \
         hint: did you mean `set_position`?\n",
        "scripts/keeper.rhai:6:5: error[wrong-arity]: `draw_line` takes 5 or 9 arguments; this call passes 8\n    \
         hint: declared: draw_line(x1: num, y1: num, x2: num, y2: num, r: num, g: num, b: num, a: num, thickness: num); \
         draw_line(x1: num, y1: num, x2: num, y2: num, opts: map)\n",
        "scripts/keeper.rhai:8:5: error[unknown-function]: no script function `game_ring_bell`\n",
        "scripts/keeper.rhai:11:1: error[unknown-callback]: `fn on_colision` is not a callback, and nothing in this script calls it\n    \
         hint: did you mean `on_collision`?\n",
    ] {
        assert!(out.contains(line), "no\n{line}in\n{out}");
    }
    // The imported module is read and its function found at its arity
    assert!(!out.contains("tide"), "{out}");
    assert!(
        out.ends_with("checked 2 scripts (1 scene): 4 errors\n"),
        "{out}"
    );

    let doc = json(&flint(&dir, &["script", "check", ".", "--json"]));
    assert_eq!(doc["valid"], false);
    assert_eq!(doc["errors"], 4);
    assert_eq!(
        doc["scripts"],
        serde_json::json!(["scripts/keeper.rhai", "scripts/lib/tide.rhai"])
    );
    let arity = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == "wrong-arity")
        .unwrap();
    assert_eq!(arity["file"], "scripts/keeper.rhai");
    assert_eq!(arity["line"], 6);
    assert_eq!(arity["column"], 5);
    assert_eq!(arity["severity"], "error");
}

#[test]
fn validate_and_check_run_the_script_pass() {
    let dir = project("validate", KEEPER);
    let o = flint(&dir, &["validate", "scenes/dock.scene.toml"]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(
        stdout(&o).contains("error[unknown-callback] 11:1: `fn on_colision`"),
        "{}",
        stdout(&o)
    );
    let doc = json(&flint(
        &dir,
        &["validate", "scenes/dock.scene.toml", "--format", "json"],
    ));
    let codes: Vec<&str> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap())
        .collect();
    assert_eq!(
        codes,
        [
            "unknown-function",
            "wrong-arity",
            "unknown-function",
            "unknown-callback"
        ]
    );

    let o = flint(&dir, &["check"]);
    assert_eq!(o.status.code(), Some(1));
    let out = stdout(&o);
    assert!(
        out.contains("scripts/keeper.rhai: 4 errors\n  error[unknown-function] 5:5:"),
        "{out}"
    );
}

#[test]
fn a_games_own_functions_pass_when_allowed_or_listed() {
    let fixed = KEEPER
        .replace("set_postion", "set_position")
        .replace("1.0, 1.0);\n    tide", "1.0, 1.0, 2.0);\n    tide")
        .replace("on_colision", "on_collision");
    let dir = project("allow", &fixed);
    let o = flint(&dir, &["script", "check"]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(stdout(&o).contains("`game_ring_bell`"));

    let o = flint(&dir, &["script", "check", "--allow", "game_ring_bell"]);
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));
    assert_eq!(stdout(&o), "checked 2 scripts (1 scene): no problems\n");

    std::fs::write(dir.join("game-api.json"), GAME_API).unwrap();
    for args in [
        &["script", "check", "--api", "game-api.json"][..],
        &["check", "--api", "game-api.json"],
        &[
            "validate",
            "scenes/dock.scene.toml",
            "--api",
            "game-api.json",
        ],
    ] {
        let o = flint(&dir, args);
        assert_eq!(o.status.code(), Some(0), "{args:?}: {}", stdout(&o));
    }
    // Listed at one argument: two is still wrong
    std::fs::write(
        dir.join("scripts/keeper.rhai"),
        fixed.replace("game_ring_bell(3)", "game_ring_bell(3, 4)"),
    )
    .unwrap();
    let o = flint(&dir, &["script", "check", "--api", "game-api.json"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("`game_ring_bell` takes 1 argument; this call passes 2"));

    // A listing that isn't one is the error, by name
    std::fs::write(dir.join("bad.json"), "{}").unwrap();
    let o = flint(&dir, &["script", "check", "--api", "bad.json"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("--api bad.json: no `functions` list"));
}

#[test]
fn a_missing_source_names_the_paths_tried() {
    let dir = project("missing", KEEPER);
    std::fs::remove_file(dir.join("scripts/keeper.rhai")).unwrap();
    let o = flint(&dir, &["script", "check", "scenes/dock.scene.toml"]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    let out = stdout(&o);
    assert!(
        out.contains(
            "scenes/dock.scene.toml: error[missing-asset] entity 'keeper' script.source: \
             script `keeper.rhai` is not found"
        ),
        "{out}"
    );
    assert!(
        out.contains("    tried:\n      scenes/scripts/keeper.rhai\n      scripts/keeper.rhai\n"),
        "{out}"
    );
}
