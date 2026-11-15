//! `flint asset where` (ADR 0114 step 3, #706): the real binary in a small
//! game whose scene sits two folders below `scenes/`. It prints the file
//! the engine would load and the paths tried before it, as text and as
//! JSON; with nothing to find it lists every path tried and exits non-zero.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const EFFECT: &str = "[[emitters]]\nname = \"a\"\n";

/// A game: the project's effects at the root, an override beside one scene,
/// and an engine subtree with an effect of its own.
const FILES: &[(&str, &str)] = &[
    ("flint.toml", "[project]\nid = \"embers\"\n"),
    ("particles/fire.particles.toml", EFFECT),
    ("particles/rain.particles.toml", EFFECT),
    ("scenes/act1/particles/rain.particles.toml", EFFECT),
    ("engine/particles/dust.particles.toml", EFFECT),
    (
        "scenes/act1/cave/deep.scene.toml",
        "[scene]\nname = \"Deep\"\nversion = \"1.0\"\n",
    ),
];

const SCENE: &str = "scenes/act1/cave/deep.scene.toml";

fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("asset_where_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in FILES {
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

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A path as the tests spell it, whatever separators the platform printed.
fn slashes(text: &str) -> String {
    text.replace('\\', "/")
}

fn json(o: &Output) -> serde_json::Value {
    let text = stdout(o);
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

/// A JSON list of paths, with forward slashes.
fn paths(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .expect("a list")
        .iter()
        .map(|p| slashes(p.as_str().expect("a path")))
        .collect()
}

#[test]
fn a_nested_scene_finds_the_projects_effect_and_says_where_it_looked() {
    let dir = project("found");
    let o = flint(
        &dir,
        &["asset", "where", "particles", "fire", "--scene", SCENE],
    );
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let text = slashes(&stdout(&o));
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    assert_eq!(lines[0], "particle effect 'fire'");
    assert!(lines[1].starts_with("project  "), "{text}");
    assert!(lines[1].ends_with("(flint.toml)"), "{text}");
    assert_eq!(lines[2], "scene    scenes/act1/cave");
    assert_eq!(lines[3], "loads    particles/fire.particles.toml");
    assert_eq!(lines[4], "tried before it:");
    assert_eq!(
        lines[5..],
        [
            "scenes/act1/cave/particles/fire.particles.toml",
            "scenes/act1/particles/fire.particles.toml",
            "scenes/particles/fire.particles.toml",
        ]
    );
}

#[test]
fn the_nearer_file_wins_and_the_engine_is_last() {
    let dir = project("order");
    // `rain` is at the root and beside act1: act1's is nearer the scene.
    let o = flint(
        &dir,
        &[
            "asset",
            "where",
            "particles",
            "rain",
            "--scene",
            SCENE,
            "--json",
        ],
    );
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let doc = json(&o);
    assert_eq!(doc["kind"], "particles");
    assert_eq!(doc["name"], "rain");
    assert_eq!(
        slashes(doc["found"].as_str().unwrap()),
        "scenes/act1/particles/rain.particles.toml"
    );
    assert_eq!(
        paths(&doc["tried"]),
        ["scenes/act1/cave/particles/rain.particles.toml"]
    );
    assert_eq!(doc["project"]["found"], "flint.toml");
    assert_eq!(doc["order_change"], serde_json::Value::Null);

    // `dust` is only in the engine subtree: found there, after everything.
    let o = flint(
        &dir,
        &[
            "asset",
            "where",
            "particles",
            "dust",
            "--scene",
            SCENE,
            "--json",
        ],
    );
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let doc = json(&o);
    assert_eq!(
        slashes(doc["found"].as_str().unwrap()),
        "engine/particles/dust.particles.toml"
    );
    assert_eq!(
        paths(&doc["tried"]),
        [
            "scenes/act1/cave/particles/dust.particles.toml",
            "scenes/act1/particles/dust.particles.toml",
            "scenes/particles/dust.particles.toml",
            "particles/dust.particles.toml",
        ]
    );
}

#[test]
fn nothing_found_lists_every_path_and_fails() {
    let dir = project("missing");
    let tried = [
        "scenes/act1/cave/particles/ice.particles.toml",
        "scenes/act1/particles/ice.particles.toml",
        "scenes/particles/ice.particles.toml",
        "particles/ice.particles.toml",
        "engine/particles/ice.particles.toml",
    ];

    let o = flint(
        &dir,
        &["asset", "where", "particles", "ice", "--scene", SCENE],
    );
    assert_eq!(o.status.code(), Some(1), "{}{}", stdout(&o), stderr(&o));
    let text = slashes(&stdout(&o));
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    assert_eq!(lines[3], "loads    nothing: no such file. Tried:");
    assert_eq!(lines[4..], tried);
    assert!(
        stderr(&o).contains("particle effect 'ice' not found (5 places tried)"),
        "{}",
        stderr(&o)
    );

    // As JSON: still one document on stdout, and still a failure.
    let o = flint(
        &dir,
        &[
            "asset",
            "where",
            "particles",
            "ice",
            "--scene",
            SCENE,
            "--json",
        ],
    );
    assert_eq!(o.status.code(), Some(1), "{}{}", stdout(&o), stderr(&o));
    let doc = json(&o);
    assert_eq!(doc["found"], serde_json::Value::Null);
    assert_eq!(paths(&doc["tried"]), tried);
}

#[test]
fn without_a_scene_the_working_directory_is_the_scenes() {
    let dir = project("cwd");
    let o = flint(
        &dir.join("scenes/act1"),
        &["asset", "where", "particles", "fire", "--json"],
    );
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let doc = json(&o);
    assert_eq!(doc["scene_dir"], ".");
    let found = slashes(doc["found"].as_str().unwrap());
    assert!(found.ends_with("/particles/fire.particles.toml"), "{found}");
    assert!(!found.contains("scenes"), "the root's, not act1's: {found}");
    assert_eq!(
        paths(&doc["tried"])[0],
        "particles/fire.particles.toml",
        "beside the working directory first"
    );
}

#[test]
fn the_old_rules_pick_outside_the_project_is_noted() {
    // The scene sits at the project root, and the old rule's "one level up"
    // lies outside the project: the effect is no longer found, and the
    // answer says which file the old rule would have loaded.
    let dir = project("outside");
    std::fs::write(dir.join("scenes/act1/cave/flint.toml"), "").unwrap();
    let act1 = dir.join("scenes/act1");
    let args = [
        "asset",
        "where",
        "particles",
        "rain",
        "--scene",
        "cave/deep.scene.toml",
    ];
    let o = flint(&act1, &[&args[..], &["--json"]].concat());
    assert_eq!(o.status.code(), Some(1), "{}{}", stdout(&o), stderr(&o));
    let doc = json(&o);
    assert_eq!(doc["found"], serde_json::Value::Null);
    let old = slashes(doc["order_change"]["old"].as_str().expect("an old pick"));
    assert_eq!(old, "particles/rain.particles.toml");
    assert_eq!(doc["order_change"]["new"], serde_json::Value::Null);
    let message = doc["order_change"]["message"].as_str().unwrap();
    assert!(message.contains("the search order changed"), "{message}");

    let o = flint(&act1, &args);
    let text = stdout(&o);
    assert!(
        text.contains("note     particle effect 'rain': the search order changed"),
        "{text}"
    );
}

#[test]
fn help_and_a_bad_kind_list_the_kinds() {
    let dir = project("help");
    let o = flint(&dir, &["asset", "where", "--help"]);
    assert!(o.status.success());
    let help = stdout(&o);
    assert!(help.contains("Kinds:"), "{help}");
    assert!(help.contains("particles/<name>.particles.toml"), "{help}");

    let o = flint(&dir, &["asset", "where", "sprockets", "fire"]);
    assert!(!o.status.success());
    let err = stderr(&o);
    assert!(err.contains("no asset kind is called 'sprockets'"), "{err}");
    assert!(err.contains("particles/<name>.particles.toml"), "{err}");
    assert!(stdout(&o).is_empty());
}

/// The look and style kinds (palettes, glyph sets, fonts, looks, papers,
/// strokes, user styles) answer from the same search, two folders down;
/// and a name the engine has built in says so instead of failing.
#[test]
fn the_style_kinds_are_found_from_a_nested_scene_and_built_ins_are_named() {
    let dir = project("styles");
    for (path, text) in [
        ("palettes/dusk.palette.toml", "colors = [\"#101010\"]\n"),
        (
            "scenes/palettes/dusk.palette.toml",
            "colors = [\"#202020\"]\n",
        ),
        ("fonts/terminal.glyphs.toml", "[ramps.r]\nglyphs = \" #\"\n"),
        ("fonts/ui.font.toml", "[font]\nglyphs = \"builtin\"\n"),
        ("fonts/Body.ttf", ""),
        ("styles/noir.look.toml", "[post_process]\nexposure = 0.5\n"),
        ("styles/glow.shader.toml", "version = 1\nname = \"glow\"\n"),
        ("papers/cotton.png", ""),
        ("strokes/hb.png", ""),
    ] {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let found = |kind: &str, name: &str| -> serde_json::Value {
        let o = flint(
            &dir,
            &["asset", "where", kind, name, "--scene", SCENE, "--json"],
        );
        assert!(stderr(&o).is_empty(), "{kind} {name}: {}", stderr(&o));
        json(&o)
    };
    for (kind, name, want) in [
        // The nearer of two, in scenes/, not the project's
        ("palettes", "dusk", "scenes/palettes/dusk.palette.toml"),
        ("glyph-sets", "terminal", "fonts/terminal.glyphs.toml"),
        ("fonts", "fonts/ui.font.toml", "fonts/ui.font.toml"),
        ("font-faces", "Body.ttf", "fonts/Body.ttf"),
        ("looks", "noir", "styles/noir.look.toml"),
        ("papers", "cotton", "papers/cotton.png"),
        ("strokes", "hb", "strokes/hb.png"),
        ("styles", "glow", "styles/glow.shader.toml"),
    ] {
        let answer = found(kind, name);
        assert_eq!(
            slashes(answer["found"].as_str().unwrap_or_default()),
            want,
            "{kind} {name}: {answer}"
        );
        assert_eq!(answer["built_in"], false);
        let tried = paths(&answer["tried"]);
        assert!(
            tried[0].starts_with("scenes/act1/cave/"),
            "{kind} {name}: nearest first: {tried:?}"
        );
    }

    // No file of the name, but one built in: said, with the places tried,
    // and not a failure
    for (kind, name) in [
        ("palettes", "amber"),
        ("looks", "graphite"),
        ("glyph-sets", "builtin"),
    ] {
        let o = flint(&dir, &["asset", "where", kind, name, "--scene", SCENE]);
        assert!(o.status.success(), "{kind} {name}: {}", stderr(&o));
        let text = slashes(&stdout(&o));
        assert!(text.contains("loads    the built-in"), "{text}");
        assert!(text.contains("scenes/act1/cave/"), "{text}");
        let answer = found(kind, name);
        assert_eq!(answer["built_in"], true, "{answer}");
        assert!(answer["found"].is_null());
    }
    // Neither a file nor built in still fails, listing where it looked
    let o = flint(
        &dir,
        &["asset", "where", "palettes", "plaid", "--scene", SCENE],
    );
    assert!(!o.status.success());
    assert!(slashes(&stdout(&o)).contains("palettes/plaid.palette.toml"));

    let help = stdout(&flint(&dir, &["asset", "where", "--help"]));
    for line in [
        "palettes/<name> | <name>.palette.toml",
        "fonts/<name> | <name>.glyphs.toml",
        "<name> | fonts/<name>.font.toml",
        "font-faces",
        "styles/<name> | <name>.look.toml",
        "papers/<name> | <name>.png | .jpg | .jpeg",
        "strokes/<name> | <name>.png | .jpg | .jpeg",
        "styles/<name>.shader.toml",
        "Built in",
    ] {
        assert!(help.contains(line), "{line}: {help}");
    }
}
