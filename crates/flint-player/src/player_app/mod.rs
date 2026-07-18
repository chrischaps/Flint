//! Player application implementing winit ApplicationHandler
//!
//! Runs the game loop with physics, input, and first-person camera.

mod hud_render;
mod input_config;
pub(crate) mod scene_loading;

use anyhow::{Context, Result};
use flint_animation::AnimationSystem;
use flint_asset::{AssetCatalog, ContentStore};
use flint_audio::AudioSystem;
use flint_debug_ui::DebugPanel as _;
use flint_core::components as comp;

/// Game-side day/night component driven by a script; the player only knows
/// it to offer the F3 time scrubber (see flint-debug-ui tod_panel).
const TIME_OF_DAY_COMPONENT: &str = "time_of_day";
/// Live-tunable camera settings applied to the render camera at scene load
/// and edited through the F3 Camera panel (see flint-debug-ui camera_panel).
const CAMERA_TUNING_COMPONENT: &str = "camera_tuning";
use flint_core::events::TRANSITION_COMPLETE;
use flint_core::Vec3 as FlintVec3;
use flint_ecs::FlintWorld;
use flint_particles::ParticleSystem;
use flint_physics::PhysicsSystem;
use flint_render::model_loader;
use flint_render::{
    Camera, GrassEntityPosition, ParticleDrawData, ParticleInstanceGpu, RenderContext,
    SceneRenderer,
};
use flint_runtime::{
    Binding, GameClock, GameEvent, GameStateMachine, InputConfig, InputState, PersistentStore,
    RebindMode, RuntimeSystem, SystemPolicy,
};
use flint_script::context::{DrawCommand, LogLevel, ScriptCommand};
use flint_script::ScriptSystem;
use gilrs::{EventType, Gilrs};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
#[cfg(target_os = "android")]
use winit::keyboard::NativeKeyCode;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

/// Tracks Android device IDs that produce gamepad button keycodes,
/// assigning each a stable u32 slot for InputState's gamepad API.
#[cfg(target_os = "android")]
struct AndroidGamepadTracker {
    known_device_ids: std::collections::HashSet<winit::event::DeviceId>,
    device_slots: HashMap<winit::event::DeviceId, u32>,
    next_slot: u32,
}

#[cfg(target_os = "android")]
impl AndroidGamepadTracker {
    fn new() -> Self {
        Self {
            known_device_ids: std::collections::HashSet::new(),
            device_slots: HashMap::new(),
            next_slot: 0,
        }
    }

    /// Register a device as a gamepad and return its slot index.
    fn register_device(&mut self, device_id: winit::event::DeviceId) -> u32 {
        self.known_device_ids.insert(device_id);
        *self.device_slots.entry(device_id).or_insert_with(|| {
            let slot = self.next_slot;
            self.next_slot += 1;
            slot
        })
    }

    fn is_gamepad(&self, device_id: winit::event::DeviceId) -> bool {
        self.known_device_ids.contains(&device_id)
    }

    fn slot_for(&self, device_id: winit::event::DeviceId) -> Option<u32> {
        self.device_slots.get(&device_id).copied()
    }
}

/// Scene transition lifecycle phase
#[derive(Debug, Clone)]
enum TransitionPhase {
    /// Normal gameplay
    Idle,
    /// Playing exit transition — scripts draw fade-out visuals
    Exiting { target_scene: String, elapsed: f32 },
    /// Loading the new scene (synchronous, happens in one frame)
    Loading { target_scene: String },
    /// Playing enter transition — scripts draw fade-in visuals
    Entering { elapsed: f32 },
}

impl TransitionPhase {
    fn is_idle(&self) -> bool {
        matches!(self, TransitionPhase::Idle)
    }
}

pub struct PlayerApp {
    // Core state
    pub world: FlintWorld,
    pub scene_path: String,

    // Systems
    pub clock: GameClock,
    pub input: InputState,
    pub physics: PhysicsSystem,
    pub audio: AudioSystem,
    pub animation: AnimationSystem,
    pub particles: ParticleSystem,
    pub script: ScriptSystem,

    // Rendering
    window: Option<Arc<Window>>,
    render_context: Option<RenderContext>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,

    // Skeletal animation: entity_id → asset name for bone matrix updates
    skeletal_entity_assets: HashMap<flint_core::EntityId, String>,

    // HUD + egui overlay
    egui_ctx: egui::Context,
    egui_winit: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,

    // Script-driven 2D draw commands
    draw_commands: Vec<DrawCommand>,
    ui_textures: HashMap<String, egui::TextureHandle>,

    // Asset catalog (optional, for content-addressed asset resolution)
    catalog: Option<AssetCatalog>,
    content_store: Option<ContentStore>,

    // Window options
    pub fullscreen: bool,
    cursor_captured: bool,

    // Environment
    pub skybox_path: Option<String>,

    // Scene-level camera configuration
    pub scene_camera: Option<flint_scene::CameraDef>,

    // Scene-level post-processing overrides
    pub scene_post_process: Option<flint_scene::PostProcessDef>,

    // Script-driven post-processing overrides (applied per-frame before render)
    pp_vignette_override: Option<f32>,
    pp_bloom_override: Option<f32>,
    pp_exposure_override: Option<f32>,
    pp_chromatic_aberration_override: Option<f32>,
    pp_radial_blur_override: Option<f32>,
    pp_ssao_intensity_override: Option<f32>,
    pp_fog_density_override: Option<f32>,
    pp_fog_color_override: Option<[f32; 3]>,

    // Input config layering + remap persistence
    input_config_override: Option<String>,
    scene_input_config: Option<String>,
    input_config_paths: Option<InputConfigPaths>,
    user_override_config: InputConfig,
    pending_rebind: Option<PendingRebind>,

    // Optional gamepad backend
    gilrs: Option<Gilrs>,

    // State machine + persistence (survive scene transitions)
    state_machine: GameStateMachine,
    persistent_store: PersistentStore,

    // Scene transition lifecycle
    transition_phase: TransitionPhase,

    // Schema paths preserved across transitions
    schema_paths: Vec<String>,

    // Terrain data for height queries
    terrain: Option<(flint_terrain::Terrain, flint_terrain::TerrainConfig)>,

    // Debug overlay panels (F3 toggle)
    debug_panels: Vec<Box<dyn flint_debug_ui::DebugPanel>>,

    // Rendering stats overlay (F2 toggle)
    show_stats: bool,
    stats_frame_times: std::collections::VecDeque<f64>,

    // Procedural generation resolver for runtime asset resolution
    procgen_resolver: flint_procgen::ProcGenResolver,

    // Chunk streaming: chunk_id → spawned entity IDs
    loaded_chunks: HashMap<String, Vec<flint_core::EntityId>>,

    // Android gamepad tracking (maps winit DeviceId to InputState gamepad slots)
    #[cfg(target_os = "android")]
    android_gamepad: AndroidGamepadTracker,

    // Android JNI axis reader for trigger/right-stick values from GameActivity
    #[cfg(target_os = "android")]
    android_axis_reader: Option<fn() -> [f32; 4]>,
}

impl PlayerApp {
    pub fn new(
        world: FlintWorld,
        scene_path: String,
        fullscreen: bool,
        input_config_override: Option<String>,
        scene_input_config: Option<String>,
    ) -> Self {
        Self {
            world,
            scene_path,
            clock: GameClock::new(),
            input: InputState::new(),
            physics: PhysicsSystem::new(),
            audio: AudioSystem::new(),
            animation: AnimationSystem::new(),
            particles: ParticleSystem::new(),
            script: ScriptSystem::new(),
            window: None,
            render_context: None,
            scene_renderer: None,
            camera: Camera::new(),
            skeletal_entity_assets: HashMap::new(),
            egui_ctx: egui::Context::default(),
            egui_winit: None,
            egui_renderer: None,
            draw_commands: Vec::new(),
            ui_textures: HashMap::new(),
            catalog: AssetCatalog::load_from_directory("assets").ok(),
            content_store: Some(ContentStore::new(".flint/assets")),
            fullscreen,
            cursor_captured: false,
            skybox_path: None,
            scene_camera: None,
            scene_post_process: None,
            pp_vignette_override: None,
            pp_bloom_override: None,
            pp_exposure_override: None,
            pp_chromatic_aberration_override: None,
            pp_radial_blur_override: None,
            pp_ssao_intensity_override: None,
            pp_fog_density_override: None,
            pp_fog_color_override: None,
            input_config_override,
            scene_input_config,
            input_config_paths: None,
            user_override_config: InputConfig {
                version: 1,
                game_id: String::new(),
                actions: Default::default(),
            },
            pending_rebind: None,
            gilrs: None,
            state_machine: GameStateMachine::new(),
            persistent_store: PersistentStore::new(),
            transition_phase: TransitionPhase::Idle,
            schema_paths: Vec::new(),
            terrain: None,
            debug_panels: Vec::new(),
            show_stats: false,
            stats_frame_times: std::collections::VecDeque::new(),
            procgen_resolver: flint_procgen::ProcGenResolver::new(),
            loaded_chunks: HashMap::new(),
            #[cfg(target_os = "android")]
            android_gamepad: AndroidGamepadTracker::new(),
            #[cfg(target_os = "android")]
            android_axis_reader: None,
        }
    }

    /// Map Android AKEYCODE_BUTTON_* values to gilrs-format button names.
    #[cfg(target_os = "android")]
    fn android_keycode_to_gamepad_button(keycode: u32) -> Option<&'static str> {
        match keycode {
            96 => Some("South"),          // AKEYCODE_BUTTON_A
            97 => Some("East"),           // AKEYCODE_BUTTON_B
            99 => Some("West"),           // AKEYCODE_BUTTON_X
            100 => Some("North"),         // AKEYCODE_BUTTON_Y
            102 => Some("LeftTrigger"),   // AKEYCODE_BUTTON_L1
            103 => Some("RightTrigger"),  // AKEYCODE_BUTTON_R1
            104 => Some("LeftTrigger2"),  // AKEYCODE_BUTTON_L2
            105 => Some("RightTrigger2"), // AKEYCODE_BUTTON_R2
            106 => Some("LeftThumb"),     // AKEYCODE_BUTTON_THUMBL
            107 => Some("RightThumb"),    // AKEYCODE_BUTTON_THUMBR
            108 => Some("Start"),         // AKEYCODE_BUTTON_START
            109 => Some("Select"),        // AKEYCODE_BUTTON_SELECT
            110 => Some("Mode"),          // AKEYCODE_BUTTON_MODE
            _ => None,
        }
    }

    /// Set the schema paths used for scene loading (preserved across transitions).
    pub fn set_schema_paths(&mut self, paths: Vec<String>) {
        self.schema_paths = paths;
    }

    /// Register a function that reads gamepad trigger/right-stick axes from the
    /// Android JNI bridge. Called each frame in `poll_gamepad_events()`.
    /// Returns [left_trigger, right_trigger, right_stick_x, right_stick_y].
    #[cfg(target_os = "android")]
    pub fn set_android_axis_reader(&mut self, reader: fn() -> [f32; 4]) {
        self.android_axis_reader = Some(reader);
    }

    /// Apply scene-level camera configuration from `CameraDef`
    fn apply_camera_def(&mut self) {
        if let Some(cam) = &self.scene_camera {
            if cam.projection == "orthographic" {
                self.camera.orthographic = true;
                if cam.ortho_height > 0.0 {
                    self.camera.ortho_height = cam.ortho_height;
                }
            } else {
                self.camera.orthographic = false;
                self.camera.ortho_height = 0.0;
            }
            if let Some(pos) = cam.position {
                self.camera.position = flint_core::Vec3::new(pos[0], pos[1], pos[2]);
            }
            if let Some(target) = cam.target {
                self.camera.target = flint_core::Vec3::new(target[0], target[1], target[2]);
            }
            if let Some(fov) = cam.fov {
                self.camera.fov = fov;
            }
            if let Some(near) = cam.near {
                self.camera.near = near;
            }
            if let Some(far) = cam.far {
                self.camera.far = far;
            }

            // Derive orbit parameters from position/target so update_orbit() stays consistent
            if cam.position.is_some() {
                let dir = self.camera.position - self.camera.target;
                let dist = dir.length();
                if dist > 0.001 {
                    self.camera.distance = dist;
                    let n = dir * (1.0 / dist);
                    self.camera.pitch = n.y.asin();
                    self.camera.yaw = n.x.atan2(n.z);
                }
            }
        }
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let window_attrs = Window::default_attributes().with_title("Flint Player");
        #[cfg(not(target_os = "android"))]
        let window_attrs = window_attrs.with_inner_size(PhysicalSize::new(1280, 720));

        let window = Arc::new(
            event_loop
                .create_window(window_attrs)
                .context("Failed to create player window")?,
        );

        if self.fullscreen {
            window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
        }

        self.window = Some(window.clone());

        // Initialize rendering
        let render_context = pollster::block_on(RenderContext::new(window.clone()))
            .context("Failed to initialize render context")?;

        self.camera.aspect = render_context.aspect_ratio();
        self.camera.fov = 70.0; // Slightly wider FOV for first-person

        let mut scene_renderer = SceneRenderer::new(&render_context, Default::default());

        // Rebuild component index as a safety net after scene loading
        self.world.rebuild_component_index();

        // Set scene_dir for font/texture resolution
        scene_renderer.scene_dir = Path::new(&self.scene_path)
            .parent()
            .map(|p| p.to_path_buf());

        // Load models from world (including skeletal data)
        let config = build_model_load_config(
            &self.scene_path,
            &self.world,
            self.catalog.as_ref(),
            self.content_store.as_ref(),
        );

        // Discover procgen specs and resolve unresolved assets before model loading
        {
            let scene_dir = Path::new(&self.scene_path).parent().unwrap_or(Path::new("."));
            let mut spec_dirs = vec![scene_dir.join("specs")];
            if let Some(parent) = scene_dir.parent() {
                spec_dirs.push(parent.join("specs"));
            }
            spec_dirs.push(scene_dir.join("models"));
            let dir_refs: Vec<&Path> = spec_dirs.iter().filter(|d| d.is_dir()).map(|d| d.as_path()).collect();
            self.procgen_resolver.discover_and_index(&dir_refs);

            resolve_procgen_assets(
                &self.world,
                &mut self.procgen_resolver,
                &mut scene_renderer,
                &render_context.device,
                &render_context.queue,
                &config,
            );
        }

        let load_result = model_loader::load_models_from_world(
            &mut self.world,
            &mut scene_renderer,
            &render_context.device,
            &render_context.queue,
            &config,
        );
        register_skeletal_data(&load_result, &mut self.animation);
        register_node_animation_data(&load_result, &mut self.animation);
        self.skeletal_entity_assets = load_result.skinned_entities;
        scene_renderer.update_from_world(&self.world, &render_context.device);

        // Load input configs with deterministic layering.
        self.configure_input_bindings()
            .unwrap_or_else(|e| tracing::warn!("Input config load error: {e:#}"));

        // Initialize gamepad backend (best-effort).
        self.gilrs = Gilrs::new().ok();

        // Initialize egui for HUD overlay
        let egui_winit = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        let egui_renderer = egui_wgpu::Renderer::new(
            &render_context.device,
            render_context.config.format,
            None,
            1,
            false,
        );
        self.egui_winit = Some(egui_winit);
        self.egui_renderer = Some(egui_renderer);

        // Generate procedural geometry from spline + spline_mesh entities
        crate::spline_gen::load_splines(
            &self.scene_path,
            &mut self.world,
            &mut scene_renderer,
            Some(&mut self.physics),
            &render_context.device,
        );

        // Refresh renderer with any new procedural meshes
        scene_renderer.update_from_world(&self.world, &render_context.device);

        // Load skybox if configured
        if let Some(skybox_rel) = &self.skybox_path {
            let scene_dir = Path::new(&self.scene_path)
                .parent()
                .unwrap_or_else(|| Path::new("."));

            // Search scene dir first, then parent (game root)
            let skybox_path = {
                let p = scene_dir.join(skybox_rel);
                if p.exists() {
                    p
                } else if let Some(parent) = scene_dir.parent() {
                    parent.join(skybox_rel)
                } else {
                    p
                }
            };

            if skybox_path.exists() {
                scene_renderer.load_skybox(
                    &render_context.device,
                    &render_context.queue,
                    &skybox_path,
                );
            } else {
                tracing::warn!("Skybox file not found: {}", skybox_path.display());
            }
        }

        // Load terrain from world
        self.debug_panels.clear();
        self.load_terrain_from_world(
            &render_context.device,
            &render_context.queue,
            &mut scene_renderer,
        );

        // Set terrain height callback for scripts
        self.update_terrain_height_fn();

        // Apply scene-level camera configuration
        self.apply_camera_def();
        self.apply_camera_tuning();

        // Apply scene-level post-processing config
        if let Some(pp_def) = &self.scene_post_process {
            scene_renderer
                .set_post_process_config(scene_loading::post_process_config_from_def(pp_def));
            scene_renderer
                .ensure_kuwahara_resources(&render_context.device, &render_context.queue);
        }

        self.render_context = Some(render_context);
        self.scene_renderer = Some(scene_renderer);

        // Initialize physics
        self.physics
            .initialize(&mut self.world)
            .context("Failed to initialize physics")?;

        // Initialize audio
        load_audio_from_world(&self.world, &mut self.audio, &self.scene_path);
        self.audio
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Audio init failed: {:?}", e));

        // Initialize animation
        load_animations_from_world(&self.scene_path, &mut self.animation);
        load_sprite_animations_from_world(&self.scene_path, &mut self.animation);
        self.animation
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Animation init failed: {:?}", e));

        // Initialize particles
        self.particles
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Particles init failed: {:?}", e));

        // Initialize scripting (state_scope required so on_init can access persist store)
        load_scripts_from_world(&self.scene_path, &mut self.script);
        self.script.set_current_scene(&self.scene_path);
        {
            let _state_scope = self.script.state_scope(
                &mut self.state_machine,
                &mut self.persistent_store,
                &self.physics,
            );
            self.script
                .initialize(&mut self.world)
                .unwrap_or_else(|e| tracing::warn!("Script init failed: {:?}", e));
        }

        // Capture cursor for first-person look (only if FPS player exists).
        // On Android, always set cursor_captured = true so touch input flows
        // without requiring a click-to-capture gate.
        #[cfg(target_os = "android")]
        {
            self.cursor_captured = true;
        }
        #[cfg(not(target_os = "android"))]
        if self.physics.has_player_entity() {
            self.capture_cursor();
        }

        Ok(())
    }

    /// Start capture mode for "press next control to bind" remapping.
    pub fn begin_rebind_capture(&mut self, action: impl Into<String>, mode: RebindMode) {
        self.pending_rebind = Some(PendingRebind {
            action: action.into(),
            mode,
        });
    }

    /// Scan world entities for `terrain` component, load heightmap, generate chunks,
    /// upload GPU resources, and register physics colliders.
    fn load_terrain_from_world(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene_renderer: &mut SceneRenderer,
    ) {
        let mut grass_info = None;
        load_terrain_from_world_inner(
            &self.world,
            &self.scene_path,
            device,
            queue,
            scene_renderer,
            &mut self.physics,
            &mut self.terrain,
            &mut grass_info,
        );

        // Create grass debug panel if grass was loaded
        if let Some(info) = grass_info {
            let panel = flint_debug_ui::GrassDebugPanel::new(
                info.config,
                std::path::PathBuf::from(&self.scene_path),
                info.terrain_entity_name,
            );
            self.debug_panels.push(Box::new(panel));
        }

        self.create_ocean_debug_panel();
        self.create_tod_debug_panel();
        self.create_camera_debug_panel();
    }

    /// Create the ocean tuning panel if the scene has an `ocean` component.
    fn create_ocean_debug_panel(&mut self) {
        let Some(&entity_id) = self
            .world
            .entities_with_component(comp::OCEAN)
            .iter()
            .next()
        else {
            return;
        };
        let Some(name) = self.world.get_name(entity_id).map(str::to_string) else {
            return;
        };
        let Some(ocean_comp) = self
            .world
            .get_components(entity_id)
            .and_then(|comps| comps.get(comp::OCEAN).cloned())
        else {
            return;
        };
        let config = flint_debug_ui::OceanPanelConfig::from_component(&ocean_comp);
        let panel = flint_debug_ui::OceanDebugPanel::new(
            config,
            std::path::PathBuf::from(&self.scene_path),
            name,
        );
        self.debug_panels.push(Box::new(panel));
    }

    /// Create the time-of-day scrubber if the scene has a `time_of_day`
    /// component (a game-side convention — see flint-debug-ui tod_panel).
    fn create_tod_debug_panel(&mut self) {
        let Some(&entity_id) = self
            .world
            .entities_with_component(TIME_OF_DAY_COMPONENT)
            .iter()
            .next()
        else {
            return;
        };
        let Some(name) = self.world.get_name(entity_id).map(str::to_string) else {
            return;
        };
        let Some(tod_comp) = self
            .world
            .get_components(entity_id)
            .and_then(|comps| comps.get(TIME_OF_DAY_COMPONENT).cloned())
        else {
            return;
        };
        let config = flint_debug_ui::TimeOfDayPanelConfig::from_component(&tod_comp);
        let panel = flint_debug_ui::TimeOfDayDebugPanel::new(
            config,
            std::path::PathBuf::from(&self.scene_path),
            name,
        );
        self.debug_panels.push(Box::new(panel));
    }

    /// Create the camera tuning panel if the scene has a `camera_tuning`
    /// component.
    fn create_camera_debug_panel(&mut self) {
        let Some(&entity_id) = self
            .world
            .entities_with_component(CAMERA_TUNING_COMPONENT)
            .iter()
            .next()
        else {
            return;
        };
        let Some(name) = self.world.get_name(entity_id).map(str::to_string) else {
            return;
        };
        let Some(cam_comp) = self
            .world
            .get_components(entity_id)
            .and_then(|comps| comps.get(CAMERA_TUNING_COMPONENT).cloned())
        else {
            return;
        };
        let config = flint_debug_ui::CameraPanelConfig::from_component(&cam_comp);
        let panel = flint_debug_ui::CameraDebugPanel::new(
            config,
            std::path::PathBuf::from(&self.scene_path),
            name,
        );
        self.debug_panels.push(Box::new(panel));
    }

    /// Apply the scene's `camera_tuning` component to the render camera.
    /// Called after `apply_camera_def()` so the tuning value wins when a
    /// scene declares both.
    fn apply_camera_tuning(&mut self) {
        let Some(&entity_id) = self
            .world
            .entities_with_component(CAMERA_TUNING_COMPONENT)
            .iter()
            .next()
        else {
            return;
        };
        if let Some(fov) = self
            .world
            .get_components(entity_id)
            .and_then(|comps| comps.get(CAMERA_TUNING_COMPONENT))
            .and_then(|c| c.get("fov_deg"))
            .and_then(flint_core::toml_util::toml_f32)
        {
            self.camera.fov = fov;
        }
    }

    /// Update the terrain height callback on the script system.
    /// Call after terrain loading or clearing.
    fn update_terrain_height_fn(&mut self) {
        if let Some((ref terrain, ref config)) = self.terrain {
            let heights = terrain.heightmap.clone_heights();
            let hm_w = terrain.heightmap.width;
            let hm_d = terrain.heightmap.depth;
            let world_w = config.width;
            let world_d = config.depth;
            let height_scale = config.height_scale;
            self.script
                .set_terrain_height_fn(Some(Box::new(move |x: f32, z: f32| {
                    // Convert world coords to normalized UV
                    let u = x / world_w;
                    let v = z / world_d;
                    // Bilinear sample from heights array
                    let fx = u * (hm_w as f32 - 1.0);
                    let fz = v * (hm_d as f32 - 1.0);
                    let ix = (fx as u32).min(hm_w - 2);
                    let iz = (fz as u32).min(hm_d - 2);
                    let tx = fx - ix as f32;
                    let tz = fz - iz as f32;
                    let idx = |col: u32, row: u32| -> f32 { heights[(row * hm_w + col) as usize] };
                    let h00 = idx(ix, iz);
                    let h10 = idx(ix + 1, iz);
                    let h01 = idx(ix, iz + 1);
                    let h11 = idx(ix + 1, iz + 1);
                    let h = h00 * (1.0 - tx) * (1.0 - tz)
                        + h10 * tx * (1.0 - tz)
                        + h01 * (1.0 - tx) * tz
                        + h11 * tx * tz;
                    h * height_scale
                })));
        } else {
            self.script.set_terrain_height_fn(None);
        }
    }

    fn configure_input_bindings(&mut self) -> Result<()> {
        self.input
            .load_bindings(InputConfig::built_in_defaults())
            .context("failed to load built-in input defaults")?;

        let paths = resolve_input_paths(
            Path::new(&self.scene_path),
            self.scene_input_config.as_deref(),
            self.input_config_override.as_deref(),
        );

        if let Some(path) = &paths.game_default {
            if path.exists() {
                let cfg = InputConfig::load_from_file(path).with_context(|| {
                    format!("failed to load game input config '{}'", path.display())
                })?;
                self.input
                    .merge_bindings(cfg)
                    .context("failed to merge game input config")?;
            }
        }

        if let Some(path) = &paths.user_override {
            if path.exists() {
                let cfg = InputConfig::load_from_file(path).with_context(|| {
                    format!("failed to load user input config '{}'", path.display())
                })?;
                self.user_override_config = cfg.clone();
                self.input
                    .merge_bindings(cfg)
                    .context("failed to merge user input config")?;
            }
        }

        if let Some(path) = &paths.cli_override {
            if path.exists() {
                let cfg = InputConfig::load_from_file(path).with_context(|| {
                    format!("failed to load CLI input config '{}'", path.display())
                })?;
                self.input
                    .merge_bindings(cfg)
                    .context("failed to merge CLI input config")?;
            }
        }

        if self.user_override_config.version == 0 {
            self.user_override_config.version = 1;
        }
        if self.user_override_config.game_id.trim().is_empty() {
            self.user_override_config.game_id = self.input.config().game_id.clone();
        }

        self.input_config_paths = Some(paths);
        Ok(())
    }

    fn poll_gamepad_events(&mut self) {
        let mut events = Vec::new();
        if let Some(gilrs) = &mut self.gilrs {
            while let Some(event) = gilrs.next_event() {
                events.push(event);
            }
        }

        for event in events {
            let gamepad = gamepad_id_to_u32(event.id);
            match event.event {
                EventType::ButtonPressed(button, _) => {
                    let name = format!("{button:?}");
                    if self.try_capture_rebind(Binding::GamepadButton {
                        button: name.clone(),
                        gamepad: flint_runtime::GamepadSelector::Any,
                    }) {
                        continue;
                    }
                    self.input.process_gamepad_button_down(gamepad, name);
                }
                EventType::ButtonReleased(button, _) => {
                    let name = format!("{button:?}");
                    self.input.process_gamepad_button_up(gamepad, name);
                }
                EventType::ButtonChanged(button, value, _) => {
                    let name = format!("{button:?}");
                    self.input
                        .process_gamepad_button_changed(gamepad, name, value);
                }
                EventType::AxisChanged(axis, value, _) => {
                    let name = format!("{axis:?}");
                    if self.pending_rebind.is_some() && value.abs() >= 0.45 {
                        let direction = if value < 0.0 {
                            Some(flint_runtime::AxisDirection::Negative)
                        } else {
                            Some(flint_runtime::AxisDirection::Positive)
                        };
                        if self.try_capture_rebind(Binding::GamepadAxis {
                            axis: name.clone(),
                            gamepad: flint_runtime::GamepadSelector::Any,
                            deadzone: 0.15,
                            scale: 1.0,
                            invert: false,
                            threshold: Some(0.35),
                            direction,
                        }) {
                            continue;
                        }
                    }
                    self.input.process_gamepad_axis(gamepad, name, value);
                }
                EventType::Disconnected => {
                    self.input.clear_gamepad(gamepad);
                }
                _ => {}
            }
        }

        // Android: poll trigger/right-stick axes from JNI bridge
        #[cfg(target_os = "android")]
        if let Some(reader) = self.android_axis_reader {
            let axes = reader();
            let [lt, rt, rs_x, rs_y] = axes;
            // Use slot 0 — the JNI bridge doesn't distinguish multiple gamepads
            let slot = 0u32;
            // Feed triggers as LeftZ/RightZ to match existing input.toml bindings
            // (accelerate = gamepad_axis "RightZ", brake = gamepad_axis "LeftZ")
            self.input.process_gamepad_axis(slot, "LeftZ", lt);
            self.input.process_gamepad_axis(slot, "RightZ", rt);
            // Right stick axes for future use
            self.input.process_gamepad_axis(slot, "RightStickX", rs_x);
            self.input.process_gamepad_axis(slot, "RightStickY", rs_y);
        }
    }

    fn try_capture_rebind(&mut self, binding: Binding) -> bool {
        let Some(pending) = self.pending_rebind.take() else {
            return false;
        };

        if let Err(e) = self
            .input
            .rebind_action(&pending.action, binding, pending.mode)
        {
            tracing::warn!("Failed to rebind action '{}': {:?}", pending.action, e);
            return true;
        }

        if let Some(action_cfg) = self.input.action_config(&pending.action) {
            self.user_override_config
                .actions
                .insert(pending.action.clone(), action_cfg);
        }
        if self.user_override_config.game_id.trim().is_empty() {
            self.user_override_config.game_id = self.input.config().game_id.clone();
        }

        if let Err(e) = self.persist_user_overrides() {
            tracing::warn!("Failed to save input overrides: {e:#}");
        }

        true
    }

    fn persist_user_overrides(&mut self) -> Result<()> {
        let Some(paths) = &mut self.input_config_paths else {
            return Ok(());
        };

        let mut target = paths.user_override.clone().unwrap_or_else(|| {
            fallback_user_override_path(Path::new(&self.scene_path), &self.input.config().game_id)
                .unwrap_or_else(|| PathBuf::from(".flint/input.user.toml"))
        });

        if let Err(err) = write_user_override_file(&target, &self.user_override_config) {
            let Some(fallback) = fallback_user_override_path(
                Path::new(&self.scene_path),
                &self.input.config().game_id,
            ) else {
                return Err(err);
            };
            if fallback != target {
                write_user_override_file(&fallback, &self.user_override_config)?;
                target = fallback;
            } else {
                return Err(err);
            }
        }

        paths.user_override = Some(target);
        Ok(())
    }

    fn capture_cursor(&mut self) {
        if let Some(window) = &self.window {
            #[cfg(not(target_os = "android"))]
            {
                // Try confined first, then locked
                let _ = window
                    .set_cursor_grab(CursorGrabMode::Confined)
                    .or_else(|_| window.set_cursor_grab(CursorGrabMode::Locked));
                window.set_cursor_visible(false);
            }
            self.cursor_captured = true;
        }
    }

    fn release_cursor(&mut self) {
        if let Some(window) = &self.window {
            #[cfg(not(target_os = "android"))]
            {
                let _ = window.set_cursor_grab(CursorGrabMode::None);
                window.set_cursor_visible(true);
            }
            self.cursor_captured = false;
        }
    }

    fn render(&mut self) {
        // On Android, window may be None between suspended() and resumed()
        if self.window.is_none() {
            return;
        }
        let Some(context) = &self.render_context else {
            return;
        };
        let Some(renderer) = &mut self.scene_renderer else {
            return;
        };

        let output = match context.surface.get_current_texture() {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                return;
            }
            Err(e) => {
                tracing::warn!("Surface error: {:?}", e);
                return;
            }
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // Apply script-driven post-processing overrides before rendering
        if self.pp_vignette_override.is_some()
            || self.pp_bloom_override.is_some()
            || self.pp_exposure_override.is_some()
            || self.pp_chromatic_aberration_override.is_some()
            || self.pp_radial_blur_override.is_some()
            || self.pp_ssao_intensity_override.is_some()
            || self.pp_fog_density_override.is_some()
            || self.pp_fog_color_override.is_some()
        {
            let mut config = renderer.post_process_config().clone();
            if let Some(v) = self.pp_vignette_override {
                config.vignette_enabled = v > 0.001;
                config.vignette_intensity = v;
            }
            if let Some(b) = self.pp_bloom_override {
                config.bloom_intensity = b;
            }
            if let Some(e) = self.pp_exposure_override {
                config.exposure = e;
            }
            if let Some(ca) = self.pp_chromatic_aberration_override {
                config.chromatic_aberration = ca;
            }
            if let Some(rb) = self.pp_radial_blur_override {
                config.radial_blur = rb;
            }
            if let Some(si) = self.pp_ssao_intensity_override {
                config.ssao_intensity = si;
            }
            if let Some(fd) = self.pp_fog_density_override {
                config.fog_density = fd;
            }
            if let Some(fc) = self.pp_fog_color_override {
                config.fog_color = fc;
            }
            renderer.set_post_process_config(config);
        }

        // Push debug panel grass config changes to renderer
        for panel in &mut self.debug_panels {
            if panel.name() == "Grass Debug" && panel.is_dirty() {
                let grass_panel = panel
                    .as_any_mut()
                    .downcast_mut::<flint_debug_ui::GrassDebugPanel>()
                    .unwrap();
                if grass_panel.density_changed() {
                    renderer.reload_grass_config(&context.device, grass_panel.config().clone());
                    grass_panel.clear_density_changed();
                } else {
                    renderer.set_grass_config(grass_panel.config().clone());
                }
                panel.clear_dirty();
            }
        }

        // Push ocean panel edits into the world's `ocean` component — the
        // renderer extraction and the script API both read from there.
        for panel in &mut self.debug_panels {
            if panel.name() == "Ocean Debug" && panel.is_dirty() {
                let ocean_panel = panel
                    .as_any_mut()
                    .downcast_mut::<flint_debug_ui::OceanDebugPanel>()
                    .unwrap();
                if let Some(entity_id) = self.world.get_id(ocean_panel.entity_name()) {
                    if let Some(comps) = self.world.get_components_mut(entity_id) {
                        for (field, value) in ocean_panel.config().to_fields() {
                            comps.set_field(comp::OCEAN, field, value);
                        }
                    }
                }
                panel.clear_dirty();
            }
        }

        // Time-of-day panel: push edits into the component; while auto time
        // advances (the game script owns time_hours), pull it back so the
        // slider tracks the sky instead of going stale.
        for panel in &mut self.debug_panels {
            if panel.name() != "Time of Day" {
                continue;
            }
            let tod_panel = panel
                .as_any_mut()
                .downcast_mut::<flint_debug_ui::TimeOfDayDebugPanel>()
                .unwrap();
            let Some(entity_id) = self.world.get_id(tod_panel.entity_name()) else {
                continue;
            };
            if tod_panel.is_dirty() {
                if let Some(comps) = self.world.get_components_mut(entity_id) {
                    for (field, value) in tod_panel.config().to_fields() {
                        comps.set_field(TIME_OF_DAY_COMPONENT, field, value);
                    }
                }
                tod_panel.clear_dirty();
            } else if let Some(hours) = self
                .world
                .get_components(entity_id)
                .and_then(|comps| comps.get(TIME_OF_DAY_COMPONENT))
                .and_then(|c| c.get("time_hours"))
                .and_then(flint_core::toml_util::toml_f32)
            {
                tod_panel.sync_time(hours);
            }
        }

        // Camera panel: edits drive the live render camera and the component
        // (so Commit to File persists them); while idle, track the camera so
        // script FOV overrides don't leave the slider stale.
        for panel in &mut self.debug_panels {
            if panel.name() != "Camera" {
                continue;
            }
            let cam_panel = panel
                .as_any_mut()
                .downcast_mut::<flint_debug_ui::CameraDebugPanel>()
                .unwrap();
            if cam_panel.is_dirty() {
                self.camera.fov = cam_panel.config().fov_deg;
                if let Some(entity_id) = self.world.get_id(cam_panel.entity_name()) {
                    if let Some(comps) = self.world.get_components_mut(entity_id) {
                        for (field, value) in cam_panel.config().to_fields() {
                            comps.set_field(CAMERA_TUNING_COMPONENT, field, value);
                        }
                    }
                }
                cam_panel.clear_dirty();
            } else {
                cam_panel.sync_fov(self.camera.fov);
            }
        }

        if let Err(e) = renderer.render(context, &self.camera, &view) {
            tracing::warn!("Render error: {:?}", e);
        }

        // Render egui HUD overlay on top of the 3D scene
        self.render_hud(&view);

        output.present();
    }

    fn tick(&mut self) {
        self.poll_gamepad_events();

        // Advance game clock
        self.clock.tick();

        // Advance transition phase timing
        self.advance_transition();

        // Read active state config to decide which systems run
        let config = self.state_machine.active_config().clone();

        let has_fps_player = self.physics.has_player_entity();

        // Fixed-timestep physics loop (skip when paused, but still consume steps to avoid spiral)
        while self.clock.should_fixed_update() {
            let dt = self.clock.fixed_timestep;

            if config.physics == SystemPolicy::Run {
                if has_fps_player {
                    self.physics
                        .update_character(&self.input, &mut self.world, dt);
                }
                self.physics
                    .fixed_update(&mut self.world, dt)
                    .unwrap_or_else(|e| tracing::warn!("Physics error: {:?}", e));
            }

            self.clock.consume_fixed_step();
        }

        // Update camera from player character position (FPS mode)
        if has_fps_player {
            let cam_pos = self.physics.camera_position(&self.world);
            let cam_target = self.physics.camera_target(cam_pos);
            self.camera.update_first_person(
                cam_pos,
                self.physics.camera_yaw(),
                self.physics.camera_pitch(),
            );
            self.camera.target = cam_target;

            if config.audio == SystemPolicy::Run {
                self.audio.update_listener(
                    cam_pos,
                    self.physics.camera_yaw(),
                    self.physics.camera_pitch(),
                );
            }
        }

        // Process physics events — scripts + audio both consume them
        // Always collect events (input always processed so pause/unpause keybinds work)
        let mut game_events = self.physics.drain_events();
        for action in self.input.actions_just_pressed() {
            game_events.push(flint_runtime::GameEvent::ActionPressed(action));
        }
        for action in self.input.actions_just_released() {
            game_events.push(flint_runtime::GameEvent::ActionReleased(action));
        }

        // Set state machine + persistent store + physics pointers for script access (RAII guard)
        let _state_scope = self.script.state_scope(
            &mut self.state_machine,
            &mut self.persistent_store,
            &self.physics,
        );
        self.script.set_current_scene(&self.scene_path);

        // Set transition state for script access
        match &self.transition_phase {
            TransitionPhase::Idle => {
                self.script.set_transition_state(-1.0, "idle");
            }
            TransitionPhase::Exiting { elapsed, .. } => {
                self.script.set_transition_state(*elapsed as f64, "exiting");
            }
            TransitionPhase::Loading { .. } => {
                self.script.set_transition_state(1.0, "loading");
            }
            TransitionPhase::Entering { elapsed } => {
                self.script
                    .set_transition_state(*elapsed as f64, "entering");
            }
        }

        // Script system: provide camera context, then run updates
        self.script
            .set_camera(self.camera.position_array(), self.camera.forward_vector());
        self.script.provide_context(
            &self.input,
            &game_events,
            self.clock.total_time,
            self.clock.delta_time,
        );
        let screen_rect = self.egui_ctx.screen_rect();
        self.script
            .set_screen_size(screen_rect.width(), screen_rect.height());

        // Sync loaded chunk IDs so scripts can query is_chunk_loaded()
        let chunk_ids: HashSet<String> = self.loaded_chunks.keys().cloned().collect();
        self.script.set_loaded_chunk_ids(chunk_ids);

        // Only run on_update when scripts are not paused
        if config.scripts == SystemPolicy::Run {
            self.script
                .update(&mut self.world, self.clock.delta_time)
                .unwrap_or_else(|e| tracing::warn!("Script error: {:?}", e));
        }

        // Apply script camera overrides (for non-FPS camera modes like chase camera)
        let (
            cam_pos_override,
            cam_target_override,
            cam_fov_override,
            cam_ortho_override,
            cam_ortho_height_override,
        ) = self.script.take_camera_overrides();
        if let Some(pos) = cam_pos_override {
            self.camera.position = flint_core::Vec3::new(pos[0], pos[1], pos[2]);
        }
        if let Some(target) = cam_target_override {
            self.camera.target = flint_core::Vec3::new(target[0], target[1], target[2]);
        }
        if let Some(fov) = cam_fov_override {
            self.camera.fov = fov;
        }
        if let Some(ortho) = cam_ortho_override {
            self.camera.orthographic = ortho;
        }
        if let Some(height) = cam_ortho_height_override {
            self.camera.ortho_height = height;
        }

        // Update audio listener for script-driven cameras (chase cam, etc.)
        if !has_fps_player && cam_pos_override.is_some() {
            let cam_pos = self.camera.position;
            let dir = flint_core::Vec3::new(
                self.camera.target.x - cam_pos.x,
                self.camera.target.y - cam_pos.y,
                self.camera.target.z - cam_pos.z,
            );
            let yaw = dir.x.atan2(dir.z);
            let horiz = (dir.x * dir.x + dir.z * dir.z).sqrt();
            let pitch = (-dir.y).atan2(horiz);
            self.audio.update_listener(cam_pos, yaw, pitch);
        }

        // on_draw_ui() ALWAYS runs (pause menus, transition visuals need to draw)
        self.script.call_draw_uis(&mut self.world);

        let script_commands = self.script.drain_commands();
        self.process_script_commands(script_commands);

        // Collect draw commands for this frame (scripts + data-driven UI)
        let mut commands = self.script.drain_draw_commands();
        let screen_rect = self.egui_ctx.screen_rect();
        let ui_commands = self
            .script
            .generate_ui_draw_commands(screen_rect.width(), screen_rect.height());
        commands.extend(ui_commands);
        self.draw_commands = commands;

        // Audio triggers from game events (skip when paused)
        if config.audio == SystemPolicy::Run {
            self.audio.process_events(&game_events, &self.world);
            self.audio
                .update(&mut self.world, self.clock.delta_time)
                .ok();
        }

        // Advance animations (skip when paused)
        if config.animation == SystemPolicy::Run {
            self.animation
                .update(&mut self.world, self.clock.delta_time)
                .ok();

            // Deliver sprite animation end events to scripts
            let sprite_events = self.animation.drain_sprite_events();
            if !sprite_events.is_empty() {
                self.script
                    .call_sprite_anim_ends(&mut self.world, &sprite_events);
            }
        }

        // Push skeletal bone matrices to GPU
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            for (entity_id, asset_name) in &self.skeletal_entity_assets {
                if let Some(matrices) = self.animation.bone_matrices(entity_id) {
                    renderer.update_bone_matrices(&context.queue, asset_name, matrices);
                }
            }
        }

        // Advance particle simulation (skip when paused)
        if config.particles == SystemPolicy::Run {
            self.particles
                .update(&mut self.world, self.clock.delta_time)
                .ok();
        }

        // Process procgen generation queue (frame-budgeted)
        {
            let camera_pos = self.camera.position_array();
            let completed = self.procgen_resolver.process_frame(camera_pos);
            if !completed.is_empty() {
                if let (Some(renderer), Some(ctx)) =
                    (&mut self.scene_renderer, &self.render_context)
                {
                    scene_loading::upload_completed_procgen_assets(
                        &completed,
                        renderer,
                        &ctx.device,
                        &ctx.queue,
                    );
                }
            }
        }

        // Refresh renderer with updated transforms
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            renderer.camera_offset = [self.camera.position.x, self.camera.position.y];
            renderer.ortho_height = if self.camera.ortho_height > 0.0 {
                self.camera.ortho_height
            } else {
                10.0
            };
            renderer.aspect_ratio = self.camera.aspect;
            renderer.update_from_world(&self.world, &context.device);
        }

        // Upload particle instance data to GPU
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            let sync_draw_data = self.particles.sync.draw_data();
            let render_draw_data: Vec<ParticleDrawData<'_>> = sync_draw_data
                .iter()
                .map(|d| {
                    let gpu_instances: &[ParticleInstanceGpu] =
                        bytemuck::cast_slice(bytemuck::cast_slice::<_, u8>(d.instances));
                    ParticleDrawData {
                        instances: gpu_instances,
                        texture: d.texture,
                        additive: d.blend_mode == flint_particles::ParticleBlendMode::Additive,
                    }
                })
                .collect();
            renderer.update_particles(&context.device, render_draw_data);
        }

        // Update grass time and entity positions for bend-on-contact
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            renderer.grass_time = self.clock.total_time as f32;
            // Ocean waves run on the same clock scripts see via total_time(),
            // keeping script-side ocean_height() queries in sync with the GPU.
            renderer.ocean_time = self.clock.total_time;

            let cam_pos = self.camera.position;
            let grass_entities = vec![GrassEntityPosition {
                position: [cam_pos.x, cam_pos.y, cam_pos.z],
                _pad: 0.0,
            }];
            renderer.update_grass_entities(&context.queue, &grass_entities);
        }

        // Drain script post-processing overrides for this frame
        let (pp_vig, pp_bloom, pp_exp, pp_ca, pp_rb, pp_ssao, pp_fog) =
            self.script.take_postprocess_overrides();
        self.pp_vignette_override = pp_vig;
        self.pp_bloom_override = pp_bloom;
        self.pp_exposure_override = pp_exp;
        self.pp_chromatic_aberration_override = pp_ca;
        self.pp_radial_blur_override = pp_rb;
        self.pp_ssao_intensity_override = pp_ssao;
        self.pp_fog_density_override = pp_fog;
        self.pp_fog_color_override = self.script.take_fog_color_override();

        // Apply audio low-pass filter override from scripts
        if let Some(cutoff) = self.script.take_audio_overrides() {
            self.audio.set_filter_cutoff(cutoff);
        }

        // Apply cursor capture request from scripts (skipped while a debug
        // panel is open — the panel needs the mouse).
        if let Some(capture) = self.script.take_cursor_capture_override() {
            let panel_open = self.debug_panels.iter().any(|p| p.is_open());
            if capture && !panel_open && !self.cursor_captured {
                self.capture_cursor();
            } else if !capture && self.cursor_captured {
                self.release_cursor();
            }
        }

        // Clear per-frame input state
        self.input.end_frame();
    }

    fn render_hud(&mut self, target_view: &wgpu::TextureView) {
        // Lazy-load any sprite textures referenced by draw commands
        // (must happen before egui_winit borrow)
        self.load_pending_sprites();

        let Some(window) = &self.window else { return };
        let Some(context) = &self.render_context else {
            return;
        };
        let Some(egui_winit) = &mut self.egui_winit else {
            return;
        };

        let raw_input = egui_winit.take_egui_input(window);

        let draw_commands = std::mem::take(&mut self.draw_commands);
        let mut debug_panels = std::mem::take(&mut self.debug_panels);
        let ui_textures = &self.ui_textures;

        let show_stats = self.show_stats;
        let stats_data = if show_stats {
            self.scene_renderer.as_ref().map(|r| {
                let mut stats = r.collect_stats();
                // FPS smoothing: rolling window of delta_time samples
                self.stats_frame_times.push_back(self.clock.delta_time);
                while self.stats_frame_times.len() > 60 {
                    self.stats_frame_times.pop_front();
                }
                let avg_dt: f64 = self.stats_frame_times.iter().sum::<f64>()
                    / self.stats_frame_times.len().max(1) as f64;
                stats.fps = (1.0 / avg_dt) as f32;
                stats.frame_time_ms = (avg_dt * 1000.0) as f32;
                if let Some(ctx) = &self.render_context {
                    stats.resolution = [ctx.config.width, ctx.config.height];
                }
                stats
            })
        } else {
            None
        };

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            for panel in debug_panels.iter_mut() {
                if panel.is_open() {
                    let panel_name = panel.name().to_owned();
                    egui::SidePanel::right(egui::Id::new(&panel_name))
                        .default_width(280.0)
                        .show(ctx, |ui| {
                            ui.heading(&panel_name);
                            ui.separator();
                            panel.ui(ui);
                        });
                }
            }
            render_draw_commands(ctx, &draw_commands, ui_textures);
            if let Some(ref stats) = stats_data {
                render_stats_overlay(ctx, stats);
            }
        });

        self.draw_commands = draw_commands;
        self.debug_panels = debug_panels;

        egui_winit.handle_platform_output(window, full_output.platform_output);

        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [context.config.width, context.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let mut egui_renderer = self.egui_renderer.take().unwrap();

        let mut encoder = context
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("HUD Encoder"),
            });

        for (id, image_delta) in &full_output.textures_delta.set {
            egui_renderer.update_texture(&context.device, &context.queue, *id, image_delta);
        }

        egui_renderer.update_buffers(
            &context.device,
            &context.queue,
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("HUD Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load, // overlay on top of 3D
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            let mut render_pass = render_pass.forget_lifetime();
            egui_renderer.render(&mut render_pass, &paint_jobs, &screen_descriptor);
        }

        context.queue.submit(std::iter::once(encoder.finish()));

        for id in &full_output.textures_delta.free {
            egui_renderer.free_texture(id);
        }

        self.egui_renderer = Some(egui_renderer);
    }

    fn process_script_commands(&mut self, commands: Vec<ScriptCommand>) {
        for cmd in commands {
            match cmd {
                ScriptCommand::PlaySound { name, volume } => {
                    if self.audio.engine.is_available() {
                        if let Err(e) = self
                            .audio
                            .engine
                            .play_non_spatial(&name, volume, 1.0, false)
                        {
                            tracing::warn!(target: "script", "play_sound error: {:?}", e);
                        }
                    }
                }
                ScriptCommand::PlaySoundAt {
                    name,
                    position,
                    volume,
                } => {
                    let pos =
                        FlintVec3::new(position.0 as f32, position.1 as f32, position.2 as f32);
                    if let Err(e) = self.audio.engine.play_at_position(&name, pos, volume) {
                        tracing::warn!(target: "script", "play_sound_at error: {:?}", e);
                    }
                }
                ScriptCommand::StopSound { name: _ } => {
                    // One-shot sounds play to completion (same as AudioCommand::Stop)
                }
                ScriptCommand::FireEvent { name, data } => {
                    // Intercept transition completion signal
                    if name == TRANSITION_COMPLETE {
                        match &self.transition_phase {
                            TransitionPhase::Exiting { target_scene, .. } => {
                                let target = target_scene.clone();
                                self.transition_phase = TransitionPhase::Loading {
                                    target_scene: target,
                                };
                            }
                            TransitionPhase::Entering { .. } => {
                                self.transition_phase = TransitionPhase::Idle;
                                println!("[transition] Transition complete");
                            }
                            _ => {}
                        }
                        continue;
                    }
                    self.physics
                        .push_event(GameEvent::Custom { name, data });
                }
                ScriptCommand::Log { level, message } => match level {
                    LogLevel::Info => tracing::info!(target: "script", "{}", message),
                    LogLevel::Warn => tracing::warn!(target: "script", "{}", message),
                    LogLevel::Error => tracing::error!(target: "script", "{}", message),
                },
                ScriptCommand::EmitBurst { entity_id, count } => {
                    let eid = flint_core::EntityId(entity_id as u64);
                    self.particles.sync.queue_burst(eid, count as u32);
                }
                ScriptCommand::LoadScene { path } => {
                    if self.transition_phase.is_idle() {
                        println!("[transition] Starting exit transition → {}", path);
                        self.transition_phase = TransitionPhase::Exiting {
                            target_scene: path,
                            elapsed: 0.0,
                        };
                    }
                }
                ScriptCommand::ReloadScene => {
                    if self.transition_phase.is_idle() {
                        let path = self.scene_path.clone();
                        println!("[transition] Reloading current scene");
                        self.transition_phase = TransitionPhase::Exiting {
                            target_scene: path,
                            elapsed: 0.0,
                        };
                    }
                }
                ScriptCommand::PushState { name } => {
                    if self.state_machine.push_state(&name) {
                        println!("[state] Pushed '{}'", name);
                    } else {
                        tracing::warn!("Unknown state template: '{}'", name);
                    }
                }
                ScriptCommand::PopState => {
                    if let Some(popped) = self.state_machine.pop_state() {
                        println!("[state] Popped '{}'", popped.name);
                    }
                }
                ScriptCommand::ReplaceState { name } => {
                    if self.state_machine.replace_state(&name) {
                        println!("[state] Replaced top with '{}'", name);
                    } else {
                        tracing::warn!("Unknown state template: '{}'", name);
                    }
                }
                ScriptCommand::SetVelocity2D { entity_id, vx, vy } => {
                    let eid = flint_core::EntityId::from_raw(entity_id as u64);
                    self.physics.set_velocity_2d(eid, vx as f32, vy as f32);
                }
                ScriptCommand::LoadChunk {
                    path,
                    offset_x,
                    offset_y,
                    chunk_id,
                } => {
                    self.load_chunk(&path, offset_x as f32, offset_y as f32, &chunk_id);
                }
                ScriptCommand::UnloadChunk { chunk_id } => {
                    self.unload_chunk(&chunk_id);
                }
            }
        }
    }

    /// Advance the transition phase based on elapsed time and script signals.
    fn advance_transition(&mut self) {
        match &self.transition_phase {
            TransitionPhase::Idle => {}
            TransitionPhase::Exiting {
                target_scene,
                elapsed,
            } => {
                let new_elapsed = elapsed + self.clock.delta_time as f32;
                let target = target_scene.clone();
                self.transition_phase = TransitionPhase::Exiting {
                    target_scene: target.clone(),
                    elapsed: new_elapsed,
                };
                // Check if a complete_transition event was fired
                // (We use a sentinel event name to signal completion)
            }
            TransitionPhase::Loading { target_scene } => {
                let target = target_scene.clone();
                self.execute_scene_transition(&target);
                self.transition_phase = TransitionPhase::Entering { elapsed: 0.0 };
            }
            TransitionPhase::Entering { elapsed } => {
                let new_elapsed = elapsed + self.clock.delta_time as f32;
                self.transition_phase = TransitionPhase::Entering {
                    elapsed: new_elapsed,
                };
            }
        }
    }

    /// Load a chunk TOML file as additional entities with offset.
    fn load_chunk(&mut self, path: &str, offset_x: f32, offset_y: f32, chunk_id: &str) {
        if self.loaded_chunks.contains_key(chunk_id) {
            tracing::warn!("Chunk '{}' already loaded, skipping", chunk_id);
            return;
        }

        // Resolve path relative to scene directory
        let scene_dir = Path::new(&self.scene_path)
            .parent()
            .unwrap_or_else(|| Path::new("."));
        let chunk_path = scene_dir.join(path);

        // Parse chunk as a scene file
        let content = match std::fs::read_to_string(&chunk_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Failed to read chunk '{}': {}", chunk_path.display(), e);
                return;
            }
        };
        let scene_file: flint_scene::SceneFile = match toml::from_str(&content) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to parse chunk '{}': {}", chunk_path.display(), e);
                return;
            }
        };

        // Load schema registry for archetype defaults
        let registry = if self.schema_paths.is_empty() {
            flint_schema::SchemaRegistry::load_from_directory("schemas")
                .unwrap_or_else(|_| flint_schema::SchemaRegistry::new())
        } else {
            let existing: Vec<&str> = self
                .schema_paths
                .iter()
                .map(|s| s.as_str())
                .filter(|p| Path::new(p).exists())
                .collect();
            if existing.is_empty() {
                flint_schema::SchemaRegistry::new()
            } else {
                flint_schema::SchemaRegistry::load_from_directories(&existing)
                    .unwrap_or_else(|_| flint_schema::SchemaRegistry::new())
            }
        };

        let mut spawned_ids = Vec::new();

        // Spawn entities with prefixed names
        for (name, entity_def) in &scene_file.entities {
            let prefixed_name = format!("{}_{}", chunk_id, name);
            let id = match self.world.spawn(prefixed_name.clone()) {
                Ok(id) => id,
                Err(e) => {
                    tracing::error!("Failed to spawn chunk entity '{}': {:?}", prefixed_name, e);
                    continue;
                }
            };
            spawned_ids.push(id);

            // Set archetype + defaults
            if let Some(archetype) = &entity_def.archetype {
                if let Some(comps) = self.world.get_components_mut(id) {
                    comps.archetype = Some(archetype.clone());
                    if let Some(arch_schema) = registry.get_archetype(archetype) {
                        for (comp_name, defaults) in &arch_schema.defaults {
                            if !comps.has(comp_name) {
                                comps.set(comp_name.clone(), defaults.clone());
                            }
                        }
                    }
                }
            }

            // Set component data
            for (comp_name, comp_data) in &entity_def.components {
                let _ = self.world.merge_component(id, comp_name, comp_data.clone());
            }

            // Apply position offset
            if let Some(transform) = self.world.get_transform(id) {
                let new_x = transform.position.x + offset_x;
                let new_y = transform.position.y + offset_y;
                let new_z = transform.position.z;
                if let Some(comps) = self.world.get_components_mut(id) {
                    comps.set_field(
                        comp::TRANSFORM,
                        "position",
                        toml::Value::Array(vec![
                            toml::Value::Float(new_x as f64),
                            toml::Value::Float(new_y as f64),
                            toml::Value::Float(new_z as f64),
                        ]),
                    );
                }
            } else {
                // Entity has no transform yet — create one with the offset
                if let Some(comps) = self.world.get_components_mut(id) {
                    comps.set_field(
                        comp::TRANSFORM,
                        "position",
                        toml::Value::Array(vec![
                            toml::Value::Float(offset_x as f64),
                            toml::Value::Float(offset_y as f64),
                            toml::Value::Float(0.0),
                        ]),
                    );
                }
            }
        }

        // Re-sync physics for new entities
        self.physics
            .sync_new_entities(&mut self.world, &spawned_ids);

        // Re-sync scripts for new entities
        let chunk_scripts_dir = chunk_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("scripts");
        for &id in &spawned_ids {
            if let Some(comps) = self.world.get_components(id) {
                if let Some(script_comp) = comps.get(comp::SCRIPT) {
                    if let Some(source) = script_comp.get("source").and_then(|v| v.as_str()) {
                        // Try chunk's own scripts dir first, then scene's scripts dir
                        let scene_scripts_dir = Path::new(&self.scene_path)
                            .parent()
                            .unwrap_or_else(|| Path::new("."))
                            .join("scripts");
                        let script_path = if chunk_scripts_dir.join(source).exists() {
                            chunk_scripts_dir.join(source)
                        } else {
                            scene_scripts_dir.join(source)
                        };
                        if script_path.exists() {
                            self.script.load_script_for_entity(id, &script_path);
                        }
                    }
                }
            }
        }

        // Initialize scripts for new entities
        self.script
            .initialize_entities(&mut self.world, &spawned_ids);

        println!(
            "[chunk] Loaded '{}' ({} entities) at offset ({}, {})",
            chunk_id,
            spawned_ids.len(),
            offset_x,
            offset_y
        );
        self.loaded_chunks.insert(chunk_id.to_string(), spawned_ids);
    }

    /// Unload a previously loaded chunk by despawning all its entities.
    fn unload_chunk(&mut self, chunk_id: &str) {
        if let Some(entity_ids) = self.loaded_chunks.remove(chunk_id) {
            let count = entity_ids.len();
            for id in &entity_ids {
                self.physics.remove_entity(*id);
                self.script.remove_entity(*id);
                let _ = self.world.despawn(*id);
            }
            println!("[chunk] Unloaded '{}' ({} entities)", chunk_id, count);
        } else {
            tracing::warn!("Chunk '{}' not loaded, cannot unload", chunk_id);
        }
    }

    /// Unload the current scene and load a new one.
    fn execute_scene_transition(&mut self, target_scene: &str) {
        println!("[transition] Unloading current scene...");

        // Call on_scene_exit on all scripts
        {
            let _state_scope = self.script.state_scope(
                &mut self.state_machine,
                &mut self.persistent_store,
                &self.physics,
            );
            self.script.call_scene_exits(&mut self.world);
        }

        // Clear all systems
        self.script.clear();
        self.audio.clear();
        self.physics.clear();
        self.animation.clear();
        self.particles.clear();
        self.procgen_resolver.clear_queue();

        // Clear world
        self.world = FlintWorld::new();

        // Clear terrain
        self.terrain = None;
        if let Some(renderer) = &mut self.scene_renderer {
            renderer.clear_terrain();
        }

        // Clear transient rendering state
        self.skeletal_entity_assets.clear();
        self.ui_textures.clear();
        self.draw_commands.clear();
        self.loaded_chunks.clear();

        println!("[transition] Loading scene: {}", target_scene);

        // Resolve scene path relative to current scene
        let new_scene_path = resolve_scene_path(&self.scene_path, target_scene);

        // Auto-discover schema dirs from the new scene path and merge
        for dir in flint_schema::discover_schema_dirs(&new_scene_path) {
            let s = dir.to_string_lossy().into_owned();
            if !self.schema_paths.contains(&s) {
                self.schema_paths.push(s);
            }
        }

        // Load schema registry
        let registry = if self.schema_paths.is_empty() {
            // Try default schemas/ dir
            flint_schema::SchemaRegistry::load_from_directory("schemas")
                .unwrap_or_else(|_| flint_schema::SchemaRegistry::new())
        } else {
            let existing: Vec<&str> = self
                .schema_paths
                .iter()
                .map(|s| s.as_str())
                .filter(|p| Path::new(p).exists())
                .collect();
            if existing.is_empty() {
                flint_schema::SchemaRegistry::new()
            } else {
                flint_schema::SchemaRegistry::load_from_directories(&existing)
                    .unwrap_or_else(|_| flint_schema::SchemaRegistry::new())
            }
        };

        // Parse and load scene
        match flint_scene::load_scene(&new_scene_path, &registry) {
            Ok((world, scene_file)) => {
                self.world = world;
                self.scene_path = new_scene_path.clone();
                self.skybox_path = scene_file
                    .environment
                    .as_ref()
                    .and_then(|env| env.skybox.clone());
                self.scene_camera = scene_file.camera.clone();
                self.scene_post_process = scene_file.post_process.clone();
                self.scene_input_config = scene_file.scene.input_config.clone();
            }
            Err(e) => {
                tracing::error!(
                    "Failed to load scene '{}': {:?}",
                    new_scene_path, e
                );
                return;
            }
        }

        // Rebuild component index after loading new scene
        self.world.rebuild_component_index();

        // Apply camera config before borrowing renderer
        self.apply_camera_def();

        // Reload models
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            renderer.scene_dir = Path::new(&self.scene_path)
                .parent()
                .map(|p| p.to_path_buf());

            let config = build_model_load_config(
                &self.scene_path,
                &self.world,
                self.catalog.as_ref(),
                self.content_store.as_ref(),
            );

            // Discover procgen specs and resolve unresolved assets before model loading
            {
                let scene_dir = Path::new(&self.scene_path).parent().unwrap_or(Path::new("."));
                let mut spec_dirs = vec![scene_dir.join("specs")];
                if let Some(parent) = scene_dir.parent() {
                    spec_dirs.push(parent.join("specs"));
                }
                spec_dirs.push(scene_dir.join("models"));
                let dir_refs: Vec<&Path> = spec_dirs.iter().filter(|d| d.is_dir()).map(|d| d.as_path()).collect();
                self.procgen_resolver.discover_and_index(&dir_refs);

                resolve_procgen_assets(
                    &self.world,
                    &mut self.procgen_resolver,
                    renderer,
                    &context.device,
                    &context.queue,
                    &config,
                );
            }

            let load_result = model_loader::load_models_from_world(
                &mut self.world,
                renderer,
                &context.device,
                &context.queue,
                &config,
            );
            register_skeletal_data(&load_result, &mut self.animation);
            register_node_animation_data(&load_result, &mut self.animation);
            self.skeletal_entity_assets = load_result.skinned_entities;
            renderer.update_from_world(&self.world, &context.device);

            // Reload splines
            crate::spline_gen::load_splines(
                &self.scene_path,
                &mut self.world,
                renderer,
                Some(&mut self.physics),
                &context.device,
            );
            renderer.update_from_world(&self.world, &context.device);

            // Reload skybox
            if let Some(skybox_rel) = &self.skybox_path {
                let scene_dir = Path::new(&self.scene_path)
                    .parent()
                    .unwrap_or_else(|| Path::new("."));
                let skybox_path = {
                    let p = scene_dir.join(skybox_rel);
                    if p.exists() {
                        p
                    } else if let Some(parent) = scene_dir.parent() {
                        parent.join(skybox_rel)
                    } else {
                        p
                    }
                };
                if skybox_path.exists() {
                    renderer.load_skybox(&context.device, &context.queue, &skybox_path);
                }
            }

            // Apply post-process config
            if let Some(pp_def) = &self.scene_post_process {
                renderer
                    .set_post_process_config(scene_loading::post_process_config_from_def(pp_def));
                renderer.ensure_kuwahara_resources(&context.device, &context.queue);
            }
        }

        // Reload terrain
        self.debug_panels.clear();
        if let (Some(renderer), Some(context)) = (&mut self.scene_renderer, &self.render_context) {
            let mut grass_info = None;
            load_terrain_from_world_inner(
                &self.world,
                &self.scene_path,
                &context.device,
                &context.queue,
                renderer,
                &mut self.physics,
                &mut self.terrain,
                &mut grass_info,
            );

            // Create grass debug panel if grass was loaded
            if let Some(info) = grass_info {
                let panel = flint_debug_ui::GrassDebugPanel::new(
                    info.config,
                    std::path::PathBuf::from(&self.scene_path),
                    info.terrain_entity_name,
                );
                self.debug_panels.push(Box::new(panel));
            }
        }
        self.create_ocean_debug_panel();
        self.create_tod_debug_panel();
        self.create_camera_debug_panel();
        self.apply_camera_tuning();

        // Update terrain height callback for scripts
        self.update_terrain_height_fn();

        // Re-initialize systems
        self.physics
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Physics init failed: {:?}", e));

        load_audio_from_world(&self.world, &mut self.audio, &self.scene_path);
        self.audio
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Audio init failed: {:?}", e));

        load_animations_from_world(&self.scene_path, &mut self.animation);
        load_sprite_animations_from_world(&self.scene_path, &mut self.animation);
        self.animation
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Animation init failed: {:?}", e));

        self.particles
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Particles init failed: {:?}", e));

        load_scripts_from_world(&self.scene_path, &mut self.script);
        self.script
            .initialize(&mut self.world)
            .unwrap_or_else(|e| tracing::warn!("Script init failed: {:?}", e));

        // Call on_scene_enter on new scripts
        self.script.set_current_scene(&self.scene_path);
        {
            let _state_scope = self.script.state_scope(
                &mut self.state_machine,
                &mut self.persistent_store,
                &self.physics,
            );
            self.script.call_scene_enters(&mut self.world);
        }

        // Recapture cursor if player exists
        if self.physics.has_player_entity() && !self.cursor_captured {
            self.capture_cursor();
        }

        println!("[transition] Scene loaded: {}", self.scene_path);
    }
}

impl ApplicationHandler for PlayerApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.render_context.is_none() {
            // First launch — full initialization
            if let Err(e) = self.initialize(event_loop) {
                tracing::error!("Failed to initialize player: {e:#}");
                event_loop.exit();
            }
        } else {
            // Subsequent resume (Android returning from background).
            // Recreate window and surface — GPU context is still valid.
            #[cfg(target_os = "android")]
            {
                let window_attrs = Window::default_attributes().with_title("Flint Player");
                match event_loop.create_window(window_attrs) {
                    Ok(window) => {
                        let window = Arc::new(window);
                        self.window = Some(window.clone());
                        if let Some(ctx) = &mut self.render_context {
                            if let Err(e) = ctx.recreate_surface(window.clone()) {
                                tracing::error!("Failed to recreate surface: {e}");
                                return;
                            }
                            self.camera.aspect = ctx.aspect_ratio();
                            let size = ctx.size;
                            if let Some(renderer) = &mut self.scene_renderer {
                                renderer.resize_postprocess(&ctx.device, size.width, size.height);
                            }
                            self.input
                                .set_screen_size(size.width as f64, size.height as f64);
                        }
                        self.cursor_captured = true;
                    }
                    Err(e) => {
                        tracing::error!("Failed to recreate window on resume: {e}");
                    }
                }
            }
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // On Android, the native surface is destroyed when the app is suspended.
        // Drop the window reference — a new one will be created in resumed().
        #[cfg(target_os = "android")]
        {
            self.window = None;
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Forward window events to egui when a debug panel is open
        // so interactive widgets (sliders, buttons) receive mouse input.
        let any_panel_open = self.debug_panels.iter().any(|p| p.is_open());
        if any_panel_open {
            if let (Some(egui_winit), Some(window)) = (&mut self.egui_winit, &self.window) {
                let response = egui_winit.on_window_event(window, &event);
                if response.consumed {
                    return;
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }

            WindowEvent::Resized(new_size) => {
                if let Some(context) = &mut self.render_context {
                    context.resize(new_size);
                    self.camera.aspect = context.aspect_ratio();
                    // Resize post-processing HDR buffer and bloom chain
                    if let Some(renderer) = &mut self.scene_renderer {
                        renderer.resize_postprocess(
                            &context.device,
                            new_size.width,
                            new_size.height,
                        );
                    }
                }
                self.input
                    .set_screen_size(new_size.width as f64, new_size.height as f64);
            }

            WindowEvent::KeyboardInput {
                device_id, event, ..
            } => {
                // Suppress unused variable warning on non-Android platforms
                let _ = &device_id;

                if let PhysicalKey::Code(key_code) = event.physical_key {
                    match event.state {
                        ElementState::Pressed => {
                            // Handle escape to toggle cursor capture
                            if key_code == KeyCode::Escape {
                                if self.cursor_captured {
                                    self.release_cursor();
                                } else {
                                    #[cfg(not(target_os = "android"))]
                                    event_loop.exit();
                                }
                                return;
                            }

                            // Debug keys
                            match key_code {
                                KeyCode::F1 => {
                                    if let Some(renderer) = &mut self.scene_renderer {
                                        let next = renderer.debug_state().mode.next();
                                        renderer.set_debug_mode(next);
                                        if let Some(context) = &self.render_context {
                                            renderer
                                                .update_from_world(&self.world, &context.device);
                                        }
                                    }
                                }
                                KeyCode::F2 => {
                                    self.show_stats = !self.show_stats;
                                }
                                KeyCode::F3 => {
                                    // Toggle all registered debug panels (grass, ocean, ...)
                                    if self.debug_panels.is_empty() {
                                        tracing::info!("No debug panels in current scene");
                                    } else {
                                        // Toggle the panels, then adjust cursor outside the borrow
                                        let mut any_open = false;
                                        for panel in &mut self.debug_panels {
                                            panel.toggle();
                                            any_open |= panel.is_open();
                                        }
                                        if any_open {
                                            self.release_cursor();
                                        } else if self.physics.has_player_entity() {
                                            self.capture_cursor();
                                        }
                                    }
                                }
                                KeyCode::F4 => {
                                    if let Some(renderer) = &mut self.scene_renderer {
                                        renderer.toggle_shadows();
                                    }
                                }
                                KeyCode::F5 => {
                                    if let Some(renderer) = &mut self.scene_renderer {
                                        let mut config = renderer.post_process_config().clone();
                                        config.bloom_enabled = !config.bloom_enabled;
                                        renderer.set_post_process_config(config);
                                    }
                                }
                                KeyCode::F6 => {
                                    if let Some(renderer) = &mut self.scene_renderer {
                                        let mut config = renderer.post_process_config().clone();
                                        config.enabled = !config.enabled;
                                        renderer.set_post_process_config(config);
                                    }
                                }
                                KeyCode::F7 => {
                                    if let Some(renderer) = &mut self.scene_renderer {
                                        let mut config = renderer.post_process_config().clone();
                                        config.ssao_enabled = !config.ssao_enabled;
                                        renderer.set_post_process_config(config);
                                    }
                                }
                                KeyCode::F8 => {
                                    if let Some(renderer) = &mut self.scene_renderer {
                                        let mut config = renderer.post_process_config().clone();
                                        config.fog_enabled = !config.fog_enabled;
                                        renderer.set_post_process_config(config);
                                    }
                                }
                                KeyCode::F9 => {
                                    if let Some(renderer) = &mut self.scene_renderer {
                                        let mut config = renderer.post_process_config().clone();
                                        config.dither_enabled = !config.dither_enabled;
                                        renderer.set_post_process_config(config);
                                    }
                                }
                                KeyCode::F10 => {
                                    if let Some(renderer) = &mut self.scene_renderer {
                                        let mut config = renderer.post_process_config().clone();
                                        config.volumetric_enabled = !config.volumetric_enabled;
                                        renderer.set_post_process_config(config);
                                    }
                                }
                                KeyCode::F11 => {
                                    if let Some(window) = &self.window {
                                        if window.fullscreen().is_some() {
                                            window.set_fullscreen(None);
                                        } else {
                                            window.set_fullscreen(Some(
                                                winit::window::Fullscreen::Borderless(None),
                                            ));
                                        }
                                    }
                                }
                                KeyCode::F12 => {
                                    if let (Some(renderer), Some(ctx)) =
                                        (&mut self.scene_renderer, &self.render_context)
                                    {
                                        let mut config = renderer.post_process_config().clone();
                                        config.kuwahara_enabled = !config.kuwahara_enabled;
                                        let enabled = config.kuwahara_enabled;
                                        renderer.set_post_process_config(config);
                                        renderer.ensure_kuwahara_resources(
                                            &ctx.device,
                                            &ctx.queue,
                                        );
                                        eprintln!("Kuwahara filter: {}",
                                            if enabled { "ON" } else { "OFF" });
                                    }
                                }
                                _ => {}
                            }

                            self.input.process_key_down(key_code);

                            // On Android, DPad maps to ArrowKeys — also fire gamepad
                            // button events so configs with gamepad DPad bindings work.
                            #[cfg(target_os = "android")]
                            {
                                let dpad_button = match key_code {
                                    KeyCode::ArrowUp => Some("DPadUp"),
                                    KeyCode::ArrowDown => Some("DPadDown"),
                                    KeyCode::ArrowLeft => Some("DPadLeft"),
                                    KeyCode::ArrowRight => Some("DPadRight"),
                                    _ => None,
                                };
                                if let Some(btn) = dpad_button {
                                    let slot = self.android_gamepad.register_device(device_id);
                                    self.input.process_gamepad_button_down(slot, btn);
                                }
                            }
                        }
                        ElementState::Released => {
                            self.input.process_key_up(key_code);

                            #[cfg(target_os = "android")]
                            {
                                let dpad_button = match key_code {
                                    KeyCode::ArrowUp => Some("DPadUp"),
                                    KeyCode::ArrowDown => Some("DPadDown"),
                                    KeyCode::ArrowLeft => Some("DPadLeft"),
                                    KeyCode::ArrowRight => Some("DPadRight"),
                                    _ => None,
                                };
                                if let Some(btn) = dpad_button {
                                    if let Some(slot) = self.android_gamepad.slot_for(device_id) {
                                        self.input.process_gamepad_button_up(slot, btn);
                                    }
                                }
                            }
                        }
                    }
                }

                // Android: intercept gamepad buttons delivered as unidentified native keycodes
                #[cfg(target_os = "android")]
                if let PhysicalKey::Unidentified(NativeKeyCode::Android(keycode)) =
                    event.physical_key
                {
                    if let Some(button_name) = Self::android_keycode_to_gamepad_button(keycode) {
                        let slot = self.android_gamepad.register_device(device_id);
                        match event.state {
                            ElementState::Pressed => {
                                self.input.process_gamepad_button_down(slot, button_name);
                            }
                            ElementState::Released => {
                                self.input.process_gamepad_button_up(slot, button_name);
                            }
                        }
                    }
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                if !self.cursor_captured {
                    if state == ElementState::Pressed && button == MouseButton::Left {
                        if !self.debug_panels.iter().any(|p| p.is_open()) {
                            self.capture_cursor();
                        }
                    }
                    return;
                }

                let btn = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => return,
                };

                match state {
                    ElementState::Pressed => self.input.process_mouse_button_down(btn),
                    ElementState::Released => self.input.process_mouse_button_up(btn),
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.input.process_mouse_move(position.x, position.y);
            }

            WindowEvent::Touch(touch) => {
                // Android: joystick MotionEvents arrive as Touch with axis values
                // in [-1, 1]. Route these to gamepad axis input instead of touch.
                #[cfg(target_os = "android")]
                {
                    let x = touch.location.x;
                    let y = touch.location.y;

                    // If this device was already identified as a gamepad via button
                    // presses, treat its touch events as stick axis data.
                    if self.android_gamepad.is_gamepad(touch.device_id) {
                        if let Some(slot) = self.android_gamepad.slot_for(touch.device_id) {
                            self.input
                                .process_gamepad_axis(slot, "LeftStickX", x as f32);
                            self.input
                                .process_gamepad_axis(slot, "LeftStickY", y as f32);
                            return;
                        }
                    }

                    // Heuristic: if coordinates are in joystick range [-1.5, 1.5]
                    // and this touch id has no prior Started event (process_touch_move
                    // would silently drop it anyway), treat it as a joystick from an
                    // unregistered gamepad device.
                    if x.abs() <= 1.5 && y.abs() <= 1.5 && !self.input.has_active_touch(touch.id) {
                        let slot = self.android_gamepad.register_device(touch.device_id);
                        self.input
                            .process_gamepad_axis(slot, "LeftStickX", x as f32);
                        self.input
                            .process_gamepad_axis(slot, "LeftStickY", y as f32);
                        return;
                    }
                }

                // Normal touch processing
                self.input.disable_touch_emulation();

                let id = touch.id;
                let x = touch.location.x;
                let y = touch.location.y;
                match touch.phase {
                    winit::event::TouchPhase::Started => {
                        self.input.process_touch_start(id, x, y);
                    }
                    winit::event::TouchPhase::Moved => {
                        self.input.process_touch_move(id, x, y);
                    }
                    winit::event::TouchPhase::Ended => {
                        // Update position from the End event before gesture detection —
                        // fast swipes may have few/no Move events between Start and End
                        self.input.process_touch_move(id, x, y);
                        self.input.process_touch_end(id);
                    }
                    winit::event::TouchPhase::Cancelled => {
                        self.input.process_touch_cancel(id);
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                self.tick();
                self.render();
            }

            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: winit::event::DeviceId,
        event: DeviceEvent,
    ) {
        if !self.cursor_captured {
            return;
        }

        if let DeviceEvent::MouseMotion { delta } = event {
            self.input.process_mouse_raw_delta(delta.0, delta.1);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

use hud_render::render_draw_commands;
use scene_loading::{
    build_model_load_config, load_animations_from_world, load_audio_from_world,
    load_scripts_from_world, load_sprite_animations_from_world, load_terrain_from_world_inner,
    register_node_animation_data, register_skeletal_data, resolve_procgen_assets,
    resolve_scene_path,
};

impl PlayerApp {
    /// Load a sprite texture for UI rendering. Called lazily when a draw_sprite
    /// command references a name not yet in ui_textures.
    pub fn load_ui_texture(&mut self, name: &str) -> bool {
        if self.ui_textures.contains_key(name) {
            return true;
        }

        let scene_dir = Path::new(&self.scene_path)
            .parent()
            .unwrap_or_else(|| Path::new("."));

        // Search: scene_dir/sprites/{name} → game_root/sprites/{name} → scene_dir/{name}
        let candidates = [
            scene_dir.join("sprites").join(name),
            scene_dir
                .parent()
                .map(|p| p.join("sprites").join(name))
                .unwrap_or_default(),
            scene_dir.join(name),
        ];

        for path in &candidates {
            if path.exists() {
                if let Ok(img) = image::open(path) {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    let color_image =
                        egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
                    let tex_handle =
                        self.egui_ctx
                            .load_texture(name, color_image, egui::TextureOptions::LINEAR);
                    self.ui_textures.insert(name.to_string(), tex_handle);
                    println!("Loaded UI sprite: {}", name);
                    return true;
                }
            }
        }

        tracing::warn!("UI sprite not found: {}", name);
        false
    }

    /// Pre-scan draw commands and load any sprite textures that haven't been loaded yet
    fn load_pending_sprites(&mut self) {
        let sprite_names: Vec<String> = self
            .draw_commands
            .iter()
            .filter_map(|cmd| {
                if let DrawCommand::Sprite { name, .. } = cmd {
                    if !self.ui_textures.contains_key(name.as_str()) {
                        Some(name.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect();

        for name in sprite_names {
            self.load_ui_texture(&name);
        }
    }
}

use input_config::{
    fallback_user_override_path, gamepad_id_to_u32, resolve_input_paths, write_user_override_file,
    InputConfigPaths, PendingRebind,
};

fn render_stats_overlay(ctx: &egui::Context, stats: &flint_render::RenderStats) {
    use flint_render::format_count;

    egui::Area::new(egui::Id::new("render_stats_overlay"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 8.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            egui::Frame::none()
                .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 209))
                .stroke(egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 38),
                ))
                .rounding(egui::Rounding::same(4.0))
                .inner_margin(egui::Margin::same(10.0))
                .show(ui, |ui| {
                    ui.style_mut().override_font_id =
                        Some(egui::FontId::monospace(11.0));
                    ui.set_min_width(180.0);

                    // Header
                    ui.colored_label(
                        egui::Color32::from_gray(136),
                        egui::RichText::new("RENDERING STATS").size(9.0),
                    );
                    ui.separator();

                    // Core metrics
                    ui.horizontal(|ui| {
                        ui.label("FPS:");
                        ui.colored_label(
                            egui::Color32::from_rgb(74, 222, 128),
                            format!("{:.0}", stats.fps),
                        );
                        ui.colored_label(
                            egui::Color32::from_gray(102),
                            format!("({:.1}ms)", stats.frame_time_ms),
                        );
                    });
                    ui.label(format!("Draw Calls: {}", stats.draw_calls));
                    ui.label(format!("Triangles: {}", format_count(stats.triangles)));

                    ui.separator();

                    // Breakdown
                    ui.label(format!("Entities: {}", stats.entity_draws));
                    if stats.skinned_draws > 0 {
                        ui.label(format!("Skinned: {}", stats.skinned_draws));
                    }
                    ui.label(format!(
                        "Terrain: {}/{} chunks",
                        stats.terrain_draws, stats.terrain_total_chunks
                    ));
                    if stats.transparent_draws > 0 {
                        ui.label(format!("Transparent: {}", stats.transparent_draws));
                    }
                    if stats.billboard_draws > 0 {
                        ui.label(format!("Billboards: {}", stats.billboard_draws));
                    }
                    if stats.particle_draws > 0 {
                        ui.label(format!(
                            "Particles: {} ({} inst)",
                            stats.particle_draws,
                            format_count(stats.particle_instances)
                        ));
                    }
                    if stats.sprite_batches > 0 {
                        ui.label(format!("Sprites: {}", stats.sprite_batches));
                    }
                    if stats.grass_instances > 0 {
                        ui.label(format!(
                            "Grass: {} inst",
                            format_count(stats.grass_instances)
                        ));
                    }

                    ui.separator();

                    // Shadow pass
                    ui.label(format!("Shadow Calls: {}", stats.shadow_draw_calls));
                    ui.label(format!(
                        "Shadow Tris: {}",
                        format_count(stats.shadow_triangles)
                    ));

                    ui.separator();

                    // Resolution
                    ui.colored_label(
                        egui::Color32::from_gray(102),
                        format!("{}x{}", stats.resolution[0], stats.resolution[1]),
                    );
                });
        });
}
