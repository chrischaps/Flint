//! #1077: frame notes, end to end through the player's own frames. A
//! scene whose script leaves at t = 2 s for a scene with a model: the frame
//! record at that moment carries the transition and, inside it, the model
//! load naming the file. A look switched on mid-run builds its style's
//! pipelines on the frame it was asked for, each named with its build
//! time. A frame of two physics steps says so.
//!
//! The project is a `MemorySource`. The model and pipeline halves need a
//! GPU adapter: without one they say so and pass, unless
//! `FLINT_REQUIRE_GPU=1`.

use super::scene_entry::SceneEntry;
use super::PlayerApp;
use flint_core::frame_notes::{FrameNote, NoteKind};
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use flint_render::{HeadlessContext, RendererConfig, SceneRenderer};
use std::sync::Arc;

const STEP: f64 = 1.0 / 60.0;

/// Leaves for the scene with the model once two seconds have passed.
const LEAVE: &str = r#"
let gone = false;
fn on_update() {
    if gone { return; }
    if total_time() >= 2.0 { gone = true; load_scene("b.scene.toml"); }
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
"#;

fn game() -> MemorySource {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert("game/scenes/a.scene.toml", SCENE_A);
    files.insert("game/scenes/b.scene.toml", SCENE_B);
    files.insert("game/scripts/leave.rhai", LEAVE);
    files.insert(
        "game/scenes/models/card.glb",
        include_bytes!("../../../../tests/fixtures/models/DoubleSidedCard.glb").to_vec(),
    );
    files
}

/// Scene A brought up as a host brings it up, stepping `STEP` a frame,
/// on `gpu` when there is one.
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
            packages: None,
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
    app.enter_scene(SceneEntry::Start).expect("scene A starts");
    app.core.clock.frame_step = Some(STEP);
    app
}

/// The notes of the frame record `index`.
fn notes_of(app: &PlayerApp, index: u64) -> Vec<FrameNote> {
    app.stats
        .perf
        .frames()
        .iter()
        .find(|f| f.index == index)
        .map(|f| f.notes.clone())
        .unwrap_or_default()
}

/// Tick until `done` says so of the game time, then once more, so the
/// frame it happened in is closed into the history. Returns each tick's
/// game time, indexed as the frame records are: tick `i` begins frame `i`.
fn tick_until(app: &mut PlayerApp, done: impl Fn(&PlayerApp) -> bool) -> Vec<f64> {
    let mut times = Vec::new();
    for _ in 0..600 {
        app.tick();
        times.push(app.core.clock.total_time);
        if done(app) {
            app.tick();
            times.push(app.core.clock.total_time);
            return times;
        }
    }
    panic!("never happened in 600 frames");
}

#[test]
fn a_scripted_transition_at_two_seconds_notes_itself_and_its_model() {
    let gpu = super::scene_entry_tests::gpu();
    if gpu.is_none() {
        flint_render::gpu_select::skip_without_gpu("frame notes' model half");
    }
    vfs::scoped(Arc::new(game()), || {
        let mut app = started(gpu);
        assert!(
            app.stats.perf.recording(),
            "frame-history is a default feature"
        );
        let times = tick_until(&mut app, |app| {
            app.core.scene_path.ends_with("b.scene.toml")
        });

        let noted: Vec<u64> = app
            .stats
            .perf
            .frames()
            .iter()
            .filter(|f| f.notes.iter().any(|n| n.kind == NoteKind::SceneTransition))
            .map(|f| f.index)
            .collect();
        assert_eq!(
            noted.len(),
            1,
            "one frame carries the transition: {noted:?}"
        );
        let at = noted[0] as usize;
        // The script asks in the frame game time reaches two seconds, and
        // with no fade to draw the scene loads at the next frame's start
        // (`advance_transition`, #362): the record of that moment
        // (compared as the script compares: 120 steps of 1/60 add up to a
        // hair under 2)
        assert!(times[at - 1] >= 2.0, "{}", times[at - 1]);
        assert!(times[at - 2] < 2.0, "{}", times[at - 2]);

        let notes = notes_of(&app, noted[0]);
        let transition = notes
            .iter()
            .find(|n| n.kind == NoteKind::SceneTransition)
            .unwrap();
        assert_eq!(transition.detail, "b.scene.toml");
        assert_eq!(transition.depth, 0);
        assert!(transition.ms.is_some_and(|ms| ms > 0.0), "{transition:?}");
        if gpu.is_some() {
            let model = notes
                .iter()
                .find(|n| n.kind == NoteKind::ModelLoad)
                .unwrap_or_else(|| panic!("no model load among {notes:#?}"));
            assert!(
                model.detail.ends_with("models/card.glb"),
                "names the file: {model}"
            );
            assert!(model.depth >= 1, "inside the transition: {model:?}");
            assert!(model.ms.is_some_and(|ms| ms > 0.0), "{model:?}");
        }
        // The frames on either side carry no transition
        assert!(!notes_of(&app, noted[0] - 1)
            .iter()
            .any(|n| n.kind == NoteKind::SceneTransition));
    });
}

#[test]
fn a_look_switched_on_mid_run_notes_each_pipeline_it_builds() {
    let Some(gpu) = super::scene_entry_tests::gpu() else {
        flint_render::gpu_select::skip_without_gpu("frame notes' pipeline half");
        return;
    };
    vfs::scoped(Arc::new(game()), || {
        let mut app = started(Some(gpu));
        for _ in 0..5 {
            app.tick();
        }
        let asked_in = app.stats.perf.frames_begun - 1;
        // Mid-frame, as a script's set_look or F4 would ask it
        app.set_look(Some("graphite")).unwrap();
        app.tick();

        let notes = notes_of(&app, asked_in);
        let style = notes
            .iter()
            .position(|n| n.kind == NoteKind::StyleBuild && n.detail == "pencil")
            .unwrap_or_else(|| panic!("pencil's build not among {notes:#?}"));
        let pipelines: Vec<&FrameNote> = notes[style + 1..]
            .iter()
            .take_while(|n| n.depth > notes[style].depth)
            .filter(|n| n.kind == NoteKind::PipelineBuild)
            .collect();
        assert!(
            !pipelines.is_empty(),
            "its pipelines, inside it: {notes:#?}"
        );
        for p in &pipelines {
            assert!(!p.detail.is_empty(), "a label: {p:?}");
            assert!(p.ms.is_some_and(|ms| ms >= 0.0), "a build time: {p:?}");
        }
        // A second frame asks for nothing new
        app.tick();
        assert!(notes_of(&app, asked_in + 1)
            .iter()
            .all(|n| n.kind != NoteKind::StyleBuild));
    });
}

/// A scene that will cut to graphite later, and says so (#1159).
const SCENE_PRELOADS: &str = r#"
[scene]
name = "Preloads"

[post_process]
preload_looks = ["graphite"]

[entities.card]
[entities.card.transform]
position = [0.0, 0.0, 0.0]
"#;

/// #1159: a scene that preloads its look builds the look's styles at its
/// entry, off the frame thread, and the frame a script cuts to it notes
/// no style or pipeline build: the cut costs what a cut to a look already
/// drawn does. Without `preload_looks` the same cut builds pencil on its
/// frame (the test above).
#[test]
fn a_cut_to_a_preloaded_look_builds_nothing_on_its_frame() {
    let Some(gpu) = super::scene_entry_tests::gpu() else {
        flint_render::gpu_select::skip_without_gpu("preloaded looks");
        return;
    };
    let mut files = game();
    files.insert("game/scenes/a.scene.toml", SCENE_PRELOADS);
    vfs::scoped(Arc::new(files), || {
        let mut app = started(Some(gpu));
        // The workers build while frames go on; each frame's style pass
        // takes in what has arrived (a headless tick draws nothing, so the
        // test calls it as `render` does), and none builds pencil itself
        let since = std::time::Instant::now();
        loop {
            let renderer = app.scene_renderer.as_ref().unwrap();
            if renderer.style_built("pencil") && renderer.styles_preparing().is_empty() {
                break;
            }
            assert!(
                since.elapsed() < std::time::Duration::from_secs(120),
                "graphite's styles never arrived: {:?}",
                renderer.styles_preparing()
            );
            app.tick();
            flint_core::frame_notes::listen(true);
            let renderer = app.scene_renderer.as_mut().unwrap();
            renderer.ensure_style_resources(&gpu.device);
            let mut notes = Vec::new();
            flint_core::frame_notes::take(&mut notes);
            assert!(
                notes.iter().all(|n| n.kind != NoteKind::StyleBuild),
                "a frame built a style the scene preloads: {notes:#?}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        for _ in 0..3 {
            app.tick();
        }
        let asked_in = app.stats.perf.frames_begun - 1;
        app.set_look(Some("graphite")).unwrap();
        app.tick();
        app.tick();
        for index in [asked_in, asked_in + 1] {
            let built: Vec<FrameNote> = notes_of(&app, index)
                .into_iter()
                .filter(|n| matches!(n.kind, NoteKind::StyleBuild | NoteKind::PipelineBuild))
                .collect();
            assert!(built.is_empty(), "the cut's frame built: {built:#?}");
        }
    });
}

#[test]
fn a_frame_of_two_physics_steps_says_so() {
    vfs::scoped(Arc::new(game()), || {
        let mut app = started(None);
        app.tick();
        app.tick();
        // Two fixed steps' worth of game time in one frame
        app.core.clock.frame_step = Some(2.0 * STEP);
        app.tick();
        let index = app.stats.perf.frames_begun - 1;
        app.core.clock.frame_step = Some(STEP);
        app.tick();
        let notes = notes_of(&app, index);
        let steps = notes
            .iter()
            .find(|n| n.kind == NoteKind::PhysicsSteps)
            .unwrap_or_else(|| panic!("{notes:#?}"));
        assert_eq!(steps.detail, "2 fixed steps");
        assert!(notes_of(&app, index + 1)
            .iter()
            .all(|n| n.kind != NoteKind::PhysicsSteps));
    });
}
