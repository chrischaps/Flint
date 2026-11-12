//! #687's done-when, with the real binary on a small project.
//!
//! - `flint check <dir>` lists every problem in one run, as text grouped by
//!   file and as one JSON document, with a non-zero exit: a scene that does
//!   not parse, an unknown archetype, an unknown component, a `joint.parent`
//!   that names no entity, a missing model (with the paths tried), a file a
//!   script names that isn't there, a constraint the scene breaks and a
//!   constraint whose query does not parse. The same project with those
//!   mended exits 0.
//! - `flint validate` on a project with no constraint files still loads and
//!   checks the scene.
//! - A constraint with a bad query fails `flint validate` and `flint check`,
//!   naming the constraint and its file.
//! - `flint bundle` reports the same missing files as `flint check`, and
//!   holds no check of its own.
//! - `flint validate --format` is a checked choice, and `--fix` /
//!   `--dry-run` answer in JSON when asked.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SCHEMAS: &[(&str, &str)] = &[
    (
        "schemas/components/place.toml",
        "[component.place]\n\n[component.place.fields]\n\
         position = { type = \"vec3\", default = [0, 0, 0] }\n",
    ),
    (
        "schemas/components/model.toml",
        "[component.model]\n\n[component.model.fields]\n\
         asset = { type = \"string\", default = \"\" }\n",
    ),
    (
        "schemas/components/joint.toml",
        "[component.joint]\n\n[component.joint.fields]\n\
         parent = { type = \"entity\", default = \"\" }\n",
    ),
    (
        "schemas/components/script.toml",
        "[component.script]\n\n[component.script.fields]\n\
         source = { type = \"string\", default = \"\" }\n",
    ),
    (
        "schemas/components/lamp.toml",
        "[component.lamp]\n\n[component.lamp.fields]\n\
         brightness = { type = \"f32\", default = 1.0 }\n",
    ),
    (
        "schemas/archetypes/prop.toml",
        "[archetype.prop]\ndescription = \"A prop\"\ncomponents = [\"place\"]\n",
    ),
];

const GOOD_RULE: &str = "[[constraint]]\nname = \"lamps_are_not_blinding\"\n\
    query = \"entities where archetype == 'prop'\"\nseverity = \"warning\"\n\
    message = \"Lamp '{name}' is too bright\"\n\n\
    [constraint.kind]\ntype = \"value_range\"\nfield = \"lamp.brightness\"\nmin = 0.0\nmax = 10.0\n";

/// A second condition: the language has one, so this never ran.
const BAD_RULE: &str = "\n[[constraint]]\nname = \"lit_props_have_a_place\"\n\
    query = \"entities where archetype == 'prop' and lamp.brightness > 0\"\nseverity = \"error\"\n\
    message = \"'{name}' has no place\"\n\n\
    [constraint.kind]\ntype = \"required_component\"\narchetype = \"prop\"\ncomponent = \"place\"\n";

const BROKEN_SCENE: &str = r#"[scene]
name = "Dock"

[entities.crate]
archetype = "porp"

[entities.boat.model]
asset = "boat"

[entities.spare.model]
asset = "boat"

[entities.buoy.lihgt]
brightness = 2.0

[entities.gate.joint]
parent = "frame"

[entities.sign.script]
source = "sign.rhai"

[entities.lantern]
archetype = "prop"
[entities.lantern.lamp]
brightness = 50.0
"#;

const CLEAN_SCENE: &str = r#"[scene]
name = "Dock"

[entities.crate]
archetype = "prop"

[entities.boat.model]
asset = "boat"

[entities.frame.place]
position = [0, 1, 0]

[entities.gate.joint]
parent = "frame"

[entities.sign.script]
source = "sign.rhai"

[entities.lantern]
archetype = "prop"
[entities.lantern.lamp]
brightness = 5.0
"#;

/// A project in a scratch directory: the schemas, a manifest, and `files`.
fn project(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("check_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    let manifest = (
        "flint.toml",
        "[project]\nid = \"dock\"\nschemas = [\"schemas\"]\nscene = \"scenes/dock.scene.toml\"\n",
    );
    for (path, text) in SCHEMAS.iter().chain(&[manifest]).chain(files) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    dir
}

/// The project with one of everything wrong.
fn broken(name: &str) -> PathBuf {
    let rules = format!("{GOOD_RULE}{BAD_RULE}");
    project(
        name,
        &[
            ("scenes/dock.scene.toml", BROKEN_SCENE),
            (
                "scenes/torn.scene.toml",
                "[scene]\nname = \"Torn\"\n\n[entities.sail\n",
            ),
            ("schemas/constraints/rules.toml", &rules),
            (
                "scripts/sign.rhai",
                "fn on_init() { play_sound(\"audio/creak.ogg\"); }\n",
            ),
        ],
    )
}

fn clean(name: &str) -> PathBuf {
    project(
        name,
        &[
            ("scenes/dock.scene.toml", CLEAN_SCENE),
            ("schemas/constraints/rules.toml", GOOD_RULE),
            (
                "scripts/sign.rhai",
                "fn on_init() { play_sound(\"audio/creak.ogg\"); }\n",
            ),
            ("audio/creak.ogg", ""),
            ("models/boat.glb", ""),
        ],
    )
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

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).replace("\r\n", "\n")
}

/// The whole of stdout as one JSON document.
fn json(o: &Output) -> serde_json::Value {
    serde_json::from_str(&stdout(o))
        .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}):\n{}", stdout(o)))
}

/// The diagnostic with `code` (and, when given, about `entity`).
fn diagnostic<'a>(
    doc: &'a serde_json::Value,
    code: &str,
    entity: Option<&str>,
) -> &'a serde_json::Value {
    doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["code"] == code && entity.is_none_or(|e| d["entity"] == e))
        .unwrap_or_else(|| panic!("no {code} diagnostic for {entity:?} in\n{doc:#}"))
}

// ── Box 1: every problem in one run, as text and as JSON ──────────────

#[test]
fn every_problem_of_a_project_is_listed_in_one_run() {
    let dir = broken("every_problem");
    let o = flint(&dir, &["check"]);
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(1), "{out}{}", stderr(&o));

    // Grouped by file, each with its count
    for header in [
        "scenes/dock.scene.toml: 4 errors, 1 warning\n",
        "scenes/torn.scene.toml: 1 error\n",
        "schemas/constraints/rules.toml: 1 error\n",
        "scripts/sign.rhai: 1 error\n",
    ] {
        assert!(out.contains(header), "no `{}` in\n{out}", header.trim());
    }
    // Each problem, with its entity and field
    for line in [
        "error[unknown-archetype] entity 'crate' field 'archetype': no archetype named `porp`",
        "error[unknown-component] entity 'buoy' component 'lihgt': no component named `lihgt`",
        "error[unknown-entity] entity 'gate' joint.parent: no entity named `frame`",
        "error[missing-asset] entity 'boat' model.asset: model `boat` is not found (also named by 'spare')",
        "warning[constraint] entity 'lantern': Lamp 'lantern' is too bright (constraint `lamps_are_not_blinding`)",
        "error[toml-parse]: TOML parse error at line 4",
        "error[bad-constraint] field 'query': constraint `lit_props_have_a_place`: `entities where archetype == 'prop' and lamp.brightness > 0` can't be evaluated",
        "error[missing-file]: a string in this script names `audio/creak.ogg`",
    ] {
        assert!(out.contains(line), "no `{line}` in\n{out}");
    }
    // A hint, and where the engine looked for the model
    assert!(out.contains("      hint: did you mean `prop`?"), "{out}");
    assert!(
        out.contains("      tried:\n        scenes/models/boat.glb\n        scenes/models/boat.gltf\n        models/boat.glb\n        models/boat.gltf\n"),
        "{out}"
    );
    assert!(
        out.ends_with("checked 2 scenes and 3 files: 7 errors, 1 warning\n"),
        "{out}"
    );
    // Nothing but the report on stdout, and nothing at all on stderr
    assert_eq!(stderr(&o), "", "stderr");
}

#[test]
fn the_same_list_is_one_json_document() {
    let dir = broken("json");
    let o = flint(&dir, &["check", ".", "--json"]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    let doc = json(&o);
    assert_eq!(doc["valid"], false);
    assert_eq!(doc["errors"], 7);
    assert_eq!(doc["warnings"], 1);
    assert_eq!(
        doc["scenes"],
        serde_json::json!(["scenes/dock.scene.toml", "scenes/torn.scene.toml"])
    );
    assert_eq!(doc["diagnostics"].as_array().unwrap().len(), 8);

    let model = diagnostic(&doc, "missing-asset", Some("boat"));
    assert_eq!(model["severity"], "error");
    assert_eq!(model["file"], "scenes/dock.scene.toml");
    assert_eq!(model["component"], "model");
    assert_eq!(model["field"], "asset");
    assert_eq!(
        model["tried"],
        serde_json::json!([
            "scenes/models/boat.glb",
            "scenes/models/boat.gltf",
            "models/boat.glb",
            "models/boat.gltf"
        ])
    );
    assert!(model["hint"]
        .as_str()
        .unwrap()
        .starts_with("`flint asset where models boat --scene scenes/dock.scene.toml`"));

    let joint = diagnostic(&doc, "unknown-entity", Some("gate"));
    assert_eq!(joint["component"], "joint");
    assert_eq!(joint["field"], "parent");
    assert_eq!(joint["tried"], serde_json::json!([]));

    let archetype = diagnostic(&doc, "unknown-archetype", Some("crate"));
    assert_eq!(archetype["hint"], "did you mean `prop`?");
    assert_eq!(
        diagnostic(&doc, "unknown-component", Some("buoy"))["component"],
        "lihgt"
    );
    assert_eq!(
        diagnostic(&doc, "toml-parse", None)["file"],
        "scenes/torn.scene.toml"
    );

    let rule = diagnostic(&doc, "bad-constraint", None);
    assert_eq!(rule["file"], "schemas/constraints/rules.toml");
    assert_eq!(rule["field"], "query");
    assert!(rule["message"]
        .as_str()
        .unwrap()
        .starts_with("constraint `lit_props_have_a_place`: "));

    let literal = diagnostic(&doc, "missing-file", None);
    assert_eq!(literal["file"], "scripts/sign.rhai");
    let tried: Vec<&str> = literal["tried"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert_eq!(tried[0], "scripts/audio/creak.ogg");
    assert!(tried.contains(&"audio/creak.ogg"), "{tried:?}");

    let broken = diagnostic(&doc, "constraint", Some("lantern"));
    assert_eq!(broken["severity"], "warning");
}

#[test]
fn a_clean_project_exits_zero() {
    let dir = clean("clean");
    let o = flint(&dir, &["check"]);
    assert_eq!(o.status.code(), Some(0), "{}{}", stdout(&o), stderr(&o));
    // The scene, its script and its model (a glTF is read for the files it
    // keeps beside it, #382)
    assert_eq!(stdout(&o), "checked 1 scene and 3 files: no problems\n");

    let doc = json(&flint(&dir, &["check", "--json"]));
    assert_eq!(doc["valid"], true);
    assert_eq!(doc["diagnostics"], serde_json::json!([]));

    // A warning does not fail it; --strict fails an undeclared field
    let scene = dir.join("scenes/dock.scene.toml");
    std::fs::write(
        &scene,
        CLEAN_SCENE.replace("brightness = 5.0", "brightnes = 5.0"),
    )
    .unwrap();
    let o = flint(&dir, &["check"]);
    assert_eq!(o.status.code(), Some(0), "{}", stdout(&o));
    assert!(
        stdout(&o).contains("warning[unknown-field]"),
        "{}",
        stdout(&o)
    );
    let o = flint(&dir, &["check", "--strict"]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(
        stdout(&o).contains("error[unknown-field]"),
        "{}",
        stdout(&o)
    );
}

/// One scene: the scene and the files it names (its script's literals),
/// and not the other scenes of the project.
#[test]
fn one_scene_is_checked_with_the_files_it_names() {
    let dir = broken("one_scene");
    let o = flint(&dir, &["check", "scenes/dock.scene.toml", "--json"]);
    assert_eq!(o.status.code(), Some(1));
    let doc = json(&o);
    assert_eq!(doc["scenes"], serde_json::json!(["scenes/dock.scene.toml"]));
    assert_eq!(doc["files"], 2, "the scene and its script");
    assert_eq!(
        diagnostic(&doc, "missing-file", None)["file"],
        "scripts/sign.rhai"
    );
    assert!(
        !doc["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "toml-parse"),
        "{doc:#}"
    );

    // A script that is not there is the scene's missing asset, with the
    // paths tried
    std::fs::remove_file(dir.join("scripts/sign.rhai")).unwrap();
    let doc = json(&flint(&dir, &["check", "scenes/dock.scene.toml", "--json"]));
    let script = diagnostic(&doc, "missing-asset", Some("sign"));
    assert_eq!(script["field"], "source");
    assert_eq!(
        script["tried"],
        serde_json::json!(["scenes/scripts/sign.rhai", "scripts/sign.rhai"])
    );

    let o = flint(&dir, &["check", "scenes/nowhere.scene.toml"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("neither a folder nor a scene file"));
}

// ── Box 2: validate with no constraint files still loads the scene ─────

#[test]
fn validate_with_no_constraints_still_loads_and_checks_the_scene() {
    // Parses, and does not load: `lid` hangs from an entity that isn't there
    let dangling = "[scene]\nname = \"Hold\"\n\n[entities.lid]\nparent = \"chest\"\n\
                    [entities.lid.place]\nposition = [0, 1, 0]\n";
    let dir = project(
        "no_constraints",
        &[
            ("scenes/hold.scene.toml", dangling),
            (
                "scenes/torn.scene.toml",
                "[scene]\nname = \"Torn\"\n\n[entities.sail\n",
            ),
            (
                "scenes/fine.scene.toml",
                "[scene]\nname = \"Fine\"\n\n[entities.a.place]\n",
            ),
        ],
    );
    assert!(!dir.join("schemas/constraints").exists());

    let o = flint(&dir, &["validate", "scenes/hold.scene.toml"]);
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(1), "{out}{}", stderr(&o));
    assert!(
        out.contains(
            "error[unknown-entity] entity 'lid' field 'parent': no entity named `chest`, so the scene does not load"
        ),
        "{out}"
    );
    assert!(out.contains("The scene does not load"), "{out}");

    let o = flint(
        &dir,
        &["validate", "scenes/hold.scene.toml", "--format", "json"],
    );
    assert_eq!(o.status.code(), Some(1));
    let doc = json(&o);
    assert_eq!(doc["valid"], false);
    assert_eq!(doc["diagnostics"][0]["code"], "unknown-entity");

    let o = flint(&dir, &["validate", "scenes/torn.scene.toml"]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(stdout(&o).contains("error[toml-parse]"), "{}", stdout(&o));

    // And a scene with nothing wrong still passes, saying what was checked
    let o = flint(&dir, &["validate", "scenes/fine.scene.toml"]);
    assert_eq!(o.status.code(), Some(0), "{}{}", stdout(&o), stderr(&o));
    assert!(
        stdout(&o).contains("No constraints found"),
        "{}",
        stdout(&o)
    );
}

// ── Box 3: a constraint with a bad query fails, naming it ──────────────

#[test]
fn a_constraint_with_a_bad_query_fails_validate_and_check() {
    let rules = format!("{GOOD_RULE}{BAD_RULE}");
    let dir = project(
        "bad_query",
        &[
            ("scenes/dock.scene.toml", CLEAN_SCENE),
            ("schemas/constraints/rules.toml", &rules),
            ("scripts/sign.rhai", ""),
            ("models/boat.glb", ""),
        ],
    );
    let named = "constraint 'lit_props_have_a_place' (schemas/constraints/rules.toml): its query \
                 `entities where archetype == 'prop' and lamp.brightness > 0` can't be evaluated: \
                 Parse error";
    let o = flint(&dir, &["validate", "scenes/dock.scene.toml"]);
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(1), "{out}{}", stderr(&o));
    assert!(
        out.contains(&format!("[ERROR] [constraint]: {named}")),
        "{out}"
    );

    let o = flint(
        &dir,
        &["validate", "scenes/dock.scene.toml", "--format", "json"],
    );
    assert_eq!(o.status.code(), Some(1));
    let doc = json(&o);
    let violation = &doc["violations"][0];
    assert_eq!(violation["constraint"], "lit_props_have_a_place");
    assert_eq!(violation["severity"], "error");
    assert!(violation["message"].as_str().unwrap().starts_with(named));

    let o = flint(&dir, &["check"]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(
        stdout(&o).contains("schemas/constraints/rules.toml: 1 error\n"),
        "{}",
        stdout(&o)
    );

    // With the query mended, the constraint runs and the project passes
    std::fs::write(dir.join("schemas/constraints/rules.toml"), GOOD_RULE).unwrap();
    assert_eq!(flint(&dir, &["check"]).status.code(), Some(0));
    assert_eq!(
        flint(&dir, &["validate", "scenes/dock.scene.toml"])
            .status
            .code(),
        Some(0)
    );

    // A constraint file that is not TOML is named too, not skipped
    std::fs::write(
        dir.join("schemas/constraints/rules.toml"),
        "[[constraint]\n",
    )
    .unwrap();
    let o = flint(&dir, &["validate", "scenes/dock.scene.toml"]);
    assert_eq!(o.status.code(), Some(1), "{}", stdout(&o));
    assert!(
        stdout(&o).contains(
            "error[bad-constraint]: Constraint load error: schemas/constraints/rules.toml:"
        ),
        "{}",
        stdout(&o)
    );
}

// ── Box 4: bundle and check share one implementation ───────────────────

/// The lines of a text report that name a missing file or asset.
fn missing_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = text
        .lines()
        .filter(|l| l.contains("error[missing-"))
        .map(|l| l.trim().to_string())
        .collect();
    lines.sort();
    lines
}

#[test]
fn bundle_and_check_report_the_same_references() {
    let dir = broken("bundle");
    let checked = flint(&dir, &["check"]);
    let packed = flint(&dir, &["bundle", ".", "--dry-run"]);
    assert_eq!(packed.status.code(), Some(1), "{}", stdout(&packed));
    let bundle_said = stderr(&packed);
    assert!(bundle_said.contains("nothing written"), "{bundle_said}");

    let from_check = missing_lines(&stdout(&checked));
    assert_eq!(from_check.len(), 2, "{from_check:?}");
    assert_eq!(from_check, missing_lines(&bundle_said), "{bundle_said}");
    // The paths tried are the same too: one search, over the disk for one
    // and over the packed files for the other
    assert!(
        bundle_said.contains("        scenes/models/boat.glb\n        scenes/models/boat.gltf\n        models/boat.glb\n"),
        "{bundle_said}"
    );
    // And what stops a bundle besides: a scene that doesn't parse, an
    // archetype in no packed schema
    assert!(bundle_said.contains("error[toml-parse]"), "{bundle_said}");
    assert!(
        bundle_said.contains("error[unknown-archetype] entity 'crate'"),
        "{bundle_said}"
    );
    // What plays with a warning is `flint check`'s to report, not a hole
    assert!(!bundle_said.contains("unknown-component"), "{bundle_said}");

    // The clean project packs
    let dir = clean("bundle_clean");
    let packed = flint(&dir, &["bundle", ".", "--dry-run"]);
    assert_eq!(packed.status.code(), Some(0), "{}", stderr(&packed));
}

/// `flint bundle` holds no check of its own: what it used to do in
/// `check_references`, `check_scenes` and `check_user_styles` is
/// `crate::check`'s, and the file no longer names what those called.
#[test]
fn no_copy_of_the_checks_remains_in_bundle_rs() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/bundle.rs");
    let text = std::fs::read_to_string(source).unwrap();
    let code = text.split("#[cfg(test)]").next().unwrap();
    for gone in [
        "fn check_references",
        "fn check_scenes",
        "fn check_user_styles",
        "fn looks_like_file",
        "fn candidates",
        "fn rhai_refs",
        "fn toml_refs",
        "load_scene",
        "get_archetype",
        "custom::load_dir",
        "look::resolve",
    ] {
        assert!(!code.contains(gone), "bundle.rs still has `{gone}`");
    }
    assert!(code.contains("check::run(&subject, Checks::BUNDLE)"));
}

// ── validate: a checked --format, and JSON from --fix and --dry-run ─────

#[test]
fn validate_format_is_a_checked_choice() {
    let dir = clean("format");
    let o = flint(
        &dir,
        &["validate", "scenes/dock.scene.toml", "--format", "yaml"],
    );
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("invalid value 'yaml'"),
        "{}",
        stderr(&o)
    );
    assert!(stderr(&o).contains("text, json"), "{}", stderr(&o));
    assert_eq!(stdout(&o), "");
    for format in ["text", "json"] {
        let o = flint(
            &dir,
            &["validate", "scenes/dock.scene.toml", "--format", format],
        );
        assert_eq!(o.status.code(), Some(0), "{format}: {}", stderr(&o));
    }
}

#[test]
fn fix_and_dry_run_answer_in_json() {
    let fixable = "[[constraint]]\nname = \"lamps_are_not_blinding\"\n\
        query = \"entities where archetype == 'prop'\"\nseverity = \"error\"\n\
        message = \"Lamp '{name}' is too bright\"\n\n\
        [constraint.kind]\ntype = \"value_range\"\nfield = \"lamp.brightness\"\nmin = 0.0\nmax = 10.0\n\n\
        [constraint.auto_fix]\nenabled = true\nstrategy = \"set_default\"\n\
        field = \"lamp.brightness\"\nvalue = 10.0\n";
    let scene = CLEAN_SCENE.replace("brightness = 5.0", "brightness = 50.0");
    let dir = project(
        "fix_json",
        &[
            ("scenes/dock.scene.toml", &scene),
            ("schemas/constraints/rules.toml", fixable),
            ("scripts/sign.rhai", ""),
            ("models/boat.glb", ""),
        ],
    );
    let path = "scenes/dock.scene.toml";

    // A dry run says what a fix would do, changes nothing, and exits as
    // `flint validate` alone does on the file as it stands
    let o = flint(&dir, &["validate", path, "--dry-run", "--format", "json"]);
    let doc = json(&o);
    assert_eq!(o.status.code(), Some(1), "{doc:#}");
    assert_eq!(doc["valid"], false);
    assert_eq!(doc["fix"]["dry_run"], true);
    assert_eq!(doc["fix"]["saved"], false);
    assert_eq!(doc["fix"]["remaining_violations"], 0);
    let action = &doc["fix"]["actions"][0];
    assert_eq!(action["constraint"], "lamps_are_not_blinding");
    assert_eq!(action["entity"], "lantern");
    assert_eq!(doc["violations"][0]["has_auto_fix"], true);
    assert_eq!(std::fs::read_to_string(dir.join(path)).unwrap(), scene);

    // The fix writes the file and what is left is nothing
    let o = flint(
        &dir,
        &[
            "validate",
            path,
            "--fix",
            "--output-diff",
            "--format",
            "json",
        ],
    );
    let doc = json(&o);
    assert_eq!(o.status.code(), Some(0), "{doc:#}");
    assert_eq!(doc["valid"], true);
    assert_eq!(doc["fix"]["dry_run"], false);
    assert_eq!(doc["fix"]["saved"], true);
    assert_eq!(doc["fix"]["actions"].as_array().unwrap().len(), 1);
    assert_eq!(doc["violations"], serde_json::json!([]));
    assert!(doc["fix"]["diff"].as_str().unwrap().contains("brightness"));
    let after = std::fs::read_to_string(dir.join(path)).unwrap();
    assert!(after.contains("brightness = 10.0"), "{after}");
}
