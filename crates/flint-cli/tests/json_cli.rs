//! #710's done-when, with the real binary.
//!
//! - Every non-interactive command takes the global `--json` and prints
//!   exactly one JSON document on stdout: each is run here in a project
//!   `flint init` made, and its whole stdout must parse as one value.
//! - A failure is exit 1 with a document still on stdout: the command's own
//!   report when it has one (`flint check` of a broken scene), else
//!   `{"error": {"message", "causes"}}`; a mistake on the command line is an
//!   error document too, with clap's exit code.
//! - An unknown schema name exits non-zero, with and without `--json`.
//! - A command that opens a window refuses `--json` rather than opening one.
//! - Every command `flint --help` lists, at every depth, is run here or is
//!   named as a window command, so a new command fails this test until it is
//!   one or the other.
//!
//! The renders (`render`, `goldens`, `edit <model> --render`) need a GPU and
//! are skipped, said on stderr, on a machine with none.
//!
//! It runs every command, so it needs every package (#1333): a build
//! without one answers that package's commands with exit 2 instead.
#![cfg(feature = "packages")]

use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

/// This checkout: the specs, examples and book the cases borrow.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The commands that open a window, play sound or read a pad: `--json`
/// refuses them. `edit` is one except for a `--render` still (any kind, #712).
const WINDOWS: &[&str] = &[
    "play",
    "edit",
    "music play",
    "music calibrate",
    "music chart play",
    "prefab view",
    "frame open",
];

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("json_cli_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .args(args)
        .current_dir(dir)
        // Keep the AI steps on the free, offline provider whatever the
        // machine has set
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .expect("flint runs")
}

/// `args` with `--json`: its stdout parsed, which must be one document and
/// nothing else.
fn json(dir: &Path, args: &[&str]) -> (Output, Value) {
    let mut words: Vec<&str> = args.to_vec();
    words.push("--json");
    let out = flint(dir, &words);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let value: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "`flint {}`: stdout is not one JSON document ({e}); exit {:?}\n--- stdout\n{stdout}\n--- stderr\n{}",
            words.join(" "),
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out, value)
}

/// The command must succeed.
fn ok(dir: &Path, args: &[&str]) -> Value {
    let (out, value) = json(dir, args);
    assert!(
        out.status.success(),
        "`flint {} --json` failed ({:?}):\n{value:#}\n{}",
        args.join(" "),
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    value
}

/// The command must fail with exit 1.
fn fails(dir: &Path, args: &[&str]) -> Value {
    let (out, value) = json(dir, args);
    assert_eq!(
        out.status.code(),
        Some(1),
        "`flint {} --json` should exit 1:\n{value:#}",
        args.join(" ")
    );
    value
}

/// Whether a GPU adapter is there to render on.
fn has_gpu(dir: &Path) -> bool {
    let value = ok(dir, &["render", "--list-gpus"]);
    let any = value["adapters"].as_array().is_some_and(|a| !a.is_empty());
    if !any {
        eprintln!("no GPU adapter: the render cases are skipped");
    }
    any
}

/// The command names `flint <path> --help` lists under `Commands:`.
fn subcommands(path: &[&str]) -> Vec<String> {
    let mut words = path.to_vec();
    words.push("--help");
    let out = Command::new(FLINT).args(&words).output().unwrap();
    let help = String::from_utf8_lossy(&out.stdout).into_owned();
    let Some((_, rest)) = help.split_once("Commands:") else {
        return Vec::new();
    };
    rest.lines()
        .skip(1)
        .take_while(|l| !l.trim().is_empty())
        .filter(|l| l.starts_with("  ") && !l.starts_with("   "))
        .filter_map(|l| l.split_whitespace().next())
        .filter(|name| *name != "help")
        .map(str::to_string)
        .collect()
}

/// Every command `--help` lists, as its words: `scene list`, `asset job status`.
fn every_command() -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut todo: Vec<Vec<String>> = subcommands(&[]).into_iter().map(|c| vec![c]).collect();
    while let Some(path) = todo.pop() {
        let words: Vec<&str> = path.iter().map(String::as_str).collect();
        let under = subcommands(&words);
        if under.is_empty() {
            found.insert(path.join(" "));
        }
        for name in under {
            let mut next = path.clone();
            next.push(name);
            todo.push(next);
        }
    }
    found
}

const TINY_SUITE: &str = r#"
schema_version = 0

[suite]
id = "one-bus"
title = "One bus"

[audio]
sample_rate = 8000

[[tempo]]
sample = 0
bpm = 120.0
time_signature = [4, 4]

[[sections]]
name = "a"
start_sample = 0

[buses.pad]
file = "stems/pad.wav"
"#;

const TINY_CHART: &str = "schema_version = 0\nsuite = \"tempo-change-test\"\n\
    [[curves]]\nchannel = \"lean\"\nbeat = 0.0\nvalue = [0.0, 0.0]\ninterp = \"smooth\"\n\
    [[pulses]]\nbeat = 4.0\n[[pulses]]\nbeat = 6.0\n";

#[test]
fn every_command_answers_with_one_json_document() {
    let top = scratch("every");
    let repo = std::path::absolute(repo()).unwrap();
    let r = |p: &str| repo.join(p).display().to_string();
    let mut ran: BTreeSet<&str> = BTreeSet::new();
    let mut run = |name: &'static str| ran.insert(name);

    // A game project, the starting point an agent has
    run("init");
    let init = ok(&top, &["init", "game"]);
    assert_eq!(init["name"], "game");
    assert!(init["next_steps"]
        .as_array()
        .unwrap()
        .contains(&"flint check".into()));
    let dir = top.join("game");
    let scene = "scenes/main.scene.toml";

    run("check");
    let check = ok(&dir, &["check"]);
    assert_eq!(check["valid"], true);
    run("validate");
    assert_eq!(ok(&dir, &["validate", scene])["valid"], true);
    // A second of the scaffold played headless (#714)
    run("playtest");
    let played = ok(&dir, &["playtest", scene, "--exit-after", "1"]);
    assert_eq!(played["passed"], true, "{played:#}");
    run("query");
    assert!(ok(&dir, &["query", "entities", "--scene", scene]).is_array());

    run("entity list");
    assert!(ok(&dir, &["entity", "list", "--scene", scene]).is_array());
    run("entity create");
    let made = ok(
        &dir,
        &[
            "entity",
            "create",
            "--archetype",
            "character",
            "--name",
            "crate",
            "--scene",
            scene,
        ],
    );
    assert_eq!(made["created"], "crate");
    run("entity show");
    assert_eq!(
        ok(&dir, &["entity", "show", "crate", "--scene", scene])["name"],
        "crate"
    );
    run("entity delete");
    assert_eq!(
        ok(&dir, &["entity", "delete", "crate", "--scene", scene])["deleted"],
        serde_json::json!(["crate"])
    );

    run("scene create");
    ok(&dir, &["scene", "create", "scenes/spare"]);
    run("scene list");
    assert!(
        ok(&dir, &["scene", "list", "scenes"])["scenes"]
            .as_array()
            .unwrap()
            .len()
            >= 2
    );
    run("scene info");
    assert!(ok(&dir, &["scene", "info", scene])["entities"].is_array());

    run("schema");
    assert_eq!(ok(&dir, &["schema", "transform"])["name"], "transform");
    assert!(ok(&dir, &["schema", "--list"])["components"].is_array());

    run("script api");
    assert!(ok(&dir, &["script", "api", "--filter", "draw_text"])["functions"].is_array());
    let book = r("docs/book");
    let checked = json(&dir, &["script", "api", "--check-book", &book]).1;
    assert!(checked.get("valid").is_some(), "{checked:#}");
    run("script check");
    assert_eq!(ok(&dir, &["script", "check"])["valid"], true);

    run("examples");
    std::fs::write(
        dir.join("index.toml"),
        "[[example]]\nscene = \"scenes/main.scene.toml\"\ntopic = \"start\"\npurpose = \"the scaffold\"\n",
    )
    .unwrap();
    assert!(ok(&dir, &["examples"]).is_array());

    // Exit 1 when a FLINT_* switch here holds a bad value, so only the
    // document is held (#715)
    run("env");
    assert!(json(&dir, &["env"]).1["switches"].is_array());

    // The engine's packages (#1331): this project names the engine by path,
    // so the list is the manifests of the tree flint was built from. The
    // shape is held by `package_list_has_the_shape_kept_in_the_repo`
    run("package list");
    assert_eq!(ok(&dir, &["package", "list"])["valid"], true);
    // What the scaffold uses (#1334): no feature package
    run("package used");
    assert!(ok(&dir, &["package", "used"])["lines"].is_array());
    // What `flint build` would run (#1335): a dry run builds nothing
    run("build");
    let build = ok(&dir, &["build", "--dry-run"]);
    assert_eq!(build["dry_run"], true);
    assert!(build["commands"][0]
        .as_str()
        .unwrap()
        .starts_with("cargo build"));

    // `play` is a window, but `--list-keys` only prints its debug keys,
    // under flint's global --json (#1184: the player's own took it)
    let keys = ok(&dir, &["play", scene, "--list-keys"]);
    assert!(keys["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k["action"] == "keys"));

    // Its takes open player windows, so the case is the plan (#722);
    // tests/capture_cli.rs captures for real
    run("capture");
    std::fs::write(
        dir.join("trailer.shots.toml"),
        format!("[[shot]]\nid = \"open\"\nscene = \"{scene}\"\nseconds = 1\n"),
    )
    .unwrap();
    let planned = ok(&dir, &["capture", "trailer.shots.toml", "--dry-run"]);
    assert_eq!(planned["takes"][0]["frames"], 31, "{planned:#}");

    run("bundle");
    let bundle = ok(&dir, &["bundle", ".", "--dry-run"]);
    assert!(bundle["files"].as_array().is_some_and(|f| !f.is_empty()));

    run("export web");
    let player = top.join("player");
    std::fs::create_dir_all(&player).unwrap();
    for file in ["index.html", "flint_player.js", "flint_player_bg.wasm"] {
        std::fs::write(player.join(file), "x").unwrap();
    }
    let player = player.display().to_string();
    let exported = ok(
        &dir,
        &[
            "export", "web", ".", "--out", "dist/web", "--player", &player,
        ],
    );
    assert_eq!(exported["game"], "game");

    run("dxc status");
    ok(&dir, &["dxc", "status"]);
    run("dxc notices");
    let _ = json(&dir, &["dxc", "notices"]); // a build without DXC says so as an error

    // flint frame: the frozen captures (#1087), read anywhere
    let two = r("tests/fixtures/frames/two_crates.flintframe");
    let one = r("tests/fixtures/frames/one_crate.flintframe");
    run("frame show");
    assert_eq!(
        ok(&dir, &["frame", "show", &two])["summary"]["draws"]["draw_calls"],
        3
    );
    assert!(ok(
        &dir,
        &[
            "frame",
            "show",
            &two,
            "--passes",
            "--draws",
            "--targets",
            "--hitch"
        ]
    )["passes"]
        .is_array());
    run("frame diff");
    assert!(ok(&dir, &["frame", "diff", &two, &one])["draws"]["vanished"].is_array());
    run("frame image");
    assert!(ok(
        &dir,
        &["frame", "image", &two, "Scene", "-o", "out/scene.png"]
    )["written"]
        .is_array());
    // The frozen captures hold no replay/: refused, as one document
    run("frame replay");
    assert!(fails(&dir, &["frame", "replay", &two])["error"]["message"]
        .as_str()
        .unwrap()
        .contains("no replay inputs"));

    // flint profile: the hand-made traces (#1098), read anywhere
    let stall = r("tests/fixtures/traces/stall.trace.json");
    let calm = r("tests/fixtures/traces/calm.trace.json");
    run("profile top");
    assert_eq!(ok(&dir, &["profile", "top", &stall])["range"]["frames"], 16);
    run("profile frame");
    assert_eq!(
        ok(&dir, &["profile", "frame", &stall, "hitch"])["frame"],
        113
    );
    run("profile explain");
    assert_eq!(
        ok(&dir, &["profile", "explain", &stall])["culprit"]["scope"],
        "stall.rhai:on_update"
    );
    run("profile diff");
    assert!(ok(&dir, &["profile", "diff", &calm, &stall])["scopes"].is_array());

    run("style new");
    assert_eq!(
        ok(&dir, &["style", "new", "glow", "--dir", "styles"])["name"],
        "glow"
    );
    run("style check");
    assert_eq!(ok(&dir, &["style", "check", "styles"])["valid"], true);
    run("style prelude");
    assert!(ok(&dir, &["style", "prelude", "styles/glow.shader.toml"])["wgsl"].is_string());
    run("style list");
    assert!(ok(&dir, &["style", "list"]).is_array());

    // flint gen: a texture, a model checked as it's written, a plan
    run("gen");
    let wall = ok(
        &dir,
        &[
            "gen",
            &r("specs/stone_wall.procgen.toml"),
            "-o",
            "out/wall.png",
        ],
    );
    assert_eq!(wall["generated"], 1);
    let beetle = ok(
        &dir,
        &[
            "gen",
            &r("specs/beetle.procgen.toml"),
            "-o",
            "out/beetle.glb",
            "--validate",
        ],
    );
    assert!(beetle["variants"][0]["validation"]["checks"].is_array());
    assert_eq!(
        ok(
            &dir,
            &["gen", &r("specs/stone_wall.procgen.toml"), "--dry-run"]
        )["dry_run"],
        true
    );
    ok(
        &dir,
        &[
            "gen",
            &r("specs/examples/oak_living.procgen.toml"),
            "--strip",
            "--dry-run",
        ],
    );
    ok(
        &dir,
        &[
            "gen",
            &r("tests/fixtures/scenes/fonts/halftone.glyphs.toml"),
            "--dry-run",
        ],
    );
    let terrain = ok(
        &dir,
        &[
            "gen",
            &r("tests/fixtures/scenes/terrain/rolling_meadow.terrain.toml"),
            "--dry-run",
        ],
    );
    assert_eq!(terrain["generator"], "terrain_v1", "{terrain:#}");
    ok(
        &dir,
        &[
            "gen",
            "--ai-create",
            "a stump",
            "--ai-provider",
            "mock",
            "--dry-run",
        ],
    );
    ok(&dir, &["gen", "--cost-report"]);

    // The asset catalog, empty to start with
    run("asset list");
    assert!(ok(&dir, &["asset", "list"]).is_array());
    run("asset import");
    let wall_png = std::fs::read_dir(dir.join("out"))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|e| e == "png"))
        .expect("gen wrote a PNG");
    let wall_png = wall_png.display().to_string();
    assert_eq!(
        ok(&dir, &["asset", "import", &wall_png, "--name", "wall"])["name"],
        "wall"
    );
    run("asset info");
    assert_eq!(ok(&dir, &["asset", "info", "wall"])["name"], "wall");
    run("asset generate");
    let generated = ok(
        &dir,
        &[
            "asset",
            "generate",
            "texture",
            "-d",
            "red brick",
            "--name",
            "brick",
            "--provider",
            "mock",
            "--width",
            "32",
            "--height",
            "32",
        ],
    );
    assert_eq!(generated["name"], "brick");
    run("asset regenerate");
    ok(
        &dir,
        &["asset", "regenerate", "brick", "--provider", "mock"],
    );
    run("asset validate");
    assert!(ok(&dir, &["asset", "validate", "out/beetle.glb"])["checks"].is_array());
    run("asset manifest");
    ok(
        &dir,
        &["asset", "manifest", "--output", "build/manifest.toml"],
    );
    run("asset resolve");
    assert_eq!(ok(&dir, &["asset", "resolve", scene])["valid"], true);
    run("asset where");
    assert!(ok(
        &dir,
        &["asset", "where", "scripts", "hud.rhai", "--scene", scene]
    )["found"]
        .is_string());
    // Nothing found is the answer, and a failure
    assert!(fails(&dir, &["asset", "where", "models", "nothing"])["found"].is_null());
    run("asset job list");
    assert!(ok(&dir, &["asset", "job", "list"])["jobs"].is_array());
    run("asset job status");
    assert!(fails(&dir, &["asset", "job", "status", "no-such-job"])["error"].is_object());
    run("asset job resume");
    assert!(fails(&dir, &["asset", "job", "resume", "no-such-job"])["error"].is_object());
    run("asset costs");
    assert!(ok(&dir, &["asset", "costs"])["total"]["calls"].is_u64());

    // Music: a one-bus suite, mixed offline; a chart replayed
    std::fs::create_dir_all(dir.join("stems")).unwrap();
    std::fs::write(dir.join("one.suite.toml"), TINY_SUITE).unwrap();
    let tone: Vec<f32> = (0..16_000)
        .flat_map(|i| {
            let v = (std::f32::consts::TAU * 220.0 * i as f32 / 8_000.0).sin() * 0.3;
            [v, v]
        })
        .collect();
    flint_music::write_wav(&dir.join("stems/pad.wav"), &tone, 8_000).unwrap();
    run("music validate");
    assert_eq!(
        ok(&dir, &["music", "validate", "one.suite.toml"])["valid"],
        true
    );
    run("music render");
    let mixed = ok(
        &dir,
        &[
            "music",
            "render",
            "one.suite.toml",
            "--output",
            "mix.wav",
            "--duration-bars",
            "1",
        ],
    );
    assert_eq!(mixed["written"], serde_json::json!(["mix.wav"]));
    run("music chart replay");
    std::fs::copy(
        repo.join("crates/flint-rhythm/tests/data/tempo_change.suite.toml"),
        dir.join("tempo.suite.toml"),
    )
    .unwrap();
    std::fs::write(dir.join("tiny.chart.toml"), TINY_CHART).unwrap();
    let replayed = ok(
        &dir,
        &[
            "music",
            "chart",
            "replay",
            "tempo.suite.toml",
            "--chart",
            "tiny.chart.toml",
            "--synthetic",
            "perfect",
            "--out",
            "judged.jsonl",
        ],
    );
    assert_eq!(replayed["hits"], 2);

    // Listings that need no GPU
    run("render");
    assert!(ok(&dir, &["render", "--list-styles"]).is_array());
    assert!(ok(&dir, &["render", "--list-effects"]).is_array());
    assert!(ok(&dir, &["render", "--look", "help"])["help"].is_string());

    run("goldens");
    if has_gpu(&dir) {
        // --json is --report json: the render's report is the document
        let report = ok(
            &dir,
            &[
                "render",
                scene,
                "--width",
                "64",
                "--height",
                "36",
                "-o",
                "renders/a.png",
            ],
        );
        assert_eq!(report["width"], 64);
        let variants = ok(
            &dir,
            &[
                "render",
                scene,
                "--width",
                "64",
                "--height",
                "36",
                "-o",
                "renders/v.png",
                "--variant",
                "plain: --no-grid",
            ],
        );
        assert!(variants["written"].is_array());

        std::fs::write(
            dir.join("tiny.goldens.toml"),
            "[suite]\nname = \"tiny\"\nroot = \".\"\nwidth = 64\nheight = 36\n\n\
             [[case]]\nname = \"main\"\nscene = \"scenes/main.scene.toml\"\n",
        )
        .unwrap();
        let goldens = ok(
            &dir,
            &[
                "goldens",
                "tiny.goldens.toml",
                "--bless",
                "--out-dir",
                "out/goldens",
            ],
        );
        assert_eq!(goldens["cases"].as_array().map(Vec::len), Some(1));

        let snap = ok(
            &dir,
            &[
                "edit",
                &r("tests/fixtures/scenes/models/Box.glb"),
                "--render",
                "renders/box.png",
                "--width",
                "64",
                "--height",
                "36",
            ],
        );
        assert_eq!(snap["written"], serde_json::json!(["renders/box.png"]));
    }

    // Every command --help lists is run above or is a window (46 of them
    // after #709 moved the music commands under `flint music`: the help
    // parse must keep finding them)
    let every = every_command();
    assert!(every.len() >= 40, "found only {every:?} in --help");
    let missing: Vec<String> = every
        .into_iter()
        .filter(|c| !ran.contains(c.as_str()) && !WINDOWS.contains(&c.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "commands with no --json case here and not listed as windows: {missing:?}"
    );
}

/// The JSON type names the shape file uses.
fn type_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// `object`'s keys, each to its type, and the shape's keys must be the
/// same; a shape type `a|b` takes either.
fn holds_shape(what: &str, object: &Value, shape: &Value) {
    let object = object.as_object().unwrap();
    let shape = shape.as_object().unwrap();
    let keys: BTreeSet<&String> = object.keys().collect();
    let wanted: BTreeSet<&String> = shape.keys().collect();
    assert_eq!(keys, wanted, "{what}: its keys");
    for (key, ty) in shape {
        let found = type_of(&object[key]);
        assert!(
            ty.as_str().unwrap().split('|').any(|t| t == found),
            "{what}: `{key}` is {found}, the shape says {ty}"
        );
    }
}

/// `flint package list --json` lists the seven packages of ADR 0136
/// decision 8 in the shape `tests/package_list.shape.json` keeps (#1331),
/// from the engine checkout itself and with `--dir`.
#[test]
fn package_list_has_the_shape_kept_in_the_repo() {
    let shape: Value = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/package_list.shape.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let repo = std::path::absolute(repo()).unwrap();
    let dir = repo.join("packages").display().to_string();
    for (from, args) in [
        (repo.clone(), vec!["package", "list"]),
        (scratch("packages"), vec!["package", "list", "--dir", &dir]),
    ] {
        let doc = ok(&from, &args);
        holds_shape("the document", &doc, &shape["document"]);
        assert_eq!(doc["valid"], true);
        assert_eq!(doc["problems"], serde_json::json!([]));
        let packages = doc["packages"].as_array().unwrap();
        let listed: Vec<Value> = packages
            .iter()
            .map(|p| serde_json::json!([p["name"], p["tier"]]))
            .collect();
        assert_eq!(Value::Array(listed), shape["packages"]);
        for p in packages {
            holds_shape(&format!("package {}", p["name"]), p, &shape["package"]);
            assert!(p["file"].as_str().unwrap().ends_with("/package.toml"));
        }
    }

    // A manifest with a mistake: listed among the problems by file and
    // key, the rest still listed, exit 1
    let broken = scratch("packages_broken");
    for name in ["engine", "living"] {
        let from = repo.join("packages").join(name).join("package.toml");
        std::fs::create_dir_all(broken.join(name)).unwrap();
        let mut text = std::fs::read_to_string(from).unwrap();
        if name == "living" {
            text = text.replace("tier = \"feature\"", "tier = \"feture\"");
        }
        std::fs::write(broken.join(name).join("package.toml"), text).unwrap();
    }
    let doc = fails(&broken, &["package", "list", "--dir", "."]);
    assert_eq!(doc["valid"], false);
    assert_eq!(doc["packages"].as_array().unwrap().len(), 1);
    let problem = &doc["problems"][0];
    assert_eq!(problem["code"], "bad-package");
    assert_eq!(problem["field"], "package.tier");
    assert_eq!(problem["file"], "living/package.toml");
    assert_eq!(problem["hint"], "did you mean `feature`?");
}

#[test]
fn an_unknown_schema_name_exits_non_zero() {
    let dir = scratch("schema");
    let out = flint(&repo(), &["schema", "no_such_component"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no_such_component"));
    let error = fails(&repo(), &["schema", "no_such_component"]);
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("no_such_component"));
    drop(dir);
}

#[test]
fn a_window_command_refuses_json() {
    let dir = scratch("windows");
    for args in [
        &["play"][..],
        &["edit", "scenes/none.scene.toml"],
        &["prefab", "view", "x.prefab.toml"],
    ] {
        let error = fails(&dir, args);
        let message = error["error"]["message"].as_str().unwrap();
        assert!(message.contains("opens a window"), "{message}");
    }
}

/// #1199's table, row by row, with `--json` where a user puts it: before
/// `play` it is flint's global flag, after it clap used to hand it to the
/// player's own (#717), which asked for `--list-keys` (exit 2) or was
/// refused as a window (#1184 made the global the only one). `--list-keys`
/// opens no window, so it answers; a plain play refuses either way.
#[test]
fn play_answers_json_only_for_its_key_table_wherever_json_goes() {
    let top = scratch("play_json");
    assert!(flint(&top, &["init", "game"]).status.success());
    let dir = top.join("game");
    let scene = "scenes/main.scene.toml";
    let stdout = |out: &Output| String::from_utf8_lossy(&out.stdout).into_owned();

    // Rows 1 and 2: the table, the same bytes whichever side --json is on
    let after = flint(&dir, &["play", "--list-keys", "--json", scene]);
    let before = flint(&dir, &["--json", "play", "--list-keys", scene]);
    for (words, out) in [
        ("play --list-keys --json", &after),
        ("--json play --list-keys", &before),
    ] {
        assert!(
            out.status.success(),
            "`flint {words}` exit {:?}:\n{}{}",
            out.status.code(),
            stdout(out),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let table: Value = serde_json::from_str(&stdout(&after)).expect("one JSON document");
    assert!(table["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k["action"] == "keys"));
    assert_eq!(stdout(&after), stdout(&before));

    // Rows 3 and 4: a plain play is a window, refused with exit 1 and an
    // error document, never clap's "--list-keys is required" (exit 2).
    // --exit-after only bounds the test if the refusal ever stops holding.
    for words in [
        &["play", "--json", scene, "--exit-after", "1"][..],
        &["--json", "play", scene, "--exit-after", "1"],
    ] {
        let out = flint(&dir, words);
        assert_eq!(out.status.code(), Some(1), "`flint {}`", words.join(" "));
        let error: Value = serde_json::from_str(&stdout(&out)).expect("one JSON document");
        let message = error["error"]["message"].as_str().unwrap();
        assert!(message.contains("opens a window"), "{message}");
    }

    // Row 5: flint-player's own --json prints the same table. Its exe sits
    // beside flint's when the workspace was built (plain `cargo test`); a
    // `-p flint-cli` run may not have it, or may have an old one
    let player =
        Path::new(FLINT).with_file_name(format!("flint-player{}", std::env::consts::EXE_SUFFIX));
    if !player.exists() {
        eprintln!(
            "no {}: flint-player's table is not compared",
            player.display()
        );
        return;
    }
    let out = Command::new(&player)
        .args([scene, "--list-keys", "--json"])
        .current_dir(&dir)
        .output()
        .expect("flint-player runs");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        stdout(&out),
        stdout(&after),
        "flint play's table is not flint-player's (an old {}? cargo build -p flint-player)",
        player.display()
    );
}

#[test]
fn a_command_line_mistake_is_one_error_document() {
    let dir = scratch("usage");
    let (out, value) = json(&dir, &["no-such-command"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(value["error"]["usage"]
        .as_str()
        .unwrap()
        .contains("no-such-command"));
    // --help still prints help
    let help = flint(&dir, &["check", "--help", "--json"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("Usage"));
}

#[test]
fn a_failing_check_prints_its_report_and_no_second_document() {
    let dir = scratch("failing");
    std::fs::create_dir_all(dir.join("schemas/components")).unwrap();
    std::fs::write(
        dir.join("schemas/components/place.toml"),
        "[component.place]\n\n[component.place.fields]\nposition = { type = \"vec3\", default = [0, 0, 0] }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("broken.scene.toml"),
        "[scene]\nname = \"Broken\"\n\n[entities.a]\narchetype = \"no_such_archetype\"\n",
    )
    .unwrap();
    let report = fails(&dir, &["check", "broken.scene.toml"]);
    assert_eq!(report["valid"], false);
    assert!(report.get("error").is_none(), "{report:#}");
    let report = fails(&dir, &["validate", "broken.scene.toml"]);
    assert_eq!(report["valid"], false);
}
