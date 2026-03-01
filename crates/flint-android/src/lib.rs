//! Android entry point for Flint engine.
//!
//! Provides `android_main()` which extracts APK assets to internal storage,
//! loads the scene, and runs the player event loop.
//!
//! Games can provide an `android.toml` at their project root to configure
//! the starting scene (and other settings). The file is bundled into the APK
//! and read at runtime.

mod asset_extractor;

use android_activity::input::Axis;
use android_activity::AndroidApp;
use std::sync::Mutex;
use winit::event_loop::EventLoop;
use winit::platform::android::EventLoopBuilderExtAndroid;

// ---- JNI bridge for gamepad trigger/right-stick axes ----

/// Axis values received from Java's dispatchGenericMotionEvent via JNI.
#[derive(Debug, Clone, Copy, Default)]
pub struct GamepadAxesState {
    pub left_trigger: f32,
    pub right_trigger: f32,
    pub right_stick_x: f32,
    pub right_stick_y: f32,
}

static GAMEPAD_AXES: Mutex<GamepadAxesState> = Mutex::new(GamepadAxesState {
    left_trigger: 0.0,
    right_trigger: 0.0,
    right_stick_x: 0.0,
    right_stick_y: 0.0,
});

/// Called from Java: FlintActivity.nativeOnGamepadAxes(lt, rt, rsX, rsY)
#[no_mangle]
pub extern "C" fn Java_com_flint_game_FlintActivity_nativeOnGamepadAxes(
    _env: *mut std::ffi::c_void,
    _class: *mut std::ffi::c_void,
    lt: f32,
    rt: f32,
    rs_x: f32,
    rs_y: f32,
) {
    if let Ok(mut state) = GAMEPAD_AXES.lock() {
        state.left_trigger = lt;
        state.right_trigger = rt;
        state.right_stick_x = rs_x;
        state.right_stick_y = rs_y;
    }
}

/// Read the latest gamepad axes from the JNI bridge.
/// Returns [left_trigger, right_trigger, right_stick_x, right_stick_y].
pub fn read_gamepad_axes() -> [f32; 4] {
    if let Ok(state) = GAMEPAD_AXES.lock() {
        [state.left_trigger, state.right_trigger, state.right_stick_x, state.right_stick_y]
    } else {
        [0.0; 4]
    }
}

/// Android entry point. Called by the GameActivity glue code.
#[no_mangle]
fn android_main(app: AndroidApp) {
    // Initialize Android logging so log::info! etc. show in logcat
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("flint"),
    );

    log::info!("Flint android_main starting");

    // Determine internal data directory
    let data_dir = app
        .internal_data_path()
        .expect("No internal data path available");

    // Extract APK assets to internal storage so std::fs works unchanged.
    // FLINT_APK_VERSION is set by Gradle to a build timestamp, ensuring assets
    // are always re-extracted during dev (defeats Android Auto Backup restoring stale files).
    let version = option_env!("FLINT_APK_VERSION").unwrap_or("dev");
    let asset_manager = app.asset_manager();
    asset_extractor::extract_assets(&asset_manager, &data_dir, version);

    // Load schemas
    let schema_dir = data_dir.join("schemas");
    let engine_schema_dir = data_dir.join("engine").join("schemas");

    let mut schema_paths: Vec<String> = Vec::new();
    if engine_schema_dir.is_dir() {
        schema_paths.push(engine_schema_dir.to_string_lossy().to_string());
    }
    if schema_dir.is_dir() {
        schema_paths.push(schema_dir.to_string_lossy().to_string());
    }

    let schema_path_refs: Vec<&str> = schema_paths.iter().map(|s| s.as_str()).collect();
    let registry = flint_schema::SchemaRegistry::load_from_directories(&schema_path_refs)
        .unwrap_or_else(|e| {
            log::warn!("Failed to load schemas: {e}");
            flint_schema::SchemaRegistry::new()
        });

    // Find the scene file — check android.toml config first, then discover
    let scene_path = find_configured_scene(&data_dir)
        .or_else(|| find_scene(&data_dir))
        .unwrap_or_else(|| {
            log::error!("No .scene.toml file found in {}", data_dir.display());
            panic!("No scene file found in extracted assets");
        });

    log::info!("Loading scene: {}", scene_path.display());

    // Parse scene
    let (world, scene_file) =
        flint_scene::load_scene(&scene_path, &registry).unwrap_or_else(|e| {
            panic!(
                "Failed to load scene {}: {e}",
                scene_path.display()
            );
        });

    // Build PlayerApp
    let scene_path_str = scene_path.to_string_lossy().to_string();
    let scene_input_config = scene_file.scene.input_config.clone();
    let mut player = flint_player::PlayerApp::new(
        world,
        scene_path_str,
        false, // not fullscreen — Android uses native surface size
        None,  // no CLI input config override
        scene_input_config,
    );

    // Set schema paths for scene transitions
    player.set_schema_paths(schema_paths);

    // Register the JNI gamepad axis reader so PlayerApp can poll trigger/right-stick values
    player.set_android_axis_reader(read_gamepad_axes);

    // Apply scene-level camera/post-process if present
    if let Some(camera_def) = scene_file.camera {
        player.scene_camera = Some(camera_def);
    }
    if let Some(pp_def) = scene_file.post_process {
        player.scene_post_process = Some(pp_def);
    }
    if let Some(env) = &scene_file.environment {
        if let Some(skybox) = &env.skybox {
            player.skybox_path = Some(skybox.clone());
        }
    }

    // Enable gamepad trigger and right-stick axes for GameActivity's native event processing.
    // By default only X and Y (left stick) are enabled. Our JNI bridge handles these too,
    // but enabling here provides a native fallback path.
    app.enable_motion_axis(Axis::Ltrigger);
    app.enable_motion_axis(Axis::Rtrigger);
    app.enable_motion_axis(Axis::Z);
    app.enable_motion_axis(Axis::Rz);
    app.enable_motion_axis(Axis::Brake);
    app.enable_motion_axis(Axis::Gas);

    // Create event loop with Android app handle
    let event_loop = EventLoop::builder()
        .with_android_app(app)
        .build()
        .expect("Failed to create Android event loop");

    log::info!("Entering event loop");
    event_loop.run_app(&mut player).expect("Event loop error");
}

/// Read the `scene` field from `android.toml` if present in extracted assets.
fn find_configured_scene(data_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let config_path = data_dir.join("android.toml");
    let content = std::fs::read_to_string(&config_path).ok()?;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("scene") {
            // Parse: scene = "path/to/scene.toml"
            if let Some(val) = trimmed.strip_prefix("scene").and_then(|s| s.trim().strip_prefix('=')) {
                let val = val.trim().trim_matches('"');
                if !val.is_empty() {
                    let scene = data_dir.join(val);
                    if scene.is_file() {
                        log::info!("Using configured scene from android.toml: {val}");
                        return Some(scene);
                    } else {
                        log::warn!("Configured scene not found: {}", scene.display());
                    }
                }
            }
        }
    }

    None
}

/// Search recursively for a `.scene.toml` file in the given directory.
fn find_scene(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    // First check for a well-known name
    let default = dir.join("scene.toml");
    if default.is_file() {
        return Some(default);
    }

    // Collect all scene files recursively, then pick deterministically
    let mut scenes = Vec::new();
    find_scene_recursive(dir, &mut scenes);
    scenes.sort();
    scenes.into_iter().next()
}

fn find_scene_recursive(dir: &std::path::Path, scenes: &mut Vec<std::path::PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut subdirs = Vec::new();

    // Check files at this level first
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(".scene.toml") {
                    scenes.push(path);
                }
            }
        } else if path.is_dir() {
            subdirs.push(path);
        }
    }

    // Then recurse into subdirectories
    for subdir in subdirs {
        find_scene_recursive(&subdir, scenes);
    }
}
