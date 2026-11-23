//! `flint playtest` (#714), end to end through the binary: a walk with
//! every kind of check passes and reports the same bytes twice, and each
//! way a run fails (a script's run-time error, a failed assertion,
//! `test_fail`, an `expect` that doesn't hold, a timeout, a load warning
//! under `--strict`) exits 1 and says where. No GPU: a playtest opens no
//! window, renderer or sound device. The Phase 4 tavern's own playtest,
//! which this file ran until the examples moved out (#984), runs in
//! flint-examples' CI.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

/// The repository's root, where the schemas are.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run flint")
}

fn document(out: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is one JSON document ({e}):\n{stdout}\nstderr:\n{}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// The crate walks right while D (`move_right`) is held; past x = 3 it
/// opens the door once: an event, a `test_log` and a `test_assert`.
const WALKER: &str = "fn on_update() {\n\
    \x20   let c = get_entity(\"crate\");\n\
    \x20   let p = get_position(c);\n\
    \x20   if is_action_pressed(\"move_right\") {\n\
    \x20       set_position(c, p.x + 2.0 * delta_time(), p.y, p.z);\n\
    \x20   }\n\
    \x20   if p.x > 3.0 && persist_get(\"opened\") != true {\n\
    \x20       persist_set(\"opened\", true);\n\
    \x20       fire_event(\"door_opened\");\n\
    \x20       test_log(\"greeted\", true);\n\
    \x20       test_assert(p.x > 3.0, \"past the door\");\n\
    \x20   }\n\
    }\n";

/// Before anything moves, the walk, the door's event through a `wait`, and
/// what the script said through `test_log` and the store.
const WALK: &str = "0.0 expect crate transform.position.x < 1\n\
    0.0 expect no entities where counter.value > 0\n\
    0.2 down KeyD\n\
    0.2 wait crate transform.position.x > 3 within 3\n\
    0.2 up KeyD\n\
    0.2 wait event door_opened within 1\n\
    0.3 expect log greeted == true\n\
    0.3 expect persist opened == true\n";

/// The issue's first box: a scene's checked-in playtest, run here. The
/// crate walks to the door and opens it; every line of the input script
/// holds.
#[test]
fn a_walk_with_every_kind_of_check_passes() {
    let dir = project("walk", WALKER, WALK);
    let (out, d) = playtest(&dir, &["--exit-after", "4"]);
    assert!(out.status.success(), "{d:#}");
    assert_eq!(d["passed"], true);
    assert_eq!(d["ended"], "exit_after");
    let checks = d["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 6, "{d:#}");
    assert!(checks.iter().all(|c| c["passed"] == true), "{d:#}");
    // The door's event, through a `wait`
    assert!(checks
        .iter()
        .any(|c| c["text"] == "event door_opened" && c["verb"] == "wait"));
    // The script said so, through test_log and test_assert
    assert_eq!(d["log"]["greeted"], true);
    let a = &d["assertions"][0];
    assert!(
        a["file"].as_str().unwrap().ends_with("scripts/game.rhai"),
        "{a}"
    );
    assert_eq!(
        (a["passed"].as_u64(), a["failed"].as_u64()),
        (Some(1), Some(0))
    );
    assert_eq!(d["script_errors"].as_array().unwrap().len(), 0);
    assert_eq!(d["warnings"]["counted"], true);
}

/// The issue's third box: two runs, byte-identical reports
#[test]
fn two_runs_write_the_same_bytes() {
    let dir = project("twice", WALKER, WALK);
    let mut reports = Vec::new();
    for n in 0..2 {
        let report = dir.join(format!("run{n}.json"));
        let report_arg = report.to_string_lossy().into_owned();
        let (out, d) = playtest(&dir, &["--exit-after", "4", "--report", &report_arg]);
        assert!(out.status.success(), "{d:#}");
        reports.push(std::fs::read(&report).unwrap());
    }
    assert!(!reports[0].is_empty());
    assert!(
        reports[0] == reports[1],
        "two runs differ:\n{}\n---\n{}",
        String::from_utf8_lossy(&reports[0]),
        String::from_utf8_lossy(&reports[1])
    );
}

// A scene with one scripted entity, its script and an input script, in a
/// folder of its own.
fn project(name: &str, script: &str, input: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("playtest_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scripts")).unwrap();
    std::fs::write(
        dir.join("game.scene.toml"),
        "[scene]\nname = \"Game\"\n\n\
         [entities.crate]\n[entities.crate.transform]\nposition = [0.0, 1.0, 0.0]\n\
         [entities.crate.counter]\nvalue = 0\n\n\
         [entities.director]\n[entities.director.script]\nsource = \"game.rhai\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("scripts/game.rhai"), script).unwrap();
    std::fs::write(dir.join("game.input.txt"), input).unwrap();
    dir
}

fn schemas() -> String {
    repo().join("schemas").to_string_lossy().into_owned()
}

fn playtest(dir: &Path, extra: &[&str]) -> (Output, Value) {
    let schemas = schemas();
    let mut args = vec![
        "--json",
        "playtest",
        "game.scene.toml",
        "--input-script",
        "game.input.txt",
        "--schemas",
        &schemas,
    ];
    args.extend(extra);
    let out = flint(dir, &args);
    let d = document(&out);
    (out, d)
}

fn kinds(d: &Value) -> Vec<String> {
    d["failures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap().to_string())
        .collect()
}

/// The issue's second box: a run-time error exits 1, with the script's file
/// and line in the JSON
#[test]
fn a_script_run_time_error_exits_1_with_file_and_line() {
    let dir = project(
        "run-time-error",
        "fn on_update() {\n    if total_time() > 0.5 {\n        let a = [1, 2];\n        let b = a[5];\n    }\n}\n",
        "",
    );
    let (out, d) = playtest(&dir, &["--exit-after", "1"]);
    assert_eq!(out.status.code(), Some(1), "{d:#}");
    assert_eq!(d["passed"], false);
    let e = &d["script_errors"][0];
    assert!(
        e["file"].as_str().unwrap().ends_with("scripts/game.rhai"),
        "{e}"
    );
    assert_eq!(e["line"], 4, "{e}");
    assert_eq!(e["callback"], "on_update");
    assert_eq!(kinds(&d), ["script_error"]);
    assert_eq!(d["failures"][0]["line"], 4);
}

/// `test_assert` and `test_fail` fail the run at their file and line; the
/// first verdict ends it
#[test]
fn an_assertion_and_test_fail_say_where() {
    let dir = project(
        "assert-and-fail",
        "fn on_update() {\n\
         \x20   test_assert(total_time() < 0.25, \"still early\");\n\
         \x20   if total_time() >= 0.5 { test_fail(\"too late\"); }\n\
         }\n",
        "",
    );
    let (out, d) = playtest(&dir, &["--exit-after", "5"]);
    assert_eq!(out.status.code(), Some(1), "{d:#}");
    assert_eq!(d["ended"], "test_fail");
    assert!(d["game_time"].as_f64().unwrap() < 0.6, "ended at test_fail");
    assert_eq!(kinds(&d), ["assertion", "test_fail"]);
    assert_eq!(d["failures"][0]["line"], 2);
    assert_eq!(d["failures"][1]["line"], 3);
    assert_eq!(d["verdict"]["message"], "too late");
}

/// `test_pass()` ends the run passing, before `--timeout`; a run that
/// doesn't end itself by then fails
#[test]
fn test_pass_ends_the_run_and_a_timeout_fails_it() {
    let dir = project(
        "pass-or-timeout",
        "fn on_update() {\n\
         \x20   let c = get_entity(\"crate\");\n\
         \x20   set_field(c, \"counter\", \"value\", get_field(c, \"counter\", \"value\") + 1);\n\
         \x20   if persist_get(\"go\") == true && total_time() >= 1.0 { test_pass(); }\n\
         \x20   test_log(\"frames\", get_field(c, \"counter\", \"value\"));\n\
         }\n\
         fn on_init() { persist_set(\"go\", true); }\n",
        "0.5 expect crate counter.value > 20\n0.5 expect persist go == true\n0.5 expect log frames >= 30\n",
    );
    let (out, d) = playtest(&dir, &["--timeout", "3", "--dump", "crate"]);
    assert!(out.status.success(), "{d:#}");
    assert_eq!(d["ended"], "test_pass");
    assert_eq!(d["persist"]["go"], true);
    assert!(d["entities"]["crate"]["counter"]["value"].as_i64().unwrap() > 55);
    assert!(d["checks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|c| c["passed"] == true));

    // The same scene with nothing that ends it
    let dir = project("timeout", "fn on_update() { }\n", "");
    let (out, d) = playtest(&dir, &["--timeout", "0.5"]);
    assert_eq!(out.status.code(), Some(1), "{d:#}");
    assert_eq!(d["ended"], "timeout");
    assert_eq!(kinds(&d), ["timeout"]);
}

/// An `expect` that doesn't hold and a `wait` that times out are failures
/// naming the input script's line and what was found
#[test]
fn a_failed_expect_names_its_line_and_what_it_found() {
    let dir = project(
        "expect",
        "fn on_update() { }\n",
        "0.2 expect crate transform.position.y > 5\n0.3 wait entities where counter.value == 9 within 0.4\n",
    );
    let (out, d) = playtest(&dir, &["--exit-after", "1"]);
    assert_eq!(out.status.code(), Some(1), "{d:#}");
    assert_eq!(kinds(&d), ["check", "check"]);
    let first = &d["checks"][0];
    assert_eq!(first["line"], 1);
    assert_eq!(first["found"], "transform.position.y = 1");
    let second = &d["checks"][1];
    assert_eq!(second["why"], "timed out after 0.4 s");
    assert_eq!(second["found"], "0 matches");
}

/// `--strict`: a warning while the scene loads fails the run (the scene's
/// `counter` is a component no schema declares), and so does what `flint
/// check` finds first. A script the scene names that isn't there is a
/// script error, strict or not.
#[test]
fn strict_fails_a_load_warning() {
    let dir = project("strict", "fn on_update() { }\n", "");
    let (out, d) = playtest(&dir, &["--exit-after", "0.2"]);
    assert!(out.status.success(), "{d:#}");
    assert!(d["warnings"]["at_load"].as_u64().unwrap() >= 1, "{d:#}");
    let (out, d) = playtest(&dir, &["--exit-after", "0.2", "--strict"]);
    assert_eq!(out.status.code(), Some(1), "{d:#}");
    let k = kinds(&d);
    assert!(k.contains(&"load_warning".to_string()), "{k:?}");
    assert!(k.contains(&"preflight".to_string()), "{k:?}");
    assert!(d["preflight"]["diagnostics"].is_array(), "{d:#}");

    let scene = std::fs::read_to_string(dir.join("game.scene.toml")).unwrap();
    std::fs::write(
        dir.join("game.scene.toml"),
        scene.replace("source = \"game.rhai\"", "source = \"gone.rhai\""),
    )
    .unwrap();
    let (out, d) = playtest(&dir, &["--exit-after", "0.2"]);
    assert_eq!(out.status.code(), Some(1), "{d:#}");
    assert_eq!(kinds(&d), ["script_error"]);
    assert_eq!(d["script_errors"][0]["callback"], "load");
}

/// The text form: PASS or FAIL, then a line a failure
#[test]
fn the_text_says_fail_and_each_failure() {
    let dir = project(
        "text",
        "fn on_update() { test_assert(false, \"never\"); }\n",
        "",
    );
    let schemas = schemas();
    let out = flint(
        &dir,
        &[
            "playtest",
            "game.scene.toml",
            "--schemas",
            &schemas,
            "--exit-after",
            "0.1",
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("FAIL game.scene.toml"), "{text}");
    assert!(text.contains("assertion: "), "{text}");
    assert!(text.contains("game.rhai:1: never"), "{text}");
}

/// Every file under `dir` with its bytes, to show a run wrote nothing.
fn files_under(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let mut found = std::collections::BTreeMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                todo.push(path);
            } else {
                found.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    found
}

/// Version 1 kept `coins`; version 2 keeps `gold`, ten to a coin.
const MIGRATE: &str = "fn on_persist_migrate(from) {\n\
    \x20   persist_set(\"gold\", persist_get(\"coins\") * 10);\n\
    \x20   persist_remove(\"coins\");\n\
    \x20   test_log(\"migrated_from\", from);\n\
    }\n\
    fn on_init() {\n\
    \x20   test_log(\"version\", persist_version());\n\
    \x20   let visits = persist_get(\"visits\");\n\
    \x20   if visits == () { visits = 0; }\n\
    \x20   persist_set(\"visits\", visits + 1);\n\
    \x20   persist_save();\n\
    }\n";

/// #1193: a returning player's save at an older version. The run loads it
/// as the player would, the scripts migrate it, the report says what the
/// run made of it, and not one file in the project changed, the save
/// least of all.
#[test]
fn a_playtest_starts_from_a_save_migrates_it_and_writes_nothing() {
    let dir = project(
        "save-from",
        MIGRATE,
        "0.1 expect persist gold == 30\n0.1 expect persist visits == 5\n",
    );
    std::fs::write(dir.join("flint.toml"), "[project]\nsave_version = 2\n").unwrap();
    std::fs::create_dir_all(dir.join("saves")).unwrap();
    let save = "coins = 3\nvisits = 4\n\n[meta]\nversion = 1\n";
    std::fs::write(dir.join("saves/returning.toml"), save).unwrap();
    let before = files_under(&dir);

    let (out, d) = playtest(
        &dir,
        &["--save-from", "saves/returning.toml", "--exit-after", "0.5"],
    );
    assert!(out.status.success(), "{d:#}");
    assert_eq!(d["log"]["migrated_from"], 1);
    assert_eq!(d["log"]["version"], 2, "on_init sees the migrated data");
    assert_eq!(d["persist"]["gold"], 30);
    assert_eq!(d["persist"]["visits"], 5);
    assert!(d["persist"].get("coins").is_none(), "{d:#}");
    let s = &d["save"];
    assert_eq!(s["file"], "saves/returning.toml");
    assert_eq!(s["loaded"], "read");
    assert_eq!(s["keys"], 2);
    assert_eq!(s["version"], 1);
    assert_eq!(s["game_version"], 2);
    assert_eq!(s["version_at_end"], 2);
    assert_eq!(s["untouched"], true);
    assert!(s.get("error").is_none());
    assert_eq!(files_under(&dir), before, "the run wrote nothing");

    // The same run again is the same bytes, the save still as it was
    let (_, again) = playtest(
        &dir,
        &["--save-from", "saves/returning.toml", "--exit-after", "0.5"],
    );
    assert_eq!(again, d);
    assert_eq!(files_under(&dir), before);

    // With no save the store begins empty, at the game's version
    let (_, fresh) = playtest(&dir, &["--exit-after", "0.5"]);
    assert_eq!(fresh["persist"]["visits"], 1);
    assert!(fresh["log"].get("migrated_from").is_none());
    assert!(fresh.get("save").is_none(), "{fresh:#}");

    // The text says where the run began
    let schemas = schemas();
    let out = flint(
        &dir,
        &[
            "playtest",
            "game.scene.toml",
            "--schemas",
            &schemas,
            "--save-from",
            "saves/returning.toml",
            "--exit-after",
            "0.2",
        ],
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(
            "from save saves/returning.toml: read 2 keys at version 1 (the game's 2), \
             at 2 by the end; the file is untouched"
        ),
        "{text}"
    );
    assert_eq!(files_under(&dir), before);
}

/// A save that doesn't parse: the store begins empty and scripts read
/// `persist_corrupt()`, as in play, but nothing is renamed `.bak`: the
/// file stays where it is with its bytes, and the run still passes.
#[test]
fn a_corrupt_save_is_reported_to_scripts_and_left_where_it_is() {
    let dir = project(
        "save-corrupt",
        "fn on_init() {\n\
         \x20   let c = persist_corrupt();\n\
         \x20   test_assert(c != (), \"the script hears the save was corrupt\");\n\
         \x20   test_log(\"corrupt_file\", c.file);\n\
         \x20   test_log(\"keys\", persist_keys().len());\n\
         \x20   persist_set(\"fresh\", true);\n\
         \x20   persist_save();\n\
         }\n",
        "",
    );
    std::fs::write(dir.join("broken.toml"), "coins = [1, 2\n").unwrap();
    let before = files_under(&dir);

    let (out, d) = playtest(&dir, &["--save-from", "broken.toml", "--exit-after", "0.3"]);
    assert!(out.status.success(), "{d:#}");
    assert_eq!(d["log"]["corrupt_file"], "broken.toml");
    assert_eq!(d["log"]["keys"], 0);
    assert_eq!(d["persist"]["fresh"], true);
    assert_eq!(d["save"]["loaded"], "corrupt");
    assert_eq!(d["save"]["keys"], 0);
    assert_eq!(d["save"]["untouched"], true);
    assert!(
        d["save"]["error"].as_str().unwrap().contains("parse error"),
        "{d:#}"
    );
    assert_eq!(files_under(&dir), before, "no .bak, no new save");
}

/// A save file that isn't there is a run that didn't start, not an empty
/// store: the run was asked to begin from that file.
#[test]
fn a_missing_save_fails_the_start() {
    let dir = project("save-missing", "fn on_update() { }\n", "");
    let (out, d) = playtest(&dir, &["--save-from", "nope.toml"]);
    assert_eq!(out.status.code(), Some(1), "{d:#}");
    assert_eq!(d["ended"], "start_failed");
    assert_eq!(kinds(&d), ["start"]);
    let message = d["failures"][0]["message"].as_str().unwrap();
    assert!(
        message.contains("--save-from: cannot read nope.toml"),
        "{message}"
    );
    assert!(!dir.join("nope.toml").exists());
}
