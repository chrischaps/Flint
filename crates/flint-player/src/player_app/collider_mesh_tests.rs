//! A model entity gets a mesh collider from TOML alone (#729): the scene
//! says `shape = "mesh"` beside its `model`, and the player reads the
//! model's triangles and hands them to physics before the bodies are made,
//! with the project's collision layers. The model has two nodes, so a
//! renderer (when there is a GPU) expands it into children and takes the
//! parent's `model` away; the triangles are asked for before that.

use super::scene_entry::SceneEntry;
use super::PlayerApp;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use flint_physics::ColliderExtents;
use flint_render::{HeadlessContext, RendererConfig, SceneRenderer};
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
layer = "world"

[entities.crate]
[entities.crate.transform]
position = [10.0, 0.0, 0.0]
[entities.crate.rigidbody]
body_type = "static"
[entities.crate.collider]
shape = "convex"
model = "steps"
"#;

/// Two nodes, each a unit triangle: one at the origin, one 3 up. Expanded,
/// they are two child entities; as a collider, both triangles.
fn steps_gltf() -> (String, Vec<u8>) {
    let positions: [[f32; 3]; 4] = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 1.0],
    ];
    let bin: Vec<u8> = positions
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .chain([0u16, 1, 2, 1, 3, 2].iter().flat_map(|i| i.to_le_bytes()))
        .collect();
    let gltf = r#"{
        "asset": { "version": "2.0" },
        "scene": 0,
        "scenes": [{ "nodes": [0, 1] }],
        "nodes": [
            { "name": "low", "mesh": 0 },
            { "name": "high", "mesh": 0, "translation": [0, 3, 0] }
        ],
        "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 }, "indices": 1 }] }],
        "buffers": [{ "uri": "steps.bin", "byteLength": 60 }],
        "bufferViews": [
            { "buffer": 0, "byteLength": 48 },
            { "buffer": 0, "byteOffset": 48, "byteLength": 12 }
        ],
        "accessors": [
            { "bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3",
              "min": [0.0, 0.0, 0.0], "max": [1.0, 0.0, 1.0] },
            { "bufferView": 1, "componentType": 5123, "count": 6, "type": "SCALAR" }
        ]
    }"#;
    (gltf.to_string(), bin)
}

fn yard() -> MemorySource {
    let (gltf, bin) = steps_gltf();
    let mut files = MemorySource::new();
    files.insert(
        "game/flint.toml",
        "[project]\nid = \"yard\"\n\n[project.physics]\nlayers = [\"world\", \"ghost\"]\n",
    );
    files.insert(SCENE, YARD);
    files.insert("game/models/steps.gltf", gltf);
    files.insert("game/models/steps.bin", bin);
    files
}

fn started(gpu: Option<&'static HeadlessContext>) -> PlayerApp {
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
        },
    );
    if let Some(gpu) = gpu {
        app.scene_renderer = Some(SceneRenderer::new_headless(
            &gpu.device,
            &gpu.queue,
            gpu.format,
            64,
            64,
            RendererConfig::default(),
        ));
        app.test_gpu = Some(gpu);
    }
    app.enter_scene(SceneEntry::Start).expect("the yard starts");
    app
}

fn check(app: &PlayerApp) {
    let physics = &app.core.physics;
    assert_eq!(
        physics.layers().names().collect::<Vec<_>>(),
        ["default", "world", "ghost"],
        "the project's layers"
    );
    let ramp = app.core.world.get_id("ramp").unwrap();
    // Both nodes' triangles, at the entity's scale 2: x and z 0..2, y 0..6
    match physics.get_entity_collider_extents(ramp) {
        Some(ColliderExtents::Mesh {
            half_extents,
            centre,
        }) => {
            assert_eq!(half_extents, [1.0, 3.0, 1.0]);
            assert_eq!(centre, [1.0, 3.0, 1.0]);
        }
        other => panic!("the ramp's collider is its model's triangles: {other:?}"),
    }
    let crate_ = app.core.world.get_id("crate").unwrap();
    assert!(
        matches!(
            physics.get_entity_collider_extents(crate_),
            Some(ColliderExtents::Convex { .. })
        ),
        "`collider.model` names a model for an entity that draws none"
    );
    // A ray down onto the upper step finds it at y = 6
    let hit = physics
        .raycast([1.0, 20.0, 1.0], [0.0, -1.0, 0.0], 50.0, None)
        .unwrap();
    assert_eq!(hit.entity_id, ramp);
    assert!((hit.point[1] - 6.0).abs() < 1e-3, "{:?}", hit.point);
    let world_only = physics.layer_mask(&["ghost"]).unwrap();
    assert!(physics
        .raycast_masked(
            [1.0, 20.0, 1.0],
            [0.0, -1.0, 0.0],
            50.0,
            None,
            Some(world_only)
        )
        .is_none());
}

#[test]
fn a_model_entity_gets_a_mesh_collider_from_toml_alone() {
    vfs::scoped(Arc::new(yard()), || {
        let mut app = started(None);
        // The query pipeline is filled by the first step
        app.core
            .physics
            .fixed_update(
                &mut app.core.world,
                1.0 / 60.0,
                &mut flint_runtime::EventBus::new(),
            )
            .unwrap();
        check(&app);
    });
}

#[test]
fn a_mesh_collider_keeps_its_triangles_when_the_renderer_expands_its_model() {
    let Some(gpu) = super::scene_entry_tests::gpu() else {
        flint_render::gpu_select::skip_without_gpu("mesh collider expansion test");
        return;
    };
    vfs::scoped(Arc::new(yard()), || {
        let mut app = started(Some(gpu));
        let ramp = app.core.world.get_id("ramp").unwrap();
        assert!(
            app.core.world.get_component(ramp, "model").is_none(),
            "the renderer expanded the two-node model into children"
        );
        app.core
            .physics
            .fixed_update(
                &mut app.core.world,
                1.0 / 60.0,
                &mut flint_runtime::EventBus::new(),
            )
            .unwrap();
        check(&app);
    });
}
