//! The scene viewer's camera and `--watch` are the host's (#1153): the
//! orbit is the shared `OrbitCameraController` every editor uses, and the
//! watch is the host's, with #429's filter as the viewer's answer to which
//! changed paths count.

use super::*;
use winit::dpi::PhysicalPosition;
use winit::event::DeviceId;

fn fixture(scene: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/scenes")
        .join(scene)
        .to_string_lossy()
        .into_owned()
}

fn state(scene_path: &str) -> Arc<Mutex<ViewerState>> {
    let (registry, constraint_registry, _) = load_schemas(scene_path, &[]).unwrap();
    let (world, _) = load_scene(scene_path, &registry).unwrap();
    Arc::new(Mutex::new(ViewerState {
        world,
        registry,
        constraint_registry,
        scene_path: scene_path.to_string(),
        needs_reload: false,
        scene_doc: None,
        scene_post: None,
        wooze: None,
        scene_camera: None,
        environment: None,
    }))
}

fn button(button: MouseButton, pressed: bool) -> WindowEvent {
    WindowEvent::MouseInput {
        device_id: DeviceId::dummy(),
        state: if pressed {
            ElementState::Pressed
        } else {
            ElementState::Released
        },
        button,
    }
}

fn moved(x: f64, y: f64) -> WindowEvent {
    WindowEvent::CursorMoved {
        device_id: DeviceId::dummy(),
        position: PhysicalPosition::new(x, y),
    }
}

/// An event as the host hands it on: to the editor, then, passed, to the
/// orbit (`host::Host::default_event`).
fn send(app: &mut ViewerApp, event: WindowEvent) -> Flow {
    let flow = app.event(&event);
    if flow == Flow::Pass {
        let (camera, orbit) = app.orbit().expect("the viewer's orbit is the host's");
        orbit.handle_event(&event, camera, false);
    }
    flow
}

/// Press `which` at (100, 100), drag to (100 + dx, 100), release.
fn drag(app: &mut ViewerApp, which: MouseButton, dx: f64) {
    for event in [
        moved(100.0, 100.0),
        button(which, true),
        moved(100.0 + dx / 2.0, 100.0),
        moved(100.0 + dx, 100.0),
        button(which, false),
    ] {
        assert_eq!(
            send(app, event),
            Flow::Pass,
            "the mouse goes on to the orbit"
        );
    }
}

#[test]
fn the_viewers_left_drag_orbits_through_the_hosts_controller() {
    let mut app = ViewerApp::new(state(&fixture("pbr_test.scene.toml")), true);
    app.orbit.auto_orbit = true;
    let yaw = app.camera.yaw;
    drag(&mut app, MouseButton::Left, 60.0);
    let turned = yaw - 60.0 * app.orbit.orbit_sensitivity;
    assert!(
        (app.camera.yaw - turned).abs() < 1e-5,
        "{} vs {turned}",
        app.camera.yaw
    );
    assert!(
        !app.orbit.auto_orbit,
        "an orbit by hand stops the turntable"
    );
    assert!(
        app.click_from.is_none(),
        "the drag's release ended the press"
    );
}

#[test]
fn a_click_is_a_pick_and_a_drag_is_not() {
    assert!(is_click((10.0, 10.0), (12.0, 13.0)));
    assert!(!is_click((10.0, 10.0), (40.0, 10.0)));
}

#[test]
fn the_view_cube_snaps_through_the_orbit_and_an_orbit_leaves_ortho() {
    let mut app = ViewerApp::new(state(&fixture("pbr_test.scene.toml")), true);
    app.orbit.snap_to(&mut app.camera, 0.0, 1.55);
    assert!(app.camera.orthographic);
    for _ in 0..120 {
        app.orbit.update(&mut app.camera, 1.0 / 60.0);
    }
    assert_eq!(
        (app.camera.yaw, app.camera.pitch),
        (0.0, 1.55),
        "the top view"
    );
    drag(&mut app, MouseButton::Left, 30.0);
    assert!(
        !app.camera.orthographic,
        "orbiting off an axis view is perspective"
    );
}

#[test]
fn the_track_editor_orbits_with_the_middle_button() {
    let config = SplineEditorConfig {
        control_points: Vec::new(),
        closed: false,
        spacing: 1.0,
        name: "track".into(),
        file_path: String::new(),
    };
    let scene = fixture("pbr_test.scene.toml");
    let mut app = ViewerApp::new_editor(state(&scene), SplineEditor::from_config(config));
    let yaw = app.camera.yaw;
    drag(&mut app, MouseButton::Left, 60.0);
    assert_eq!(
        app.camera.yaw, yaw,
        "the left button picks and drags points"
    );
    drag(&mut app, MouseButton::Middle, 60.0);
    assert_ne!(app.camera.yaw, yaw);
}

#[test]
fn watch_asks_the_host_for_the_scene_its_folder_and_the_schemas() {
    let scene = fixture("pbr_test.scene.toml");
    let schemas = vec![PathBuf::from("schemas")];
    let mut app = ViewerApp::new(state(&scene), true);
    assert!(app.watched_files().is_empty() && app.watched_folders().is_empty());

    app.watch = Some(SceneWatch::new(&scene, &schemas));
    assert_eq!(app.watched_files(), vec![PathBuf::from(&scene)]);
    let folders = app.watched_folders();
    assert_eq!(
        folders[0],
        scene_dir_of(&scene),
        "the scene's folder, recursively"
    );
    assert_eq!(&folders[1..], &schemas[..]);

    // #429: a user style's own sources are left to the style's reload
    let dir = scene_dir_of(&scene);
    assert!(!app.file_change_counts(&dir.join("styles").join("duotone.wgsl")));
    assert!(!app.file_change_counts(&dir.join("styles").join("duotone.shader.toml")));
    assert!(app.file_change_counts(Path::new(&scene)));
    assert!(app.file_change_counts(&dir.join("styles").join("dusk.look.toml")));

    app.on_file_changed();
    assert!(app.state.lock().unwrap().needs_reload);
}

#[test]
fn a_bare_scene_name_watches_the_working_directory() {
    assert_eq!(scene_dir_of("demo.scene.toml"), Path::new("."));
}
