//! Android entry point for Flint engine.
//!
//! Provides `android_main()` which extracts APK assets to internal storage,
//! loads the scene, and runs the player event loop.

mod asset_extractor;

use android_activity::AndroidApp;
use winit::event_loop::EventLoop;
use winit::platform::android::EventLoopBuilderExtAndroid;

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

    // Find the scene file
    let scene_path = find_scene(&data_dir).unwrap_or_else(|| {
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

    // Create event loop with Android app handle
    let event_loop = EventLoop::builder()
        .with_android_app(app)
        .build()
        .expect("Failed to create Android event loop");

    log::info!("Entering event loop");
    event_loop.run_app(&mut player).expect("Event loop error");
}

/// Search recursively for a `.scene.toml` file in the given directory.
fn find_scene(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    // First check for a well-known name
    let default = dir.join("scene.toml");
    if default.is_file() {
        return Some(default);
    }

    find_scene_recursive(dir)
}

fn find_scene_recursive(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();

    // Check files at this level first
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(".scene.toml") {
                    return Some(path);
                }
            }
        } else if path.is_dir() {
            subdirs.push(path);
        }
    }

    // Then recurse into subdirectories
    for subdir in subdirs {
        if let Some(found) = find_scene_recursive(&subdir) {
            return Some(found);
        }
    }

    None
}
