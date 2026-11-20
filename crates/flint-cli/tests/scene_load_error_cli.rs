//! A scene that fails to load is named in the failure (#1044). Every
//! command that loads a scene is run on one whose two entities are each
//! the other's parent: the load's own error names the entities, never the
//! file, so with a folder of scenes it is the command's line that says
//! which one to open. Each must print `Failed to load scene
//! levels/cart.scene.toml`, and the cause under it.
//!
//! The window commands (`play`, `spline-edit`, `edit` without `--render`)
//! fail at the load, before any window opens, so they run here too, as
//! `render` does before it asks for a GPU.

use std::path::{Path, PathBuf};
use std::process::Command;

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

/// A parent cycle: what `flint_scene` refuses to load (#680).
const CART: &str = "\
[scene]
name = \"Cart\"

[entities.cart]
parent = \"wheel\"

[entities.wheel]
parent = \"cart\"
";

/// Where the scene is, from the project root: what each message must name.
const SCENE: &str = "levels/cart.scene.toml";

/// A project of one broken scene under the test's own temp directory.
fn project() -> PathBuf {
    let root = std::env::temp_dir()
        .join("flint-scene-load-error-cli")
        .join(std::process::id().to_string());
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("levels")).unwrap();
    std::fs::write(root.join("flint.toml"), "[project]\nid = \"cart\"\n").unwrap();
    std::fs::write(root.join(SCENE), CART).unwrap();
    root
}

/// `flint <args>` in `root`: it must fail, and what it said (stderr, then
/// stdout).
fn failed(root: &Path, args: &[&str]) -> String {
    let out = Command::new(FLINT)
        .args(args)
        .current_dir(root)
        .output()
        .expect("run flint");
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        !out.status.success(),
        "flint {args:?} should have failed:\n{said}"
    );
    said
}

#[test]
fn every_command_names_the_scene_that_failed_to_load() {
    let root = project();
    let commands: &[&[&str]] = &[
        &["render", SCENE, "-o", "cart.png"],
        &["play", SCENE, "--exit-after", "1"],
        &["playtest", SCENE, "--timeout", "1"],
        &["edit", SCENE, "--render", "edit.png"],
        &["spline-edit", SCENE],
        &["entity", "list", "--scene", SCENE],
        &["entity", "show", "cart", "--scene", SCENE],
        &["entity", "delete", "ghost", "--scene", SCENE],
        &["query", "entities", "--scene", SCENE],
    ];
    let mut unnamed = Vec::new();
    for args in commands {
        let said = failed(&root, args);
        // The cause still comes with it
        assert!(said.contains("Parent cycle"), "flint {args:?}:\n{said}");
        if !said.contains(&format!("Failed to load scene {SCENE}")) {
            unnamed.push(format!("flint {}:\n{said}", args.join(" ")));
        }
    }
    assert!(
        unnamed.is_empty(),
        "these didn't name {SCENE}:\n\n{}",
        unnamed.join("\n")
    );
    let _ = std::fs::remove_dir_all(&root);
}
