//! An inline `particle_emitter` is held to the emitter's own reader by
//! `flint validate` and `flint check` (#690): a key the emitter doesn't read
//! and a value it can't use fail the run, each named by entity and key,
//! where the first used to be a warning and the second, with a value the
//! schema has no type for (a curve), a warning that the key is unknown,
//! while the player quietly drew the default emitter. An emitter written in
//! the keys the schema doesn't list (`lifetime = [a, b]`, a `size` curve, a
//! shape table) passes, where it used to warn and even fail. The engine's
//! own schemas, so the test follows the real `particle_emitter` file.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SCENE: &str = r#"[scene]
name = "Emitters"

[entities.campfire.transform]
position = [0.0, 0.0, 0.0]

[entities.campfire.particle_emitter]
emission_rate = 50.0
emision_rate = 80.0
color = "orange"
lifetime = [0.5, 1.2]

[entities.sparks.transform]
position = [2.0, 0.0, 0.0]

[entities.sparks.particle_emitter]
emission_rate = 30.0
shape = "donut"

[entities.smoke.transform]
position = [4.0, 0.0, 0.0]

[entities.smoke.particle_emitter]
emission_rate = 10.0
lifetime = [2.0, 4.0]
size = { start = [0.2, 0.2], end = [1.0, 1.0] }
shape = { kind = "sphere", radius = 0.3 }
"#;

fn project(name: &str, scene: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("particle_emitter_check_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    let schemas = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas")
        .canonicalize()
        .unwrap();
    let schemas = schemas.to_string_lossy().replace('\\', "/");
    let schemas = schemas.trim_start_matches("//?/");
    std::fs::write(
        dir.join("flint.toml"),
        format!("[project]\nid = \"emitters\"\nschemas = ['{schemas}']\n"),
    )
    .unwrap();
    std::fs::write(dir.join("scenes/fire.scene.toml"), scene).unwrap();
    dir
}

fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run flint")
}

/// (code, entity, field) of every diagnostic about a `particle_emitter`,
/// and the whole document.
fn emitter_rows(o: &Output) -> (Vec<(String, String, String)>, serde_json::Value) {
    let text = String::from_utf8_lossy(&o.stdout);
    let doc: serde_json::Value = serde_json::from_str(&text).expect("one JSON document");
    let rows = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["component"] == "particle_emitter")
        .map(|d| {
            let s = |key: &str| d[key].as_str().unwrap_or("").to_string();
            (s("code"), s("entity"), s("field"))
        })
        .collect();
    (rows, doc)
}

#[test]
fn validate_fails_on_a_bad_inline_emitter_naming_entity_and_key() {
    let dir = project("validate", SCENE);
    let o = flint(
        &dir,
        &["validate", "scenes/fire.scene.toml", "--format", "json"],
    );
    let (mut rows, doc) = emitter_rows(&o);
    assert_eq!(o.status.code(), Some(1), "{doc}");
    rows.sort();
    let row = |e: &str, f: &str| ("bad-emitter".to_string(), e.to_string(), f.to_string());
    assert_eq!(
        rows,
        [
            row("campfire", "color"),
            row("campfire", "emision_rate"),
            row("sparks", "shape"),
        ],
        "{doc}"
    );
    let typo = doc["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["field"] == "emision_rate")
        .unwrap();
    assert_eq!(typo["severity"], "error");
    assert_eq!(typo["hint"], "did you mean `emission_rate`?");
}

#[test]
fn check_reports_the_same_and_a_clean_emitter_passes() {
    let dir = project("check", SCENE);
    let o = flint(&dir, &["check", "scenes/fire.scene.toml", "--json"]);
    assert_eq!(o.status.code(), Some(1));
    let (rows, doc) = emitter_rows(&o);
    assert_eq!(rows.len(), 3, "{doc}");
    assert!(rows.iter().all(|(code, ..)| code == "bad-emitter"), "{doc}");
    assert!(
        !rows.iter().any(|(_, entity, _)| entity == "smoke"),
        "{doc}"
    );

    // The smoke alone: modern keys, a curve and a shape table are no
    // diagnostic at all (they were three warnings and an error)
    let smoke = SCENE.split("[entities.smoke.transform]").nth(1).unwrap();
    let clean = format!("[scene]\nname = \"Smoke\"\n\n[entities.smoke.transform]{smoke}");
    let dir = project("clean", &clean);
    let o = flint(
        &dir,
        &["validate", "scenes/fire.scene.toml", "--format", "json"],
    );
    let (rows, doc) = emitter_rows(&o);
    assert_eq!(o.status.code(), Some(0), "{doc}");
    assert_eq!(rows, [], "{doc}");
}
