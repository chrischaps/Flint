//! Main viewer application — combines wgpu scene rendering with egui panels

use crate::panels::{EntityInspector, RenderStats, SceneTree};
use flint_constraint::{ConstraintEvaluator, ConstraintRegistry};
use flint_ecs::FlintWorld;
use flint_import::import_gltf;
use flint_render::{Camera, RenderContext, SceneRenderer};
use flint_scene::load_scene;
use flint_schema::SchemaRegistry;
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

struct ViewerState {
    world: FlintWorld,
    registry: SchemaRegistry,
    constraint_registry: ConstraintRegistry,
    scene_path: String,
    needs_reload: bool,
}

/// Run the viewer application
pub fn run(
    scene_path: &str,
    watch: bool,
    schemas_path: &str,
    inspector: bool,
) -> anyhow::Result<()> {
    let registry = if Path::new(schemas_path).exists() {
        SchemaRegistry::load_from_directory(schemas_path)?
    } else {
        println!("Warning: Schemas directory not found: {}", schemas_path);
        SchemaRegistry::new()
    };

    let constraint_registry =
        ConstraintRegistry::load_from_directory(schemas_path).unwrap_or_default();

    let (world, scene_file) = load_scene(scene_path, &registry)?;

    println!("Loaded scene: {}", scene_file.scene.name);
    println!("Entities: {}", world.entity_count());

    let state = Arc::new(Mutex::new(ViewerState {
        world,
        registry,
        constraint_registry,
        scene_path: scene_path.to_string(),
        needs_reload: false,
    }));

    let _watcher = if watch {
        let state_clone = Arc::clone(&state);
        let (tx, rx) = mpsc::channel();

        let mut debouncer = new_debouncer(Duration::from_millis(500), tx)?;
        debouncer
            .watcher()
            .watch(Path::new(scene_path), RecursiveMode::NonRecursive)?;

        std::thread::spawn(move || {
            for result in rx {
                match result {
                    Ok(_events) => {
                        if let Ok(mut state) = state_clone.lock() {
                            state.needs_reload = true;
                        }
                    }
                    Err(e) => {
                        eprintln!("Watch error: {:?}", e);
                    }
                }
            }
        });

        println!("Watching for changes...");
        Some(debouncer)
    } else {
        None
    };

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = ViewerApp::new(state, inspector);
    event_loop.run_app(&mut app)?;

    Ok(())
}

/// The main viewer application
pub struct ViewerApp {
    state: Arc<Mutex<ViewerState>>,
    window: Option<Arc<Window>>,
    render_context: Option<RenderContext>,
    scene_renderer: Option<SceneRenderer>,
    camera: Camera,

    // Input state
    mouse_pressed: bool,
    last_mouse_pos: Option<(f64, f64)>,
    right_mouse_pressed: bool,

    // egui state
    egui_ctx: egui::Context,
    egui_winit: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    show_inspector: bool,

    // Panel state
    scene_tree: SceneTree,
    entity_inspector: EntityInspector,
    render_stats: RenderStats,

    // Constraint violations cache
    violation_count: usize,
    violation_messages: Vec<String>,
}

impl ViewerApp {
    fn new(state: Arc<Mutex<ViewerState>>, show_inspector: bool) -> Self {
        Self {
            state,
            window: None,
            render_context: None,
            scene_renderer: None,
            camera: Camera::new(),
            mouse_pressed: false,
            last_mouse_pos: None,
            right_mouse_pressed: false,
            egui_ctx: egui::Context::default(),
            egui_winit: None,
            egui_renderer: None,
            show_inspector,
            scene_tree: SceneTree::new(),
            entity_inspector: EntityInspector::new(),
            render_stats: RenderStats::new(),
            violation_count: 0,
            violation_messages: Vec::new(),
        }
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) {
        let window_attrs = Window::default_attributes()
            .with_title("Flint Viewer")
            .with_inner_size(PhysicalSize::new(1600, 900));

        let window = Arc::new(event_loop.create_window(window_attrs).unwrap());
        self.window = Some(window.clone());

        let render_context = pollster::block_on(RenderContext::new(window.clone())).unwrap();

        self.camera.aspect = render_context.aspect_ratio();
        self.camera.update_orbit();

        // Initialize egui
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

        let mut scene_renderer = SceneRenderer::new(&render_context);

        // Load models and update meshes from world
        {
            let state = self.state.lock().unwrap();
            load_models_from_world(
                &state.world,
                &mut scene_renderer,
                &render_context.device,
                &render_context.queue,
                &state.scene_path,
            );
            scene_renderer.update_from_world(&state.world, &render_context.device);
        }

        // Run initial constraint evaluation
        self.update_constraints();

        // Build initial scene tree
        {
            let state = self.state.lock().unwrap();
            self.scene_tree.update(&state.world);
        }

        self.render_context = Some(render_context);
        self.scene_renderer = Some(scene_renderer);
        self.egui_winit = Some(egui_winit);
        self.egui_renderer = Some(egui_renderer);
    }

    fn update_constraints(&mut self) {
        let state = self.state.lock().unwrap();
        let evaluator = ConstraintEvaluator::new(
            &state.world,
            &state.registry,
            &state.constraint_registry,
        );
        let report = evaluator.validate();
        self.violation_count = report.violations.len();
        self.violation_messages = report
            .violations
            .iter()
            .map(|v| format!("[{:?}] {}: {}", v.severity, v.entity_name, v.message))
            .collect();
    }

    fn render(&mut self) {
        if self.render_context.is_none()
            || self.scene_renderer.is_none()
            || self.window.is_none()
        {
            return;
        }

        let output = match self
            .render_context
            .as_ref()
            .unwrap()
            .surface
            .get_current_texture()
        {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => return,
            Err(e) => {
                eprintln!("Surface error: {:?}", e);
                return;
            }
        };

        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        self.render_stats.record_frame();

        // Render the 3D scene
        {
            let context = self.render_context.as_ref().unwrap();
            let renderer = self.scene_renderer.as_mut().unwrap();
            renderer.render(context, &self.camera, &view).ok();
        }

        // Render egui overlay if inspector is enabled
        if self.show_inspector {
            self.render_egui(&view);
        }

        output.present();
    }

    fn render_egui(&mut self, target_view: &wgpu::TextureView) {
        // Extract references to disjoint fields to satisfy the borrow checker
        let window = match &self.window {
            Some(w) => w.clone(),
            None => return,
        };
        let context = match &self.render_context {
            Some(c) => c,
            None => return,
        };
        let egui_winit = match &mut self.egui_winit {
            Some(e) => e,
            None => return,
        };

        let raw_input = egui_winit.take_egui_input(&window);

        // Build the UI — we need to collect data for the closure without borrowing self
        let scene_tree = &mut self.scene_tree;
        let entity_inspector = &self.entity_inspector;
        let render_stats = &self.render_stats;
        let violation_count = self.violation_count;
        let violation_messages = &self.violation_messages;
        let state = &self.state;

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            // Left side panel: scene tree
            egui::SidePanel::left("scene_tree_panel")
                .default_width(220.0)
                .resizable(true)
                .show(ctx, |ui| {
                    scene_tree.ui(ui);
                });

            // Right side panel: entity inspector
            egui::SidePanel::right("inspector_panel")
                .default_width(300.0)
                .resizable(true)
                .show(ctx, |ui| {
                    let selected = scene_tree.selected_entity();
                    if let Some(entity_id) = selected {
                        let st = state.lock().unwrap();
                        entity_inspector.ui(ui, &st.world, entity_id);
                    } else {
                        ui.heading("Entity Inspector");
                        ui.label("Select an entity in the scene tree.");
                    }
                });

            // Bottom panel: stats + constraint violations
            egui::TopBottomPanel::bottom("status_panel")
                .default_height(100.0)
                .resizable(true)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        render_stats.ui(ui);
                        ui.separator();

                        if violation_count == 0 {
                            ui.colored_label(
                                egui::Color32::from_rgb(100, 200, 100),
                                "No violations",
                            );
                        } else {
                            ui.colored_label(
                                egui::Color32::from_rgb(255, 100, 100),
                                format!("{} violation(s)", violation_count),
                            );
                        }
                    });

                    if !violation_messages.is_empty() {
                        ui.separator();
                        egui::ScrollArea::vertical().max_height(60.0).show(ui, |ui| {
                            for msg in violation_messages {
                                ui.label(msg);
                            }
                        });
                    }
                });
        });

        egui_winit.handle_platform_output(&window, full_output.platform_output);

        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [context.config.width, context.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let mut egui_renderer = self.egui_renderer.take().unwrap();

        let mut encoder =
            context
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("egui Encoder"),
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
                label: Some("egui Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
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

        context
            .queue
            .submit(std::iter::once(encoder.finish()));

        for id in &full_output.textures_delta.free {
            egui_renderer.free_texture(id);
        }

        self.egui_renderer = Some(egui_renderer);
    }

    fn check_reload(&mut self) {
        let (needs_reload, scene_path) = {
            let state = self.state.lock().unwrap();
            (state.needs_reload, state.scene_path.clone())
        };

        if needs_reload {
            {
                let mut state = self.state.lock().unwrap();
                state.needs_reload = false;
            }

            let reload_result = {
                let state = self.state.lock().unwrap();
                load_scene(&scene_path, &state.registry)
            };

            match reload_result {
                Ok((new_world, scene_file)) => {
                    println!("Reloaded scene: {}", scene_file.scene.name);

                    {
                        let mut state = self.state.lock().unwrap();
                        state.world = new_world;
                    }

                    if let (Some(context), Some(renderer)) =
                        (&self.render_context, &mut self.scene_renderer)
                    {
                        let state = self.state.lock().unwrap();
                        load_models_from_world(
                            &state.world,
                            renderer,
                            &context.device,
                            &context.queue,
                            &state.scene_path,
                        );
                        renderer.update_from_world(&state.world, &context.device);
                    }

                    {
                        let state = self.state.lock().unwrap();
                        self.scene_tree.update(&state.world);
                    }
                    self.update_constraints();
                }
                Err(e) => {
                    eprintln!("Failed to reload scene: {:?}", e);
                }
            }
        }
    }
}

/// Scan the world for entities with model components and load the referenced glTF files
fn load_models_from_world(
    world: &FlintWorld,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene_path: &str,
) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    for entity in world.all_entities() {
        let model_asset = world
            .get_components(entity.id)
            .and_then(|components| components.get("model").cloned())
            .and_then(|model| {
                model
                    .get("asset")
                    .and_then(|v| v.as_str().map(String::from))
            });

        if let Some(asset_name) = model_asset {
            if renderer.mesh_cache().contains(&asset_name) {
                continue;
            }

            let model_path = scene_dir.join("models").join(format!("{}.glb", asset_name));

            if model_path.exists() {
                match import_gltf(&model_path) {
                    Ok(import_result) => {
                        println!(
                            "Loaded model: {} ({} meshes, {} materials)",
                            asset_name,
                            import_result.meshes.len(),
                            import_result.materials.len()
                        );
                        renderer.load_model(device, queue, &asset_name, &import_result);
                    }
                    Err(e) => {
                        eprintln!("Failed to load model '{}': {:?}", asset_name, e);
                    }
                }
            } else {
                eprintln!(
                    "Model file not found: {} (tried {})",
                    asset_name,
                    model_path.display()
                );
            }
        }
    }

    // Also load texture files referenced by material components
    load_textures_from_world(world, renderer, device, queue, scene_path);
}

/// Scan the world for entities with material.texture and pre-load the referenced image files
fn load_textures_from_world(
    world: &FlintWorld,
    renderer: &mut SceneRenderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    scene_path: &str,
) {
    let scene_dir = Path::new(scene_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));

    let mut loaded: std::collections::HashSet<String> = std::collections::HashSet::new();

    for entity in world.all_entities() {
        let texture_name = world
            .get_components(entity.id)
            .and_then(|components| components.get("material").cloned())
            .and_then(|material| {
                material
                    .get("texture")
                    .and_then(|v| v.as_str().map(String::from))
            });

        if let Some(tex_name) = texture_name {
            if loaded.contains(&tex_name) {
                continue;
            }
            loaded.insert(tex_name.clone());

            let tex_path = scene_dir.join(&tex_name);
            if tex_path.exists() {
                match renderer.load_texture_file(device, queue, &tex_name, &tex_path) {
                    Ok(true) => {
                        println!("Loaded texture: {}", tex_name);
                    }
                    Ok(false) => {}
                    Err(e) => {
                        eprintln!("Failed to load texture '{}': {}", tex_name, e);
                    }
                }
            } else {
                eprintln!(
                    "Texture file not found: {} (tried {})",
                    tex_name,
                    tex_path.display()
                );
            }
        }
    }
}

impl ApplicationHandler for ViewerApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            self.initialize(event_loop);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Let egui handle the event first
        if let Some(egui_winit) = &mut self.egui_winit {
            if let Some(window) = &self.window {
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
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Escape) => {
                            event_loop.exit();
                        }
                        PhysicalKey::Code(KeyCode::KeyR) => {
                            if let Ok(mut state) = self.state.lock() {
                                state.needs_reload = true;
                            }
                        }
                        PhysicalKey::Code(KeyCode::Space) => {
                            self.camera = Camera::new();
                            self.camera.update_orbit();
                            if let Some(context) = &self.render_context {
                                self.camera.aspect = context.aspect_ratio();
                            }
                        }
                        PhysicalKey::Code(KeyCode::F1) => {
                            if let Some(renderer) = &mut self.scene_renderer {
                                let next = renderer.debug_state().mode.next();
                                renderer.set_debug_mode(next);
                                println!("Debug mode: {}", next.label());

                                if let Some(context) = &self.render_context {
                                    let state = self.state.lock().unwrap();
                                    renderer.update_from_world(&state.world, &context.device);
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::F2) => {
                            if let Some(renderer) = &mut self.scene_renderer {
                                let on = renderer.toggle_wireframe_overlay();
                                println!(
                                    "Wireframe overlay: {}",
                                    if on { "ON" } else { "OFF" }
                                );

                                if let Some(context) = &self.render_context {
                                    let state = self.state.lock().unwrap();
                                    renderer.update_from_world(&state.world, &context.device);
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::F3) => {
                            if let Some(renderer) = &mut self.scene_renderer {
                                let on = renderer.toggle_normal_arrows();
                                println!(
                                    "Normal arrows: {}",
                                    if on { "ON" } else { "OFF" }
                                );

                                if let Some(context) = &self.render_context {
                                    let state = self.state.lock().unwrap();
                                    renderer.update_from_world(&state.world, &context.device);
                                }
                            }
                        }
                        PhysicalKey::Code(KeyCode::F4) => {
                            if let Some(renderer) = &mut self.scene_renderer {
                                let on = renderer.toggle_shadows();
                                println!("Shadows: {}", if on { "ON" } else { "OFF" });
                            }
                        }
                        PhysicalKey::Code(KeyCode::Tab) => {
                            self.show_inspector = !self.show_inspector;
                            println!(
                                "Inspector: {}",
                                if self.show_inspector { "ON" } else { "OFF" }
                            );
                        }
                        _ => {}
                    }
                }
            }

            WindowEvent::MouseInput { state, button, .. } => match button {
                MouseButton::Left => {
                    self.mouse_pressed = state == ElementState::Pressed;
                }
                MouseButton::Right => {
                    self.right_mouse_pressed = state == ElementState::Pressed;
                }
                _ => {}
            },

            WindowEvent::CursorMoved { position, .. } => {
                if let Some((last_x, last_y)) = self.last_mouse_pos {
                    let dx = (position.x - last_x) as f32;
                    let dy = (position.y - last_y) as f32;

                    if self.mouse_pressed {
                        self.camera.orbit_horizontal(-dx * 0.01);
                        self.camera.orbit_vertical(-dy * 0.01);
                    }

                    if self.right_mouse_pressed {
                        self.camera.pan(-dx * 0.02, dy * 0.02);
                    }
                }

                self.last_mouse_pos = Some((position.x, position.y));
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / 100.0,
                };
                self.camera.zoom(scroll);
            }

            WindowEvent::RedrawRequested => {
                self.check_reload();
                self.render();
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}
