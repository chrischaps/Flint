//! A `mesh` or `convex` collider on a model with no file (#1222): one a
//! procgen spec makes at load or on the queue, and one a game uploads at
//! runtime. Each takes the triangles it is drawn with, in the player's own
//! frames, on a GPU (procgen resolves into the renderer, and a runtime mesh
//! is uploaded to it). Skipped without one.

use super::scene_entry::SceneEntry;
use super::state::ProcGenMode;
use super::PlayerApp;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use flint_physics::ColliderExtents;
use flint_render::{HeadlessContext, RendererConfig, SceneRenderer};
use std::sync::{Arc, Mutex};

const ROCKS: &str = "game/scenes/rocks.scene.toml";
const HUT: &str = "game/scenes/hut.scene.toml";

/// The mock generator's unit cube, drawn 4 m wide and 1 m tall: its top
/// is y = 0.5. The collider's `size` is a 0.2 m box (0.8 m wide at this
/// scale), so a ball 1.5 m out lands on the rock only if the collider is
/// the drawn hull.
const ROCKS_SCENE: &str = r#"
[scene]
name = "Rocks"

[entities.rock]
[entities.rock.transform]
scale = [4.0, 1.0, 4.0]
[entities.rock.model]
asset = "rock"
[entities.rock.rigidbody]
body_type = "static"
[entities.rock.collider]
shape = "convex"
size = [0.2, 0.2, 0.2]

[entities.ball]
[entities.ball.transform]
position = [1.5, 8.0, 0.0]
[entities.ball.rigidbody]
body_type = "dynamic"
[entities.ball.collider]
shape = "sphere"
size = [1.0, 1.0, 1.0]
"#;

/// A runtime mesh drawn at 2x, and a script casting `raycast()` down at
/// x = z = 1.5: inside the mesh (4 m wide drawn), outside the 1 m box of
/// `size` (2 m drawn) it would be without its triangles.
const HUT_SCENE: &str = r#"
[scene]
name = "Hut"

[entities.hut]
[entities.hut.transform]
scale = [2.0, 2.0, 2.0]
[entities.hut.model]
asset = "hut"
runtime = true
[entities.hut.rigidbody]
body_type = "static"
[entities.hut.collider]
shape = "mesh"

[entities.probe]
[entities.probe.script]
source = "probe.rhai"
"#;

const PROBE: &str = r#"
fn on_update() {
    let hit = raycast(1.5, 10.0, 1.5, 0.0, -1.0, 0.0, 50.0);
    if type_of(hit) == "()" {
        record("none");
    } else {
        record(`${entity_name(hit.entity)} ${hit.point_y}`);
    }
}
"#;

fn files() -> MemorySource {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "[project]\nid = \"rocks\"\n");
    files.insert(
        "game/specs/rock.procgen.toml",
        "generator = \"mock\"\n[meta]\nname = \"rock\"\nversion = \"1.0.0\"\n\
         [seed]\nmode = \"fixed\"\nvalue = 7\n",
    );
    files.insert(ROCKS, ROCKS_SCENE);
    files.insert(HUT, HUT_SCENE);
    files.insert("game/scripts/probe.rhai", PROBE);
    files
}

type Log = Arc<Mutex<Vec<String>>>;

/// The scene started on the GPU, stepped 1/60 a frame, with `record(s)`
/// for its scripts.
fn started(gpu: &'static HeadlessContext, scene: &str, mode: ProcGenMode) -> (PlayerApp, Log) {
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(scene, &registry).expect("the scene loads");
    let mut app = PlayerApp::new(world, scene.into(), false, None, None);
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
    app.scene_renderer = Some(SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        gpu.format,
        64,
        64,
        RendererConfig::default(),
    ));
    app.test_gpu = Some(gpu);
    app.launch.procgen_mode = mode;
    app.core.clock.frame_step = Some(1.0 / 60.0);
    let log = Log::default();
    {
        let log = log.clone();
        app.core
            .script
            .rhai_engine_mut()
            .register_fn("record", move |s: &str| log.lock().unwrap().push(s.into()));
    }
    app.enter_scene(SceneEntry::Start)
        .expect("the scene starts");
    (app, log)
}

fn ticks(app: &mut PlayerApp, frames: usize) {
    for _ in 0..frames {
        app.tick();
    }
}

fn y_of(app: &PlayerApp, name: &str) -> f32 {
    let id = app.core.world.get_id(name).unwrap();
    app.core.world.get_transform(id).unwrap().position.y
}

fn reports(app: &PlayerApp) -> Vec<String> {
    app.core
        .world
        .read_reports()
        .snapshot()
        .iter()
        .map(|d| d.one_line())
        .collect()
}

fn the_rock_is_its_hull(app: &PlayerApp) {
    let rock = app.core.world.get_id("rock").unwrap();
    match app.core.physics.get_entity_collider_extents(rock) {
        Some(ColliderExtents::Convex {
            half_extents,
            centre,
        }) => {
            for (got, want) in half_extents.iter().zip([2.0, 0.5, 2.0]) {
                assert!((got - want).abs() < 1e-4, "{half_extents:?}");
            }
            assert!(centre.iter().all(|c| c.abs() < 1e-4), "{centre:?}");
        }
        other => panic!("the rock's collider is the cube it draws: {other:?}"),
    }
    assert!(
        !reports(app).iter().any(|r| r.contains("no triangles")),
        "{:#?}",
        reports(app)
    );
}

#[test]
fn a_procgen_rocks_convex_collider_rests_a_ball_on_its_drawn_surface() {
    let Some(gpu) = super::scene_entry_tests::gpu() else {
        flint_render::gpu_select::skip_without_gpu("procgen collider test");
        return;
    };
    vfs::scoped(Arc::new(files()), || {
        let (mut app, _) = started(gpu, ROCKS, ProcGenMode::Immediate);
        the_rock_is_its_hull(&app);
        ticks(&mut app, 180);
        // On the cube's top (0.5) with radius 0.5
        let y = y_of(&app, "ball");
        assert!(
            (y - 1.0).abs() < 0.05,
            "the ball rests on the rock: y = {y}"
        );
    });
}

#[test]
fn a_procgen_rock_the_queue_makes_later_gets_its_hull_then() {
    let Some(gpu) = super::scene_entry_tests::gpu() else {
        flint_render::gpu_select::skip_without_gpu("deferred procgen collider test");
        return;
    };
    vfs::scoped(Arc::new(files()), || {
        let (mut app, _) = started(gpu, ROCKS, ProcGenMode::Deferred);
        let rock = app.core.world.get_id("rock").unwrap();
        assert!(
            matches!(
                app.core.physics.get_entity_collider_extents(rock),
                Some(ColliderExtents::Box { .. })
            ),
            "a placeholder stands at load, and the collider waits as a box"
        );
        assert!(
            !reports(&app).iter().any(|r| r.contains("no triangles")),
            "a queued model isn't missing: {:#?}",
            reports(&app)
        );
        // The queue finishes it after the bodies are made. A frame lends
        // its GPU to the queue's share only from a window's context (a
        // frame here would make the rock and drop it), so the tests' GPU
        // does that share by hand, through the same calls
        // (`GameCore::tick_procgen`), before the first frame
        apply_queue(&mut app, gpu);
        ticks(&mut app, 240);
        the_rock_is_its_hull(&app);
        let y = y_of(&app, "ball");
        assert!(
            (y - 1.0).abs() < 0.05,
            "the ball rests on the rock: y = {y}"
        );
    });
}

/// The frame's procgen share (`GameCore::tick_procgen`) on the tests' GPU:
/// the queue run until the rock is made, then swapped in.
fn apply_queue(app: &mut PlayerApp, gpu: &HeadlessContext) {
    let mut completed = Vec::new();
    for _ in 0..100 {
        completed.extend(app.core.procgen_resolver.process_frame([0.0; 3]));
        if !completed.is_empty() {
            break;
        }
    }
    assert_eq!(completed.len(), 1, "the queue made the rock");
    let config = flint_render::model_loader::ModelLoadConfig::in_project(
        app.core.project.clone(),
        &app.core.scene_path,
    );
    let mut sink = super::procgen_assets::GpuSink {
        renderer: app.scene_renderer.as_mut().unwrap(),
        device: &gpu.device,
        queue: &gpu.queue,
        config: &config,
    };
    let mut recording = super::collider_meshes::Recording {
        sink: &mut sink,
        sources: &mut app.core.collider_sources,
    };
    super::procgen_assets::apply_completed(&completed, &mut recording);
}

/// A 2 x 1 x 2 block with its base at y = 0, as a game would build it.
fn block() -> (Vec<flint_render::Vertex>, Vec<u32>) {
    let corners = [
        [-1.0, 0.0, -1.0],
        [1.0, 0.0, -1.0],
        [1.0, 0.0, 1.0],
        [-1.0, 0.0, 1.0],
        [-1.0, 1.0, -1.0],
        [1.0, 1.0, -1.0],
        [1.0, 1.0, 1.0],
        [-1.0, 1.0, 1.0],
    ];
    let vertices = corners
        .iter()
        .map(|&position| flint_render::Vertex {
            position,
            normal: [0.0, 1.0, 0.0],
            color: [1.0; 4],
            uv: [0.0; 2],
        })
        .collect();
    let indices = vec![
        0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 1, 2, 6, 1, 6, 5, 2, 3, 7, 2, 7, 6,
        3, 0, 4, 3, 4, 7,
    ];
    (vertices, indices)
}

/// What `ExtensionGpu::upload_mesh_with` does, by hand: the tests' GPU
/// isn't a window, which is what lends an extension the renderer.
fn upload(app: &mut PlayerApp, gpu: &HeadlessContext, ray_target: bool) {
    let (vertices, indices) = block();
    let renderer = app.scene_renderer.as_mut().unwrap();
    renderer.load_procedural_mesh(
        &gpu.device,
        "hut",
        &vertices,
        &indices,
        flint_import::ImportedMaterial::default(),
    );
    if ray_target {
        let positions: Vec<[f32; 3]> = vertices.iter().map(|v| v.position).collect();
        renderer.set_ray_mesh(
            "hut",
            Some(Arc::new(flint_geometry::ray_mesh::RayMesh::new(
                &positions, &indices,
            ))),
        );
    }
}

#[test]
fn a_runtime_mesh_collider_is_hit_by_raycast_where_it_draws() {
    let Some(gpu) = super::scene_entry_tests::gpu() else {
        flint_render::gpu_select::skip_without_gpu("runtime mesh collider test");
        return;
    };
    vfs::scoped(Arc::new(files()), || {
        let (mut app, log) = started(gpu, HUT, ProcGenMode::Immediate);
        assert!(
            !reports(&app).iter().any(|r| r.contains("no triangles")),
            "a runtime mesh isn't missing, only not uploaded yet: {:#?}",
            reports(&app)
        );
        ticks(&mut app, 2);
        upload(&mut app, gpu, true);
        ticks(&mut app, 3);
        let seen = log.lock().unwrap().clone();
        // Before the upload the ray passes the 2 m box; the frame of the
        // upload its scripts run before physics takes the triangles; after,
        // it meets the drawn top at y = 2
        assert_eq!(&seen[..3], ["none", "none", "none"], "{seen:?}");
        assert_eq!(&seen[3..], ["hut 2.0", "hut 2.0"], "{seen:?}");
        let hut = app.core.world.get_id("hut").unwrap();
        assert!(matches!(
            app.core.physics.get_entity_collider_extents(hut),
            Some(ColliderExtents::Mesh { .. })
        ));

        // Replaced (a game rebuilt it, then dropped its ray target): the
        // collider keeps what it last had until a ray target comes again
        upload(&mut app, gpu, false);
        ticks(&mut app, 2);
        assert_eq!(log.lock().unwrap().last().unwrap(), "hut 2.0");
    });
}
