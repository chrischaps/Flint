//! `flint validate --fix` writes only what its fixes change (#677). It used to
//! rewrite the scene from the world, which dropped `[post_process]`, `[camera]`,
//! `[environment]`, `[wooze]`, `[living]`, the prefabs and every comment, and
//! renamed the scene "scene".

use std::path::{Path, PathBuf};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

const SCENE: &str = r#"# The porch at dusk. This comment stays.
[scene]
name = "Porch at dusk"
description = "A door that opens too far"

[camera]
position = [0.0, 3.0, 10.0]
target = [0.0, 1.0, 0.0]

[environment]
ambient = 0.25

[post_process]
bloom_intensity = 0.05 # soft
look = "ink_wash"

[living]
stagger = "per_level"

[wooze]
preset = "linklater"

[prefabs.porch]
template = "lamp"
prefix = "porch"

# The door the fix closes
[entities.front_door]
archetype = "door"

[entities.front_door.door]
open_angle = 200.0 # too far
locked = false

[entities.front_door.transform]
position = [0, 0, 0]
"#;

const LAMP: &str =
    "[prefab]\nname = \"lamp\"\n\n[entities.bulb]\n[entities.bulb.light]\nintensity = 1.0\n";

/// What the `add_child` fix makes: an archetype the examples' layer lacks
const HANDLE: &str = r#"[archetype.handle]
description = "A door's handle"
components = ["transform"]
"#;

// Named apart from the examples' `door_angle_range`, which has no fix: the
// fixer finds that one by the name and fixes nothing
const CONSTRAINTS: &str = r#"
[[constraint]]
name = "door_opens_too_far"
query = "entities where archetype == 'door'"
severity = "warning"
message = "Door '{name}' opens too far"

[constraint.kind]
type = "value_range"
field = "door.open_angle"
min = 0.0
max = 180.0

[constraint.auto_fix]
enabled = true
strategy = "set_default"
field = "door.open_angle"
value = 90.0

[[constraint]]
name = "doors_have_handles"
query = "entities where archetype == 'door'"
severity = "warning"
message = "Door '{name}' has no handle"

[constraint.kind]
type = "required_child"
child_archetype = "handle"

[constraint.auto_fix]
enabled = true
strategy = "add_child"
archetype = "handle"
"#;

fn project(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("validate_fix_cli")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("schemas/constraints")).unwrap();
    std::fs::create_dir_all(dir.join("schemas/archetypes")).unwrap();
    std::fs::create_dir_all(dir.join("prefabs")).unwrap();
    std::fs::write(dir.join("schemas/constraints/fix.toml"), CONSTRAINTS).unwrap();
    std::fs::write(dir.join("schemas/archetypes/handle.toml"), HANDLE).unwrap();
    std::fs::write(dir.join("prefabs/lamp.prefab.toml"), LAMP).unwrap();
    std::fs::write(dir.join("porch.scene.toml"), SCENE).unwrap();
    dir
}

#[test]
fn fix_keeps_every_table_the_comments_and_the_name() {
    let dir = project("keeps");
    let scene = dir.join("porch.scene.toml");
    // A --schemas flag is the whole list (ADR 0114): the engine's and the
    // examples' (`door`, `handle`) first, or every component is unknown
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let o = std::process::Command::new(FLINT)
        .arg("validate")
        .arg(&scene)
        .arg("--fix")
        .arg("--output-diff")
        .arg("--schemas")
        .arg(&engine)
        .arg("--schemas")
        .arg(engine.join("examples"))
        .arg("--schemas")
        .arg(dir.join("schemas"))
        .output()
        .expect("run flint");
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(o.status.success(), "{out}");
    assert!(out.contains("Scene saved"), "{out}");

    let after = std::fs::read_to_string(&scene).unwrap();
    // The fixes landed: the angle in place, its comment kept, and a handle
    assert!(after.contains("open_angle = 90.0 # too far"), "{after}");
    assert!(after.contains("[entities.front_door_handle]"), "{after}");
    assert!(after.contains("parent = \"front_door\""), "{after}");
    // ...and nothing else moved: every other line of the scene, in order
    let mut rest = after.lines();
    for line in SCENE
        .lines()
        .filter(|l| *l != "open_angle = 200.0 # too far")
    {
        assert!(rest.any(|l| l == line), "lost or moved {line:?}:\n{after}");
    }
    // The prefab's entity stays the prefab's, not baked into the file
    assert!(!after.contains("porch_bulb"), "{after}");

    let file: flint_scene::SceneFile = toml::from_str(&after).unwrap();
    assert_eq!(file.scene.name, "Porch at dusk");
    assert!(file.camera.is_some() && file.environment.is_some());
    assert!(file.post_process.is_some());
    assert!(file.tables.contains_key("living") && file.tables.contains_key("wooze"));
    assert!(file.prefabs.contains_key("porch"));

    // The diff is of the file, not of two world dumps
    assert!(out.contains("open_angle"), "{out}");

    // Fixed once, a second run has nothing to fix and leaves the file alone
    let again = std::process::Command::new(FLINT)
        .arg("validate")
        .arg(&scene)
        .arg("--fix")
        .arg("--schemas")
        .arg(&engine)
        .arg("--schemas")
        .arg(engine.join("examples"))
        .arg("--schemas")
        .arg(dir.join("schemas"))
        .output()
        .expect("run flint");
    assert!(again.status.success());
    assert_eq!(std::fs::read_to_string(&scene).unwrap(), after);
}

const BRIGHT_PORCH: &str = r#"[scene]
name = "Bright porch"

# The porch light, too bright in its template
[prefabs.porch]
template = "glare"
prefix = "porch"
"#;

const GLARE: &str = r#"[prefab]
name = "glare"

[entities.bulb]
archetype = "bulb"

[entities.bulb.light]
intensity = 50.0
"#;

const BULB: &str = r#"[archetype.bulb]
description = "A light on its own"
components = ["light"]
"#;

const DIM: &str = r#"
[[constraint]]
name = "bulbs_not_blinding"
query = "entities where archetype == 'bulb'"
severity = "warning"
message = "Bulb '{name}' is blinding"

[constraint.kind]
type = "value_range"
field = "light.intensity"
min = 0.0
max = 10.0

[constraint.auto_fix]
enabled = true
strategy = "set_default"
field = "light.intensity"
value = 8.0
"#;

/// #1046: a fix to an entity a prefab instance makes is written into the
/// instance's overrides, as the viewer saves one (#679), not refused
#[test]
fn fix_to_a_prefab_entity_writes_the_instance_overrides() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("validate_fix_cli")
        .join("prefab");
    let _ = std::fs::remove_dir_all(&dir);
    for sub in ["schemas/constraints", "schemas/archetypes", "prefabs"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    std::fs::write(dir.join("schemas/constraints/dim.toml"), DIM).unwrap();
    std::fs::write(dir.join("schemas/archetypes/bulb.toml"), BULB).unwrap();
    std::fs::write(dir.join("prefabs/glare.prefab.toml"), GLARE).unwrap();
    let scene = dir.join("porch.scene.toml");
    std::fs::write(&scene, BRIGHT_PORCH).unwrap();

    // A --schemas flag is the whole list (ADR 0114): the engine's set (for
    // `light`), then the project's archetype and constraint
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let run = || {
        std::process::Command::new(FLINT)
            .arg("validate")
            .arg(&scene)
            .arg("--fix")
            .arg("--schemas")
            .arg(&engine)
            .arg("--schemas")
            .arg(dir.join("schemas"))
            .output()
            .expect("run flint")
    };
    let o = run();
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(o.status.success(), "{out}");
    assert!(!out.contains("not written"), "{out}");

    let after = std::fs::read_to_string(&scene).unwrap();
    // The file keeps what it said, and gains the override
    assert!(after.starts_with(BRIGHT_PORCH), "{after}");
    let file: flint_scene::SceneFile = toml::from_str(&after).unwrap();
    assert_eq!(
        file.prefabs["porch"].overrides["bulb"]["light"]["intensity"],
        toml::Value::Float(8.0),
        "{after}"
    );
    // The bulb is still the prefab's, not baked into [entities]
    assert!(!after.contains("[entities"), "{after}");

    // Read back, the fix holds: a second run has nothing to fix
    let again = run();
    assert!(again.status.success());
    assert_eq!(std::fs::read_to_string(&scene).unwrap(), after);
}

const TWO_PORCHES: &str = r#"[scene]
name = "Two porches"

# Both glare: `porch_back_bulb` could be either's
[prefabs.porch]
template = "glare"
prefix = "porch"

[prefabs.back]
template = "glare"
prefix = "porch_back"
"#;

/// #1116: a fix the file can't take is no fix. `porch_back_bulb` could be
/// `[prefabs.porch]`'s `back_bulb` or `[prefabs.back]`'s `bulb`, so its
/// override has no home: the run says how many fixes it dropped, on its
/// last line, fails, and the next run still finds the bulb blinding.
#[test]
fn a_fix_the_file_cannot_take_fails_the_run_and_is_counted() {
    let dir = std::env::temp_dir().join(format!("flint-1116-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for sub in ["schemas/constraints", "schemas/archetypes", "prefabs"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    std::fs::write(dir.join("schemas/constraints/dim.toml"), DIM).unwrap();
    std::fs::write(dir.join("schemas/archetypes/bulb.toml"), BULB).unwrap();
    std::fs::write(dir.join("prefabs/glare.prefab.toml"), GLARE).unwrap();
    let scene = dir.join("porches.scene.toml");
    std::fs::write(&scene, TWO_PORCHES).unwrap();

    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let run = |extra: &[&str]| {
        std::process::Command::new(FLINT)
            .arg("validate")
            .arg(&scene)
            .args(extra)
            .arg("--schemas")
            .arg(&engine)
            .arg("--schemas")
            .arg(dir.join("schemas"))
            .output()
            .expect("run flint")
    };
    let o = run(&["--fix"]);
    let stderr = String::from_utf8_lossy(&o.stderr).to_string();
    let out = format!("{}{stderr}", String::from_utf8_lossy(&o.stdout));
    assert!(!o.status.success(), "a dropped fix exited 0:\n{out}");
    assert!(out.contains("not written: 'porch_back_bulb'"), "{out}");
    let last = stderr.lines().rev().find(|l| !l.trim().is_empty()).unwrap();
    assert_eq!(
        last,
        format!(
            "1 of 2 fix(es) not written to {}, which still has what they fixed: porch_back_bulb",
            scene.display()
        ),
        "{out}"
    );
    // The bulb that has a home is written, and the report is the file's:
    // the other is still blinding there
    let after = std::fs::read_to_string(&scene).unwrap();
    let file: flint_scene::SceneFile = toml::from_str(&after).unwrap();
    assert_eq!(
        file.prefabs["porch"].overrides["bulb"]["light"]["intensity"],
        toml::Value::Float(8.0),
        "{after}"
    );
    assert!(out.contains("1 violation(s) remain after fixes"), "{out}");
    assert!(
        out.contains("porch_back_bulb: Bulb 'porch_back_bulb' is blinding"),
        "{out}"
    );

    // The JSON names the dropped fix, and fails the same way
    std::fs::write(&scene, TWO_PORCHES).unwrap();
    let o = run(&["--fix", "--format", "json"]);
    assert!(!o.status.success());
    let doc: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let dropped = doc["fix"]["dropped"].as_array().unwrap();
    assert_eq!(dropped.len(), 1, "{doc}");
    assert_eq!(dropped[0]["entity"], "porch_back_bulb");
    assert_eq!(doc["fix"]["not_written"].as_array().unwrap().len(), 1);

    // Read back, the bulb is still what the constraint fixes
    let again = run(&[]);
    let text = String::from_utf8_lossy(&again.stdout);
    assert!(
        text.contains("porch_back_bulb") && text.contains("[fixable]"),
        "{text}"
    );
    assert!(!text.contains("Bulb 'porch_bulb'"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}
