//! The player reads the asset catalog from the project's root, as `flint
//! validate` and `flint check` do (#1065), not from the working directory.
//!
//! The files are a `MemorySource`, whose root is the working directory: the
//! game is under `game/`, so the player runs from outside it, as `flint
//! play game/scenes/yard.scene.toml` from an engine checkout does. The
//! game's model is only in its catalog, and the working directory has a
//! catalog of its own (the engine's `assets/`, say) naming the same model
//! with other content. Headless: the model's triangles become the ramp's
//! mesh collider, which tells the two models apart without a GPU.

use super::scene_entry::SceneEntry;
use super::scene_loading::build_model_load_config;
use super::PlayerApp;
use flint_asset::HeldAssets;
use flint_core::project::{Project, SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use flint_ecs::FlintWorld;
use flint_physics::ColliderExtents;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SCENE: &str = "game/scenes/yard.scene.toml";

const YARD: &str = r#"
[scene]
name = "Yard"

[entities.ramp]
[entities.ramp.transform]
position = [0.0, 0.0, 0.0]
scale = [2.0, 2.0, 2.0]
[entities.ramp.model]
asset = "steps"
[entities.ramp.rigidbody]
body_type = "static"
[entities.ramp.collider]
shape = "mesh"
"#;

/// The game's `steps`, as its catalog registers it.
const GAME_HASH: &str = "ab12000000000000000000000000000000000000000000000000000000000000";
/// The working directory's `steps`, which the player must not read.
const DECOY_HASH: &str = "cd34000000000000000000000000000000000000000000000000000000000000";

/// A GLB of one unit triangle pair per entry of `heights`, each a node
/// raised that far: two nodes (0 and 3) are the game's steps, one is the
/// decoy's flat slab.
fn steps_glb(heights: &[f32]) -> Vec<u8> {
    let positions: [[f32; 3]; 4] = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 1.0],
    ];
    let mut bin: Vec<u8> = positions
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .chain([0u16, 1, 2, 1, 3, 2].iter().flat_map(|i| i.to_le_bytes()))
        .collect();
    let nodes: Vec<String> = heights
        .iter()
        .map(|y| format!(r#"{{ "mesh": 0, "translation": [0, {y}, 0] }}"#))
        .collect();
    let indices: Vec<String> = (0..heights.len()).map(|i| i.to_string()).collect();
    let mut json = format!(
        r#"{{
        "asset": {{ "version": "2.0" }},
        "scene": 0,
        "scenes": [{{ "nodes": [{}] }}],
        "nodes": [{}],
        "meshes": [{{ "primitives": [{{ "attributes": {{ "POSITION": 0 }}, "indices": 1 }}] }}],
        "buffers": [{{ "byteLength": 60 }}],
        "bufferViews": [
            {{ "buffer": 0, "byteLength": 48 }},
            {{ "buffer": 0, "byteOffset": 48, "byteLength": 12 }}
        ],
        "accessors": [
            {{ "bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3",
               "min": [0.0, 0.0, 0.0], "max": [1.0, 0.0, 1.0] }},
            {{ "bufferView": 1, "componentType": 5123, "count": 6, "type": "SCALAR" }}
        ]
    }}"#,
        indices.join(", "),
        nodes.join(", ")
    )
    .into_bytes();
    // Each chunk is padded to four bytes: the JSON with spaces, the blob with 0
    json.resize(json.len().next_multiple_of(4), b' ');
    bin.resize(bin.len().next_multiple_of(4), 0);

    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut glb = Vec::with_capacity(total);
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total as u32).to_le_bytes());
    for (kind, chunk) in [(b"JSON", &json), (b"BIN\0", &bin)] {
        glb.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        glb.extend_from_slice(kind);
        glb.extend_from_slice(chunk);
    }
    glb
}

fn sidecar(hash: &str) -> String {
    format!(
        "[asset]\nname = \"steps\"\ntype = \"mesh\"\nhash = \"sha256:{hash}\"\n\
         source_path = \"steps.glb\"\n"
    )
}

/// A catalog and store under `root` (`""` for the working directory)
/// registering `steps` as `hash`, its content `glb`.
fn catalogued(files: &mut MemorySource, root: &str, hash: &str, glb: Vec<u8>) {
    files.insert(format!("{root}assets/steps.asset.toml"), sidecar(hash));
    files.insert(
        format!("{root}.flint/assets/{}/{hash}.glb", &hash[..2]),
        glb,
    );
}

/// The game under `game/` with `steps` in its catalog alone (no
/// `models/steps.glb`), and the working directory's own catalog beside it.
fn yard() -> MemorySource {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "[project]\nid = \"yard\"\n");
    files.insert(SCENE, YARD);
    catalogued(&mut files, "game/", GAME_HASH, steps_glb(&[0.0, 3.0]));
    catalogued(&mut files, "", DECOY_HASH, steps_glb(&[0.0]));
    files
}

fn started() -> PlayerApp {
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(SCENE, &registry).expect("the yard loads");
    let mut app = PlayerApp::new(world, SCENE.into(), false, None, None);
    app.apply_scene_settings(&file);
    app.set_schemas(
        registry,
        SchemaDirs {
            dirs: Vec::new(),
            source: SchemaSource::Flags,
            dropped: Vec::new(),
            packages: None,
        },
    );
    app.enter_scene(SceneEntry::Start).expect("the yard starts");
    app
}

#[test]
fn a_model_only_in_the_projects_catalog_loads_when_run_from_outside_the_project() {
    vfs::scoped(Arc::new(yard()), || {
        let app = started();
        assert_eq!(app.project().root, Path::new("game"));
        let ramp = app.core.world.get_id("ramp").unwrap();
        // The game's two steps at scale 2: x and z 0..2, y 0..6. The
        // working directory's slab would be y 0..0.
        match app.core.physics.get_entity_collider_extents(ramp) {
            Some(ColliderExtents::Mesh {
                half_extents,
                centre,
            }) => {
                assert_eq!(half_extents, [1.0, 3.0, 1.0], "the game's model");
                assert_eq!(centre, [1.0, 3.0, 1.0]);
            }
            other => panic!("the ramp's collider is its catalogued model's: {other:?}"),
        }
    });
}

#[test]
fn the_catalog_path_is_the_projects_store_and_validate_agrees() {
    vfs::scoped(Arc::new(yard()), || {
        let project = Project::for_scene(Path::new(SCENE));
        let mut app = started();
        let (world, _) =
            flint_scene::load_scene(SCENE, &flint_schema::SchemaRegistry::new()).unwrap();
        let config = build_model_load_config(&project, SCENE, &world, app.assets.of(&project));
        let stored = PathBuf::from(format!("game/.flint/assets/ab/{GAME_HASH}.glb"));
        assert_eq!(config.overrides.get("steps"), Some(&stored));
        assert_eq!(config.asked_first[0], "the asset catalog");
        // The one check `flint validate` and `flint check` ask (#804)
        assert_eq!(
            HeldAssets::of(&project).and_then(|held| held.path_of("steps")),
            Some(stored)
        );
    });
}

#[test]
fn a_project_with_no_catalog_asks_none_even_beside_one() {
    let mut files = yard();
    files.insert("other/flint.toml", "");
    files.insert("other/a.scene.toml", "");
    vfs::scoped(Arc::new(files), || {
        let project = Project::for_scene(Path::new("other/a.scene.toml"));
        let mut sources = super::state::AssetSources::default();
        assert!(
            sources.of(&project).is_none(),
            "the working directory's isn't it"
        );
        let config = build_model_load_config(
            &project,
            "other/a.scene.toml",
            &FlintWorld::new(),
            sources.of(&project),
        );
        assert!(!config.asked_first.contains(&"the asset catalog"));
        // And the game's, asked next, is read for its own root
        let game = Project::for_scene(Path::new(SCENE));
        assert!(sources.of(&game).is_some_and(|held| held.holds("steps")));
    });
}
