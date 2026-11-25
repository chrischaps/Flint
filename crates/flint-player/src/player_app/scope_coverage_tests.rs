//! #1096: the profiler's scopes reach below the frame's phases. A headless
//! frame with a script, a falling body and a drawn UI records a scope for
//! each phase's systems under `tick`; a scripted transition records the
//! scene it went to and, inside it, the model it imported, by file name.
//!
//! Each test reads its own thread's ring (the recorder keeps one a
//! thread), so tests running beside it can't add to what it sees. The
//! model half needs a GPU adapter: without one it says so and passes,
//! unless `FLINT_REQUIRE_GPU=1`.

use super::scene_entry::SceneEntry;
use super::PlayerApp;
use flint_core::profile;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use flint_render::{HeadlessContext, RendererConfig, SceneRenderer};
use std::sync::Arc;

const STEP: f64 = 1.0 / 60.0;

/// Every frame: an update and a UI draw. Leaves for scene B after a second.
const SCRIPT: &str = r#"
let gone = false;
fn on_update() {
    if gone { return; }
    if total_time() >= 1.0 { gone = true; load_scene("b.scene.toml"); }
}
fn on_draw_ui() {
    draw_text(10.0, 10.0, "hello", 14.0, 1.0, 1.0, 1.0, 1.0);
}
"#;

const SCENE_A: &str = r#"
[scene]
name = "A"

[entities.controller]
[entities.controller.script]
source = "leave.rhai"

[entities.crate]
[entities.crate.transform]
position = [0.0, 3.0, 0.0]
[entities.crate.rigidbody]
body_type = "dynamic"
[entities.crate.collider]
shape = "box"
size = [1.0, 1.0, 1.0]
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
    files.insert("game/scripts/leave.rhai", SCRIPT);
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

/// One finished scope of this thread's ring.
#[derive(Debug, Clone)]
struct Seen {
    name: &'static str,
    depth: u16,
    frame: u32,
    detail: Option<String>,
}

/// What this thread recorded while `f` ran, in the order the scopes closed.
/// Recording is on for the whole process while it runs, so the tests that
/// record take turns, and turn it off after: every thread that scopes
/// anything while it's on makes a ring of its own.
fn recorded(f: impl FnOnce()) -> Vec<Seen> {
    // The recorder and its frame index are the process's: one test at a
    // time records, this file's and the traces' (#1097)
    let _turn = super::trace::recorder_test_turn();
    let thread = std::thread::current()
        .name()
        .expect("a test thread is named")
        .to_string();
    let before = written_on(&thread);
    profile::set_recording(true);
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    profile::set_recording(false);
    if let Err(panic) = ran {
        std::panic::resume_unwind(panic);
    }
    let mut seen = Vec::new();
    profile::with_rings(|ring| {
        if ring.thread() == thread {
            let new = (ring.written() - before) as usize;
            let held: Vec<_> = ring.records().collect();
            assert!(new <= held.len(), "the ring wrapped: make the run shorter");
            for r in &held[held.len() - new..] {
                seen.push(Seen {
                    name: r.site.name,
                    depth: r.depth,
                    frame: r.frame,
                    detail: ring.detail(r),
                });
            }
        }
    });
    seen
}

fn written_on(thread: &str) -> u64 {
    let mut written = 0;
    profile::with_rings(|ring| {
        if ring.thread() == thread {
            written = ring.written();
        }
    });
    written
}

fn named<'a>(seen: &'a [Seen], name: &str) -> Vec<&'a Seen> {
    seen.iter().filter(|s| s.name == name).collect()
}

#[test]
fn each_phase_of_a_headless_frame_is_broken_into_its_systems() {
    vfs::scoped(Arc::new(game()), || {
        let mut app = started(None);
        let seen = recorded(|| {
            for _ in 0..10 {
                app.tick();
            }
        });
        let ticks = named(&seen, "tick");
        assert_eq!(ticks.len(), 10, "one tick a frame");
        assert!(ticks.iter().all(|t| t.depth == 0));
        // Each perf phase's work, by the scope that names it, under the tick
        for name in [
            // Input
            "poll_devices",
            "collect_game_events",
            "advance_transition",
            // Scripts
            "run_scripts",
            "script::update",
            "script::hot_reload_poll",
            "script::discover",
            "script::on_update",
            // Physics
            "tick_fixed_physics",
            "physics::fixed_update",
            "physics::sync_in",
            "physics::step",
            "physics::sync_out",
            // Script UI
            "call_draw_uis",
            "script::on_draw_ui",
            "collect_script_ui",
            "script::ui_layout",
            // Anim/fx
            "sound",
            "animate",
            "animation::update",
            "animation::skeletal",
            "after_animation",
            "animation::ik",
            "particles::simulate",
            // Sync
            "sync_renderer",
            "finish_frame",
        ] {
            let found = named(&seen, name);
            assert!(!found.is_empty(), "no {name} scope in {seen:#?}");
            assert!(
                found.iter().all(|s| s.depth > 0),
                "{name} isn't under the tick"
            );
        }
        // One scope a script call, named by its file, inside its callback's
        let calls = named(&seen, "script::call");
        assert!(calls.iter().any(|c| c
            .detail
            .as_deref()
            .is_some_and(|d| d.ends_with("leave.rhai"))));
        let update = named(&seen, "script::on_update")[0];
        assert!(calls.iter().any(|c| c.depth == update.depth + 1));
        // The rapier step sits in the fixed update, in the frame's loop
        let fixed = named(&seen, "physics::fixed_update")[0];
        let step = named(&seen, "physics::step")[0];
        assert_eq!(step.depth, fixed.depth + 1);
        assert_eq!(step.detail.as_deref(), Some("1"), "one body in the step");
    });
}

#[test]
fn a_model_loaded_mid_play_is_a_scope_named_by_its_file() {
    let gpu = super::scene_entry_tests::gpu();
    if gpu.is_none() {
        flint_render::gpu_select::skip_without_gpu("scope coverage's model half");
    }
    vfs::scoped(Arc::new(game()), || {
        let mut app = started(gpu);
        let seen = recorded(|| {
            for _ in 0..600 {
                app.tick();
                if app.core.scene_path.ends_with("b.scene.toml") {
                    return;
                }
            }
            panic!("scene B never came");
        });
        let transition = named(&seen, "scene_transition");
        assert_eq!(transition.len(), 1, "{seen:#?}");
        let transition = transition[0];
        assert!(
            transition
                .detail
                .as_deref()
                .is_some_and(|d| d.ends_with("b.scene.toml")),
            "{transition:?}"
        );
        if gpu.is_none() {
            return;
        }
        // The import, by file, inside the transition on the same frame
        let import = named(&seen, "import_model");
        let import = import
            .iter()
            .find(|s| s.detail.as_deref().is_some_and(|d| d.ends_with("card.glb")))
            .unwrap_or_else(|| panic!("no import of card.glb in {seen:#?}"));
        assert_eq!(import.frame, transition.frame);
        assert!(import.depth > transition.depth);
        // and its upload, by asset
        assert!(named(&seen, "upload_model")
            .iter()
            .any(|s| s.detail.as_deref() == Some("card") && s.frame == transition.frame));
    });
}
