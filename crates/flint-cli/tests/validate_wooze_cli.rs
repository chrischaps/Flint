//! End-to-end tests for `flint validate` on a scene's `[wooze]` table (#213): run
//! the real binary and hold the message and the exit code to the ticket.

use std::path::{Path, PathBuf};
use std::process::Output;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A scene holding `table` in a directory of its own, validated with the engine's
/// schemas, reporting as `format`.
fn validate(name: &str, table: &str, format: &str) -> Output {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("validate_wooze_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    // The spec the species scenes' oak is grown from: `model.asset` is
    // checked to name a model file or a spec (#684)
    std::fs::create_dir_all(dir.join("specs")).unwrap();
    std::fs::write(
        dir.join("specs/grove_oak.procgen.toml"),
        "generator = \"tree\"\n\n[meta]\nname = \"grove_oak\"\n",
    )
    .unwrap();
    let path = dir.join("grove.scene.toml");
    std::fs::write(&path, format!("[scene]\nname = \"grove\"\n{table}")).unwrap();
    std::process::Command::new(FLINT)
        .arg("validate")
        .arg(&path)
        .arg("--schemas")
        .arg(repo_root().join("schemas"))
        .arg("--format")
        .arg(format)
        .output()
        .expect("run flint")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

const GOOD: &str = r#"
[wooze]
preset = "linklater"

[[wooze.zone]]
name = "pocket"
shape = "sphere"
center = [0, 0, 12]
radius = 6
falloff = 4
glide = 0.8
"#;

#[test]
fn a_good_table_passes() {
    let o = validate("good", GOOD, "text");
    assert!(o.status.success(), "{}", text(&o));
}

#[test]
fn a_scene_without_one_passes() {
    let o = validate("none", "", "text");
    assert!(o.status.success(), "{}", text(&o));
}

#[test]
fn a_malformed_zone_fails_with_its_name_in_the_message() {
    let o = validate("radius", &GOOD.replace("radius = 6", "radius = -6"), "text");
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let out = text(&o);
    assert!(out.contains("pocket"), "{out}");
    assert!(out.contains("radius"), "{out}");
}

#[test]
fn every_way_of_being_malformed_names_the_zone() {
    for (name, from, to, says) in [
        ("no_radius", "radius = 6\n", "", "radius"),
        ("glide", "glide = 0.8", "glide = 1.8", "glide"),
        ("wrong_type", "radius = 6", "radius = \"six\"", "radius"),
        ("shape", "\"sphere\"", "\"cone\"", "cone"),
        ("misspelt", "falloff = 4", "fallof = 4", "fallof"),
        ("no_center", "center = [0, 0, 12]\n", "", "center"),
    ] {
        let o = validate(name, &GOOD.replace(from, to), "text");
        assert_eq!(o.status.code(), Some(1), "{name}: {}", text(&o));
        let out = text(&o);
        assert!(out.contains("pocket"), "{name}: {out}");
        assert!(out.contains(says), "{name}: {out}");
    }
}

#[test]
fn an_unnamed_zone_is_named_by_its_place() {
    let table = GOOD
        .replace("name = \"pocket\"\n", "")
        .replace("radius = 6", "radius = 0");
    let o = validate("unnamed", &table, "text");
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("zone 1"), "{}", text(&o));
}

#[test]
fn an_unknown_preset_fails_too() {
    let o = validate("preset", &GOOD.replace("linklater", "goddard"), "text");
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("goddard"), "{}", text(&o));
}

/// One oak, alive, and a `[living.species.<spec>]` table for `species` (#249).
fn grove_with_species(species: &str, body: &str) -> String {
    format!(
        "{GOOD}\n[living.species.{species}]\n{body}\n\n\
         [entities.oak_1.transform]\nposition = [0.0, 0.0, 0.0]\n\
         [entities.oak_1.model]\nasset = \"grove_oak\"\n\
         [entities.oak_1.living_prop]\nlineage_seed = 3\n"
    )
}

#[test]
fn a_species_table_for_a_spec_the_scene_grows_passes() {
    let o = validate(
        "species_good",
        &grove_with_species("grove_oak", "step = 0.25"),
        "text",
    );
    assert!(o.status.success(), "{}", text(&o));
}

#[test]
fn a_misspelt_species_table_fails_and_is_named() {
    let o = validate(
        "species_typo",
        &grove_with_species("grove_oke", "step = 0.25"),
        "text",
    );
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let out = text(&o);
    assert!(out.contains("[living.species.grove_oke]"), "{out}");
    assert!(
        out.contains("grove_oak"),
        "says what the props grow from: {out}"
    );
}

#[test]
fn a_misspelt_key_in_a_species_table_fails_and_is_named() {
    let o = validate(
        "species_key",
        &grove_with_species("grove_oak", "ehoes = { count = 1 }"),
        "text",
    );
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("ehoes"), "{}", text(&o));
}

#[test]
fn json_reads_as_a_violation_of_the_zone() {
    let o = validate("json", &GOOD.replace("radius = 6", "radius = -6"), "json");
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    let report: serde_json::Value = serde_json::from_slice(&o.stdout).expect("json on stdout");
    assert_eq!(report["valid"], false);
    let violation = report["violations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["constraint"] == "wooze")
        .expect("a wooze violation");
    assert_eq!(violation["entity"], "pocket");
    assert!(violation["message"].as_str().unwrap().contains("radius"));
}
