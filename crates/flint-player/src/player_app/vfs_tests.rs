//! A game read entirely from memory (#97): its schemas, its scene, a prefab
//! found one directory up from the scene, and a script, all from a
//! `MemorySource` and none from the disk, the way the web player will run.
//! Headless: no window.

use super::PlayerApp;
use flint_core::vfs::{self, MemorySource};
use flint_schema::SchemaRegistry;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The engine's own schemas under `engine/schemas/`, plus a small game under
/// `bundle/` that exists nowhere on disk.
fn bundle() -> MemorySource {
    let mut src = MemorySource::new();
    let schemas = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    for kind in ["components", "archetypes"] {
        for entry in std::fs::read_dir(schemas.join(kind)).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            src.insert(
                format!("engine/schemas/{kind}/{name}"),
                std::fs::read(&path).unwrap(),
            );
        }
    }
    src.insert(
        "bundle/levels/porch.scene.toml",
        r#"
[scene]
name = "porch"

[prefabs.lamp]
template = "lamp"
prefix = "porch"

[entities.keeper]
[entities.keeper.transform]
position = [0.0, 0.0, 0.0]
[entities.keeper.script]
source = "keeper.rhai"
"#,
    );
    src.insert(
        "bundle/prefabs/lamp.prefab.toml",
        r#"
[prefab]
name = "lamp"

[entities.bulb]
[entities.bulb.transform]
position = [0.0, 2.0, 0.0]
"#,
    );
    src.insert(
        "bundle/scripts/keeper.rhai",
        "fn on_update() { record(entity_name(self_entity())); }",
    );
    src
}

#[test]
fn a_scene_its_schemas_and_a_script_load_from_memory() {
    assert!(!Path::new("bundle/levels/porch.scene.toml").exists());

    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    vfs::scoped(Arc::new(bundle()), || {
        let registry = SchemaRegistry::load_from_directories(&["engine/schemas"]).unwrap();
        assert!(registry.get_component("transform").is_some());
        assert!(registry.get_component("script").is_some());

        let scene_path = "bundle/levels/porch.scene.toml";
        let (world, _) = flint_scene::load_scene(scene_path, &registry).unwrap();
        assert!(
            world.get_id("porch_bulb").is_some(),
            "the prefab, found at levels/../prefabs, expanded"
        );

        let mut app = PlayerApp::new(world, scene_path.into(), false, None, None);
        {
            let seen = seen.clone();
            app.script
                .rhai_engine_mut()
                .register_fn("record", move |s: String| seen.lock().unwrap().push(s));
        }
        app.script.load_scripts_from_scene(scene_path);
        app.tick();
        app.tick();
    });

    assert_eq!(*seen.lock().unwrap(), ["keeper", "keeper"]);
}
