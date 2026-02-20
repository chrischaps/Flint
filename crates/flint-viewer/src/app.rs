//! Main viewer application — combines wgpu scene rendering with egui panels.
//! Supports an optional spline editor mode for interactive track editing.

use anyhow::{Context, Result};
use crate::panels::{CameraView, EntityInspector, GizmoAction, InspectorAction, RenderStats, SceneTree, SplinePanelAction, ViewGizmo};
use crate::spline_editor::{DragMode, SplineEditor, SplineEditorConfig};
use crate::transform_gizmo::{TransformGizmo, UndoEntry};
use flint_constraint::{ConstraintEvaluator, ConstraintRegistry};
use flint_ecs::FlintWorld;
use flint_render::model_loader::{self, ModelLoadConfig};
use flint_render::{Camera, RenderContext, RendererConfig, SceneRenderer};
use flint_scene::load_scene;
use flint_schema::SchemaRegistry;
use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowId};

struct ViewerState {
    world: FlintWorld,
    registry: SchemaRegistry,
    constraint_registry: ConstraintRegistry,
    scene_path: String,
    needs_reload: bool,
}

/// Run the viewer application (standard viewer mode)
pub fn run(
    scene_path: &str,
    watch: bool,
    schemas_path: &str,
    inspector: bool,
) -> Result<()> {
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
        let scene_file = Path::new(scene_path);
        let scene_dir = scene_file.parent().unwrap_or_else(|| Path::new("."));

        debouncer
            .watcher()
            .watch(scene_file, RecursiveMode::NonRecursive)?;
        debouncer
            .watcher()
            .watch(scene_dir, RecursiveMode::Recursive)?;

        let schemas_dir = Path::new(schemas_path);
        if schemas_dir.exists() {
            debouncer
                .watcher()
                .watch(schemas_dir, RecursiveMode::Recursive)?;
        }

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

        println!("Watching for changes in scene and related assets...");
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

/// Run the viewer with a pre-built world (no file loading or hot-reload).
/// `scene_path_anchor` is used for model path resolution only.
pub fn run_with_world(
    world: FlintWorld,
    registry: SchemaRegistry,
    scene_path_anchor: &str,
    inspector: bool,
) -> Result<()> {
    let constraint_registry = ConstraintRegistry::default();

    println!("Entities: {}", world.entity_count());

    let state = Arc::new(Mutex::new(ViewerState {
        world,
        registry,
        constraint_registry,
        scene_path: scene_path_anchor.to_string(),
        needs_reload: false,
    }));

    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = ViewerApp::new(state, inspector);
    event_loop.run_app(&mut app)?;

    Ok(())
}

/// Run the viewer in editor mode with a spline editor configuration.
pub fn run_editor(
    scene_path: &str,
    watch: bool,
    schemas_paths: &[&str],
    editor_config: SplineEditorConfig,
) -> Result<()> {
    let existing: Vec<&str> = schemas_paths
        .iter()
        .copied()
        .filter(|p| Path::new(p).exists())
        .collect();
    let registry = if !existing.is_empty() {
        SchemaRegistry::load_from_directories(&existing)?
    } else {
        println!("Warning: No schemas directories found");
        SchemaRegistry::new()
    };

    let constraint_registry = existing
        .first()
        .and_then(|p| ConstraintRegistry::load_from_directory(p).ok())
        .unwrap_or_default();

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
        let scene_file_path = Path::new(scene_path);
        let scene_dir = scene_file_path.parent().unwrap_or_else(|| Path::new("."));

        debouncer
            .watcher()
            .watch(scene_file_path, RecursiveMode::NonRecursive)?;
        debouncer
            .watcher()
            .watch(scene_dir, RecursiveMode::Recursive)?;

        for sp in schemas_paths {
            let schemas_dir = Path::new(sp);
            if schemas_dir.exists() {
                debouncer
                    .watcher()
                    .watch(schemas_dir, RecursiveMode::Recursive)?;
            }
        }

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

    let editor = SplineEditor::from_config(editor_config);
    let mut app = ViewerApp::new_editor(state, editor);
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
    middle_mouse_pressed: bool,
    modifiers: ModifiersState,

    // egui state
    egui_ctx: egui::Context,
    egui_winit: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    show_inspector: bool,

    // Panel state
    scene_tree: SceneTree,
    entity_inspector: EntityInspector,
    render_stats: RenderStats,
    view_gizmo: ViewGizmo,

    // Camera snap animation
    camera_snap_target: Option<(f32, f32)>,
    last_frame_time: Instant,

    // Constraint violations cache
    violation_count: usize,
    violation_messages: Vec<String>,

    // Editor mode (None = standard viewer)
    editor: Option<SplineEditor>,

    // Transform gizmo (active in non-editor viewer mode)
    transform_gizmo: TransformGizmo,
    gizmo_suppresses_orbit: bool,
    status_message: Option<(String, Instant)>,
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
            middle_mouse_pressed: false,
            modifiers: ModifiersState::empty(),
            egui_ctx: egui::Context::default(),
            egui_winit: None,
            egui_renderer: None,
            show_inspector,
            scene_tree: SceneTree::new(),
            entity_inspector: EntityInspector::new(),
            render_stats: RenderStats::new(),
            view_gizmo: ViewGizmo::new(),
            camera_snap_target: None,
            last_frame_time: Instant::now(),
            violation_count: 0,
            violation_messages: Vec::new(),
            editor: None,
            transform_gizmo: TransformGizmo::new(),
            gizmo_suppresses_orbit: false,
            status_message: None,
        }
    }

    fn new_editor(state: Arc<Mutex<ViewerState>>, editor: SplineEditor) -> Self {
        Self {
            state,
            window: None,
            render_context: None,
            scene_renderer: None,
            camera: Camera::new(),
            mouse_pressed: false,
            last_mouse_pos: None,
            right_mouse_pressed: false,
            middle_mouse_pressed: false,
            modifiers: ModifiersState::empty(),
            egui_ctx: egui::Context::default(),
            egui_winit: None,
            egui_renderer: None,
            show_inspector: true,
            scene_tree: SceneTree::new(),
            entity_inspector: EntityInspector::new(),
            render_stats: RenderStats::new(),
            view_gizmo: ViewGizmo::new(),
            camera_snap_target: None,
            last_frame_time: Instant::now(),
            violation_count: 0,
            violation_messages: Vec::new(),
            editor: Some(editor),
            transform_gizmo: TransformGizmo::new(),
            gizmo_suppresses_orbit: false,
            status_message: None,
        }
    }

    fn initialize(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let title = if self.editor.is_some() {
            "Flint Track Editor"
        } else {
            "Flint Viewer"
        };

        let window_attrs = Window::default_attributes()
            .with_title(title)
            .with_inner_size(PhysicalSize::new(1600, 900));

        let window = Arc::new(
            event_loop
                .create_window(window_attrs)
                .context("Failed to create viewer window")?,
        );
        self.window = Some(window.clone());

        let render_context = pollster::block_on(RenderContext::new(window.clone()))
            .context("Failed to initialize viewer render context")?;

        self.camera.aspect = render_context.aspect_ratio();

        // In editor mode, start with a good birds-eye view
        if self.editor.is_some() {
            self.camera.distance = 150.0;
            self.camera.pitch = 1.0; // ~57 degrees, nearly top-down
            self.camera.yaw = 0.0;
            // Center on approximate track center
            if let Some(editor) = &self.editor {
                if !editor.control_points.is_empty() {
                    let n = editor.control_points.len() as f32;
                    let cx: f32 = editor.control_points.iter().map(|p| p.position[0]).sum::<f32>() / n;
                    let cz: f32 = editor.control_points.iter().map(|p| p.position[2]).sum::<f32>() / n;
                    self.camera.target = flint_core::Vec3::new(cx, 0.0, cz);
                }
            }
        }

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

        let mut scene_renderer = SceneRenderer::new(&render_context, RendererConfig { show_grid: true });

        // Load models and update meshes from world
        {
            let mut state = self.state.lock().unwrap();
            let config = ModelLoadConfig::from_scene_path(&state.scene_path);
            model_loader::load_models_from_world(
                &mut state.world,
                &mut scene_renderer,
                &render_context.device,
                &render_context.queue,
                &config,
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

        Ok(())
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

        // Animate camera snap transitions
        self.animate_camera();

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
            renderer.set_selected_entity(self.scene_tree.selected_entity());
            renderer.render(context, &self.camera, &view).ok();
        }

        // Always render egui overlay (gizmo is always visible, panels are conditional)
        let gizmo_action = self.render_egui(&view);

        // Process gizmo interaction
        if let Some(action) = gizmo_action {
            match action {
                GizmoAction::SnapToView { yaw, pitch } => {
                    self.camera_snap_target = Some((yaw, pitch));
                    self.camera.orthographic = true;
                }
                GizmoAction::OrbitDelta { dyaw, dpitch } => {
                    self.camera.orbit_horizontal(dyaw);
                    self.camera.orbit_vertical(dpitch);
                    self.camera_snap_target = None;
                    self.camera.orthographic = false;
                }
                GizmoAction::SwitchToPerspective => {
                    self.camera.orthographic = false;
                }
            }
        }

        output.present();
    }

    fn animate_camera(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last_frame_time).as_secs_f32().min(0.1);
        self.last_frame_time = now;

        if let Some((target_yaw, target_pitch)) = self.camera_snap_target {
            let t = 1.0 - (-12.0 * dt).exp();
            self.camera.yaw = lerp_angle(self.camera.yaw, target_yaw, t);
            self.camera.pitch = lerp(self.camera.pitch, target_pitch, t);
            self.camera.update_orbit();

            let remaining = shortest_angle_diff(self.camera.yaw, target_yaw).abs()
                + (self.camera.pitch - target_pitch).abs();
            if remaining < 0.002 {
                self.camera.yaw = target_yaw;
                self.camera.pitch = target_pitch;
                self.camera.update_orbit();
                self.camera_snap_target = None;
            }
        }
    }

    fn render_egui(&mut self, target_view: &wgpu::TextureView) -> Option<GizmoAction> {
        // Extract references to disjoint fields to satisfy the borrow checker
        let window = match &self.window {
            Some(w) => w.clone(),
            None => return None,
        };
        let context = match &self.render_context {
            Some(c) => c,
            None => return None,
        };
        let egui_winit = match &mut self.egui_winit {
            Some(e) => e,
            None => return None,
        };

        let raw_input = egui_winit.take_egui_input(&window);

        // Snapshot camera state for gizmo (avoids holding &self.camera across the closure)
        let cam_view = CameraView::from_camera(&self.camera);

        // Build the UI — we need to collect data for the closure without borrowing self
        let scene_tree = &mut self.scene_tree;
        let entity_inspector = &mut self.entity_inspector;
        let render_stats = &self.render_stats;
        let violation_count = self.violation_count;
        let violation_messages = &self.violation_messages;
        let state = &self.state;
        let view_gizmo = &mut self.view_gizmo;
        let show_panels = self.show_inspector;
        let editor = &mut self.editor;
        let transform_gizmo = &self.transform_gizmo;
        let status_message = &self.status_message;

        let mut gizmo_action = None;
        let mut panel_actions: Vec<SplinePanelAction> = Vec::new();
        let mut inspector_action = InspectorAction::None;
        let camera_ref = &self.camera;

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            if show_panels {
                // Left side panel: scene tree
                egui::SidePanel::left("scene_tree_panel")
                    .default_width(220.0)
                    .resizable(true)
                    .show(ctx, |ui| {
                        scene_tree.ui(ui);
                    });

                // Right side panel: spline editor (in editor mode) or entity inspector
                egui::SidePanel::right("inspector_panel")
                    .default_width(300.0)
                    .resizable(true)
                    .show(ctx, |ui| {
                        if let Some(ed) = editor.as_mut() {
                            panel_actions = crate::panels::spline_panel::spline_editor_panel(ui, ed);
                        } else {
                            let selected = scene_tree.selected_entity();
                            if let Some(entity_id) = selected {
                                let st = state.lock().unwrap();
                                inspector_action = entity_inspector.ui(ui, &st.world, entity_id);
                            } else {
                                ui.heading("Entity Inspector");
                                ui.label("Select an entity in the scene tree.");
                            }
                        }
                    });

                // Bottom panel: stats + constraint violations + mode indicator
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

                            // Mode indicator for non-editor mode
                            if editor.is_none() {
                                ui.separator();
                                ui.colored_label(
                                    egui::Color32::from_rgb(180, 180, 200),
                                    "Translate",
                                );
                            }

                            // Status message (e.g., "Saved!" or "Save not supported")
                            if let Some((msg, time)) = status_message {
                                let elapsed = time.elapsed().as_secs_f32();
                                if elapsed < 3.0 {
                                    ui.separator();
                                    let alpha = ((3.0 - elapsed) / 0.5).min(1.0);
                                    ui.colored_label(
                                        egui::Color32::from_rgba_unmultiplied(
                                            200, 220, 255,
                                            (alpha * 255.0) as u8,
                                        ),
                                        msg,
                                    );
                                }
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
            }

            // View orientation gizmo (always visible)
            gizmo_action = view_gizmo.draw(ctx, &cam_view);

            // Draw spline overlay on the central area
            if let Some(ed) = editor.as_ref() {
                let screen_rect = ctx.screen_rect();
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("spline_overlay"),
                ));
                ed.draw_overlay(&painter, camera_ref, [screen_rect.width(), screen_rect.height()]);
            }

            // Draw transform gizmo overlay when entity selected in non-editor mode
            if editor.is_none() {
                if let Some(entity_id) = scene_tree.selected_entity() {
                    let st = state.lock().unwrap();
                    if let Some(world_pos) = st.world.get_world_position(entity_id) {
                        let pos = [world_pos.x, world_pos.y, world_pos.z];
                        let screen_rect = ctx.screen_rect();
                        let painter = ctx.layer_painter(egui::LayerId::new(
                            egui::Order::Foreground,
                            egui::Id::new("transform_gizmo_overlay"),
                        ));
                        transform_gizmo.draw_overlay(
                            &painter,
                            camera_ref,
                            [screen_rect.width(), screen_rect.height()],
                            pos,
                        );
                    }
                }
            }
        });

        // Process panel actions outside the egui closure
        for action in panel_actions {
            if let Some(ed) = &mut self.editor {
                match action {
                    SplinePanelAction::Save => {
                        if let Err(e) = ed.save() {
                            eprintln!("Save failed: {}", e);
                        }
                    }
                    SplinePanelAction::Undo => {
                        ed.undo();
                    }
                    SplinePanelAction::InsertPoint(idx) => {
                        ed.insert_point(idx);
                    }
                    SplinePanelAction::DeletePoint(idx) => {
                        ed.delete_point(idx);
                    }
                    SplinePanelAction::Resample => {
                        ed.resample();
                    }
                }
            }
        }

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

        // Process inspector action (deferred to avoid borrow conflicts with egui).
        // Skip while gizmo is dragging — the gizmo already moves the entity each frame,
        // and the inspector would detect those changes as edits, flooding the undo stack.
        if !self.transform_gizmo.is_dragging() {
            if let InspectorAction::TransformChanged {
                entity_id,
                entity_name,
                old_position,
                new_position,
            } = inspector_action
            {
                self.apply_entity_position(entity_id, new_position);
                self.transform_gizmo.push_undo(UndoEntry {
                    entity_id: entity_id.raw(),
                    entity_name,
                    old_position,
                    new_position,
                });
                self.refresh_renderer();
            }
        }

        gizmo_action
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
                        let mut state = self.state.lock().unwrap();
                        let config = ModelLoadConfig::from_scene_path(&state.scene_path);
                        model_loader::load_models_from_world(
                            &mut state.world,
                            renderer,
                            &context.device,
                            &context.queue,
                            &config,
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

    /// Get screen size from render context.
    fn screen_size(&self) -> [f32; 2] {
        if let Some(ctx) = &self.render_context {
            [ctx.config.width as f32, ctx.config.height as f32]
        } else {
            [1600.0, 900.0]
        }
    }

    // --- Editor-specific input handlers ---

    fn handle_editor_mouse_press(&mut self, button: MouseButton) {
        match button {
            MouseButton::Left => {
                self.mouse_pressed = true;
                let screen = self.screen_size();
                let alt_held = self.modifiers.alt_key();
                // Try to pick a control point
                if let (Some(editor), Some((mx, my))) = (&mut self.editor, self.last_mouse_pos) {
                    if let Some(idx) = editor.pick(&self.camera, screen, mx as f32, my as f32) {
                        editor.selected = Some(idx);
                        editor.dragging = true;
                        editor.drag_start_pos = editor.control_points[idx].position;
                        editor.push_undo();
                        editor.drag_mode = if alt_held {
                            DragMode::VerticalY
                        } else {
                            DragMode::HorizontalXZ
                        };
                    } else {
                        // Click on empty space: deselect
                        editor.selected = None;
                    }
                }
            }
            MouseButton::Middle => {
                self.middle_mouse_pressed = true;
            }
            MouseButton::Right => {
                self.right_mouse_pressed = true;
            }
            _ => {}
        }
    }

    fn handle_editor_mouse_release(&mut self, button: MouseButton) {
        match button {
            MouseButton::Left => {
                self.mouse_pressed = false;
                if let Some(editor) = &mut self.editor {
                    editor.dragging = false;
                }
            }
            MouseButton::Middle => {
                self.middle_mouse_pressed = false;
            }
            MouseButton::Right => {
                self.right_mouse_pressed = false;
            }
            _ => {}
        }
    }

    fn handle_editor_cursor_moved(&mut self, position: winit::dpi::PhysicalPosition<f64>) {
        if let Some((last_x, last_y)) = self.last_mouse_pos {
            let dx = (position.x - last_x) as f32;
            let dy = (position.y - last_y) as f32;

            // Middle-drag = orbit (replaces left-drag in standard mode)
            if self.middle_mouse_pressed {
                self.camera.orbit_horizontal(-dx * 0.01);
                self.camera.orbit_vertical(-dy * 0.01);
                self.camera_snap_target = None;
                self.camera.orthographic = false;
            }

            // Right-drag = pan (same as standard mode)
            if self.right_mouse_pressed {
                self.camera.pan(-dx * 0.02, dy * 0.02);
                self.camera_snap_target = None;
            }

            // Left-drag with dragging = move control point
            if self.mouse_pressed {
                let screen = self.screen_size();
                if let Some(editor) = &mut self.editor {
                    if editor.dragging {
                        editor.handle_drag(
                            &self.camera,
                            screen,
                            position.x as f32,
                            position.y as f32,
                        );
                    }
                }
            }
        }

        self.last_mouse_pos = Some((position.x, position.y));

        // Update hover state
        let screen = self.screen_size();
        if let Some(editor) = &mut self.editor {
            editor.update_hover(
                &self.camera,
                screen,
                position.x as f32,
                position.y as f32,
            );
        }
    }

    fn handle_editor_key(&mut self, key: KeyCode) -> bool {
        let ctrl = self.modifiers.control_key();
        let shift = self.modifiers.shift_key();

        match key {
            KeyCode::KeyS if ctrl => {
                if let Some(editor) = &mut self.editor {
                    if let Err(e) = editor.save() {
                        eprintln!("Save failed: {}", e);
                    }
                }
                true
            }
            KeyCode::KeyZ if ctrl => {
                if let Some(editor) = &mut self.editor {
                    editor.undo();
                }
                true
            }
            KeyCode::Delete | KeyCode::Backspace => {
                if let Some(editor) = &mut self.editor {
                    if let Some(idx) = editor.selected {
                        editor.delete_point(idx);
                    }
                }
                true
            }
            KeyCode::KeyI => {
                if let Some(editor) = &mut self.editor {
                    if let Some(idx) = editor.selected {
                        editor.insert_point(idx);
                    }
                }
                true
            }
            KeyCode::Tab => {
                if let Some(editor) = &mut self.editor {
                    let n = editor.control_points.len();
                    if n > 0 {
                        editor.selected = Some(match editor.selected {
                            Some(idx) => {
                                if shift {
                                    if idx == 0 { n - 1 } else { idx - 1 }
                                } else {
                                    (idx + 1) % n
                                }
                            }
                            None => 0,
                        });
                    }
                }
                true
            }
            KeyCode::Escape => {
                if let Some(editor) = &mut self.editor {
                    if editor.dragging {
                        editor.cancel_drag();
                        return true;
                    }
                }
                false // Let standard handler process Escape (close window)
            }
            _ => false,
        }
    }

    // --- Transform gizmo helpers ---

    /// Apply a new position to an entity in the world via merge_component.
    fn apply_entity_position(&mut self, entity_id: flint_core::EntityId, pos: [f32; 3]) {
        let mut state = self.state.lock().unwrap();
        let mut table = toml::value::Table::new();
        let mut pos_table = toml::value::Table::new();
        pos_table.insert("x".to_string(), toml::Value::Float(pos[0] as f64));
        pos_table.insert("y".to_string(), toml::Value::Float(pos[1] as f64));
        pos_table.insert("z".to_string(), toml::Value::Float(pos[2] as f64));
        table.insert("position".to_string(), toml::Value::Table(pos_table));
        let _ = state.world.merge_component(entity_id, "transform", toml::Value::Table(table));
    }

    /// Refresh the scene renderer from the current world state.
    fn refresh_renderer(&mut self) {
        if let (Some(context), Some(renderer)) =
            (&self.render_context, &mut self.scene_renderer)
        {
            let state = self.state.lock().unwrap();
            renderer.update_from_world(&state.world, &context.device);
        }
    }

    /// Save the current world state back to the scene file.
    fn save_scene(&mut self) {
        let state = self.state.lock().unwrap();
        let scene_path = state.scene_path.clone();
        drop(state);

        // Check if scene path looks like a real scene file (not a prefab anchor)
        if !scene_path.ends_with(".scene.toml") {
            self.status_message = Some(("Save not supported for this view".to_string(), Instant::now()));
            return;
        }

        let state = self.state.lock().unwrap();
        // Use save_scene which rebuilds from the world
        match flint_scene::save_scene(&scene_path, &state.world, "") {
            Ok(()) => {
                drop(state);
                println!("Saved scene: {}", scene_path);
                self.status_message = Some(("Saved!".to_string(), Instant::now()));
                self.transform_gizmo.modified = false;
            }
            Err(e) => {
                drop(state);
                eprintln!("Save failed: {}", e);
                self.status_message = Some((format!("Save failed: {}", e), Instant::now()));
            }
        }
    }
}

impl ApplicationHandler for ViewerApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            if let Err(e) = self.initialize(event_loop) {
                eprintln!("Failed to initialize viewer: {e:#}");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let is_editor = self.editor.is_some();

        // Track modifier keys
        if let WindowEvent::ModifiersChanged(mods) = &event {
            self.modifiers = mods.state();
        }

        // Handle Tab — in editor mode it cycles selection; in viewer mode it toggles inspector.
        if let WindowEvent::KeyboardInput { event: ref key_event, .. } = event {
            if key_event.state == ElementState::Pressed && !key_event.repeat {
                if is_editor {
                    // In editor mode, handle editor-specific keys first
                    if let PhysicalKey::Code(code) = key_event.physical_key {
                        if self.handle_editor_key(code) {
                            return;
                        }
                    }
                } else {
                    // Non-editor viewer mode keyboard shortcuts
                    if let PhysicalKey::Code(code) = key_event.physical_key {
                        match code {
                            KeyCode::Tab if self.modifiers.is_empty() => {
                                self.show_inspector = !self.show_inspector;
                                println!(
                                    "Inspector: {}",
                                    if self.show_inspector { "ON" } else { "OFF" }
                                );
                                return;
                            }
                            KeyCode::KeyZ if self.modifiers.control_key() && self.modifiers.shift_key() => {
                                // Ctrl+Shift+Z = redo
                                if let Some(entry) = self.transform_gizmo.redo() {
                                    self.apply_entity_position(
                                        flint_core::EntityId::from_raw(entry.entity_id),
                                        entry.new_position,
                                    );
                                    self.entity_inspector.invalidate_cache();
                                    self.refresh_renderer();
                                }
                                return;
                            }
                            KeyCode::KeyZ if self.modifiers.control_key() => {
                                // Ctrl+Z = undo
                                if let Some(entry) = self.transform_gizmo.undo() {
                                    self.apply_entity_position(
                                        flint_core::EntityId::from_raw(entry.entity_id),
                                        entry.old_position,
                                    );
                                    self.entity_inspector.invalidate_cache();
                                    self.refresh_renderer();
                                }
                                return;
                            }
                            KeyCode::KeyS if self.modifiers.control_key() => {
                                // Ctrl+S = save scene
                                self.save_scene();
                                return;
                            }
                            KeyCode::Escape if self.transform_gizmo.is_dragging() => {
                                // Cancel gizmo drag
                                if let Some(old_pos) = self.transform_gizmo.cancel_drag() {
                                    if let Some(entity_id) = self.scene_tree.selected_entity() {
                                        self.apply_entity_position(entity_id, old_pos);
                                        self.entity_inspector.invalidate_cache();
                                        self.refresh_renderer();
                                    }
                                }
                                return;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

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
                    if let Some(renderer) = &mut self.scene_renderer {
                        renderer.resize_postprocess(
                            &context.device,
                            new_size.width,
                            new_size.height,
                        );
                    }
                }
            }

            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed {
                    match event.physical_key {
                        PhysicalKey::Code(KeyCode::Escape) if !self.transform_gizmo.is_dragging() => {
                            event_loop.exit();
                        }
                        PhysicalKey::Code(KeyCode::KeyR) => {
                            if let Ok(mut state) = self.state.lock() {
                                state.needs_reload = true;
                            }
                        }
                        PhysicalKey::Code(KeyCode::Space) if !is_editor => {
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
                        _ => {}
                    }
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                if is_editor {
                    if state == ElementState::Pressed {
                        self.handle_editor_mouse_press(button);
                    } else {
                        self.handle_editor_mouse_release(button);
                    }
                } else {
                    match button {
                        MouseButton::Left => {
                            if state == ElementState::Pressed {
                                // Check gizmo pick before orbit
                                let screen = self.screen_size();
                                if let Some((mx, my)) = self.last_mouse_pos {
                                    if let Some(entity_id) = self.scene_tree.selected_entity() {
                                        let world_pos = {
                                            let st = self.state.lock().unwrap();
                                            st.world.get_world_position(entity_id)
                                                .map(|p| [p.x, p.y, p.z])
                                        };
                                        if let Some(pos) = world_pos {
                                            if let Some(axis) = self.transform_gizmo.pick(
                                                &self.camera, screen,
                                                mx as f32, my as f32, pos,
                                            ) {
                                                self.transform_gizmo.begin_drag(
                                                    axis, &self.camera, screen,
                                                    mx as f32, my as f32, pos,
                                                );
                                                self.gizmo_suppresses_orbit = true;
                                                self.mouse_pressed = true;
                                                return;
                                            }
                                        }
                                    }
                                }
                                self.mouse_pressed = true;
                            } else {
                                // Release
                                if self.transform_gizmo.is_dragging() {
                                    if let Some(entity_id) = self.scene_tree.selected_entity() {
                                        let (name, world_pos) = {
                                            let st = self.state.lock().unwrap();
                                            let name = st.world.all_entities().iter()
                                                .find(|e| e.id == entity_id)
                                                .map(|e| e.name.clone())
                                                .unwrap_or_default();
                                            let pos = st.world.get_world_position(entity_id)
                                                .map(|p| [p.x, p.y, p.z])
                                                .unwrap_or([0.0; 3]);
                                            (name, pos)
                                        };
                                        self.transform_gizmo.end_drag(
                                            entity_id.raw(), &name, world_pos,
                                        );
                                    }
                                    self.gizmo_suppresses_orbit = false;
                                }
                                self.mouse_pressed = false;
                            }
                        }
                        MouseButton::Right => {
                            self.right_mouse_pressed = state == ElementState::Pressed;
                        }
                        _ => {}
                    }
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                if is_editor {
                    self.handle_editor_cursor_moved(position);
                } else {
                    if self.transform_gizmo.is_dragging() {
                        // Gizmo drag in progress — move entity
                        let screen = self.screen_size();
                        if let Some(new_pos) = self.transform_gizmo.handle_drag(
                            &self.camera, screen,
                            position.x as f32, position.y as f32,
                        ) {
                            if let Some(entity_id) = self.scene_tree.selected_entity() {
                                self.apply_entity_position(entity_id, new_pos);
                                self.refresh_renderer();
                            }
                        }
                    } else {
                        if let Some((last_x, last_y)) = self.last_mouse_pos {
                            let dx = (position.x - last_x) as f32;
                            let dy = (position.y - last_y) as f32;

                            if self.mouse_pressed && !self.gizmo_suppresses_orbit {
                                self.camera.orbit_horizontal(-dx * 0.01);
                                self.camera.orbit_vertical(-dy * 0.01);
                                self.camera_snap_target = None;
                                self.camera.orthographic = false;
                            }

                            if self.right_mouse_pressed {
                                self.camera.pan(-dx * 0.02, dy * 0.02);
                                self.camera_snap_target = None;
                            }
                        }

                        // Update gizmo hover state
                        if let Some(entity_id) = self.scene_tree.selected_entity() {
                            let world_pos = {
                                let st = self.state.lock().unwrap();
                                st.world.get_world_position(entity_id)
                                    .map(|p| [p.x, p.y, p.z])
                            };
                            if let Some(pos) = world_pos {
                                let screen = self.screen_size();
                                self.transform_gizmo.update_hover(
                                    &self.camera, screen,
                                    position.x as f32, position.y as f32, pos,
                                );
                            }
                        }
                    }

                    self.last_mouse_pos = Some((position.x, position.y));
                }
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

// --- Angle interpolation helpers ---

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn shortest_angle_diff(from: f32, to: f32) -> f32 {
    use std::f32::consts::PI;
    let mut diff = to - from;
    while diff > PI {
        diff -= 2.0 * PI;
    }
    while diff < -PI {
        diff += 2.0 * PI;
    }
    diff
}

fn lerp_angle(from: f32, to: f32, t: f32) -> f32 {
    from + shortest_angle_diff(from, to) * t
}
