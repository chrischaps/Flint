//! #1107: models that arrive after their scene. A chunk loaded at t = 2 s
//! with a model the scene doesn't use draws it, and the frame it loads on
//! carries a `model load` note naming the file, inside the chunk's note. A
//! script's `spawn_entity` with a `model` draws too, and a script spawning
//! one a frame reads the file once. Before #1107 the player loaded models
//! only when a scene was entered, and such an entity drew nothing.
//!
//! The project is a `MemorySource`. The tests need a GPU adapter: without
//! one they say so and pass, unless `FLINT_REQUIRE_GPU=1`.

use super::scene_entry::SceneEntry;
use super::PlayerApp;
use flint_core::frame_notes::{FrameNote, NoteKind};
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use flint_render::{FrameInputs, HeadlessContext, RendererConfig, SceneRenderer};
use std::sync::Arc;

const STEP: f64 = 1.0 / 60.0;

/// Streams the cave in once two seconds have passed.
const STREAM: &str = r#"
let streamed = false;
fn on_update() {
    if streamed { return; }
    if total_time() >= 2.0 { streamed = true; load_chunk("cave.chunk.toml", 4.0, 0.0, "cave"); }
}
"#;

/// Spawns a card a frame from t = 1 s, ten of them.
const SPAWN: &str = r#"
let made = 0;
fn on_update() {
    if made >= 10 || total_time() < 1.0 { return; }
    let id = spawn_entity("card_" + made);
    set_field(id, "transform", "position", [made.to_float(), 0.0, 0.0]);
    set_field(id, "model", "asset", "card");
    made += 1;
}
"#;

fn scene(script: &str) -> String {
    format!(
        r#"
[scene]
name = "A"

[entities.controller]
[entities.controller.script]
source = "{script}"
"#
    )
}

/// The chunk: one card the scene itself never names.
const CAVE: &str = r#"
[scene]
name = "cave"

[entities.card]
[entities.card.transform]
position = [0.0, 0.0, 0.0]
[entities.card.model]
asset = "card"
"#;

fn game() -> MemorySource {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert("game/scenes/stream.scene.toml", scene("stream.rhai"));
    files.insert("game/scenes/spawn.scene.toml", scene("spawn.rhai"));
    files.insert("game/scenes/cave.chunk.toml", CAVE);
    files.insert("game/scripts/stream.rhai", STREAM);
    files.insert("game/scripts/spawn.rhai", SPAWN);
    files.insert(
        "game/scenes/models/card.glb",
        include_bytes!("../../../../tests/fixtures/models/DoubleSidedCard.glb").to_vec(),
    );
    files
}

/// `path` brought up as a host brings it up, on `gpu`, stepping `STEP`.
fn started(path: &str, gpu: &'static HeadlessContext) -> PlayerApp {
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(path, &registry).expect("scene loads");
    let mut app = PlayerApp::new(world, path.into(), false, None, None);
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
    app.enter_scene(SceneEntry::Start).expect("scene starts");
    app.core.clock.frame_step = Some(STEP);
    app
}

/// The opaque mesh draws the renderer makes of the world as it stands.
fn mesh_draws(app: &mut PlayerApp, gpu: &HeadlessContext) -> u32 {
    let renderer = app.scene_renderer.as_mut().unwrap();
    renderer.prepare(
        &gpu.device,
        &gpu.queue,
        &app.core.world,
        &FrameInputs::default(),
    );
    renderer.collect_stats().entity_draws
}

/// How many meshes the card is (its file has one a face).
fn card_meshes(app: &PlayerApp) -> u32 {
    let cache = app.scene_renderer.as_ref().unwrap().mesh_cache();
    cache.get("card").expect("the card is loaded").len() as u32
}

/// Every frame record's notes of `kind`, with the frame's index.
fn notes_of(app: &PlayerApp, kind: NoteKind) -> Vec<(u64, FrameNote)> {
    app.stats
        .perf
        .frames()
        .iter()
        .flat_map(|f| {
            f.notes
                .iter()
                .filter(|n| n.kind == kind)
                .map(|n| (f.index, n.clone()))
        })
        .collect()
}

#[test]
fn a_chunks_model_draws_after_load_chunk_and_its_frame_notes_the_file() {
    let Some(gpu) = super::scene_entry_tests::gpu() else {
        flint_render::gpu_select::skip_without_gpu("a chunk's model");
        return;
    };
    vfs::scoped(Arc::new(game()), || {
        let mut app = started("game/scenes/stream.scene.toml", gpu);
        assert!(!app
            .scene_renderer
            .as_ref()
            .unwrap()
            .mesh_cache()
            .contains("card"));
        assert_eq!(mesh_draws(&mut app, gpu), 0, "the scene draws no mesh");

        // Up to the chunk's frame, and one more, so its record is closed
        for _ in 0..600 {
            app.tick();
            if app.loaded_chunks.contains_key("cave") {
                break;
            }
        }
        assert!(app.loaded_chunks.contains_key("cave"), "the chunk loaded");
        app.tick();

        let card = app
            .core
            .world
            .get_id("cave_card")
            .expect("the chunk's card");
        let meshes = card_meshes(&app);
        assert_eq!(mesh_draws(&mut app, gpu), meshes, "the chunk's card draws");
        // The card's file is a node a face: the chunk's entity takes them
        // as children, as a scene's entity does
        assert!(!app.core.world.children(card).is_empty(), "expanded");

        let chunks = notes_of(&app, NoteKind::ChunkLoad);
        assert_eq!(chunks.len(), 1, "{chunks:?}");
        let models = notes_of(&app, NoteKind::ModelLoad);
        assert_eq!(models.len(), 1, "the file read once: {models:?}");
        let (frame, model) = &models[0];
        assert_eq!(*frame, chunks[0].0, "on the chunk's frame");
        assert!(
            model.detail.ends_with("models/card.glb"),
            "names the file: {model}"
        );
        assert!(
            model.depth > chunks[0].1.depth,
            "inside the chunk: {model:?}"
        );
        assert!(model.ms.is_some_and(|ms| ms > 0.0), "{model:?}");
    });
}

#[test]
fn a_scripts_spawned_model_draws_and_its_file_is_read_once() {
    let Some(gpu) = super::scene_entry_tests::gpu() else {
        flint_render::gpu_select::skip_without_gpu("a script's spawned model");
        return;
    };
    vfs::scoped(Arc::new(game()), || {
        let mut app = started("game/scenes/spawn.scene.toml", gpu);
        assert_eq!(mesh_draws(&mut app, gpu), 0);
        for _ in 0..120 {
            app.tick();
        }
        assert!(app.core.world.get_id("card_9").is_some(), "ten spawned");
        let meshes = card_meshes(&app);
        assert_eq!(
            mesh_draws(&mut app, gpu),
            10 * meshes,
            "each spawn draws its card"
        );
        let models = notes_of(&app, NoteKind::ModelLoad);
        assert_eq!(
            models.len(),
            1,
            "the first spawn reads the file, the rest draw the cached mesh: {models:?}"
        );
        assert!(models[0].1.detail.ends_with("models/card.glb"));
    });
}
