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

/// #1578's render box: `flint render` of the Woozy Grove in a project with
/// `procgen = false` makes nothing from a spec (no `[procgen]` load line, no
/// grown prop) and builds no living subsystem, as the player does: each
/// prop is drawn as a still model and living's left-out line is said once.
/// Before, a capture had no `[packages]` and grew the whole grove.
#[test]
fn render_of_a_project_that_leaves_procgen_out_makes_no_spec_asset() {
    let game = grove("render_off");
    set_packages(&game, "procgen = false\n");
    let o = flint(
        &game,
        &[
            "render",
            "scenes/woozy_grove/woozy_grove.scene.toml",
            "-o",
            "grove.png",
            "--width",
            "160",
            "--height",
            "90",
            "--report",
            "json",
        ],
    );
    let stderr = String::from_utf8_lossy(&o.stderr);
    if !o.status.success() && stderr.contains("headless render context") {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    }
    assert!(o.status.success(), "{}", said(&o));
    assert!(
        !stderr.lines().any(|l| l.contains("[procgen] ")),
        "no spec is made: {stderr}"
    );
    let left_out: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("[living] [packages] in flint.toml leaves out `living`"))
        .collect();
    assert_eq!(left_out.len(), 1, "{stderr}");
    let doc = json(&o);
    let still = doc["still_props"].as_array().expect("still_props");
    assert_eq!(still.len(), 38, "every prop stands still: {doc:#}");
}

/// #1536's done-when: on a project whose files name no procgen spec, `flint
/// package used` names `procgen = false` as a line to add, never
/// `debug-hud`, and `--write` writes no `= false` line.
#[test]
fn package_used_names_procgen_to_leave_out_where_no_file_names_a_spec() {
    let dir = scratch("leave_out");
    let o = flint(&dir, &["init", "game", "--template", "blank"]);
    assert!(o.status.success(), "{}", said(&o));
    let game = dir.join("game");

    let o = flint(&game, &["package", "used", "--json"]);
    assert!(o.status.success(), "{}", said(&o));
    let doc = json(&o);
    assert_eq!(doc["lines"], serde_json::json!([]), "{doc:#}");
    // The blank template animates nothing either, so animation, flipbook
    // and rig (standard since #1620 and #1621) are named too
    assert_eq!(
        doc["leave_out"],
        serde_json::json!([
            { "name": "animation", "line": "animation = false" },
            { "name": "flipbook", "line": "flipbook = false" },
            { "name": "procgen", "line": "procgen = false" },
            { "name": "rig", "line": "rig = false" }
        ]),
        "{doc:#}"
    );

    let text = stdout(&flint(&game, &["package", "used"]));
    assert!(text.contains("no package but the kernel is used"), "{text}");
    assert!(
        text.contains("[packages]\nanimation = false   # nothing scanned uses it\n")
            && text.contains("\nprocgen = false   # nothing scanned uses it\n"),
        "{text}"
    );
    assert!(!text.contains("debug-hud"), "{text}");

    // `--write` names the line and leaves it out of the file
    let o = flint(&game, &["package", "used", "--write"]);
    assert!(o.status.success(), "{}", said(&o));
    assert!(
        stdout(&o).contains("not written, under [packages]:\nanimation = false"),
        "{}",
        said(&o)
    );
    let manifest = std::fs::read_to_string(game.join("flint.toml")).unwrap();
    assert!(!manifest.contains("procgen = false"), "{manifest}");

    // Added by hand, the line is kept and nothing more is named
    set_packages(
        &game,
        "procgen = false\nanimation = false\nflipbook = false\nrig = false\n",
    );
    let doc = json(&flint(&game, &["package", "used", "--json"]));
    assert_eq!(doc["leave_out"], serde_json::json!([]), "{doc:#}");
    let o = flint(&game, &["build", "--dry-run"]);
    assert!(o.status.success(), "{}", said(&o));
    assert!(
        stdout(&o)
            .contains("left out  animation, flipbook, procgen, rig, ai, living, music, rhythm"),
        "{}",
        said(&o)
    );
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
    assert_eq!(
        packages,
        ["animation", "debug-hud", "flipbook", "procgen", "rig"],
        "{doc:#}"
    );
    // The launcher (#1552): the kernel, then the packages with a part in
    // the player (debug-hud, animation, flipbook and rig are standard: in
    // unless set false)
    assert_eq!(
        doc["launcher"]["packages"],
        serde_json::json!([
            "engine",
            "animation",
            "debug-hud",
            "flipbook",
            "procgen",
            "rig"
        ]),
        "{doc:#}"
    );
    for bin in ["flint-player", "flint"] {
        let path = doc["binaries"][bin].as_str().unwrap();
        assert!(path.contains("/debug/"), "{bin}: {path}");
    }
}

/// #1552's done-when: a project that lists only `procgen` (its standard
/// package set false) is planned a player whose launcher holds only that
/// package, after the kernel, and the plan says so in its text and JSON.
#[test]
fn build_dry_run_names_a_launcher_holding_only_the_listed_package() {
    let game = game("build_launcher");
    set_packages(
        &game,
        "procgen = \"*\"\ndebug-hud = false\nanimation = false\nflipbook = false\nrig = false\n",
    );
    let o = flint(&game, &["build", "--dry-run"]);
    assert!(o.status.success(), "{}", said(&o));
    let text = stdout(&o);
    assert!(
        text.contains(
            "launcher  flint-player, its stock list from these features: engine, procgen\n"
        ),
        "{text}"
    );
    let doc = json(&flint(&game, &["build", "--dry-run", "--json"]));
    assert_eq!(doc["launcher"]["binary"], "flint-player", "{doc:#}");
    assert_eq!(
        doc["launcher"]["packages"],
        serde_json::json!(["engine", "procgen"]),
        "{doc:#}"
    );
    let line = doc["commands"][0].as_str().unwrap();
    assert!(line.contains("flint-player/procgen"), "{line}");
    assert!(!line.contains("debug-hud"), "{line}");
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

/// A `flint init` blank project with one scene of two `music_session`s,
/// each the table `session` (#1581).
fn music_game(name: &str, session: &str) -> PathBuf {
    let dir = scratch(name);
    let o = flint(&dir, &["init", "game", "--template", "blank"]);
    assert!(o.status.success(), "{}", said(&o));
    let game = dir.join("game");
    std::fs::write(
        game.join("scenes/main.scene.toml"),
        format!(
            "[scene]\nname = \"song\"\n\n[entities.music.music_session]\n{session}\n\n\
             [entities.band.music_session]\n{session}\n"
        ),
    )
    .unwrap();
    game
}

const CHARTED: &str = "manifest = \"music/tempo_change.suite.toml\"\n\
                       chart = \"music/tempo_change.chart.toml\"";

/// The packages `flint package used --json` names, in order.
fn used_names(doc: &serde_json::Value) -> Vec<String> {
    doc["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect()
}

/// #1581's done-when: a `music_session` that sets `chart` uses rhythm,
/// which the scan names with the field as the use and `--write` lists; a
/// session with no chart uses music alone.
#[test]
fn package_used_names_rhythm_for_a_music_session_with_a_chart() {
    let game = music_game("used_chart", CHARTED);
    let doc = json(&flint(&game, &["package", "used", "--json"]));
    assert_eq!(used_names(&doc), ["music", "rhythm"], "{doc:#}");
    let rhythm = &doc["packages"][1];
    assert_eq!(
        rhythm["uses"],
        serde_json::json!([{
            "file": "scenes/main.scene.toml",
            "kind": "field",
            "item": "music_session.chart",
        }]),
        "{doc:#}"
    );
    assert_eq!(
        doc["lines"],
        serde_json::json!(["music = \"*\"", "rhythm = \"*\""])
    );
    let text = stdout(&flint(&game, &["package", "used"]));
    assert!(
        text.contains("field music_session.chart in scenes/main.scene.toml"),
        "{text}"
    );

    let o = flint(&game, &["package", "used", "--write"]);
    assert!(o.status.success(), "{}", said(&o));
    let manifest = std::fs::read_to_string(game.join("flint.toml")).unwrap();
    assert!(manifest.contains("music = \"*\""), "{manifest}");
    assert!(manifest.contains("rhythm = \"*\""), "{manifest}");

    let plain = music_game(
        "used_no_chart",
        "manifest = \"music/tempo_change.suite.toml\"",
    );
    let doc = json(&flint(&plain, &["package", "used", "--json"]));
    assert_eq!(used_names(&doc), ["music"], "{doc:#}");
}

/// #1581's done-when: with only `music` listed, `flint check` says the
/// chart is rhythm's, once for the file, with `rhythm = "*"` as the line
/// to add; with rhythm listed, or with no chart, it says nothing of it.
#[test]
fn check_names_rhythm_for_a_chart_with_only_music_listed() {
    let game = music_game("check_chart", CHARTED);
    set_packages(&game, "music = \"*\"\n");
    let o = flint(&game, &["check", "scenes/main.scene.toml", "--json"]);
    assert_eq!(o.status.code(), Some(1), "{}", said(&o));
    let doc = json(&o);
    let unlisted = with_code(&doc, "unlisted-package");
    assert_eq!(unlisted.len(), 1, "{doc:#}");
    let chart = unlisted[0];
    assert_eq!(chart["entity"], "band", "{chart:#}");
    assert_eq!(chart["component"], "music_session");
    assert_eq!(chart["field"], "chart");
    assert_eq!(
        chart["message"],
        "the field `music_session.chart` is the `rhythm` package's, and [packages] doesn't \
         list it (1 other entity too)"
    );
    assert!(
        chart["hint"]
            .as_str()
            .unwrap()
            .starts_with("add `rhythm = \"*\"` under [packages] in "),
        "{chart:#}"
    );
    let text = stdout(&flint(&game, &["check", "scenes/main.scene.toml"]));
    assert!(text.contains("error[unlisted-package]"), "{text}");
    assert!(text.contains("rhythm = \"*\""), "{text}");

    // `flint validate` says it too, from the same pass
    let o = flint(
        &game,
        &["validate", "scenes/main.scene.toml", "--format", "json"],
    );
    assert_eq!(o.status.code(), Some(1), "{}", said(&o));
    assert!(stdout(&o).contains("`music_session.chart`"), "{}", said(&o));

    set_packages(&game, "music = \"*\"\nrhythm = \"*\"\n");
    let doc = json(&flint(
        &game,
        &["check", "scenes/main.scene.toml", "--json"],
    ));
    assert!(with_code(&doc, "unlisted-package").is_empty(), "{doc:#}");

    let plain = music_game(
        "check_no_chart",
        "manifest = \"music/tempo_change.suite.toml\"",
    );
    set_packages(&plain, "music = \"*\"\n");
    let doc = json(&flint(
        &plain,
        &["check", "scenes/main.scene.toml", "--json"],
    ));
    assert!(with_code(&doc, "unlisted-package").is_empty(), "{doc:#}");
}
