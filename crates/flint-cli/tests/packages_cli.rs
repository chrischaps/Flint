//! #1334's done-when, with the real binary: `[packages]` in a project made
//! by `flint init` decides which packages' schemas and script modules it
//! has, and a scene or script that uses a package it leaves out names the
//! line to add.
//!
//! - A scene with a `living_prop` in a project that lists no `living`
//!   fails `flint check` with exit 1 and a hint holding `living = "*"`.
//! - There, `flint schema` lists no `living_prop` and `flint script api`
//!   no `living` module.
//! - `flint package used` on the Woozy Grove reports `living` and
//!   `procgen`, and `--write` lists them.
//! - With no `[packages]` key, everything is as before.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("packages_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("flint runs")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn said(o: &Output) -> String {
    format!(
        "{}\n{}",
        stdout(o),
        String::from_utf8_lossy(&o.stderr).into_owned()
    )
}

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_str(&stdout(o)).unwrap_or_else(|e| panic!("{e}: {}", said(o)))
}

/// A `flint init` walk project with a grove scene of one living prop, and
/// a script that tunes it.
fn game(name: &str) -> PathBuf {
    let dir = scratch(name);
    let o = flint(&dir, &["init", "game"]);
    assert!(o.status.success(), "{}", said(&o));
    let game = dir.join("game");
    std::fs::write(
        game.join("scenes/grove.scene.toml"),
        "[scene]\nname = \"grove\"\n\n[entities.oak.transform]\nposition = [0.0, 0.0, 0.0]\n\n\
         [entities.oak.living_prop]\nlineage_seed = 3\n",
    )
    .unwrap();
    std::fs::write(
        game.join("scripts/tune.rhai"),
        "fn on_update() {\n    remove_wooze_zone(\"storm\");\n}\n",
    )
    .unwrap();
    game
}

fn set_packages(game: &Path, lines: &str) {
    let file = game.join("flint.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    let at = text.find("[packages]").expect("init writes [packages]");
    std::fs::write(&file, format!("{}[packages]\n{lines}", &text[..at])).unwrap();
}

fn remove_packages(game: &Path) {
    let file = game.join("flint.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    let at = text.find("[packages]").unwrap();
    std::fs::write(&file, &text[..at]).unwrap();
}

/// Every diagnostic of a `flint check --json` with `code`.
fn with_code<'a>(doc: &'a serde_json::Value, code: &str) -> Vec<&'a serde_json::Value> {
    doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == code)
        .collect()
}

#[test]
fn a_scene_using_an_unlisted_package_names_the_line_to_add() {
    let game = game("unlisted");
    // init's list names no feature package
    let o = flint(&game, &["check", "scenes/grove.scene.toml"]);
    assert_eq!(o.status.code(), Some(1), "{}", said(&o));
    let text = stdout(&o);
    assert!(text.contains("error[unlisted-package]"), "{text}");
    assert!(text.contains("living = \"*\""), "{text}");

    let o = flint(&game, &["check", "--json"]);
    assert_eq!(o.status.code(), Some(1));
    let doc = json(&o);
    let unlisted = with_code(&doc, "unlisted-package");
    let prop = unlisted
        .iter()
        .find(|d| d["component"] == "living_prop")
        .unwrap_or_else(|| panic!("{doc:#}"));
    assert_eq!(prop["entity"], "oak");
    assert_eq!(prop["file"], "scenes/grove.scene.toml");
    assert_eq!(prop["severity"], "error");
    assert!(prop["hint"]
        .as_str()
        .unwrap()
        .starts_with("add `living = \"*\"` under [packages] in "));
    // The script's call of a living-package function, at its line
    let call = unlisted
        .iter()
        .find(|d| d["file"] == "scripts/tune.rhai")
        .unwrap_or_else(|| panic!("{doc:#}"));
    assert_eq!(call["line"], 2);
    assert!(call["message"]
        .as_str()
        .unwrap()
        .contains("`remove_wooze_zone`"));
    let o = flint(&game, &["script", "check", "scripts/tune.rhai"]);
    assert_eq!(o.status.code(), Some(1), "{}", said(&o));
    assert!(stdout(&o).contains("unlisted-package"), "{}", said(&o));

    // Listing it: the prop and the call are the package's again
    set_packages(&game, "living = \"*\"\n");
    let doc = json(&flint(&game, &["check", "--json"]));
    assert!(with_code(&doc, "unlisted-package").is_empty(), "{doc:#}");
    assert!(with_code(&doc, "unknown-component").is_empty(), "{doc:#}");
}

#[test]
fn a_left_out_package_has_no_schemas_and_no_script_module() {
    let game = game("left_out");
    let components = |game: &Path| -> Vec<String> {
        let doc = json(&flint(game, &["schema", "--list", "--json"]));
        doc["components"]
            .as_array()
            .unwrap_or_else(|| panic!("{doc:#}"))
            .iter()
            .map(|c| {
                c.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| c["name"].as_str().unwrap().to_string())
            })
            .collect()
    };
    let modules = |game: &Path| -> Vec<String> {
        let doc = json(&flint(game, &["script", "api", "--json"]));
        let mut out: Vec<String> = doc["functions"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|f| f["module"].as_str().map(str::to_string))
            .collect();
        out.dedup();
        out
    };

    let listed = components(&game);
    assert!(!listed.iter().any(|c| c == "living_prop"), "{listed:?}");
    assert!(!listed.iter().any(|c| c == "music_session"), "{listed:?}");
    assert!(listed.iter().any(|c| c == "transform"), "{listed:?}");
    let api = modules(&game);
    assert!(
        !api.iter().any(|m| m == "living" || m == "wooze"),
        "{api:?}"
    );
    let doc = json(&flint(&game, &["script", "api", "--json"]));
    let left_out: Vec<&str> = doc["left_out"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["package"].as_str().unwrap())
        .collect();
    assert!(left_out.contains(&"living"), "{doc}");
    let text = stdout(&flint(&game, &["script", "api"]));
    assert!(
        text.contains("[packages] leaves their packages out"),
        "{text}"
    );

    // Listed, they are back
    set_packages(&game, "living = \"*\"\n");
    assert!(components(&game).iter().any(|c| c == "living_prop"));
    assert!(modules(&game).iter().any(|m| m == "living"));
    assert!(!components(&game).iter().any(|c| c == "music_session"));

    // No key at all: every package, as before #1334
    remove_packages(&game);
    let all = components(&game);
    assert!(all.iter().any(|c| c == "living_prop"), "{all:?}");
    assert!(all.iter().any(|c| c == "music_session"), "{all:?}");
    let doc = json(&flint(
        &game,
        &["check", "scenes/grove.scene.toml", "--json"],
    ));
    assert!(with_code(&doc, "unlisted-package").is_empty(), "{doc:#}");
}

#[test]
fn a_mistake_in_the_list_is_named_with_the_nearest_package() {
    let game = game("mistake");
    set_packages(&game, "livng = \"*\"\n");
    let o = flint(&game, &["check", "--json"]);
    assert_eq!(o.status.code(), Some(1), "{}", said(&o));
    let doc = json(&o);
    let bad = with_code(&doc, "bad-manifest");
    assert_eq!(bad.len(), 1, "{doc:#}");
    assert_eq!(bad[0]["field"], "packages.livng");
    assert_eq!(bad[0]["hint"], "did you mean `living`?");
    // Every command that reads schemas stops on it, saying the line
    let o = flint(&game, &["schema", "--list"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(said(&o).contains("packages.livng"), "{}", said(&o));

    set_packages(&game, "living = true\n");
    let o = flint(&game, &["check"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(said(&o).contains("not `true`"), "{}", said(&o));
}

/// The Woozy Grove's frozen copy, in a project of its own.
fn grove(name: &str) -> PathBuf {
    let dir = scratch(name);
    let o = flint(&dir, &["init", "game", "--template", "blank"]);
    assert!(o.status.success(), "{}", said(&o));
    let game = dir.join("game");
    let from = repo().join("tests/fixtures/scenes/woozy_grove");
    let to = game.join("scenes/woozy_grove");
    std::fs::create_dir_all(to.join("specs")).unwrap();
    std::fs::copy(
        from.join("woozy_grove.scene.toml"),
        to.join("woozy_grove.scene.toml"),
    )
    .unwrap();
    for spec in std::fs::read_dir(from.join("specs")).unwrap() {
        let spec = spec.unwrap().path();
        std::fs::copy(&spec, to.join("specs").join(spec.file_name().unwrap())).unwrap();
    }
    game
}

#[test]
fn package_used_on_the_woozy_grove_reports_living_and_procgen() {
    let game = grove("used");
    let o = flint(&game, &["package", "used", "--json"]);
    assert!(o.status.success(), "{}", said(&o));
    let doc = json(&o);
    let names: Vec<&str> = doc["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["procgen", "living"], "{doc:#}");
    assert_eq!(
        doc["lines"],
        serde_json::json!(["procgen = \"*\"", "living = \"*\""])
    );
    let text = stdout(&flint(&game, &["package", "used"]));
    assert!(
        text.contains("components living_prop, shallow_wooze in scenes/woozy_grove/"),
        "{text}"
    );
    assert!(text.contains("required by living"), "{text}");
    assert!(text.contains("living = \"*\""), "{text}");

    // Unlisted, the grove fails the check; `--write` lists what it uses
    let o = flint(
        &game,
        &["check", "scenes/woozy_grove/woozy_grove.scene.toml"],
    );
    assert_eq!(o.status.code(), Some(1), "{}", said(&o));
    assert!(stdout(&o).contains("unlisted-package"), "{}", said(&o));
    let o = flint(&game, &["package", "used", "--write"]);
    assert!(o.status.success(), "{}", said(&o));
    let manifest = std::fs::read_to_string(game.join("flint.toml")).unwrap();
    assert!(
        manifest.contains("[packages]\nprocgen = \"*\"\nliving = \"*\"\n"),
        "{manifest}"
    );
    // init's comment above the table is kept
    assert!(
        manifest.contains("flint package used --write"),
        "{manifest}"
    );
    let doc = json(&flint(
        &game,
        &[
            "check",
            "scenes/woozy_grove/woozy_grove.scene.toml",
            "--json",
        ],
    ));
    assert!(with_code(&doc, "unlisted-package").is_empty(), "{doc:#}");
}

/// #1335's first done-when: in a project listing only `procgen`, `flint
/// build --dry-run` prints a cargo command with `--no-default-features`
/// and none of `living`, `music` or `rhythm`, and builds nothing.
#[test]
fn build_dry_run_for_procgen_alone_leaves_the_feature_packages_out() {
    let game = game("build_procgen");
    set_packages(&game, "procgen = \"*\"\n");
    let o = flint(&game, &["build", "--dry-run"]);
    assert!(o.status.success(), "{}", said(&o));
    let text = stdout(&o);
    let commands: Vec<&str> = text.lines().filter(|l| l.starts_with("cargo ")).collect();
    assert_eq!(commands.len(), 1, "{text}");
    let line = commands[0];
    assert!(
        line.starts_with("cargo build -p flint-player -p flint-cli"),
        "{line}"
    );
    assert!(line.contains(" --no-default-features "), "{line}");
    assert!(line.contains("flint-player/procgen"), "{line}");
    for left_out in ["living", "music", "rhythm"] {
        assert!(!line.contains(left_out), "{left_out}: {line}");
    }
    assert!(
        text.contains("left out  ai, living, music, rhythm"),
        "{text}"
    );
    assert!(text.contains("--dry-run built nothing"), "{text}");

    let doc = json(&flint(&game, &["build", "--dry-run", "--json"]));
    assert_eq!(doc["dry_run"], true);
    assert_eq!(doc["commands"][0], line);
    let packages: Vec<&str> = doc["features"]["flint-player"]["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    assert_eq!(packages, ["debug-hud", "procgen"], "{doc:#}");
    for bin in ["flint-player", "flint"] {
        let path = doc["binaries"][bin].as_str().unwrap();
        assert!(path.contains("/debug/"), "{bin}: {path}");
    }
}

/// The default build has every package, so a project listing `living`
/// is never told to build: the error is for a binary that lacks one.
#[test]
fn the_default_flint_has_every_package_a_project_lists() {
    let game = game("build_default");
    set_packages(&game, "living = \"*\"\nmusic = \"*\"\nrhythm = \"*\"\n");
    let doc = json(&flint(&game, &["check", "--json"]));
    assert!(with_code(&doc, "unbuilt-package").is_empty(), "{doc:#}");
}
