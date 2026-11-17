#![allow(clippy::disallowed_methods)] // writes a game to the disk and reads back what the capture wrote
//! #1086, through the player's own frames, headless: an input script's
//! `capture frame` line and a hitch each ask for a capture, which is armed
//! on the next frame, drawn with the HUD's pass after it, read back on the
//! frames after without waiting, and written by a writer thread, manifest
//! last. The scene is a folder on the disk (the writer hashes its files);
//! the frame is drawn into a `HeadlessContext` the way `render()` draws into
//! the window, through the same `FrameCaptures` calls.
//!
//! These need a GPU adapter: without one they say so and pass, unless
//! `FLINT_REQUIRE_GPU=1`. The trigger and naming rules are unit tests in
//! `frame_capture.rs`; the detector passing over a captured frame is in
//! `hitch.rs`.

use super::frame_capture::{FrameFacts, Targets};
use super::input_script::InputScript;
use super::scene_entry::SceneEntry;
use super::PlayerApp;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_render::frame_capture::FrameDescription;
use flint_render::frame_replay::{FrameInputsState, ReplayInputs};
use flint_render::{FrameInputs, HeadlessContext, RendererConfig, SceneRenderer};
use std::path::{Path, PathBuf};

const STEP: f64 = 1.0 / 60.0;

const SCENE: &str = r#"
[scene]
name = "Capture Bench"

[entities.card]
[entities.card.transform]
position = [0.0, 0.0, 0.0]
[entities.card.model]
asset = "card"

[entities.sun]
[entities.sun.light]
type = "directional"
direction = [0.3, 1.0, 0.2]

[entities.controller]
[entities.controller.script]
source = "game.rhai"
"#;

/// Stalls 150 ms on the frame game time crosses 2.5 s: past the start,
/// whose hitches `--capture-on-hitch` passes over (#1207).
const STALL: &str = r#"
fn on_update() {
    let t = total_time();
    if t - delta_time() < 2.51 && t >= 2.51 {
        let start = timestamp();
        while start.elapsed < 0.15 { }
    }
}
"#;

const CALM: &str = "fn on_update() { let t = total_time(); }";

/// A game folder of its own under the temp directory: the scene, its
/// script and model, and an empty manifest.
fn game_on_disk(name: &str, script: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("flint-capture-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scenes/models")).unwrap();
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(root.join("flint.toml"), "").unwrap();
    std::fs::write(root.join("scenes/bench.scene.toml"), SCENE).unwrap();
    std::fs::write(root.join("scripts/game.rhai"), script).unwrap();
    std::fs::write(
        root.join("scenes/models/card.glb"),
        include_bytes!("../../../../tests/fixtures/models/DoubleSidedCard.glb"),
    )
    .unwrap();
    root
}

/// The scene brought up as a host brings it up, on `gpu`, stepping `STEP`
/// a frame, its captures written under `<root>/out`.
fn started(root: &Path, gpu: &'static HeadlessContext) -> PlayerApp {
    let path = root.join("scenes/bench.scene.toml");
    let path = path.to_string_lossy().into_owned();
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(&path, &registry).expect("the scene loads");
    let mut app = PlayerApp::new(world, path, false, None, None);
    app.apply_scene_settings(&file);
    app.set_schemas(
        registry,
        SchemaDirs {
            dirs: Vec::new(),
            source: SchemaSource::Flags,
            dropped: Vec::new(),
        },
    );
    app.scene_renderer = Some(SceneRenderer::new_headless(
        &gpu.device,
        &gpu.queue,
        gpu.format,
        gpu.width,
        gpu.height,
        RendererConfig::default(),
    ));
    app.test_gpu = Some(gpu);
    app.enter_scene(SceneEntry::Start)
        .expect("the scene starts");
    app.core.clock.frame_step = Some(STEP);
    app.capture_frames(Vec::new(), 0, Some(root.join("out")));
    app
}

/// One frame: `tick`, then what `sync_renderer` and `render` do for a
/// capture, into the headless target. The renderer's draw is the window's
/// with no HUD to paint; its `UI` pass is still logged and read.
fn frame(app: &mut PlayerApp, gpu: &'static HeadlessContext) {
    app.tick();
    let renderer = app.scene_renderer.as_mut().unwrap();
    let inputs = FrameInputs {
        time: app.core.clock.total_time,
        dt: app.core.clock.delta_time as f32,
        ..Default::default()
    };
    if app.frame_captures.take_for_frame() {
        app.frame_captures
            .keep_inputs(FrameInputsState::of(&inputs, &app.core.world));
    }
    renderer.prepare(&gpu.device, &gpu.queue, &app.core.world, &inputs);

    let armed = app.frame_captures.armed();
    let targets = Targets {
        device: &gpu.device,
        queue: &gpu.queue,
        surface: Some(&gpu.color_texture),
        depth: &gpu.depth_texture,
        host: flint_render::FrameHost {
            adapter: Some(&gpu.adapter_info),
            present_mode: None,
        },
    };
    if armed {
        app.frame_captures.arm(renderer, &targets);
    }
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Capture Test Frame"),
        });
    renderer.encode_frame_to(
        &gpu.device,
        &gpu.queue,
        &mut encoder,
        gpu.depth_target(),
        &app.camera,
        &gpu.color_view,
        Some(&app.core.world),
    );
    if armed {
        app.frame_captures
            .host_pass(renderer, &gpu.device, &mut encoder, &gpu.color_texture);
        app.frame_captures
            .record_replay(renderer, &gpu.device, &mut encoder, &app.core.world);
    }
    renderer.encode_gpu_timings(&gpu.device, &mut encoder);
    gpu.queue.submit(std::iter::once(encoder.finish()));
    renderer.frame_submitted();
    if armed {
        let facts = FrameFacts {
            world: &app.core.world,
            camera: &app.camera,
            particles: None,
            scene_path: &app.core.scene_path,
            project: &app.core.project,
            index: Some(app.stats.perf.under_way_index()),
            game_time: app.core.clock.total_time,
            dt: app.core.clock.delta_time,
        };
        let said = app.frame_captures.submitted(renderer, &targets, &facts);
        assert!(said.is_some_and(|s| s.contains("capturing the frame")));
        app.stats.perf.capturing();
    }
    let timed = app.core.script.timing();
    if app.frame_captures.poll(
        renderer,
        &gpu.device,
        &gpu.queue,
        &app.stats.perf,
        app.stats.hitches.hitches(),
        timed,
    ) {
        app.stats.perf.capturing();
    }
}

/// The one capture under `<root>/out`, read back.
fn the_capture(root: &Path) -> (PathBuf, FrameDescription) {
    let dirs: Vec<PathBuf> = std::fs::read_dir(root.join("out"))
        .expect("captures were written")
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(dirs.len(), 1, "one capture: {dirs:?}");
    let dir = dirs[0].clone();
    let json = std::fs::read_to_string(dir.join("manifest.json")).expect("its manifest");
    (
        dir,
        serde_json::from_str(&json).expect("a manifest that reads"),
    )
}

#[test]
fn an_input_scripts_capture_line_writes_the_next_frame_without_waiting_on_it() {
    let Some(gpu) = super::scene_entry_tests::gpu() else {
        flint_render::gpu_select::skip_without_gpu("the player's frame capture");
        return;
    };
    let root = game_on_disk("script", CALM);
    let mut app = started(&root, gpu);
    app.capture.input_script = Some(InputScript::parse("0.5 capture frame\n").unwrap());
    while app.core.clock.total_time < 1.0 {
        frame(&mut app, gpu);
    }
    // Written by its thread while the run went on, or here at its end
    app.finish_frame_captures();
    assert!(
        app.frame_captures.failure().is_none(),
        "{:?}",
        app.frame_captures.failure()
    );

    let (dir, manifest) = the_capture(&root);
    assert!(
        dir.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("bench-t0.5"),
        "named by scene and game time: {dir:?}"
    );
    assert_eq!(manifest.written_by.as_deref(), Some("flint-player"));
    let trigger = manifest.trigger.as_ref().unwrap();
    assert_eq!(trigger.kind, "script");
    // The first frame at or past 0.5 s: 30 steps of 1/60 fall a hair short
    let t = manifest.frame.game_time.unwrap();
    assert!((0.49..0.52).contains(&t), "drawn at {t}");
    assert!(manifest.frame.capturing);
    let scene = manifest.scene.as_ref().unwrap();
    assert_eq!(scene.name, "Capture Bench");
    assert_eq!(scene.sha256.as_ref().map(String::len), Some(64));

    // The renderer's passes, then the host's UI pass with its image
    let last = manifest.passes.last().unwrap();
    assert_eq!(last.name, "UI");
    assert_eq!(last.images.len(), 1, "{last:?}");
    assert!(dir.join(&last.images[0].file).is_file());
    let images = manifest.images();
    assert!(images.len() > 2, "every pass's image: {}", images.len());
    for image in &images {
        assert!(dir.join(&image.file).is_file(), "{} is written", image.file);
    }

    // The player's side: the history around the frame, the frame's own record
    let cpu = manifest.cpu.as_ref().unwrap();
    let own = cpu.frame.as_ref().expect("the captured frame's record");
    assert_eq!(Some(own.index), manifest.frame.index);
    assert!(own.capturing, "marked, so the detector passes over it");
    assert!(cpu.window.len() > 20, "{}", cpu.window.len());
    assert!(own.phases_ms.contains_key("render"));

    // Replay inputs, keyed by name
    let replay = manifest.replay.as_ref().unwrap();
    assert!(replay.available);
    for file in &replay.files {
        assert!(dir.join(file).is_file(), "{file}");
    }
    let card = replay.assets.iter().find(|a| a.name == "card").unwrap();
    assert_eq!(card.kind, "model");
    assert!(card.path.ends_with("card.glb"), "{card:?}");
    assert!(card.sha256.is_some() && card.bytes > 0);
    let inputs: ReplayInputs =
        serde_json::from_str(&std::fs::read_to_string(dir.join("replay/inputs.json")).unwrap())
            .unwrap();
    assert_eq!(inputs.version, 1);
    assert!(
        (inputs.frame.time - t).abs() < 1e-9,
        "the inputs are the frame's"
    );
    assert_eq!(inputs.camera.position, {
        let p = app.camera.position;
        [p.x, p.y, p.z]
    });
    let world = std::fs::read_to_string(dir.join("replay/world.toml")).unwrap();
    assert!(world.contains("[entities.card"), "{world}");

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_hitch_is_captured_on_the_next_frame_with_its_window_and_cause() {
    let Some(gpu) = super::scene_entry_tests::gpu() else {
        flint_render::gpu_select::skip_without_gpu("the player's capture on a hitch");
        return;
    };
    let root = game_on_disk("hitch", STALL);
    let mut app = started(&root, gpu);
    app.capture_frames(Vec::new(), 1, Some(root.join("out")));
    assert!(app.core.script.timing(), "hitches name their script");
    // A busy machine's own hiccups (several agents build on this one) don't
    // count: the stall is 150 ms
    app.stats.hitches.thresholds.floor_ms = 60.0;
    while app.core.clock.total_time < 3.1 {
        frame(&mut app, gpu);
    }
    app.finish_frame_captures();
    assert!(
        app.frame_captures.failure().is_none(),
        "{:?}",
        app.frame_captures.failure()
    );

    let (_, manifest) = the_capture(&root);
    assert_eq!(manifest.trigger.as_ref().unwrap().kind, "hitch");
    let cpu = manifest.cpu.as_ref().unwrap();
    let hitch = cpu.hitch.as_ref().expect("the hitch that asked");
    assert_eq!(hitch.class, "player");
    assert!(hitch.ms >= 140.0, "{hitch:?}");
    assert!(
        hitch
            .script
            .as_deref()
            .is_some_and(|s| s.contains("game.rhai")),
        "names the script: {hitch:?}"
    );
    // The capture is a later frame, and the window holds the hitch's
    // record with what it cost by phase
    let captured = manifest.frame.index.unwrap();
    assert!(captured > hitch.frame, "{captured} after {}", hitch.frame);
    let record = cpu.window.iter().find(|f| f.index == hitch.frame).unwrap();
    assert!(record.hitch);
    assert!(record.phases_ms["scripts"] >= 140.0, "{record:?}");
    assert!(cpu.window.first().unwrap().index < hitch.frame);
    // Just the one hitch: the capture's own frames weren't counted
    assert_eq!(app.stats.hitches.session().hitches, 1);
    assert_eq!(
        app.frame_captures.on_hitch, 0,
        "--capture-on-hitch 1 is spent"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn shift_f2_asks_for_a_capture_and_f2_alone_still_steps_the_stats() {
    use winit::keyboard::{KeyCode, ModifiersState};
    let mut app = PlayerApp::new(
        flint_ecs::FlintWorld::new(),
        "capture_key.scene.toml".into(),
        false,
        None,
        None,
    );
    app.focus.key_modifiers = ModifiersState::SHIFT;
    app.handle_debug_key(KeyCode::F2);
    assert_eq!(
        app.hud.key_note.showing(),
        Some(("Shift+F2", "no renderer active — no frame to capture"))
    );
    assert!(!app.frame_captures.busy());
    assert_eq!(app.stats.view, super::StatsView::Off, "Shift+F2 isn't F2");

    if let Some(gpu) = super::scene_entry_tests::gpu() {
        app.scene_renderer = Some(SceneRenderer::new_headless(
            &gpu.device,
            &gpu.queue,
            gpu.format,
            64,
            64,
            RendererConfig::default(),
        ));
        app.handle_debug_key(KeyCode::F2);
        assert!(app.frame_captures.busy(), "asked for the next frame");
        assert!(app.frame_captures.take_for_frame());
        app.handle_debug_key(KeyCode::F2);
        assert_eq!(
            app.hud.key_note.showing().map(|n| n.1),
            Some("waits for the capture already on its way")
        );
    }
    app.focus.key_modifiers = ModifiersState::empty();
    app.handle_debug_key(KeyCode::F2);
    assert_ne!(app.stats.view, super::StatsView::Off);
}
