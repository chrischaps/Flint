//! What a spec may say, from the binary (#718): `flint gen --list-generators`
//! and `--describe`, and the one validator `flint gen` runs before
//! generating and `flint validate` / `flint check` run on the specs a scene
//! names.
#![allow(clippy::disallowed_methods)] // a test: writes and reads its files off the disk

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A fresh, empty scratch directory for one test, under the checkout (so a
/// scene there resolves the checkout's schemas, as an example's does).
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("gen_params_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn flint(args: &[&str]) -> Output {
    Command::new(FLINT)
        .args(args)
        .current_dir(repo_root())
        .output()
        .expect("run flint")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// A texture pipeline whose third op has `columns = "x"`.
const BAD_BRICKS: &str = r#"generator = "texture_v1"
[meta]
name = "bad_bricks"
version = "1.0.0"
[seed]
mode = "fixed"
value = 7
[params]
width = 32
height = 32
pattern = "pipeline"
[[params.ops]]
type = "noise_layer"
[[params.ops]]
type = "cell_height"
[[params.ops]]
type = "brick_grid"
columns = "x"
"#;

/// The repo's oak with `leaf_density` (0..1) at 1.6 and a crown key its
/// L-system doesn't read.
fn bad_oak() -> String {
    let oak = std::fs::read_to_string(repo_root().join("specs/oak_tree.procgen.toml")).unwrap();
    let bad = oak
        .replace("name = \"oak_tree_01\"", "name = \"bad_oak\"")
        .replace(
            "leaf_density = 0.9",
            "leaf_density = 1.6\ncrown_shape = \"ellipsoid\"",
        );
    assert_ne!(oak, bad, "the oak spec no longer sets leaf_density = 0.9");
    bad
}

// ── --list-generators and --describe ───────────────────────────────────────

#[test]
fn list_generators_names_every_generator_on_stdout_only() {
    let o = flint(&["gen", "--list-generators"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stderr(&o), "", "nothing but stdout");
    let text = stdout(&o);
    for name in [
        "creature_v1",
        "form_v1",
        "mock",
        "sweep_v1",
        "texture_v1",
        "tree_v1",
    ] {
        assert!(text.contains(name), "{name} missing:\n{text}");
    }
    assert!(text.contains("can live"), "{text}");

    let o = flint(&["gen", "--list-generators", "--json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stderr(&o), "");
    let doc: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    let list = doc["generators"].as_array().unwrap();
    let tree = list.iter().find(|g| g["name"] == "tree_v1").unwrap();
    assert_eq!(tree["output"], "mesh");
    assert_eq!(tree["format"], "glb");
    assert_eq!(tree["living"], true);
    assert!(tree["revision"].as_u64().unwrap() >= 1);
    let texture = list.iter().find(|g| g["name"] == "texture_v1").unwrap();
    assert_eq!(texture["living"], false);
    assert_eq!(texture["format"], "png");
}

#[test]
fn describe_prints_a_generators_parameters_in_three_forms() {
    let o = flint(&["gen", "--describe", "tree_v1"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stderr(&o), "");
    let text = stdout(&o);
    assert!(text.contains("leaf_density"), "{text}");
    assert!(text.contains("number 0–1"), "{text}");
    assert!(
        text.contains("Only when branch_method is \"space_colonization\""),
        "{text}"
    );

    // JSON: the schema the validator checks against and the prompt embeds
    let o = flint(&["gen", "--describe", "tree_v1", "--json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stderr(&o), "");
    let doc: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(doc["name"], "tree_v1");
    assert_eq!(
        doc["schema"]["properties"]["leaf_density"]["maximum"],
        serde_json::json!(1.0)
    );

    let o = flint(&["gen", "--describe", "tree_v1", "--markdown"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("| `leaf_density` | number 0–1 |"));
}

#[test]
fn describe_gives_texture_v1_each_op_with_its_ports() {
    let o = flint(&["gen", "--describe", "texture_v1", "--json"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let doc: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    let ops = doc["ops"].as_array().unwrap();
    assert!(ops.len() >= 30, "{} ops", ops.len());
    let brick = ops.iter().find(|op| op["type"] == "brick_grid").unwrap();
    assert_eq!(brick["label"], "Brick Grid");
    assert!(brick["writes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c == "cell_id"));
    assert_eq!(
        brick["schema"]["properties"]["columns"]["type"],
        serde_json::json!("integer")
    );

    let text = stdout(&flint(&["gen", "--describe", "texture_v1"]));
    assert!(text.contains("brick_grid (Brick Grid)"), "{text}");
    assert!(text.contains("ports: writes cell_id"), "{text}");
}

/// A key holding tables lists their keys too (#1173): an agent learns
/// `limb_chains[].segments` from `--describe`, not from a validator error.
#[test]
fn describe_lists_the_keys_of_nested_tables() {
    let o = flint(&["gen", "--describe", "creature_v1"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let text = stdout(&o);
    let line = |name: &str| {
        text.lines()
            .find(|l| l.trim_start().starts_with(&format!("{name} ")))
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_else(|| panic!("no {name} in\n{text}"))
    };
    assert_eq!(
        line("limb_chains[].segments"),
        "limb_chains[].segments array of tables, required"
    );
    assert_eq!(
        line("limb_chains[].segments[].length"),
        "limb_chains[].segments[].length number, required"
    );
    assert_eq!(line("bones[].name"), "bones[].name string, required");
    assert_eq!(
        line("materials.<name>.roughness"),
        "materials.<name>.roughness number 0–1, default 0.5"
    );

    let md = stdout(&flint(&["gen", "--describe", "sweep_v1", "--markdown"]));
    assert!(
        md.contains("Each `[[params.control_points]]` entry:"),
        "{md}"
    );
    assert!(md.contains("| `twist` | number | `0.0` |"), "{md}");
}

#[test]
fn describing_an_unknown_generator_fails_suggesting_the_name() {
    let o = flint(&["gen", "--describe", "tree"]);
    assert!(!o.status.success());
    let err = stderr(&o);
    assert!(err.contains("did you mean 'tree_v1'?"), "{err}");
    assert!(err.contains("texture_v1"), "{err}");
}

// ── the validator, before generating ───────────────────────────────────────

#[test]
fn a_wrong_type_inside_an_op_stops_gen_by_its_path_and_range() {
    let dir = scratch("bad_op");
    let spec = dir.join("bad_bricks.procgen.toml");
    std::fs::write(&spec, BAD_BRICKS).unwrap();
    let out = dir.join("bricks.png");
    let o = flint(&["gen", s(&spec), "-o", s(&out)]);
    assert!(!o.status.success(), "{}", stdout(&o));
    let err = stderr(&o);
    assert!(
        err.contains("`ops[2].columns` should be an integer in the range [1, inf), not a string"),
        "{err}"
    );
    assert!(err.contains("nothing was generated"), "{err}");
    assert!(!out.exists() && !dir.join("bricks_albedo.png").exists());

    // A dry run checks the same
    let o = flint(&["gen", s(&spec), "--dry-run"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("`ops[2].columns`"));
}

#[test]
fn an_out_of_range_tree_parameter_stops_gen_naming_the_range() {
    let dir = scratch("bad_oak");
    let spec = dir.join("bad_oak.procgen.toml");
    std::fs::write(&spec, bad_oak()).unwrap();
    let out = dir.join("oak.glb");
    let o = flint(&["gen", s(&spec), "-o", s(&out)]);
    assert!(!o.status.success(), "{}", stdout(&o));
    let err = stderr(&o);
    assert!(
        err.contains("error: ")
            && err.contains("`leaf_density` = 1.6 is outside the allowed range [0, 1]"),
        "{err}"
    );
    // A key the generator ignores is still only a warning
    assert!(
        err.contains("warning: ") && err.contains("`crown_shape` is ignored"),
        "{err}"
    );
    assert!(!out.exists());
}

#[test]
fn an_ignored_key_alone_warns_and_strict_refuses() {
    let dir = scratch("ignored");
    let spec = dir.join("oak.procgen.toml");
    let oak = bad_oak().replace("leaf_density = 1.6", "leaf_density = 0.9");
    std::fs::write(&spec, oak).unwrap();
    let o = flint(&["gen", s(&spec), "--dry-run"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stderr(&o).contains("`crown_shape` is ignored"));
    let o = flint(&["gen", s(&spec), "--dry-run", "--strict"]);
    assert!(!o.status.success());
}

#[test]
fn gen_knows_the_generators_the_runtime_resolver_knows() {
    // `mock` is registered by the resolver; `flint gen` refused it before #718
    let dir = scratch("mock");
    let spec = dir.join("box.procgen.toml");
    std::fs::write(
        &spec,
        "generator = \"mock\"\n[meta]\nname = \"box\"\nversion = \"1\"\n[seed]\nmode = \"fixed\"\nvalue = 1\n",
    )
    .unwrap();
    let o = flint(&["gen", s(&spec), "-o", s(&dir.join("box.glb"))]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(dir.join("box.glb").exists());
}

// ── the same, for the specs a scene names ──────────────────────────────────

/// A scene naming both bad specs, beside them in `specs/`.
fn scene_with_bad_specs(name: &str) -> PathBuf {
    let dir = scratch(name);
    std::fs::create_dir_all(dir.join("specs")).unwrap();
    std::fs::write(dir.join("specs/bad_bricks.procgen.toml"), BAD_BRICKS).unwrap();
    std::fs::write(dir.join("specs/bad_oak.procgen.toml"), bad_oak()).unwrap();
    let scene = dir.join("grove.scene.toml");
    std::fs::write(
        &scene,
        r#"[scene]
name = "grove"

[entities.oak.transform]
position = [0, 0, 0]
[entities.oak.model]
asset = "bad_oak"

[entities.floor.transform]
position = [0, 0, 0]
[entities.floor.material]
texture = "bad_bricks"

[entities.second_oak.transform]
position = [4, 0, 0]
[entities.second_oak.model]
asset = "bad_oak"
"#,
    )
    .unwrap();
    scene
}

#[test]
fn validate_reports_the_specs_a_scene_names() {
    let scene = scene_with_bad_specs("validate");
    let o = flint(&["validate", s(&scene)]);
    assert!(!o.status.success(), "{}", stdout(&o));
    let text = format!("{}{}", stdout(&o), stderr(&o));
    assert!(
        text.contains("error[spec-param]")
            && text.contains("`ops[2].columns` should be an integer in the range [1, inf)"),
        "{text}"
    );
    assert!(
        text.contains("`leaf_density` = 1.6 is outside the allowed range [0, 1]"),
        "{text}"
    );
    assert!(
        text.contains("warning[spec-param]") && text.contains("`crown_shape` is ignored"),
        "{text}"
    );
    // One spec named twice is reported once
    assert_eq!(text.matches("`leaf_density` = 1.6").count(), 1, "{text}");
    assert!(text.contains("entity 'floor', material.texture"), "{text}");

    let o = flint(&["validate", s(&scene), "--format", "json"]);
    let doc: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    let found: Vec<&serde_json::Value> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "spec-param")
        .collect();
    assert_eq!(found.len(), 3, "{doc:#}");
    assert!(found
        .iter()
        .any(|d| d["field"] == "ops[2].columns" && d["severity"] == "error"));
}

#[test]
fn check_reports_the_same() {
    let scene = scene_with_bad_specs("check");
    let o = flint(&["check", s(&scene), "--json"]);
    assert!(!o.status.success());
    let doc: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    let fields: Vec<&str> = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "spec-param")
        .filter_map(|d| d["field"].as_str())
        .collect();
    assert_eq!(
        fields,
        ["ops[2].columns", "crown_shape", "leaf_density"],
        "{doc:#}"
    );
}
