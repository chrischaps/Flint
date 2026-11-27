//! A spec's asset in a project whose `[packages]` leaves the procgen
//! package out (#1578, ADR 0137 decision 8): a stock player, which has the
//! package, makes nothing from the spec, says one line naming `[packages]`,
//! and draws the name as a build without procgen does. A project that lists
//! procgen, or has no `[packages]`, makes it as before.
//!
//! On a GPU (procgen resolves into the renderer); skipped without one, and
//! passed over by a launcher without the procgen package, which makes
//! nothing anyway.

use super::scene_entry::SceneEntry;
use super::state::ProcGenMode;
use super::PlayerApp;
use flint_core::vfs::{self, MemorySource};
use flint_render::{HeadlessContext, RendererConfig, SceneRenderer};
use std::sync::{Arc, Mutex};

const SCENE: &str = "game/scenes/rocks.scene.toml";

/// One rock, its model named by the spec beside the scene.
const ROCKS: &str = r#"
[scene]
name = "Rocks"

[entities.rock]
[entities.rock.transform]
position = [0, 0, 0]
[entities.rock.model]
asset = "rock"
"#;

const SPEC: &str = "generator = \"mock\"\n[meta]\nname = \"rock\"\nversion = \"1.0.0\"\n\
                    [seed]\nmode = \"fixed\"\nvalue = 7\n";

/// Every package of this tree, by name, with its manifest: the list
/// resolves against them as it does on disk.
const MANIFESTS: &[(&str, &str)] = &[
    ("ai", include_str!("../../../../packages/ai/package.toml")),
    (
        "debug-hud",
        include_str!("../../../../packages/debug-hud/package.toml"),
    ),
    (
        "engine",
        include_str!("../../../../packages/engine/package.toml"),
    ),
    (
        "living",
        include_str!("../../../../packages/living/package.toml"),
    ),
    (
        "music",
        include_str!("../../../../packages/music/package.toml"),
    ),
    (
        "procgen",
        include_str!("../../../../packages/procgen/package.toml"),
    ),
    (
        "rhythm",
        include_str!("../../../../packages/rhythm/package.toml"),
    ),
];

/// The game, with the engine at `engine/`, its `flint.toml` ending in
/// `packages` (a `[packages]` table, or nothing).
fn game(packages: &str) -> Arc<MemorySource> {
    let mut files = MemorySource::new();
    files.insert("game/engine/crates/flint-core/Cargo.toml", "");
    for component in ["transform", "model"] {
        files.insert(
            format!("game/engine/schemas/components/{component}.toml"),
            match component {
                "transform" => include_str!("../../../../schemas/components/transform.toml"),
                _ => include_str!("../../../../schemas/components/model.toml"),
            },
        );
    }
    for (name, manifest) in MANIFESTS {
        files.insert(
            format!("game/engine/packages/{name}/package.toml"),
            *manifest,
        );
    }
    files.insert(
        "game/flint.toml",
        format!("[project]\nschemas = [\"engine/schemas\"]\n{packages}"),
    );
    files.insert("game/specs/rock.procgen.toml", SPEC);
    files.insert(SCENE, ROCKS);
    Arc::new(files)
}

/// The scene started on the GPU as `prepare` brings it up (the one schema
/// resolver, so the `[packages]` list rides in the directories), then
/// `frames` frames.
fn play(gpu: &'static HeadlessContext, mode: ProcGenMode, frames: usize) -> PlayerApp {
    let flags: &[&str] = &[];
    let (registry, dirs) =
        flint_schema::SchemaRegistry::resolve(Some(SCENE.as_ref()), flags).expect("schemas");
    let (world, file) = flint_scene::load_scene(SCENE, &registry).expect("the scene loads");
    let mut app = PlayerApp::new(world, SCENE.into(), false, None, None);
    app.apply_scene_settings(&file);
    app.set_schemas(registry, dirs);
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
    app.enter_scene(SceneEntry::Start)
        .expect("the scene starts");
    for _ in 0..frames {
        app.tick();
    }
    app
}

/// What `run` logged at warn and above, each line without decoration.
fn warned<T>(run: impl FnOnce() -> T) -> (T, String) {
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let log = Captured::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .with_level(false)
        .finish();
    let out = tracing::subscriber::with_default(subscriber, run);
    let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    (out, text)
}

/// Whether the spec made the rock's mesh.
fn made(app: &PlayerApp) -> bool {
    app.scene_renderer
        .as_ref()
        .is_some_and(|r| r.mesh_cache().is_procedural("rock"))
}

/// The lines naming the procgen package.
fn procgen_lines(log: &str) -> Vec<&str> {
    log.lines()
        .map(str::trim)
        .filter(|l| l.starts_with("[procgen] "))
        .collect()
}

fn gpu_and_procgen() -> Option<&'static HeadlessContext> {
    if !crate::packages::registry().has("procgen") {
        return None;
    }
    let gpu = super::scene_entry_tests::gpu();
    if gpu.is_none() {
        flint_render::gpu_select::skip_without_gpu("procgen left-out test");
    }
    gpu
}

/// The ticket's first box: `procgen = false` makes nothing from the spec,
/// at load or on the queue, and says so once, with the line to take out.
#[test]
fn a_project_that_leaves_procgen_out_makes_nothing_from_a_spec() {
    let Some(gpu) = gpu_and_procgen() else {
        return;
    };
    for mode in [ProcGenMode::Immediate, ProcGenMode::Deferred] {
        vfs::scoped(game("\n[packages]\nprocgen = false\n"), || {
            let (app, log) = warned(|| play(gpu, mode, 30));
            assert!(!made(&app), "{mode:?}: the spec made nothing");
            assert_eq!(
                procgen_lines(&log),
                [
                    "[procgen] [packages] in flint.toml leaves out `procgen`: the scene's 1 asset \
                     named by a spec (rock) load from files, where there are any (remove \
                     `procgen = false` from [packages] in game/flint.toml)"
                ],
                "{mode:?}: {log}"
            );
        });
    }
}

/// The ticket's last box: a project that lists procgen, or has no
/// `[packages]`, makes the spec's mesh and says nothing of it.
#[test]
fn a_project_that_lists_procgen_or_no_packages_makes_the_spec() {
    let Some(gpu) = gpu_and_procgen() else {
        return;
    };
    for packages in ["\n[packages]\nprocgen = \"*\"\n", ""] {
        vfs::scoped(game(packages), || {
            let (app, log) = warned(|| play(gpu, ProcGenMode::Immediate, 1));
            assert!(made(&app), "{packages:?}: the spec made the rock");
            assert!(procgen_lines(&log).is_empty(), "{packages:?}: {log}");
        });
    }
}
