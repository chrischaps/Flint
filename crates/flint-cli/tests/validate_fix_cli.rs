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

const CONSTRAINTS: &str = r#"
[[constraint]]
name = "door_angle_range"
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
    std::fs::create_dir_all(dir.join("prefabs")).unwrap();
    std::fs::write(dir.join("schemas/constraints/fix.toml"), CONSTRAINTS).unwrap();
    std::fs::write(dir.join("prefabs/lamp.prefab.toml"), LAMP).unwrap();
    std::fs::write(dir.join("porch.scene.toml"), SCENE).unwrap();
    dir
}

#[test]
fn fix_keeps_every_table_the_comments_and_the_name() {
    let dir = project("keeps");
    let scene = dir.join("porch.scene.toml");
    let o = std::process::Command::new(FLINT)
        .arg("validate")
        .arg(&scene)
        .arg("--fix")
        .arg("--output-diff")
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
        .arg(dir.join("schemas"))
        .output()
        .expect("run flint");
    assert!(again.status.success());
    assert_eq!(std::fs::read_to_string(&scene).unwrap(), after);
}
