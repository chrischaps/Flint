//! `flint bundle <scene>` (#382): one scene packed alone with what it
//! reaches, from its project, by the real binary over a fixture project.
//!
//! The scene names a model, a prefab (which names another model) and a
//! script; the script names a sound and a second scene, which names a third
//! model. A third scene, a model and a sound nobody reaches sit beside them
//! and must stay out. The schemas are the engine's own, named by an
//! absolute path as `flint init --engine path` writes them, so they go in
//! at `engine/schemas`.

use flint_core::bundle::Bundle;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FLINT: &str = env!("CARGO_BIN_EXE_flint");

fn engine_schemas() -> String {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    std::fs::canonicalize(dir)
        .unwrap()
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('\\', "/")
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn project(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("flint-bundle-scene-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    write(
        &dir,
        "flint.toml",
        &format!(
            "[project]\nid = \"meadows\"\nschemas = [\"{}\", \"schemas\"]\n",
            engine_schemas()
        ),
    );
    write(&dir, "schemas/components/.gitkeep", "");
    write(
        &dir,
        "levels/meadow.scene.toml",
        r#"[scene]
name = "Meadow"
[entities.walker]
archetype = "character"
[entities.walker.transform]
position = [0, 0, 0]
[entities.bench.model]
asset = "bench"
[entities.gate.script]
source = "gate.rhai"
[prefabs.lamp]
template = "lamp"
prefix = "lamp"
"#,
    );
    write(
        &dir,
        "scripts/gate.rhai",
        "import \"doors\" as doors;\nfn on_interact() {\n    play_sound(\"creak.ogg\");\n    \
         load_scene(\"levels/cellar.scene.toml\");\n}\n",
    );
    write(
        &dir,
        "levels/cellar.scene.toml",
        "[scene]\nname = \"Cellar\"\n[entities.barrel.model]\nasset = \"barrel\"\n",
    );
    // The prefab names its effect by a bare name the check doesn't follow,
    // so effects go in as the scene's search sees them, with what they name
    write(
        &dir,
        "prefabs/lamp.prefab.toml",
        "[prefab]\nname = \"lamp\"\n[entities.post.model]\nasset = \"lamp_post\"\n\
         [entities.post.particle_effect]\neffect = \"flame\"\n",
    );
    write(
        &dir,
        "particles/flame.particles.toml",
        "[effect]\nname = \"flame\"\n[emitters.core]\ntexture = \"spark.png\"\n",
    );
    write(&dir, "particles/spark.png", "png");
    write(
        &dir,
        "animations/bob.anim.toml",
        "[animation]\nname = \"bob\"\nduration = 1.0\n",
    );
    for model in ["bench", "lamp_post", "unused"] {
        write(&dir, &format!("models/{model}.glb"), "glb");
    }
    // A glTF keeps its texture outside it, as the tavern's models do
    write(
        &dir,
        "models/barrel.gltf",
        r#"{"asset": {"version": "2.0"}, "images": [{"uri": "../textures/oak%20staves.png"}]}"#,
    );
    write(&dir, "textures/oak staves.png", "png");
    write(&dir, "textures/unused.png", "png");
    // A module the script imports: scripts/lib/
    write(&dir, "scripts/lib/doors.rhai", "fn creak() { 1 }\n");
    write(&dir, "audio/creak.ogg", "ogg");
    write(&dir, "audio/unused.ogg", "ogg");
    write(
        &dir,
        "levels/other.scene.toml",
        "[scene]\nname = \"Other\"\n[entities.x.model]\nasset = \"unused\"\n",
    );
    dir
}

fn flint(dir: &Path, args: &[&str]) -> Output {
    Command::new(FLINT)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run flint")
}

fn ok(o: &Output) -> String {
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(
        o.status.success(),
        "flint failed\nstdout:\n{out}\nstderr:\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
    out
}

fn packed(path: &Path) -> (Bundle, BTreeSet<String>) {
    let bundle = Bundle::parse(std::fs::read(path).unwrap()).unwrap();
    let files = bundle.files().map(|(p, _)| p.to_string()).collect();
    (bundle, files)
}

#[test]
fn a_scene_packs_alone_with_what_it_reaches() {
    let dir = project("reach");
    ok(&flint(&dir, &["bundle", "levels/meadow.scene.toml"]));

    // Beside the scene, named for it
    let (main, files) = packed(&dir.join("levels/dist/meadow.flintpack"));
    assert_eq!(main.manifest.name, "meadow");
    assert_eq!(
        main.manifest.scene.as_deref(),
        Some("levels/meadow.scene.toml")
    );
    assert_eq!(main.manifest.schemas, ["engine/schemas"]);
    for reached in [
        "flint.toml",
        "levels/meadow.scene.toml",
        "models/bench.glb",
        "scripts/gate.rhai",
        "prefabs/lamp.prefab.toml",
        "models/lamp_post.glb",
        "levels/cellar.scene.toml",
        "models/barrel.gltf",
        "textures/oak staves.png",
        "particles/flame.particles.toml",
        "particles/spark.png",
        "scripts/lib/doors.rhai",
        "animations/bob.anim.toml",
        "engine/schemas/components/transform.toml",
        "engine/schemas/archetypes/character.toml",
    ] {
        assert!(files.contains(reached), "{reached} not packed: {files:?}");
    }
    for unreached in [
        "models/unused.glb",
        "textures/unused.png",
        "levels/other.scene.toml",
    ] {
        assert!(!files.contains(unreached), "{unreached} packed: {files:?}");
    }
    // An empty schema directory has nothing to list
    assert!(!main.manifest.schemas.contains(&"schemas".to_string()));

    // The sound the script names, in the late part, and only that one
    let (_, audio) = packed(&dir.join("levels/dist/meadow.audio.flintpack"));
    assert_eq!(audio, BTreeSet::from(["audio/creak.ogg".to_string()]));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_scene_pack_takes_a_name_and_an_out_dir_and_refuses_scene() {
    let dir = project("flags");
    let said = ok(&flint(
        &dir,
        &[
            "bundle",
            "levels/cellar.scene.toml",
            "--out-dir",
            "web",
            "--name",
            "cellar_only",
            "--list",
        ],
    ));
    assert!(said.contains("models/barrel.gltf"), "{said}");
    assert!(!said.contains("models/bench.glb"), "{said}");
    let (main, _) = packed(&dir.join("web/cellar_only.flintpack"));
    assert_eq!(
        main.manifest.scene.as_deref(),
        Some("levels/cellar.scene.toml")
    );

    let o = flint(
        &dir,
        &[
            "bundle",
            "levels/cellar.scene.toml",
            "--scene",
            "x.scene.toml",
        ],
    );
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("is the scene already"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_scene_naming_a_missing_file_writes_nothing() {
    let dir = project("missing");
    // The texture a model reached two scenes away names
    std::fs::remove_file(dir.join("textures/oak staves.png")).unwrap();
    let o = flint(&dir, &["bundle", "levels/meadow.scene.toml"]);
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("the model names `../textures/oak staves.png`"),
        "{err}"
    );
    assert!(!dir.join("levels/dist").exists());
    let _ = std::fs::remove_dir_all(&dir);
}
