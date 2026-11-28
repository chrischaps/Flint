//! #1703, end to end through the binary: a scene named through a subfolder
//! and back (`../relief.scene.toml` from `shots/`, as `flint capture` did
//! for a shot list kept in a subfolder) is the project's scene. The player
//! finds the game's flint.toml, loads its schemas and reads `load_data`
//! paths from the project root, exactly as it does for the plain path.
//!
//! Before the fix the walk up to flint.toml started at `<game>/shots/..`,
//! which has no parent by its spelling, so it stopped at once: `No schemas
//! directories found`, and `load_data('data/tour.toml')` read
//! `<game>/shots/data/tour.toml`. No GPU: `play --list-keys` prepares the
//! scene without a window, and a playtest opens none.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

/// A game outside the engine's checkout (a project under the engine's own
/// `target/` would be the engine's), laid out as flint-demos is: flint.toml,
/// the schemas and the scripts at the root, a demo two folders down with its
/// shot lists in a `shots/` of its own. Its own component, a data file and a
/// director script that needs both. Returns the root.
fn game(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("flint-1703-{}", std::process::id()))
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    for sub in ["demos/relief/shots", "schemas/components", "scripts"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    let engine = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas")
        .canonicalize()
        .unwrap();
    let engine = flint_core::project::shown(&dunce(engine));
    std::fs::write(
        dir.join("flint.toml"),
        format!("[project]\nschemas = [\"{engine}\", \"schemas\"]\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("schemas/components/beacon.toml"),
        "[component.beacon]\ndescription = \"A tour station\"\n\n\
         [component.beacon.fields]\nlit = { type = \"bool\", default = false }\n",
    )
    .unwrap();
    std::fs::write(dir.join("demos/relief/tour.toml"), "stations = 3\n").unwrap();
    std::fs::write(
        dir.join("scripts/director.rhai"),
        "fn on_init() {\n\
         \x20   let tour = load_data(\"demos/relief/tour.toml\");\n\
         \x20   test_assert(type_of(tour) == \"map\", \"the tour loaded\");\n\
         \x20   if type_of(tour) == \"map\" { test_log(\"stations\", tour.stations); }\n\
         }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("demos/relief/relief.scene.toml"),
        "[scene]\nname = \"Relief\"\n\n\
         [entities.station]\n[entities.station.transform]\nposition = [0.0, 0.0, 0.0]\n\
         [entities.station.beacon]\nlit = true\n\n\
         [entities.director]\n[entities.director.script]\nsource = \"director.rhai\"\n",
    )
    .unwrap();
    dir
}

/// `canonicalize` spells a Windows path `\\?\C:\..`; a manifest wants `C:/..`
fn dunce(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p,
    }
}

fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .current_dir(dir)
        .args(args)
        .env_remove("RUST_LOG")
        .output()
        .expect("run flint")
}

fn said(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The ways the issue's table spelled the scene: relative and absolute
/// through `shots/..`, the plain absolute path, and from the game's root.
fn spellings(game: &Path) -> Vec<(PathBuf, String)> {
    let relief = game.join("demos").join("relief");
    let shots = relief.join("shots");
    vec![
        (shots.clone(), "../relief.scene.toml".to_string()),
        (
            shots.clone(),
            shots
                .join("..")
                .join("relief.scene.toml")
                .display()
                .to_string(),
        ),
        (
            shots,
            relief.join("relief.scene.toml").display().to_string(),
        ),
        (relief, "relief.scene.toml".to_string()),
    ]
}

/// The first box: `flint play ../x.scene.toml` from a subfolder finds the
/// project. `--list-keys` runs the player's whole `prepare` (schemas, the
/// scene loaded and settled) and stops before the window.
#[test]
fn play_from_a_subfolder_through_dot_dot_finds_the_project() {
    let game = game("play");
    for (from, scene) in spellings(&game) {
        let out = flint(&from, &["play", &scene, "--list-keys"]);
        let all = said(&out);
        assert!(out.status.success(), "{scene}: {all}");
        assert!(
            !all.contains("No schemas directories found"),
            "{scene} from {}: {all}",
            from.display()
        );
        assert!(!all.contains("unknown-component"), "{scene}: {all}");
    }
}

/// The player's own frames from a subfolder: the game's component is known
/// (no load warning) and `load_data` reads from the project root, which the
/// director asserts.
#[test]
fn a_playtest_through_dot_dot_loads_the_games_schemas_and_data() {
    let game = game("playtest");
    for (from, scene) in spellings(&game) {
        let out = flint(
            &from,
            &["--json", "playtest", &scene, "--exit-after", "0.5"],
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let d: Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("{scene}: one JSON document ({e}): {}", said(&out)));
        assert_eq!(d["passed"], true, "{scene} from {}: {d:#}", from.display());
        assert_eq!(d["log"]["stations"], 3, "{scene}: {d:#}");
        let warnings = d["warnings"].to_string();
        assert!(!warnings.contains("No schemas"), "{scene}: {warnings}");
        assert!(!warnings.contains("load_data"), "{scene}: {warnings}");
    }
}

fn has_gpu(dir: &Path) -> bool {
    let out = flint(dir, &["render", "--list-gpus", "--json"]);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    let any = v["adapters"].as_array().is_some_and(|a| !a.is_empty());
    if !any {
        eprintln!("no GPU adapter: the capture tests are skipped");
    }
    any
}

const SHOTS: &str = "[capture]\nfps = 10\nwidth = 64\nheight = 36\n\n\
    [[shot]]\nid = \"station\"\nscene = \"../relief.scene.toml\"\nseconds = 0.3\n";

fn capture(dir: &Path, extra: &[&str]) -> (Output, Value) {
    let mut args = vec!["--json", "capture", "relief.shots.toml"];
    args.extend(extra);
    let out = flint(dir, &args);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let report: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("one JSON document ({e}): {}", said(&out)));
    (out, report)
}

/// The second box: a shot list in a subfolder whose scene is
/// `../relief.scene.toml` captures the scene with its schemas. The take's
/// log says nothing of missing schemas or a `load_data` miss, and the run
/// names no take as warned. Each take opens a small window.
#[test]
fn a_shot_list_in_a_subfolder_captures_its_scene_with_the_projects_schemas() {
    let game = game("capture");
    if !has_gpu(&game) {
        return;
    }
    let shots = game.join("demos/relief/shots");
    std::fs::write(shots.join("relief.shots.toml"), SHOTS).unwrap();
    let (out, report) = capture(&shots, &[]);
    assert!(out.status.success(), "{report:#}\n{}", said(&out));
    assert_eq!(report["warned"], serde_json::json!([]), "{report:#}");
    let take = &report["takes"][0];
    assert_eq!(take["ok"], true, "{take:#}");
    assert_eq!(take["warnings"], serde_json::json!([]), "{take:#}");
    let scene_handed = take["command"][1].as_str().unwrap();
    assert!(!scene_handed.contains(".."), "{scene_handed}");
    // The take's folder is spelled from where the capture ran
    let folder = shots.join(take["folder"].as_str().unwrap());
    let log = std::fs::read_to_string(folder.join("player.log")).unwrap();
    assert!(!log.contains("No schemas directories found"), "{log}");
    assert!(!log.contains("load_data"), "{log}");
}

/// The third line of the scope: a take whose player found no schemas made
/// all its frames, but it is the scene played outside any project, and the
/// run names it. `--strict` fails it.
#[test]
fn a_take_that_lost_its_project_is_named_and_strict_fails_it() {
    let lost = std::env::temp_dir()
        .join(format!("flint-1703-{}", std::process::id()))
        .join("lost");
    let _ = std::fs::remove_dir_all(&lost);
    let shots = lost.join("shots");
    std::fs::create_dir_all(&shots).unwrap();
    if !has_gpu(&lost) {
        return;
    }
    std::fs::write(
        lost.join("relief.scene.toml"),
        "[scene]\nname = \"Lost\"\n\n[entities.post]\n[entities.post.transform]\n\
         position = [0.0, 0.0, 0.0]\n",
    )
    .unwrap();
    std::fs::write(shots.join("relief.shots.toml"), SHOTS).unwrap();

    let (out, report) = capture(&shots, &[]);
    assert!(out.status.success(), "{report:#}\n{}", said(&out));
    let warned = report["warned"].as_array().unwrap();
    assert_eq!(warned.len(), 1, "{report:#}");
    assert!(
        warned[0]
            .as_str()
            .unwrap()
            .contains("station/main: the player found no schemas"),
        "{report:#}"
    );
    assert!(
        report["summary"]
            .as_str()
            .unwrap()
            .contains("1 of them warned"),
        "{report:#}"
    );

    let (out, report) = capture(&shots, &["--strict"]);
    assert_eq!(out.status.code(), Some(1), "{report:#}\n{}", said(&out));
    assert_eq!(report["failed"], true);
    assert_eq!(report["takes"][0]["ok"], false, "{report:#}");
}
