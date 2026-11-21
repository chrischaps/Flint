//! #1092: load reports through the player's own bring-up. A scene whose
//! script leaves at t = 1 s for a scene with a model and a script: the
//! transition gets a table of its own, whose stages are the new scene's
//! read, the old one's teardown, its model, its script and `on_init`, and
//! whose rows and "not named" add up to its measured time. A chunk loaded
//! later gets one too.
//!
//! The project is a `MemorySource`. The model row needs a GPU adapter:
//! without one it is left unasked, unless `FLINT_REQUIRE_GPU=1`.

use super::scene_entry::SceneEntry;
use super::PlayerApp;
use flint_core::load_report::{self, LoadKind, LoadReport, Stage};
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use flint_render::{HeadlessContext, RendererConfig, SceneRenderer};
use std::sync::Arc;

const STEP: f64 = 1.0 / 60.0;

const LEAVE: &str = r#"
let gone = false;
fn on_update() {
    if gone { return; }
    if total_time() >= 1.0 { gone = true; load_scene("b.scene.toml"); }
}
"#;

/// Waits a little in `on_init`, so the stage has a time to find
const SETTLE: &str = r#"
let chunked = false;
fn on_init() {
    let t = timestamp();
    while t.elapsed < 0.02 {}
}
fn on_update() {
    if chunked { return; }
    if total_time() >= 2.0 { chunked = true; load_chunk("cave.chunk.toml", 0.0, 0.0, "cave"); }
}
"#;

const SCENE_A: &str = r#"
[scene]
name = "A"

[entities.controller]
[entities.controller.script]
source = "leave.rhai"
"#;

const SCENE_B: &str = r#"
[scene]
name = "B"

[entities.card]
[entities.card.transform]
position = [0.0, 0.0, 0.0]
[entities.card.model]
asset = "card"

[entities.keeper]
[entities.keeper.script]
source = "settle.rhai"
"#;

const CHUNK: &str = r#"
[scene]
name = "cave"

[entities.rock]
[entities.rock.transform]
position = [1.0, 0.0, 0.0]
"#;

fn game() -> MemorySource {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert("game/scenes/a.scene.toml", SCENE_A);
    files.insert("game/scenes/b.scene.toml", SCENE_B);
    files.insert("game/scenes/cave.chunk.toml", CHUNK);
    files.insert("game/scripts/leave.rhai", LEAVE);
    files.insert("game/scripts/settle.rhai", SETTLE);
    files.insert(
        "game/scenes/models/card.glb",
        include_bytes!("../../../../tests/fixtures/models/DoubleSidedCard.glb").to_vec(),
    );
    files
}

fn started(gpu: Option<&'static HeadlessContext>) -> PlayerApp {
    let path = "game/scenes/a.scene.toml";
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(path, &registry).expect("scene A loads");
    let mut app = PlayerApp::new(world, path.into(), false, None, None);
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
    app.launch.load_report = true;
    app.enter_scene(SceneEntry::Start).expect("scene A starts");
    app.core.clock.frame_step = Some(STEP);
    app
}

/// A headless app presents nothing: the frame after a load stands in for
/// its first frame on screen.
fn tick_presenting(app: &mut PlayerApp, frames: usize) {
    for _ in 0..frames {
        app.tick();
        app.frame_presented_for_loads();
    }
}

fn sums_to_its_total(report: &LoadReport) {
    let sum = report.named_ms() + report.unnamed_ms;
    assert!(
        (sum - report.total_ms).abs() <= 0.05,
        "the rows and what nothing named add up to the total: {report:#?}"
    );
}

#[test]
fn a_scripted_transition_and_a_chunk_each_print_their_own_table() {
    let gpu = super::scene_entry_tests::gpu();
    if gpu.is_none() {
        flint_render::gpu_select::skip_without_gpu("the load report's model row");
    }
    vfs::scoped(Arc::new(game()), || {
        load_report::abandon();
        let mut app = started(gpu);
        assert!(
            app.load_reports().is_empty(),
            "a start-up is begun by the host, and none was"
        );
        // To t = 1 s, the transition, its frame, then the chunk at 2 s
        tick_presenting(&mut app, 150);
        assert!(app.core.scene_path.ends_with("b.scene.toml"));

        let reports = app.load_reports();
        assert_eq!(reports.len(), 2, "{reports:#?}");
        let transition = &reports[0];
        assert_eq!(transition.kind, LoadKind::Transition);
        assert!(transition.name.ends_with("b.scene.toml"), "{transition:#?}");
        sums_to_its_total(transition);
        for stage in [
            Stage::SceneRead,
            Stage::Teardown,
            Stage::Scripts,
            Stage::ScriptInit,
            Stage::Physics,
            Stage::FirstFrame,
        ] {
            assert!(
                transition.row(stage).is_some(),
                "{} has a row: {transition:#?}",
                stage.label()
            );
        }
        let init = transition.row(Stage::ScriptInit).unwrap();
        assert!(init.ms >= 20.0, "on_init's busy wait is its own: {init:?}");
        assert!(
            transition
                .slowest
                .iter()
                .any(|a| a.kind == "script" && a.name.ends_with("settle.rhai")),
            "the script is named: {transition:#?}"
        );
        if gpu.is_some() {
            let models = transition.row(Stage::Models).expect("the card's model");
            assert_eq!(models.count, 1);
            assert!(
                transition
                    .slowest
                    .iter()
                    .any(|a| a.kind == "model" && a.name.ends_with("card.glb")),
                "{transition:#?}"
            );
        }
        // Nearly all of it is named: the bring-up's stages cover it
        assert!(
            transition.unnamed_ms <= transition.total_ms * 0.25,
            "{transition:#?}"
        );
        let table = transition.table();
        assert!(table.starts_with("load (transition) "), "{table}");
        assert!(table.contains("on_init"), "{table}");

        let chunk = &reports[1];
        assert_eq!(chunk.kind, LoadKind::Chunk);
        assert_eq!(chunk.name, "cave");
        sums_to_its_total(chunk);
        assert!(chunk.row(Stage::FirstFrame).is_some(), "{chunk:#?}");
        assert!(!load_report::recording());
    });
}

#[test]
fn a_load_with_no_frame_after_it_is_said_at_the_end() {
    vfs::scoped(Arc::new(game()), || {
        load_report::begin(LoadKind::Start, "");
        let mut app = started(None);
        assert!(load_report::waiting_for_first_frame());
        app.finish_load_reports();
        let reports = app.load_reports();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].kind, LoadKind::Start);
        assert!(reports[0].name.ends_with("a.scene.toml"), "{reports:#?}");
        assert!(reports[0].row(Stage::Scripts).is_some(), "{reports:#?}");
        sums_to_its_total(&reports[0]);
    });
}
