//! One bring-up for startup and transitions (#723): a scene with
//! post-processing, a skybox, an input config, ambient light and an
//! orthographic camera hands over to a scene with none of them, and the
//! second scene gets the defaults, not what the first left. A scene that
//! can't be read leaves the one playing and fails the run.
//!
//! The project is a `MemorySource`, so every read goes through the vfs.
//! The renderer half needs a GPU adapter: without one that test says so
//! and passes, unless `FLINT_REQUIRE_GPU=1`.

use super::scene_entry::SceneEntry;
use super::PlayerApp;
use flint_core::project::{SchemaDirs, SchemaSource};
use flint_core::vfs::{self, MemorySource};
use flint_render::{HeadlessContext, RendererConfig, SceneRenderer};
use std::sync::{Arc, OnceLock};

const A: &str = "game/scenes/a.scene.toml";

const SCENE_A: &str = r#"
[scene]
name = "A"
input_config = "a_input.toml"

[environment]
skybox = "sky.png"
ambient_sky = [0.9, 0.2, 0.1]
diffuse_wrap = 0.4

[camera]
projection = "orthographic"
ortho_height = 12.0

[post_process]
vignette_enabled = true
vignette_intensity = 0.9
fog_enabled = true
exposure = 2.5

[entities.marker]
[entities.marker.transform]
position = [1.0, 2.0, 3.0]
"#;

const SCENE_B: &str = r#"
[scene]
name = "B"

[entities.other]
[entities.other.transform]
position = [0.0, 0.0, 0.0]
"#;

/// Scene A's own layer: one action no other config has.
const A_INPUT: &str = r#"
version = 1
game_id = "entry_test"

[actions.a_only]
kind = "button"
[[actions.a_only.bindings]]
type = "key"
code = "KeyQ"
"#;

/// The project's layer, which a scene with no `input_config` gets.
const PROJECT_INPUT: &str = r#"
version = 1
game_id = "entry_test"

[actions.project_wide]
kind = "button"
[[actions.project_wide.bindings]]
type = "key"
code = "KeyP"
"#;

/// A 2 x 1 panorama.
fn png() -> Vec<u8> {
    let mut bytes = Vec::new();
    image::RgbaImage::from_pixel(2, 1, image::Rgba([40, 90, 200, 255]))
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

fn game() -> MemorySource {
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert(A, SCENE_A);
    files.insert("game/scenes/b.scene.toml", SCENE_B);
    files.insert("game/scenes/a_input.toml", A_INPUT);
    files.insert("game/config/input.toml", PROJECT_INPUT);
    files.insert("game/scenes/sky.png", png());
    files
}

/// Scene A brought up the way a host and `finish_initialize` bring it up:
/// `new`, `apply_scene_settings`, the registry, then `enter_scene(Start)`.
fn started_in_a(gpu: Option<&'static HeadlessContext>) -> PlayerApp {
    let registry = flint_schema::SchemaRegistry::new();
    let (world, file) = flint_scene::load_scene(A, &registry).expect("scene A loads");
    let mut app = PlayerApp::new(
        world,
        A.into(),
        false,
        None,
        file.scene.input_config.clone(),
    );
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
    app.enter_scene(SceneEntry::Start).expect("scene A starts");
    app
}

fn has_action(app: &PlayerApp, action: &str) -> bool {
    app.input.action_config(action).is_some()
}

#[test]
fn a_transition_takes_the_next_scenes_settings_and_bindings() {
    vfs::scoped(Arc::new(game()), || {
        let mut app = started_in_a(None);
        assert!(
            has_action(&app, "a_only"),
            "A's own input config loads at startup"
        );
        assert!(app.camera.orthographic);
        assert_eq!(app.scene.skybox_path.as_deref(), Some("sky.png"));

        assert!(app.execute_scene_transition("b.scene.toml"));
        assert_eq!(
            std::path::Path::new(&app.core.scene_path),
            std::path::Path::new("game/scenes/b.scene.toml")
        );
        assert!(
            !has_action(&app, "a_only"),
            "a scene reached by a transition loads its input config (none: the project's)"
        );
        assert!(has_action(&app, "project_wide"));
        assert!(app.scene.skybox_path.is_none());
        assert!(app.scene.ambient.is_none());
        assert!(app.scene.diffuse_wrap.is_none());
        assert!(app.scene.post_process.is_none());
        assert!(!app.camera.orthographic, "B has no [camera]: a fresh one");
        assert_eq!(app.camera.fov, 70.0);
        assert!(app.run_failure().is_none());

        // And back: A's layer comes back with A
        assert!(app.execute_scene_transition("a.scene.toml"));
        assert!(has_action(&app, "a_only"));
        assert!(app.camera.orthographic);
    });
}

#[test]
fn a_scene_that_cannot_load_leaves_the_current_one_and_fails_the_run() {
    vfs::scoped(Arc::new(game()), || {
        let mut app = started_in_a(None);
        let entities = app.core.world.entity_count();
        assert!(app.core.world.get_id("marker").is_some());

        assert!(!app.execute_scene_transition("missing.scene.toml"));
        assert_eq!(app.core.scene_path, A, "the scene playing is kept");
        assert_eq!(app.core.world.entity_count(), entities);
        assert!(
            app.core.world.get_id("marker").is_some(),
            "its world was not torn down"
        );
        assert!(has_action(&app, "a_only"));

        let failure = app.run_failure().expect("the run fails");
        assert!(failure.contains("missing.scene.toml"), "{failure}");
        assert!(
            failure.contains("failed to load from"),
            "names the path it was read from: {failure}"
        );

        // The phase still moves on: with no script to draw a fade, straight
        // back to idle (#362), not stuck loading
        app.transition_phase = super::TransitionPhase::Loading {
            target_scene: "missing.scene.toml".into(),
        };
        app.advance_transition();
        assert!(matches!(app.transition_phase, super::TransitionPhase::Idle));
    });
}

/// A HUD that draws its fades and says when each is done, so the player
/// waits for it (#362).
const FADER: &str = r#"
fn on_update() {
    if transition_phase() == "entering" { complete_transition(); }
}
"#;

#[test]
fn a_scene_that_cannot_load_fades_back_in_over_the_current_one() {
    let mut files = game();
    files.insert(
        A,
        format!("{SCENE_A}\n[entities.fader]\n[entities.fader.script]\nsource = \"fader.rhai\"\n"),
    );
    files.insert("game/scripts/fader.rhai", FADER);
    vfs::scoped(Arc::new(files), || {
        let mut app = started_in_a(None);
        assert!(app.core.script.can_complete_transition());

        app.transition_phase = super::TransitionPhase::Loading {
            target_scene: "missing.scene.toml".into(),
        };
        app.advance_transition();
        assert_eq!(app.core.scene_path, A, "the scene playing is kept");
        assert!(app.run_failure().is_some());
        assert!(
            matches!(
                app.transition_phase,
                super::TransitionPhase::Entering { .. }
            ),
            "its fader fades back in over it"
        );
    });
}

/// One device for this binary, never dropped: a device per test deadlocked
/// NVIDIA's Vulkan driver as test threads exited (#598).
fn gpu() -> Option<&'static HeadlessContext> {
    static GPU: OnceLock<Option<&'static HeadlessContext>> = OnceLock::new();
    *GPU.get_or_init(|| {
        pollster::block_on(HeadlessContext::new(64, 64))
            .ok()
            .map(|ctx| &*Box::leak(Box::new(ctx)))
    })
}

#[test]
fn a_transition_resets_the_renderer_to_the_next_scene() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("scene_entry renderer test");
        return;
    };
    vfs::scoped(Arc::new(game()), || {
        let mut app = started_in_a(Some(gpu));
        let defaults = format!("{:?}", flint_render::PostProcessConfig::default());
        let levers = |app: &PlayerApp| {
            format!(
                "{:?}",
                app.scene_renderer.as_ref().unwrap().lighting_levers()
            )
        };
        let default_levers = {
            let mut fresh = SceneRenderer::new_headless(
                &gpu.device,
                &gpu.queue,
                gpu.format,
                64,
                64,
                RendererConfig::default(),
            );
            fresh.reset_ambient();
            format!("{:?}", fresh.lighting_levers())
        };
        {
            let renderer = app.scene_renderer.as_ref().unwrap();
            assert!(renderer.has_skybox(), "A's skybox is drawn");
            let post = renderer.post_process_config();
            assert!(post.vignette_enabled && post.fog_enabled);
            assert_eq!(post.exposure, 2.5);
        }
        assert_ne!(levers(&app), default_levers, "A's ambient and wrap are set");

        assert!(app.execute_scene_transition("b.scene.toml"));
        let renderer = app.scene_renderer.as_ref().unwrap();
        assert!(!renderer.has_skybox(), "B names no skybox: none is drawn");
        assert_eq!(
            format!("{:?}", renderer.post_process_config()),
            defaults,
            "B has no [post_process]: the renderer's defaults, not A's"
        );
        assert_eq!(levers(&app), default_levers, "and the default lighting");
        assert!(!has_action(&app, "a_only"));
        assert!(has_action(&app, "project_wide"));
    });
}

/// #312: a game whose levels live in folders of their own, each with its
/// own `fonts/`. Both name their font `pixel`: the town's is the built-in
/// 8x8 cells with a ramp of its own, the dungeon's is drawn by hand in 3x4
/// cells, so the atlas says which file was read.
fn two_folder_game() -> MemorySource {
    let scene = |name: &str| {
        format!(
            "[scene]\nname = \"{name}\"\n\n\
             [entities.title.transform]\nposition = [0.0, 0.0, 0.0]\n\
             [entities.title.ui_text]\ntext = \"{name}\"\nfont = \"pixel\"\n"
        )
    };
    let mut files = MemorySource::new();
    files.insert("game/flint.toml", "");
    files.insert("game/levels/town/town.scene.toml", scene("Town"));
    files.insert(
        "game/levels/town/fonts/pixel.glyphs.toml",
        "[ramps.dots]\nglyphs = \" .:\"\n",
    );
    files.insert("game/levels/dungeon/dungeon.scene.toml", scene("Dungeon"));
    files.insert(
        "game/levels/dungeon/fonts/pixel.glyphs.toml",
        "[font]\nfile = \"pixel.txt\"\ncell = [3, 4]\n[ramps.r]\nglyphs = \" o\"\n\
         [edges]\nhorizontal = \"o\"\nvertical = \"o\"\ndiagonal = \"o\"\nantidiagonal = \"o\"\n",
    );
    files.insert(
        "game/levels/dungeon/fonts/pixel.txt",
        "== space\n...\n...\n...\n...\n== o\n...\n.#.\n...\n...\n",
    );
    files
}

/// #312: after a transition into a scene in another folder, its `ui_text`
/// font and the styles' glyph sets (the ASCII style's `font`) are found
/// beside the new scene, not the first one.
#[test]
fn fonts_resolve_beside_the_scene_a_transition_reaches() {
    let Some(gpu) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("scene_entry font test");
        return;
    };
    const TOWN: &str = "game/levels/town/town.scene.toml";
    vfs::scoped(Arc::new(two_folder_game()), || {
        let registry = flint_schema::SchemaRegistry::new();
        let (world, file) = flint_scene::load_scene(TOWN, &registry).expect("town loads");
        let mut app = PlayerApp::new(world, TOWN.into(), false, None, None);
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
            64,
            64,
            RendererConfig::default(),
        ));
        app.test_gpu = Some(gpu);
        app.enter_scene(SceneEntry::Start).expect("town starts");

        let town = std::path::Path::new("game/levels/town");
        let dungeon = std::path::Path::new("game/levels/dungeon");
        // What the scene's `ui_text` draws with, and what a style that
        // names `pixel` (the ASCII style's `font`) gets from the scope the
        // renderer hands it
        let fonts = |app: &PlayerApp| {
            let renderer = app.scene_renderer.as_ref().unwrap();
            let ui_text = renderer
                .loaded_ui_font("pixel")
                .expect("the ui_text font loaded")
                .atlas()
                .cell_size();
            let scope = renderer.asset_scope();
            let styles = flint_render::glyph_atlas::GlyphSetCache::new((*scope).clone())
                .get("pixel")
                .expect("the styles find the glyph set")
                .atlas()
                .cell_size();
            (
                renderer.scene_dir.clone(),
                scope.scene_dir.clone(),
                ui_text,
                styles,
            )
        };
        assert_eq!(
            fonts(&app),
            (Some(town.into()), town.into(), [8, 8], [8, 8]),
            "the town's fonts at startup"
        );

        assert!(app.execute_scene_transition("dungeon/dungeon.scene.toml"));
        assert_eq!(
            std::path::Path::new(&app.core.scene_path),
            std::path::Path::new("game/levels/dungeon/dungeon.scene.toml")
        );
        assert_eq!(
            fonts(&app),
            (Some(dungeon.into()), dungeon.into(), [3, 4], [3, 4]),
            "the dungeon's own fonts, not the town's"
        );

        // And back: the town's `pixel` again, its atlas uploaded afresh
        assert!(app.execute_scene_transition("town/town.scene.toml"));
        assert_eq!(
            fonts(&app),
            (Some(town.into()), town.into(), [8, 8], [8, 8])
        );
        let renderer = app.scene_renderer.as_ref().unwrap();
        let uploaded = renderer
            .texture_cache()
            .and_then(|c| c.get_dimensions("font:pixel"))
            .expect("the atlas is uploaded");
        let atlas = renderer.loaded_ui_font("pixel").unwrap().atlas().size();
        assert_eq!(
            uploaded,
            (atlas[0], atlas[1]),
            "the texture is the town's atlas, not the dungeon's left over"
        );
    });
}
