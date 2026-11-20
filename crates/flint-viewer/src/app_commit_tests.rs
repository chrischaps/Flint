//! F4's Commit to scene writes one style into the scene file, and the
//! watcher reloads the file it wrote (#516). Before, that reload set the
//! post config from the file, so every F4 tweak never written anywhere
//! (bloom, fog, another style's) snapped back to the authored value without
//! a word. The viewer's own commit's reload now leaves the config on
//! screen; any other reload still takes the file's `[post_process]`.
//!
//! These need a GPU adapter. Without one each test prints a note and passes.

use super::binding_tests::gpu;
use super::*;

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

const SCENE: &str = "# A comment the commit keeps
[scene]
name = \"Commit reload\"
version = \"1.0\"

[post_process]
bloom_intensity = 0.04
fog_density = 0.0

[post_process.styles.ascii]
cell = 8

[entities.marker]
[entities.marker.transform]
position = [0, 0, 0]
";

/// A scene file of its own for `test`, in a folder the test may write.
fn scene_file(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("flint-516-{}-{test}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("commit.scene.toml");
    std::fs::write(&path, SCENE).unwrap();
    path
}

/// The viewer as `run` leaves it after load: the scene staged, its
/// authored `[post_process]` on screen.
fn viewer(path: &Path, device: &wgpu::Device, queue: &wgpu::Queue) -> ViewerApp {
    let scene_path = path.to_string_lossy().into_owned();
    let schemas = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let (registry, constraint_registry, _) =
        load_schemas(&scene_path, &[schemas.to_string_lossy().into_owned()]).unwrap();
    let (world, scene_file) = load_scene(&scene_path, &registry).unwrap();
    let mut state = ViewerState {
        world,
        registry,
        constraint_registry,
        scene_path: scene_path.clone(),
        needs_reload: false,
        scene_doc: SceneDocument::from_file(&scene_path).ok(),
        scene_post: scene_file.post_process.clone(),
        wooze: field_of(&scene_file),
        scene_camera: scene_file.camera.clone(),
        environment: scene_file.environment.clone(),
    };
    let mut renderer =
        SceneRenderer::new_headless(device, queue, FORMAT, SIZE, SIZE, RendererConfig::default());
    let config = ModelLoadConfig::from_scene_path(&scene_path);
    let binding = scene_binding(&scene_path);
    stage_scene(&mut renderer, device, queue, &mut state, binding, &config);
    let authored =
        post_process_config_from_def(state.scene_post.as_ref().unwrap(), renderer.styles());
    renderer.set_post_process_config(authored);

    let mut app = ViewerApp::new(Arc::new(Mutex::new(state)), false);
    app.render_context = Some(Gpu {
        device: device.clone(),
        queue: queue.clone(),
        format: FORMAT,
        width: SIZE,
        height: SIZE,
    });
    app.scene_renderer = Some(renderer);
    app
}

/// F4 tuned: bloom and fog up, and ASCII's cell from 8 to 12.
fn tune(app: &mut ViewerApp) -> PostProcessConfig {
    let renderer = app.scene_renderer.as_mut().unwrap();
    let mut tuned = renderer.post_process_config().clone();
    tuned.bloom_intensity = 0.6;
    tuned.fog_density = 0.03;
    let errors = flint_render::stylize::apply_style_specs(
        &mut tuned,
        &["ascii:cell=12".into()],
        renderer.styles(),
    );
    assert!(errors.is_empty(), "{errors:?}");
    renderer.set_post_process_config(tuned.clone());
    tuned
}

/// What the watcher does when the file changes, then the frame's poll.
fn reload(app: &mut ViewerApp) {
    app.state.lock().unwrap().needs_reload = true;
    app.check_reload();
}

fn on_screen(app: &ViewerApp) -> PostProcessConfig {
    app.scene_renderer
        .as_ref()
        .unwrap()
        .post_process_config()
        .clone()
}

#[test]
fn committing_a_style_keeps_the_unsaved_bloom_and_fog_on_screen() {
    let Some((device, queue)) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let path = scene_file("keeps");
    let mut app = viewer(&path, device, queue);
    let mut tuned = tune(&mut app);

    // Commit to scene, as the button does it, then what the viewer does
    // on the panel's styles_committed flag
    let mut editor = flint_debug_ui::StylesEditor::default();
    editor.set_scene_file(Some(path.clone()));
    let ascii = flint_render::stylize::builtin("ascii").unwrap();
    editor.commit(ascii, &mut tuned).unwrap();
    app.own_commit_until = Some(Instant::now() + OWN_COMMIT_RELOAD);
    reload(&mut app);

    let after = on_screen(&app);
    assert_eq!(
        after.bloom_intensity, 0.6,
        "the tuned bloom is still on screen"
    );
    assert_eq!(after.fog_density, 0.03, "the tuned fog is still on screen");
    assert!(after.styles.same_style("ascii", &tuned.styles));

    // The file holds the commit and only the commit, and the reload read it
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("cell = 12"), "{text}");
    assert!(text.contains("bloom_intensity = 0.04"), "{text}");
    assert!(text.starts_with("# A comment the commit keeps"), "{text}");
    let state = app.state.lock().unwrap();
    let authored = post_process_config_from_def(
        state.scene_post.as_ref().unwrap(),
        &flint_render::stylize::StyleRegistry::new(),
    );
    assert!(authored.styles.same_style("ascii", &after.styles));
    assert_eq!(authored.bloom_intensity, 0.04);
    drop(state);
    assert!(app.own_commit_until.is_none(), "the mark is one reload's");
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

#[test]
fn an_edit_of_the_file_still_hot_reloads_its_post_process() {
    let Some((device, queue)) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let path = scene_file("external");
    let mut app = viewer(&path, device, queue);
    tune(&mut app);

    // Someone edits [post_process] in the file: it wins over F4, as before
    let edited = SCENE.replace("bloom_intensity = 0.04", "bloom_intensity = 0.2");
    std::fs::write(&path, &edited).unwrap();
    reload(&mut app);
    assert_eq!(on_screen(&app).bloom_intensity, 0.2);
    assert_eq!(on_screen(&app).fog_density, 0.0);

    // A commit mark the watcher never answered doesn't outlive its window
    tune(&mut app);
    app.own_commit_until = Some(Instant::now() - Duration::from_millis(1));
    std::fs::write(&path, SCENE).unwrap();
    reload(&mut app);
    assert_eq!(on_screen(&app).bloom_intensity, 0.04);
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

/// The change list's lines, parsed
fn change_list(path: &Path) -> Vec<serde_json::Value> {
    let log = crate::edits::edit_log_path(path);
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// #1046: a Commit to scene is in the change list, a line a key it wrote,
/// in a save's shape, and a Ctrl+S after it doesn't write it back out
#[test]
fn a_commit_appends_its_keys_to_the_change_list() {
    let Some((device, queue)) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let path = scene_file("logged");
    let mut app = viewer(&path, device, queue);
    let mut tuned = tune(&mut app);

    let mut editor = flint_debug_ui::StylesEditor::default();
    editor.set_scene_file(Some(path.clone()));
    let ascii = flint_render::stylize::builtin("ascii").unwrap();
    editor.commit(ascii, &mut tuned).unwrap();
    // What the viewer does on the panel's styles_committed flag
    log_commit(&mut app.state.lock().unwrap());

    let lines = change_list(&path);
    let cell = lines
        .iter()
        .find(|l| l["field"] == "cell")
        .unwrap_or_else(|| panic!("no line for the cell: {lines:?}"));
    assert_eq!(cell["entity"], serde_json::Value::Null);
    assert_eq!(cell["component"], "post_process");
    assert_eq!(cell["table"], "post_process.styles.ascii");
    assert_eq!(cell["old"], 8);
    assert_eq!(cell["new"], 12);
    assert_eq!(cell["scene"], "commit.scene.toml");
    assert!(cell["saved_at"].as_u64().unwrap() > 0);
    // Only what the commit wrote: the unsaved bloom and fog aren't in it
    assert!(
        lines.iter().all(|l| l["table"] != "post_process"),
        "{lines:?}"
    );

    // An entity edit saved before the watcher's reload keeps the commit
    let marker = app.state.lock().unwrap().world.get_id("marker").unwrap();
    app.pending_edits.push(EditAction {
        entity_id: marker,
        component: "transform".into(),
        field: "position".into(),
        old_value: toml::Value::Array(vec![0.into(), 0.into(), 0.into()]),
        new_value: toml::Value::Array(vec![1.0.into(), 0.0.into(), 0.0.into()]),
    });
    app.apply_pending_edits();
    app.save();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("cell = 12"),
        "the save wrote the commit away:\n{text}"
    );
    assert!(text.contains("position = [1.0, 0.0, 0.0]"), "{text}");
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}

/// #1046: the human moves the marker and saves; an agent then edits the
/// file. The reload keeps the human's history: undo puts the marker back,
/// the scene is dirty, and the save writes it into the agent's file
#[test]
fn undo_after_an_agents_edit_still_undoes_the_humans_last_edit() {
    let Some((device, queue)) = gpu() else {
        flint_render::gpu_select::skip_without_gpu("no GPU adapter");
        return;
    };
    let path = scene_file("undo");
    let mut app = viewer(&path, device, queue);
    let marker = app.state.lock().unwrap().world.get_id("marker").unwrap();
    app.pending_edits.push(EditAction {
        entity_id: marker,
        component: "transform".into(),
        field: "position".into(),
        old_value: toml::Value::Array(vec![0.into(), 0.into(), 0.into()]),
        new_value: toml::Value::Array(vec![2.0.into(), 0.0.into(), 0.0.into()]),
    });
    app.apply_pending_edits();
    app.save();
    app.suppress_reload_until = None;

    // The agent turns the bloom up in the file, past the save's own window
    let saved = std::fs::read_to_string(&path).unwrap();
    let agents = saved.replace("bloom_intensity = 0.04", "bloom_intensity = 0.3");
    assert_ne!(saved, agents);
    std::fs::write(&path, &agents).unwrap();
    reload(&mut app);
    assert!(!app.dirty);
    assert!(
        app.undo_stack.can_undo(),
        "the reload threw the history away"
    );

    app.undo();
    assert!(app.dirty, "an undo after the reload is an edit");
    let position = |app: &ViewerApp| {
        let st = app.state.lock().unwrap();
        let id = st.world.get_id("marker").unwrap();
        st.world
            .get_components(id)
            .unwrap()
            .get_field("transform", "position")
            .cloned()
    };
    assert_eq!(
        position(&app),
        Some(toml::Value::Array(vec![0.into(), 0.into(), 0.into()]))
    );
    app.save();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("bloom_intensity = 0.3"),
        "the agent's edit: {text}"
    );
    let file: toml::Table = toml::from_str(&text).unwrap();
    let back = &file["entities"]["marker"]["transform"]["position"];
    assert_eq!(back.as_array().unwrap().len(), 3, "{text}");
    assert!(
        back.as_array()
            .unwrap()
            .iter()
            .all(|v| v.as_float().or(v.as_integer().map(|i| i as f64)) == Some(0.0)),
        "{text}"
    );
    std::fs::remove_dir_all(path.parent().unwrap()).ok();
}
