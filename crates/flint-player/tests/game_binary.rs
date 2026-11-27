//! A game's own binary holds its project's list to its build (#1532): a
//! `PlayerApp` whose launcher lists no package is told to run `flint build`
//! for a project listing `living`, whether the game handed over the
//! resolver's answer, a plain list or nothing. One test: one launch a run.

use flint_player::PlayerApp;
use std::path::{Path, PathBuf};

/// A project at `dir` on the engine's schemas with `packages`, and its scene.
fn project(dir: &Path, packages: &str) -> PathBuf {
    let engine = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let engine = engine.to_string_lossy().replace('\\', "/");
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    let manifest = format!("[project]\nschemas = [\"{engine}\"]\n{packages}\n");
    std::fs::write(dir.join("flint.toml"), manifest).unwrap();
    let scene = dir.join("scenes/main.scene.toml");
    std::fs::write(&scene, "[scene]\nname = \"Game\"\n").unwrap();
    scene
}

fn app(scene: &Path) -> PlayerApp {
    let path = scene.to_string_lossy().into_owned();
    PlayerApp::new(flint_ecs::FlintWorld::new(), path, false, None, None)
}

#[test]
fn a_game_binary_without_a_listed_package_names_flint_build() {
    assert!(flint_player::packages::launch(&[]));
    let dir = std::env::temp_dir().join(format!("flint-game-binary-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // living brings procgen; the debug HUD, standard, is left out
    let listed = "[packages]\nliving = \"*\"\ndebug-hud = false";
    let scene = project(&dir.join("listed"), listed);

    // The resolver's answer handed over, as cave_game does
    let (registry, dirs) =
        flint_schema::SchemaRegistry::resolve(Some(&scene), &[] as &[&str]).unwrap();
    let mut player = app(&scene);
    player.set_schemas(registry, dirs);
    let unbuilt = player.unbuilt_packages().expect("lacked");
    assert_eq!(unbuilt.packages, ["living", "procgen"]);
    let said = unbuilt.to_string();
    assert!(
        said.contains("built without the `living` and `procgen` packages"),
        "{said}"
    );
    assert!(said.contains("run `flint build`") && said.contains("game's own binary"));

    // A plain list (the book's game), and nothing: the scene's project says
    let mut player = app(&scene);
    player.set_schema_paths(vec!["schemas".into()]);
    assert_eq!(player.unbuilt_packages(), Some(unbuilt.clone()));
    assert_eq!(app(&scene).unbuilt_packages(), Some(unbuilt));

    // The kernel alone, and no list
    let kernel = project(
        &dir.join("kernel"),
        "[packages]\ndebug-hud = false\nprocgen = false",
    );
    assert_eq!(app(&kernel).unbuilt_packages(), None);
    assert_eq!(
        app(&project(&dir.join("none"), "")).unbuilt_packages(),
        None
    );
    let _ = std::fs::remove_dir_all(&dir);
}
